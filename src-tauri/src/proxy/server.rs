//! 本地模型代理的 HTTP 服务
//!
//! 只听 `127.0.0.1`。两个协议面：
//! - `POST /v1/chat/completions` → OpenAI 形状（`Authorization: Bearer`）
//! - `POST /v1/messages`         → Anthropic 形状（`x-api-key` + `anthropic-version`）
//!
//! 请求带 `x-orrery-harness: <id>` 时按路由表覆盖 `model` 字段，这样各 harness 只要把
//! base_url 指到这里，就能在 Orrery 里改模型。响应原样透传（含 SSE 流式），不缓冲、不改写。
//!
//! 密钥只在转发瞬间从环境变量读，不落盘、不进日志、不回传界面。

use super::config::{ProxyConfig, Wire};
use super::state::Metrics;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};

pub const HARNESS_HEADER: &str = "x-orrery-harness";
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AppState {
    pub config: RwLock<ProxyConfig>,
    pub metrics: Arc<Metrics>,
    pub client: reqwest::Client,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/messages", post(messages))
        .with_state(state)
}

async fn health(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "orrery-proxy",
        "requests": state.metrics.requests(),
        "uptime_ms": state.metrics.uptime_ms(),
    }))
}

/// 路由表里配置过的模型，方便 harness 的模型列表能拉到东西
async fn models(State(state): State<Arc<AppState>>) -> Json<Value> {
    let cfg = state.config.read().unwrap();
    let data: Vec<Value> = cfg
        .models
        .iter()
        .filter(|m| !m.id.trim().is_empty())
        .map(|m| {
            let owner = m.provider.clone();
            json!({ "id": m.id, "object": "model", "owned_by": owner })
        })
        .collect();
    Json(json!({ "object": "list", "data": data }))
}

async fn chat_completions(state: State<Arc<AppState>>, headers: HeaderMap, body: Json<Value>) -> Response {
    forward(state, headers, body.0, Wire::Openai, "chat/completions").await
}

async fn messages(state: State<Arc<AppState>>, headers: HeaderMap, body: Json<Value>) -> Response {
    forward(state, headers, body.0, Wire::Anthropic, "messages").await
}

fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    let message = message.into();
    (status, Json(json!({ "error": { "type": "orrery_proxy", "message": message } }))).into_response()
}

async fn forward(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut body: Value,
    wire: Wire,
    suffix: &str,
) -> Response {
    state.metrics.request_started();

    let harness = headers.get(HARNESS_HEADER).and_then(|v| v.to_str().ok()).map(str::to_owned);
    // 解析路由：读锁范围内拿到需要的值就放锁，不跨 await 持有
    let (url, provider_name, key, model, overridden) = {
        let cfg = state.config.read().unwrap();
        let requested = body.get("model").and_then(Value::as_str).unwrap_or("").to_string();
        let routed = cfg.route_model(harness.as_deref()).map(str::to_owned);
        let model = routed.clone().unwrap_or_else(|| requested.clone());
        if model.is_empty() {
            drop(cfg);
            return state.metrics.fail(error_response(StatusCode::BAD_REQUEST, "no model in request and no route configured"));
        }
        let Some((name, provider)) = cfg.provider_for(&model, wire) else {
            drop(cfg);
            return state.metrics.fail(error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("no provider for model {model}; register it in the Models page (or set a model_prefixes on a provider)"),
            ));
        };
        let url = format!("{}/{suffix}", provider.base_url.trim_end_matches('/'));
        let Some(key) = provider.resolve_key() else {
            let hint = if provider.api_key_env.trim().is_empty() {
                format!("{name}: set an API key in Orrery (Models → provider)")
            } else {
                format!("{name}: set API key in Orrery, or environment variable {}", provider.api_key_env)
            };
            drop(cfg);
            return state.metrics.fail(error_response(StatusCode::SERVICE_UNAVAILABLE, hint));
        };
        (url, name.to_string(), key, model.clone(), routed.is_some_and(|r| r != requested))
    };

    if overridden {
        body["model"] = Value::String(model.clone());
    }

    let mut req = state.client.post(&url).json(&body);
    req = match wire {
        Wire::Openai => req.header("authorization", format!("Bearer {key}")),
        Wire::Anthropic => {
            let version = headers
                .get("anthropic-version")
                .and_then(|v| v.to_str().ok())
                .unwrap_or(ANTHROPIC_VERSION)
                .to_string();
            req.header("x-api-key", key).header("anthropic-version", version)
        }
    };
    // 这些头由调用方决定语义，原样带上；鉴权头不透传，一律由代理按供应商重建
    for name in ["accept", "accept-encoding", "user-agent", "anthropic-beta", "openai-beta"] {
        if let Some(v) = headers.get(name) {
            req = req.header(name, v.clone());
        }
    }

    let upstream = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            return state.metrics.fail(error_response(
                StatusCode::BAD_GATEWAY,
                format!("{provider_name}: {e}"),
            ))
        }
    };

    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut out = Response::builder().status(status);
    for name in ["content-type", "cache-control", "x-request-id", "anthropic-request-id"] {
        if let Some(v) = upstream.headers().get(name) {
            if let (Ok(n), Ok(v)) = (HeaderName::try_from(name), HeaderValue::from_bytes(v.as_bytes())) {
                out = out.header(n, v);
            }
        }
    }
    state.metrics.finish(&provider_name, &model, status.as_u16());

    // 流式：上游字节来一块转一块，不缓冲整包
    let stream = upstream.bytes_stream().map(|chunk| chunk.map_err(std::io::Error::other));
    out.body(Body::from_stream(stream))
        .unwrap_or_else(|e| error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

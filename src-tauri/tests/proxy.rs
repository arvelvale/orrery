//! 代理转发的集成测试：上游是本地起的假服务，不连真实厂商、不使用真实密钥。
//!
//! 覆盖：模型覆盖与鉴权头、Anthropic 形状、SSE 流式透传、缺密钥、上游错误码透传。

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 假上游收到的请求
#[derive(Debug, Clone, Default)]
struct Seen {
    path: String,
    model: String,
    authorization: Option<String>,
    api_key: Option<String>,
    anthropic_version: Option<String>,
    stream: bool,
}

type Recorder = Arc<Mutex<Vec<Seen>>>;

async fn record(path: &str, seen: &Recorder, headers: &HeaderMap, body: &Value) {
    let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).map(str::to_owned);
    seen.lock().unwrap().push(Seen {
        path: path.into(),
        model: body.get("model").and_then(Value::as_str).unwrap_or("").into(),
        authorization: h("authorization"),
        api_key: h("x-api-key"),
        anthropic_version: h("anthropic-version"),
        stream: body.get("stream").and_then(Value::as_bool).unwrap_or(false),
    });
}

async fn upstream_chat(State(seen): State<Recorder>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    record("chat/completions", &seen, &headers, &body).await;
    if body.get("stream").and_then(Value::as_bool) == Some(true) {
        // 分三次发送，每块之间留间隔：代理必须边收边转，不能等全部结束
        let stream = async_stream::stream! {
            for chunk in ["data: {\"delta\":\"a\"}\n\n", "data: {\"delta\":\"b\"}\n\n", "data: [DONE]\n\n"] {
                tokio::time::sleep(Duration::from_millis(60)).await;
                yield Ok::<_, std::io::Error>(axum::body::Bytes::from(chunk));
            }
        };
        return Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(axum::body::Body::from_stream(stream))
            .unwrap();
    }
    if body.get("model").and_then(Value::as_str) == Some("boom") {
        return (axum::http::StatusCode::TOO_MANY_REQUESTS, Json(json!({ "error": "rate limited" }))).into_response();
    }
    Json(json!({ "id": "resp_1", "model": body["model"], "object": "chat.completion" })).into_response()
}

async fn upstream_messages(State(seen): State<Recorder>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    record("messages", &seen, &headers, &body).await;
    Json(json!({ "id": "msg_1", "model": body["model"], "type": "message" })).into_response()
}

/// 启动假上游，返回 base_url
async fn start_upstream(seen: Recorder) -> String {
    let app = Router::new()
        .route("/chat/completions", post(upstream_chat))
        .route("/messages", post(upstream_messages))
        .with_state(seen);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// 用沙盒 HOME 起代理：配置写在临时目录，绝不碰真实 ~/.openplane
fn sandbox_home(dir: &std::path::Path, upstream: &str) {
    std::env::set_var("OPENPLANE_HOME", dir);
    let cfg = json!({
        "listen": "127.0.0.1:0",
        "auto_start": false,
        "routes": { "kimi": "kimi-test-model" },
        "providers": {
            "fake-openai": {
                "base_url": upstream,
                "api_key_env": "OPENPLANE_TEST_OPENAI_KEY",
                "wire": "openai",
                "model_prefixes": ["gpt", "kimi", "boom"]
            },
            "fake-anthropic": {
                "base_url": upstream,
                "api_key_env": "OPENPLANE_TEST_ANTHROPIC_KEY",
                "wire": "anthropic",
                "model_prefixes": ["claude"]
            }
        }
    });
    std::fs::create_dir_all(dir.join(".openplane")).unwrap();
    std::fs::write(dir.join(".openplane").join("proxy.json"), serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn forwards_streams_and_reports_errors() {
    let seen: Recorder = Arc::default();
    let upstream = start_upstream(seen.clone()).await;

    let tmp = std::env::temp_dir().join(format!("openplane-proxy-test-{}", std::process::id()));
    sandbox_home(&tmp, &upstream);
    std::env::set_var("OPENPLANE_TEST_OPENAI_KEY", "test-openai-key");
    std::env::set_var("OPENPLANE_TEST_ANTHROPIC_KEY", "test-anthropic-key");

    // listen 端口写 0 会让系统分配，status() 里能拿到实际端口
    let status = tokio::task::spawn_blocking(openplane_lib::proxy::start).await.unwrap().unwrap();
    let base = format!("http://{}", status.listen);
    assert!(status.running, "proxy should be running: {}", status.message);

    let client = reqwest::Client::new();

    // 1. OpenAI 形状：带 harness 头 → 模型被路由覆盖，鉴权头由代理按供应商重建
    let r = client
        .post(format!("{base}/v1/chat/completions"))
        .header("x-openplane-harness", "kimi")
        .json(&json!({ "model": "whatever-client-said", "messages": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["model"], "kimi-test-model");
    let last = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.path, "chat/completions");
    assert_eq!(last.model, "kimi-test-model", "route should override the requested model");
    assert_eq!(last.authorization.as_deref(), Some("Bearer test-openai-key"));
    assert!(last.api_key.is_none(), "openai wire must not send x-api-key");

    // 2. 不带 harness 头 → 保留请求里的模型
    client
        .post(format!("{base}/v1/chat/completions"))
        .json(&json!({ "model": "gpt-test", "messages": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(seen.lock().unwrap().last().unwrap().model, "gpt-test");

    // 3. Anthropic 形状：x-api-key + anthropic-version
    let r = client
        .post(format!("{base}/v1/messages"))
        .json(&json!({ "model": "claude-test", "messages": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let last = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.path, "messages");
    assert_eq!(last.api_key.as_deref(), Some("test-anthropic-key"));
    assert_eq!(last.anthropic_version.as_deref(), Some("2023-06-01"));
    assert!(last.authorization.is_none(), "anthropic wire must not send Authorization");

    // 4. 流式：分块到达，不是最后一次性返回
    let r = client
        .post(format!("{base}/v1/chat/completions"))
        .json(&json!({ "model": "gpt-test", "messages": [], "stream": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers().get("content-type").unwrap(), "text/event-stream");
    let mut chunks = 0;
    let mut text = String::new();
    let started = std::time::Instant::now();
    let mut first_chunk_at = None;
    let mut stream = r.bytes_stream();
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        if first_chunk_at.is_none() {
            first_chunk_at = Some(started.elapsed());
        }
        chunks += 1;
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(chunks >= 2, "expected chunked passthrough, got {chunks} chunk(s)");
    assert!(text.contains("[DONE]"), "stream body: {text}");
    // 第一块应在最后一块之前明显到达（上游每块间隔 60ms，共 3 块）
    assert!(first_chunk_at.unwrap() < Duration::from_millis(150), "first chunk took {:?}", first_chunk_at);
    assert!(seen.lock().unwrap().last().unwrap().stream);

    // 5. 上游错误码原样透传
    let r = client
        .post(format!("{base}/v1/chat/completions"))
        .json(&json!({ "model": "boom", "messages": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 429);

    // 6. 缺环境变量 → 503，错误信息里说清楚缺哪个变量，且不含密钥
    std::env::remove_var("OPENPLANE_TEST_ANTHROPIC_KEY");
    let r = client
        .post(format!("{base}/v1/messages"))
        .json(&json!({ "model": "claude-test", "messages": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 503);
    let body: Value = r.json().await.unwrap();
    let msg = body["error"]["message"].as_str().unwrap();
    assert!(msg.contains("OPENPLANE_TEST_ANTHROPIC_KEY"), "{msg}");
    assert!(!msg.contains("test-anthropic-key"), "error must not leak the key: {msg}");

    // 7. /health 与状态计数
    let health: Value = client.get(format!("{base}/health")).send().await.unwrap().json().await.unwrap();
    assert_eq!(health["status"], "ok");
    let status = tokio::task::spawn_blocking(openplane_lib::proxy::status).await.unwrap();
    assert!(status.requests >= 6, "requests={}", status.requests);
    assert!(status.failures >= 2, "failures={}", status.failures);
    assert_eq!(status.last_request.as_ref().unwrap().provider, "fake-openai");
    assert!(status.providers.iter().any(|p| p.name == "fake-openai" && p.key_present));
    assert!(status.providers.iter().any(|p| p.name == "fake-anthropic" && !p.key_present));

    // 8. 停止后端口不再接受新连接
    tokio::task::spawn_blocking(openplane_lib::proxy::stop).await.unwrap().unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(client.get(format!("{base}/health")).timeout(Duration::from_secs(2)).send().await.is_err());

    std::fs::remove_dir_all(&tmp).ok();
}

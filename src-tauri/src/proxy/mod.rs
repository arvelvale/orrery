//! 本地模型代理：启停、状态、路由配置
//!
//! 只允许监听回环地址；密钥只在转发瞬间从环境变量读取，不落盘、不回传界面。

pub mod config;
mod server;
mod state;

pub use config::Wire;

use config::Provider;
use serde::Serialize;
use server::AppState;
use state::{LastRequest, Metrics};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

struct Running {
    listen: String,
    shutdown: tokio::sync::watch::Sender<bool>,
    state: Arc<AppState>,
    metrics: Arc<Metrics>,
}

fn slot() -> &'static Mutex<Option<Running>> {
    static SLOT: OnceLock<Mutex<Option<Running>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderStatus {
    pub name: String,
    pub base_url: String,
    pub wire: Wire,
    pub key_env: String,
    /// 只报告环境变量有没有值，不回传值本身
    pub key_present: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProxyStatus {
    /// 代理是否由本进程启动并在运行
    pub running: bool,
    /// 端口上有监听（可能是本进程，也可能是别的程序占着）
    pub online: bool,
    pub endpoint: String,
    pub listen: String,
    pub auto_start: bool,
    pub uptime_ms: u64,
    pub requests: u64,
    pub failures: u64,
    pub latency_ms: Option<u64>,
    pub last_request: Option<LastRequest>,
    pub last_error: Option<String>,
    pub providers: Vec<ProviderStatus>,
    pub routes: std::collections::BTreeMap<String, String>,
    pub config_path: String,
    pub message: String,
}

fn endpoint_of(listen: &str) -> String {
    format!("http://{listen}/v1")
}

/// 只允许回环地址：代理会带着用户的密钥转发，绝不对外网开放
fn resolve_loopback(listen: &str) -> Result<SocketAddr, String> {
    let addr = listen
        .to_socket_addrs()
        .map_err(|e| format!("invalid listen address {listen}: {e}"))?
        .next()
        .ok_or_else(|| format!("invalid listen address {listen}"))?;
    if !addr.ip().is_loopback() {
        return Err(format!("refusing to listen on {addr}: only loopback addresses are allowed"));
    }
    Ok(addr)
}

pub fn start() -> Result<ProxyStatus, String> {
    {
        let guard = slot().lock().unwrap();
        if guard.is_some() {
            drop(guard);
            return Ok(status());
        }
    }
    let cfg = config::load();
    let addr = resolve_loopback(&cfg.listen)?;

    let metrics = Arc::new(Metrics::default());
    metrics.start();
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let app_state = Arc::new(AppState {
        config: RwLock::new(cfg),
        metrics: metrics.clone(),
        client,
    });

    // 端口写 0 时由系统分配，实际端口要绑定后才知道
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<SocketAddr, String>>();
    let (shutdown, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let thread_state = app_state.clone();
    let thread_metrics = metrics.clone();

    std::thread::Builder::new()
        .name("orrery-proxy".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            rt.block_on(async move {
                let listener = match tokio::net::TcpListener::bind(addr).await {
                    Ok(l) => {
                        let bound = l.local_addr().unwrap_or(addr);
                        let _ = ready_tx.send(Ok(bound));
                        l
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("cannot bind {addr}: {e}")));
                        return;
                    }
                };
                let served = axum::serve(listener, server::router(thread_state))
                    .with_graceful_shutdown(async move {
                        let _ = shutdown_rx.changed().await;
                    })
                    .await;
                if let Err(e) = served {
                    thread_metrics.note_error(e.to_string());
                }
            });
        })
        .map_err(|e| e.to_string())?;

    let bound = match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(bound)) => bound,
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err("proxy did not start within 5s".into()),
    };

    *slot().lock().unwrap() = Some(Running { listen: bound.to_string(), shutdown, state: app_state, metrics });
    eprintln!("[orrery] proxy listening on {bound}");
    Ok(status())
}

pub fn stop() -> Result<ProxyStatus, String> {
    if let Some(running) = slot().lock().unwrap().take() {
        let _ = running.shutdown.send(true);
        eprintln!("[orrery] proxy stopping on {}", running.listen);
    }
    // 优雅关闭要等在途请求结束（流式响应可能还在传），端口不会立刻释放
    Ok(status())
}

fn probe(addr: &str) -> (bool, Option<u64>) {
    let Ok(Some(target)) = addr.to_socket_addrs().map(|mut a| a.next()) else {
        return (false, None);
    };
    let started = std::time::Instant::now();
    match TcpStream::connect_timeout(&target, Duration::from_millis(300)) {
        Ok(_) => (true, Some(started.elapsed().as_millis() as u64)),
        Err(_) => (false, None),
    }
}

pub fn status() -> ProxyStatus {
    let guard = slot().lock().unwrap();
    let cfg = match guard.as_ref() {
        Some(r) => r.state.config.read().unwrap().clone(),
        None => config::load(),
    };
    let listen = guard.as_ref().map(|r| r.listen.clone()).unwrap_or_else(|| cfg.listen.clone());
    let (online, latency_ms) = probe(&listen);
    let running = guard.is_some();
    let metrics = guard.as_ref().map(|r| r.metrics.clone());
    drop(guard);

    let providers = cfg
        .providers
        .iter()
        .map(|(name, p): (&String, &Provider)| ProviderStatus {
            name: name.clone(),
            base_url: p.base_url.clone(),
            wire: p.wire,
            key_env: p.api_key_env.clone(),
            key_present: std::env::var(&p.api_key_env).is_ok_and(|v| !v.trim().is_empty()),
        })
        .collect();

    ProxyStatus {
        running,
        online,
        endpoint: endpoint_of(&listen),
        listen: listen.clone(),
        auto_start: cfg.auto_start,
        uptime_ms: metrics.as_ref().map(|m| m.uptime_ms()).unwrap_or(0),
        requests: metrics.as_ref().map(|m| m.requests()).unwrap_or(0),
        failures: metrics.as_ref().map(|m| m.failures()).unwrap_or(0),
        latency_ms,
        last_request: metrics.as_ref().and_then(|m| m.last_request()),
        last_error: metrics.as_ref().and_then(|m| m.last_error()),
        providers,
        routes: cfg.routes.clone(),
        config_path: config::config_path().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
        message: if running {
            "running".into()
        } else if online {
            "port in use by another program".into()
        } else {
            "stopped".into()
        },
    }
}

/// 「检测连通」：探活并刷新状态
pub fn ping() -> ProxyStatus {
    status()
}

/// 改某个 harness 的默认模型：写配置文件，并让正在运行的代理立刻生效
pub fn save_route(harness: &str, model: &str) -> Result<(), String> {
    let mut cfg = config::load();
    if model.trim().is_empty() {
        cfg.routes.remove(harness);
    } else {
        cfg.routes.insert(harness.to_string(), model.to_string());
    }
    config::save(&cfg)?;
    if let Some(running) = slot().lock().unwrap().as_ref() {
        *running.state.config.write().unwrap() = cfg;
    }
    Ok(())
}

pub fn set_auto_start(enabled: bool) -> Result<(), String> {
    let mut cfg = config::load();
    cfg.auto_start = enabled;
    config::save(&cfg)?;
    if let Some(running) = slot().lock().unwrap().as_ref() {
        running.state.config.write().unwrap().auto_start = enabled;
    }
    Ok(())
}

/// 应用启动时调用：写出默认配置，按需自动拉起
pub fn init() {
    config::ensure_exists();
    if config::load().auto_start {
        if let Err(e) = start() {
            eprintln!("[orrery] proxy auto-start failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use state::now_ms;

    #[test]
    fn only_loopback_is_accepted() {
        assert!(resolve_loopback("127.0.0.1:8787").is_ok());
        assert!(resolve_loopback("localhost:8787").is_ok());
        let err = resolve_loopback("0.0.0.0:8787").unwrap_err();
        assert!(err.contains("only loopback"), "{err}");
        assert!(resolve_loopback("not-an-address").is_err());
    }

    #[test]
    fn endpoint_format() {
        assert_eq!(endpoint_of("127.0.0.1:8787"), "http://127.0.0.1:8787/v1");
    }

    #[test]
    fn now_ms_is_recent() {
        assert!(now_ms() > 1_700_000_000_000);
    }
}

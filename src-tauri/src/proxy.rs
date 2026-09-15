//! Model proxy status + route persistence.
//! 真实 HTTP 代理在后续阶段；这里先落配置与健康探测。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const PROXY_PORT: u16 = 8787;
pub const PROXY_URL: &str = "http://127.0.0.1:8787/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyStatus {
    pub online: bool,
    pub endpoint: String,
    pub latency_ms: Option<u64>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProxyConfig {
    pub routes: HashMap<String, String>,
}

fn config_path() -> Option<PathBuf> {
    crate::adapters::home_dir().map(|h| h.join(".openplane").join("proxy.json"))
}

fn load_config() -> ProxyConfig {
    let Some(path) = config_path() else {
        return ProxyConfig::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(cfg: &ProxyConfig) -> Result<(), String> {
    let path = config_path().ok_or("cannot resolve ~/.openplane")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

pub fn status() -> ProxyStatus {
    let online = probe_tcp();
    ProxyStatus {
        online,
        endpoint: PROXY_URL.into(),
        latency_ms: None,
        message: if online {
            "tcp accept".into()
        } else {
            "no listener on 8787".into()
        },
    }
}

pub fn ping() -> ProxyStatus {
    let start = Instant::now();
    let online = probe_tcp();
    let latency_ms = start.elapsed().as_millis() as u64;
    ProxyStatus {
        online,
        endpoint: PROXY_URL.into(),
        latency_ms: Some(latency_ms),
        message: if online {
            "tcp accept".into()
        } else {
            "no listener on 8787".into()
        },
    }
}

fn probe_tcp() -> bool {
    use std::net::{Ipv4Addr, SocketAddrV4};
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, PROXY_PORT);
    TcpStream::connect_timeout(&addr.into(), Duration::from_millis(200)).is_ok()
}

pub fn save_route(harness: &str, model: &str) -> Result<(), String> {
    let mut cfg = load_config();
    cfg.routes.insert(harness.to_string(), model.to_string());
    save_config(&cfg)
}

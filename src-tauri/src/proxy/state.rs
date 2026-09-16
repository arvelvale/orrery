//! 代理运行状态：计数、最近一次转发、最近一次错误（都只在内存里）

use axum::response::Response;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Default)]
pub struct LastRequest {
    pub provider: String,
    pub model: String,
    pub status: u16,
    pub at_ms: u64,
}

#[derive(Debug, Default)]
pub struct Metrics {
    started_ms: AtomicU64,
    requests: AtomicU64,
    failures: AtomicU64,
    last_request: Mutex<Option<LastRequest>>,
    last_error: Mutex<Option<String>>,
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Metrics {
    pub fn start(&self) {
        self.started_ms.store(now_ms(), Ordering::Relaxed);
        self.requests.store(0, Ordering::Relaxed);
        self.failures.store(0, Ordering::Relaxed);
        *self.last_request.lock().unwrap() = None;
        *self.last_error.lock().unwrap() = None;
    }

    pub fn request_started(&self) {
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn requests(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }

    pub fn failures(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }

    pub fn uptime_ms(&self) -> u64 {
        now_ms().saturating_sub(self.started_ms.load(Ordering::Relaxed))
    }

    pub fn finish(&self, provider: &str, model: &str, status: u16) {
        if status >= 400 {
            self.failures.fetch_add(1, Ordering::Relaxed);
            *self.last_error.lock().unwrap() = Some(format!("{provider} {model} → HTTP {status}"));
        }
        *self.last_request.lock().unwrap() = Some(LastRequest {
            provider: provider.into(),
            model: model.into(),
            status,
            at_ms: now_ms(),
        });
    }

    /// 代理自身拒绝的请求（缺密钥、无供应商、上游连不上）；错误信息里不含密钥
    pub fn fail(&self, response: Response) -> Response {
        self.failures.fetch_add(1, Ordering::Relaxed);
        *self.last_error.lock().unwrap() = Some(format!("HTTP {}", response.status().as_u16()));
        response
    }

    pub fn note_error(&self, message: impl Into<String>) {
        *self.last_error.lock().unwrap() = Some(message.into());
    }

    pub fn last_request(&self) -> Option<LastRequest> {
        self.last_request.lock().unwrap().clone()
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap().clone()
    }
}

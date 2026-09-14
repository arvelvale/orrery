//! Harness adapters: discover local agent sessions.

mod claude_code;
mod kimi_code;

use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub harness: String,
    pub title: String,
    pub project: String,
    pub model: String,
    pub status: String,
    /// 最后更新时间（Unix 毫秒）。相对时间由前端按界面语言格式化，后端不产出自然语言
    pub updated_ms: u64,
    /// `usage.total()` 的短格式
    pub tokens: String,
    pub usage: TokenUsage,
    pub excerpt: String,
    pub path: String,
    pub log: Vec<(String, String)>,
    /// 磁盘占用：主会话文件 + 附属目录（子 agent 记录、工具输出、媒体）
    pub size_bytes: u64,
    /// 子 agent 会话数
    pub subagents: u32,
}

/// 会话累计 token（主 agent + 子 agent，按 API 调用去重后求和）
///
/// 口径对照：
/// | 字段        | Claude Code                    | Kimi Code          |
/// |-------------|--------------------------------|--------------------|
/// | input       | input_tokens                   | inputOther         |
/// | cache_write | cache_creation_input_tokens    | inputCacheCreation |
/// | cache_read  | cache_read_input_tokens        | inputCacheRead     |
/// | output      | output_tokens                  | output             |
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct TokenUsage {
    pub input: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    pub output: u64,
    /// 计入的 API 调用次数
    pub calls: u64,
}

impl TokenUsage {
    pub fn add(&mut self, other: &TokenUsage) {
        self.input += other.input;
        self.cache_write += other.cache_write;
        self.cache_read += other.cache_read;
        self.output += other.output;
        self.calls += other.calls;
    }

    pub fn total(&self) -> u64 {
        self.input + self.cache_write + self.cache_read + self.output
    }
}

/// 单个 harness 的本地会话存储统计（只读元数据，不读内容）
#[derive(Debug, Clone, Serialize)]
pub struct HarnessStorage {
    pub harness: String,
    /// 是否已有真实适配器；false 时 sessions/bytes 无意义
    pub connected: bool,
    pub sessions: u32,
    /// 会话相关文件合计
    pub session_bytes: u64,
    /// 会话根目录整体占用（含非会话文件）
    pub root_bytes: u64,
    pub root: String,
}

pub fn list_all_sessions() -> Result<Vec<SessionSummary>, String> {
    let t0 = Instant::now();
    let mut out = claude_code::list_sessions()?;
    let t_cc = t0.elapsed();
    let n_cc = out.len();

    let t1 = Instant::now();
    out.extend(kimi_code::list_sessions()?);
    let t_kimi = t1.elapsed();

    eprintln!(
        "[openplane] list_sessions: cc {} in {:?} · kimi {} in {:?}",
        n_cc,
        t_cc,
        out.len() - n_cc,
        t_kimi
    );

    out.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));
    out.truncate(300);
    Ok(out)
}

pub fn storage_stats() -> Vec<HarnessStorage> {
    let pending = |id: &str, root: &str| HarnessStorage {
        harness: id.into(),
        connected: false,
        sessions: 0,
        session_bytes: 0,
        root_bytes: 0,
        root: root.into(),
    };
    vec![
        claude_code::storage(),
        kimi_code::storage(),
        pending("dsh", "~/.dsh/sessions/"),
        pending("mimo", "~/.local/share/mimocode/sessions/"),
    ]
}

/* ── 解析缓存：文件没变就不重读 ── */

type Memo = HashMap<PathBuf, (u64, SessionSummary)>;

fn memo_store() -> &'static Mutex<Memo> {
    static MEMO: OnceLock<Mutex<Memo>> = OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `sig` 由参与解析的文件的 (长度, mtime) 折叠而成，变了才重新 `build`
pub(crate) fn memoized(
    key: &Path,
    sig: u64,
    build: impl FnOnce() -> Option<SessionSummary>,
) -> Option<SessionSummary> {
    if let Ok(map) = memo_store().lock() {
        if let Some((old, s)) = map.get(key) {
            if *old == sig {
                return Some(s.clone());
            }
        }
    }
    let s = build()?;
    if let Ok(mut map) = memo_store().lock() {
        map.insert(key.to_path_buf(), (sig, s.clone()));
    }
    Some(s)
}

pub(crate) fn file_sig(paths: &[PathBuf]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in paths {
        p.hash(&mut h);
        if let Ok(m) = fs::metadata(p) {
            m.len().hash(&mut h);
            if let Ok(t) = m.modified() {
                t.hash(&mut h);
            }
        }
    }
    h.finish()
}

/* ── 共用工具 ── */

pub(crate) fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0, // 符号链接不跟随，避免重复计数或成环
        })
        .sum()
}

pub(crate) fn system_time_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub(crate) fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}

/// 一位小数并去掉多余的 `.0`：128000 → "128k"，42100 → "42.1k"
fn trim_zero(v: f64, unit: &str) -> String {
    let s = format!("{v:.1}");
    format!("{}{unit}", s.strip_suffix(".0").unwrap_or(&s))
}

pub(crate) fn format_tokens(n: u64) -> String {
    if n == 0 {
        "—".into()
    } else if n >= 1_000_000 {
        trim_zero(n as f64 / 1_000_000.0, "M")
    } else if n >= 1_000 {
        trim_zero(n as f64 / 1_000.0, "k")
    } else {
        n.to_string()
    }
}

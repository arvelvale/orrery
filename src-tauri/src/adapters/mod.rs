//! Harness adapters: discover local agent sessions.

mod claude_code;
pub mod cleanup;
mod codex;
mod dsh;
mod kimi_code;

use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read};
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
/// 口径对照（所有字段互不重叠，`total()` 直接相加）：
/// | 字段        | Claude Code                 | Kimi Code          | DSH（DeepSeek）       | Codex                                  |
/// |-------------|-----------------------------|--------------------|-----------------------|----------------------------------------|
/// | input       | input_tokens                | inputOther         | inputTokens（已不含缓存） | input_tokens − cached_input_tokens（原值含缓存） |
/// | cache_write | cache_creation_input_tokens | inputCacheCreation | cacheWriteTokens      | cache_write_input_tokens               |
/// | cache_read  | cache_read_input_tokens     | inputCacheRead     | cacheReadTokens       | cached_input_tokens                    |
/// | output      | output_tokens               | output             | outputTokens（含推理）  | output_tokens（含 reasoning_output）     |
/// | unsplit     | —                           | —                  | —                     | 旧版/导入会话只有 total_tokens、分项全 0     |
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct TokenUsage {
    pub input: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    pub output: u64,
    /// 只有总数、没有分项的用量（不猜测拆分）
    pub unsplit: u64,
    /// 计入的 API 调用次数
    pub calls: u64,
}

impl TokenUsage {
    pub fn add(&mut self, other: &TokenUsage) {
        self.input += other.input;
        self.cache_write += other.cache_write;
        self.cache_read += other.cache_read;
        self.output += other.output;
        self.unsplit += other.unsplit;
        self.calls += other.calls;
    }

    pub fn total(&self) -> u64 {
        self.input + self.cache_write + self.cache_read + self.output + self.unsplit
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
    let adapters: [(&str, fn() -> Result<Vec<SessionSummary>, String>); 4] = [
        ("cc", claude_code::list_sessions),
        ("kimi", kimi_code::list_sessions),
        ("dsh", dsh::list_sessions),
        ("codex", codex::list_sessions),
    ];
    let mut out = Vec::new();
    let mut timing = Vec::new();
    for (name, list) in adapters {
        let t = Instant::now();
        // 单个适配器失败不拖垮其他 harness，错误进日志
        match list() {
            Ok(rows) => {
                timing.push(format!("{name} {} in {:?}", rows.len(), t.elapsed()));
                out.extend(rows);
            }
            Err(e) => timing.push(format!("{name} ERROR {e}")),
        }
    }
    eprintln!("[openplane] list_sessions: {}", timing.join(" · "));

    out.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));
    out.truncate(500);
    Ok(out)
}

pub fn storage_stats() -> Vec<HarnessStorage> {
    vec![
        claude_code::storage(),
        kimi_code::storage(),
        dsh::storage(),
        codex::storage(),
    ]
}

/// harness 目录不存在时的统一返回
pub(crate) fn storage_absent(harness: &str, root: &str) -> HarnessStorage {
    HarnessStorage {
        harness: harness.into(),
        connected: false,
        sessions: 0,
        session_bytes: 0,
        root_bytes: 0,
        root: root.into(),
    }
}

/// 逐行回调原始字节（不做 UTF-8 校验，按需再解析）；回调返回 false 提前结束
pub(crate) fn for_each_line<R: Read>(reader: R, mut f: impl FnMut(&[u8]) -> bool) {
    let mut reader = BufReader::with_capacity(1 << 16, reader);
    let mut buf = Vec::with_capacity(1 << 12);
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if !f(&buf) {
                    break;
                }
            }
        }
    }
}

pub(crate) fn contains(hay: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(hay, needle).is_some()
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

/// 删除后清掉已不存在路径的解析缓存
pub(crate) fn forget_memo(removed: &[PathBuf]) {
    if let Ok(mut map) = memo_store().lock() {
        map.retain(|k, _| !removed.iter().any(|r| k.starts_with(r)));
    }
}

/* ── 数据目录 ── */

/// 用户主目录。调试构建可用 `OPENPLANE_HOME` 指向沙盒（端到端测试删除时不碰真实数据），
/// 发布构建忽略该变量
pub(crate) fn home_dir() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        if let Some(h) = std::env::var_os("OPENPLANE_HOME").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(h));
        }
    }
    dirs::home_dir()
}

/// harness 的数据根目录：优先该工具自己的环境变量（与工具本身读同一处），否则 `~/<default>`。
/// 沙盒模式下忽略工具环境变量，避免指回真实目录
pub(crate) fn tool_home(env: &str, default: &str) -> Option<PathBuf> {
    let sandboxed = cfg!(debug_assertions) && std::env::var_os("OPENPLANE_HOME").is_some();
    if !sandboxed {
        if let Some(v) = std::env::var_os(env).filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(v));
        }
    }
    Some(home_dir()?.join(default))
}

pub(crate) fn claude_home() -> Option<PathBuf> {
    tool_home("CLAUDE_CONFIG_DIR", ".claude")
}
pub(crate) fn kimi_home() -> Option<PathBuf> {
    tool_home("KIMI_CODE_HOME", ".kimi-code")
}
pub(crate) fn dsh_home() -> Option<PathBuf> {
    tool_home("DSH_HOME", ".dsh")
}
pub(crate) fn codex_home() -> Option<PathBuf> {
    tool_home("CODEX_HOME", ".codex")
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

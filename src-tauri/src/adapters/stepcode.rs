//! StepCode sessions from `<agent dir>/sessions/--<cwd>--/<timestamp>_<id>.jsonl`
//!
//! StepCode（内部代号 pi，CLI 是 `step`）是阶跃星辰的 coding agent。会话按 JSONL
//! 一行一条追加，条目之间用 `id`/`parentId` 成树（v3）。
//!
//! # 目录布局（2026-10-09 本机实测 + 官方 `docs/session-format.md`）
//!
//! ```text
//! <root>/--<编码后的 cwd>--/<ISO 时间戳>_<sessionId>.jsonl
//! ```
//!
//! - `<root>` 三级解析：`STEP_CODING_AGENT_SESSION_DIR` → `<STEP_CODING_AGENT_DIR>/sessions`
//!   → `~/.stepcode/agent/sessions`。`--session-dir` 是 CLI 参数，读不到，忽略。
//! - 目录名把 `:` `\` `/` 全替换成 `-` 并用 `--` 包住（`D:\bigproject\orrery` →
//!   `--D--bigproject-orrery--`），**有损**。好在首行 `{"type":"session",…,"cwd":…}`
//!   带精确 cwd，优先用它，目录名只作兜底。
//! - 文件名的时间戳部分把 `:` 换成 `-`；会话 id 取 header 里的 `id`，文件名只作兜底。
//! - 子 agent 会话的文件名是 `<ts>_subagent-<uuid>.jsonl`，header 的 `id` 也是
//!   `subagent-<uuid>`。格式里**没有父会话 id**（`parentSession` 只在 `/fork`、
//!   `/clone` 场景出现），所以只能按时间包含关系推断归属，见 `fold_subagents`。
//!
//! # 条目类型
//!
//! | type | 有用字段 |
//! |---|---|
//! | `session` | `version` / `id` / `cwd` / 可选 `parentSession` |
//! | `message` | `message.role` = user / assistant / toolResult |
//! | `model_change` | `provider` / `modelId` |
//! | `thinking_level_change` | `thinkingLevel`（与 token 无关，跳过） |
//! | `compaction` | `summary` / `tokensBefore` / **`usage`** |
//! | `branch_summary` | `summary` / **`usage`** |
//! | `custom` | 扩展状态，不进 LLM 上下文，无用量 |
//! | `custom_message` | 扩展注入的上下文（如 `ultraloop-discovery`），不进标题 |
//! | `label` / `session_info` | `session_info.name` = `/name` 设置的显示名 |
//!
//! # token 口径
//!
//! - 用量在 assistant 条目的 `message.usage`：`input` / `output` / `cacheRead` /
//!   `cacheWrite` / `reasoning` / `totalTokens`。实测 599/599 条满足
//!   `totalTokens == input + cacheRead + cacheWrite + output`，四个桶互不重叠，直接映射。
//! - `reasoning` 本机 596 条恒为 0（推理算在 output 里）。将来供应商若把它单独记账，
//!   `totalTokens − 分项和` 会落到 `unsplit`，不猜比例。
//! - `compaction` / `branch_summary` 也带 `usage`（生成摘要那次调用），计入。
//! - `toolResult` 的 `details.results[].usage` 是**子 agent** 的用量：子 agent 有
//!   自己的会话文件且会被折叠进来，计了就是双倍，必须跳过。
//! - 每条 assistant 条目就是一次 API 调用（不像 Claude Code 流式按内容块拆行重复写），
//!   逐条 +1，不去重。
//!
//! # 删除与恢复
//!
//! - 删除：官方文档明说"Sessions can be removed by deleting their `.jsonl` files"。
//!   没有需要清理的文本索引，也不写任何别人的数据库。
//! - 恢复：`step --session <jsonl 绝对路径>`。沙盒实测 `--session <uuid>` /
//!   `--resume <uuid>` / `--session-id <uuid>` 全部找不到已存在的会话（见
//!   `resume.rs` 的实测记录），只有绝对路径可靠。路径由后端从 sessions 根目录反查。
//! - 状态：StepCode 没有 `~/.claude/sessions/<pid>.json` 这类活会话登记处，
//!   无法判断哪条在跑，一律 `idle`。

use super::{
    dir_size, file_sig, format_tokens, memoized, tool_home, truncate, HarnessStorage,
    SessionSummary, TokenUsage,
};
use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// 会话文件名的前缀：`<ISO 时间戳>_`。时间戳里没有 `_`，按第一个 `_` 切开就是 id
const TS_SEP: char = '_';
/// 子 agent 会话的 id 前缀（header 的 `id` 与文件名都是这个形状）
const SUBAGENT_PREFIX: &str = "subagent-";

/// `<agent dir>/sessions`：优先工具自己的环境变量，与 step 本身读同一处
fn sessions_root() -> Option<PathBuf> {
    // STEP_CODING_AGENT_SESSION_DIR 直接指会话目录；tool_home 自带沙盒判断
    let sandboxed = cfg!(debug_assertions) && std::env::var_os("ORRERY_HOME").is_some();
    if !sandboxed {
        if let Some(d) = std::env::var_os("STEP_CODING_AGENT_SESSION_DIR").filter(|v| !v.is_empty())
        {
            let p = PathBuf::from(d);
            if p.is_dir() {
                return Some(p);
            }
        }
    }
    let root = tool_home("STEP_CODING_AGENT_DIR", ".stepcode/agent")?.join("sessions");
    root.is_dir().then_some(root)
}

pub fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    let Some(root) = sessions_root() else {
        return Ok(vec![]);
    };
    // 同一批文件先按 cwd 目录分组：子 agent 折叠只在同目录内进行
    let groups = collect_by_dir(&root);
    let mut out = vec![];
    for (_dir, files) in &groups {
        let (mains, subs) = split_main_and_sub(files);
        let folded = fold_subagents(&mains, &subs);
        for (main, subs) in &folded {
            let mut sig_files = vec![main.clone()];
            sig_files.extend(subs.iter().cloned());
            let sig = file_sig(&sig_files);
            let Some(mut s) = memoized(main, sig, || parse_session(main, subs)) else {
                continue;
            };
            s.size_bytes = files_bytes(main, subs);
            out.push(s);
        }
        // 匹配不到父会话的子 agent：父会话不在本机，单独列出
        let orphaned: Vec<&PathBuf> = subs
            .iter()
            .filter(|s| !folded.iter().any(|(_, ss)| ss.iter().any(|x| x == *s)))
            .collect();
        for sub in orphaned {
            let sig = file_sig(std::slice::from_ref(sub));
            let Some(mut s) = memoized(sub, sig, || parse_session(sub, &[])) else {
                continue;
            };
            s.kind = "subagent".into();
            s.size_bytes = fs::metadata(sub).map(|m| m.len()).unwrap_or(0);
            out.push(s);
        }
    }
    Ok(out)
}

pub fn storage() -> HarnessStorage {
    let mut st = HarnessStorage {
        harness: "stepcode".into(),
        connected: true,
        sessions: 0,
        session_bytes: 0,
        root_bytes: 0,
        root: "~/.stepcode/agent/sessions/".into(),
    };
    let Some(root) = sessions_root() else {
        return st;
    };
    for (_, files) in collect_by_dir(&root) {
        let (mains, _subs) = split_main_and_sub(&files);
        // 会话数只数主会话，子 agent 并进父会话；字节数要连子 agent 一起算
        st.sessions += mains.len() as u32;
        st.session_bytes += files
            .iter()
            .map(|f| fs::metadata(f).map(|m| m.len()).unwrap_or(0))
            .sum::<u64>();
    }
    st.root_bytes = dir_size(&root);
    st
}

/// 按 cwd 目录分组，目录内按文件名（= 时间戳）排序，保证处理顺序稳定
fn collect_by_dir(root: &Path) -> Vec<(PathBuf, Vec<PathBuf>)> {
    let mut acc: Vec<(PathBuf, Vec<PathBuf>)> = vec![];
    let Ok(dirs) = fs::read_dir(root) else {
        return acc;
    };
    for dir in dirs.flatten() {
        let path = dir.path();
        if !path.is_dir() {
            continue;
        }
        let mut files: Vec<PathBuf> = fs::read_dir(&path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
            .collect();
        files.sort();
        if !files.is_empty() {
            acc.push((path, files));
        }
    }
    acc.sort();
    acc
}

fn split_main_and_sub(files: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut mains = vec![];
    let mut subs = vec![];
    for f in files {
        if is_subagent_file(f) {
            subs.push(f.clone());
        } else {
            mains.push(f.clone());
        }
    }
    (mains, subs)
}

/// 文件名 `<ts>_subagent-<uuid>.jsonl` 或 header id 以 `subagent-` 开头。
/// 先看文件名（不用读文件），拿不准再读 header
pub(super) fn is_subagent_file(path: &Path) -> bool {
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if let Some((_, id)) = stem.split_once(TS_SEP) {
            return id.starts_with(SUBAGENT_PREFIX);
        }
    }
    head_of(path)
        .map(|h| h.id.starts_with(SUBAGENT_PREFIX))
        .unwrap_or(false)
}

/// 每个主会话 → 折叠进来的子 agent 文件。
///
/// 格式里没有父 id，只能按时间包含关系推：子 agent 由主会话 spawn，它的
/// [首条, 末条] 时间区间必然落在主会话的区间内。同目录下能包住它、且区间最窄的
/// 主会话中标（并行开多个会话时取最紧的那个）。匹配不上就不折叠，由调用方单独列出。
fn fold_subagents(mains: &[PathBuf], subs: &[PathBuf]) -> Vec<(PathBuf, Vec<PathBuf>)> {
    let spans: Vec<(PathBuf, u64, u64)> = mains
        .iter()
        .map(|m| {
            let (a, b) = time_span(m);
            (m.clone(), a, b)
        })
        .collect();
    let mut out: Vec<(PathBuf, Vec<PathBuf>)> =
        spans.iter().map(|(m, _, _)| (m.clone(), vec![])).collect();
    for sub in subs {
        let (a, b) = time_span(sub);
        let mut best: Option<usize> = None;
        let mut best_width = u64::MAX;
        for (i, (_, ma, mb)) in spans.iter().enumerate() {
            if a >= *ma && b <= *mb {
                let width = mb - ma;
                // 并行开多个会话时取区间最窄的那个（is_none_or 是 1.82 才稳定的，别用）
                if width < best_width {
                    best = Some(i);
                    best_width = width;
                }
            }
        }
        if let Some(i) = best {
            out[i].1.push(sub.clone());
        }
    }
    // 子 agent 按文件名（时间戳）排序，列表和占用统计稳定
    for (_, subs) in out.iter_mut() {
        subs.sort();
    }
    out
}

/// 会话文件的 [首条, 末条] 时间戳（Unix 毫秒）。读首行 header 的 `timestamp`
/// 与最后一条非空行的 `timestamp`
pub(super) fn time_span(path: &Path) -> (u64, u64) {
    let Ok(file) = File::open(path) else {
        return (0, 0);
    };
    let reader = BufReader::new(file);
    let mut first = 0u64;
    let mut last = 0u64;
    for line in reader.lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        if first == 0 {
            if let Ok(h) = serde_json::from_str::<Value>(&line) {
                first = h
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .and_then(parse_iso8601_ms)
                    .unwrap_or(0);
            }
        }
        last = line_mtime(&line).max(last);
    }
    (first, last)
}

/// 一行的 `timestamp`：条目级是 ISO 8601 字符串，message 级是 Unix 毫秒数字
fn line_mtime(line: &str) -> u64 {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return 0;
    };
    let raw = v.get("timestamp");
    raw.and_then(Value::as_u64)
        .or_else(|| raw.and_then(Value::as_str).and_then(parse_iso8601_ms))
        .or_else(|| v.pointer("/message/timestamp").and_then(Value::as_u64))
        .unwrap_or(0)
}

fn files_bytes(main: &Path, subs: &[PathBuf]) -> u64 {
    let mut total = fs::metadata(main).map(|m| m.len()).unwrap_or(0);
    for s in subs {
        total += fs::metadata(s).map(|m| m.len()).unwrap_or(0);
    }
    total
}

/* ── 解析 ── */

struct Head {
    id: String,
    cwd: String,
}

/// 只读首行 header：`{"type":"session","version":3,"id":…,"cwd":…}`
fn head_of(path: &Path) -> Option<Head> {
    let file = File::open(path).ok()?;
    let line = BufReader::new(file).lines().next()?.ok()?;
    let v: Value = serde_json::from_str(&line).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("session") {
        return None;
    }
    Some(Head {
        id: v
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        cwd: v
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// 给 `cleanup` 用：header 里的会话 id
pub(super) fn head_id(path: &Path) -> Option<String> {
    head_of(path).map(|h| h.id).filter(|s| !s.is_empty())
}

fn parse_session(main: &Path, subagent_files: &[PathBuf]) -> Option<SessionSummary> {
    let head = head_of(main);
    // id 以 header 为准，文件名兜底
    let id = head
        .as_ref()
        .map(|h| h.id.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| file_id(main))
        .unwrap_or_default();
    if id.is_empty() {
        return None;
    }

    let mut scan = Scan::default();
    scan_file(main, true, &mut scan);
    for f in subagent_files {
        scan_file(f, false, &mut scan);
    }

    // 目录名编码有损，只在 header 没有 cwd 时兜底
    let project = head
        .as_ref()
        .map(|h| normalize_cwd(&h.cwd))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| decode_dir_name(main));

    // 标题：`/name` 的显示名 → 第一条真实用户输入 → 第一条可见文本
    let title = scan
        .name
        .clone()
        .or_else(|| scan.first_user.clone())
        .or_else(|| scan.excerpt.clone())
        .unwrap_or_default();
    let excerpt = scan.excerpt.clone().unwrap_or_default();
    if scan.updated_ms == 0 {
        return None;
    }
    // 空标题留给前端按界面语言兜底
    Some(SessionSummary {
        id,
        harness: "stepcode".into(),
        title: truncate(&title, 80),
        project,
        model: scan.model.clone().unwrap_or_else(|| "—".into()),
        status: "idle".into(),
        updated_ms: scan.updated_ms,
        tokens: format_tokens(scan.usage.total()),
        usage: scan.usage,
        excerpt: truncate(&excerpt, 200),
        path: main.to_string_lossy().to_string(),
        log: vec![],
        size_bytes: 0,
        subagents: subagent_files.len() as u32,
        kind: String::new(),
    })
}

#[derive(Default)]
struct Scan {
    /// `/name` 设置的显示名
    name: Option<String>,
    /// 第一条真实用户输入（标题的第二选择）
    first_user: Option<String>,
    /// 第一条可见文本（摘要，用户或助手）
    excerpt: Option<String>,
    model: Option<String>,
    updated_ms: u64,
    usage: TokenUsage,
}

/// `want_meta` = 主会话才取标题/模型；子 agent 只要用量和时间
fn scan_file(path: &Path, want_meta: bool, scan: &mut Scan) {
    let Ok(file) = File::open(path) else { return };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let ms = line_mtime(&line);
        scan.updated_ms = scan.updated_ms.max(ms);

        // 只有这几类有用；其余行（custom / label / thinking_level_change）直接跳过。
        // `"model` 不带收尾引号：要同时命中 `"model"`、`"modelId"`、`"type":"model_change"`
        let has_usage = line.contains("\"usage\"")
            && (line.contains("\"role\":\"assistant\"")
                || line.contains("\"type\":\"compaction\"")
                || line.contains("\"type\":\"branch_summary\""));
        let want_name = want_meta && line.contains("\"session_info\"");
        let want_text = want_meta && scan.excerpt.is_none() && line.contains("\"role\":\"");
        let want_user =
            want_text && scan.first_user.is_none() && line.contains("\"role\":\"user\"");
        let want_model = want_meta && scan.model.is_none() && line.contains("\"model");
        if !(has_usage || want_name || want_text || want_model) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");

        if want_name && ty == "session_info" {
            if let Some(n) = v
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                // 官方取最后一条，覆盖式的
                scan.name = Some(n.to_string());
            }
        }

        if want_model && scan.model.is_none() {
            if let Some(m) = v
                .pointer("/message/model")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                scan.model = Some(m.to_string());
            } else if ty == "model_change" {
                if let Some(m) = v
                    .get("modelId")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    scan.model = Some(m.to_string());
                }
            }
        }

        if want_text && ty == "message" {
            let role = v
                .pointer("/message/role")
                .and_then(Value::as_str)
                .unwrap_or("");
            if let Some(text) = message_text(v.pointer("/message")) {
                // 扩展注入的上下文块不是用户写的输入，不当标题也不当摘要
                if !is_injected(&text) {
                    if want_user && role == "user" && scan.first_user.is_none() {
                        scan.first_user = Some(text.clone());
                    }
                    if scan.excerpt.is_none() {
                        scan.excerpt = Some(text);
                    }
                }
            }
        }

        if has_usage {
            // toolResult 里子 agent 的用量会双倍计数，只要 assistant / compaction / branch_summary
            let usage = match ty {
                "message" => v.pointer("/message/usage"),
                "compaction" | "branch_summary" => v.get("usage"),
                _ => None,
            };
            if let Some(u) = usage {
                scan.usage.add(&entry_usage(u));
            }
        }
    }
}

/// `{"input":…,"output":…,"cacheRead":…,"cacheWrite":…,"reasoning":…,"totalTokens":…}`
fn entry_usage(u: &Value) -> TokenUsage {
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let input = n("input");
    let output = n("output");
    let cache_read = n("cacheRead");
    let cache_write = n("cacheWrite");
    let total = n("totalTokens");
    TokenUsage {
        input,
        output,
        cache_read,
        cache_write,
        // 分项之和小于 total：多出来的没有分项可依（例如 reasoning 被单独记账），
        // 单列不猜；本机 599/599 条都是 0
        unsplit: total.saturating_sub(input + output + cache_read + cache_write),
        calls: 1,
    }
}

/// user / assistant 消息里的可见文本。工具调用、思考块、图片都不取
fn message_text(m: Option<&Value>) -> Option<String> {
    let content = m?.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    let arr = content.as_array()?;
    let mut parts = vec![];
    for item in arr {
        if item.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(s) = item.get("text").and_then(Value::as_str) {
                parts.push(s.to_string());
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// StepCode 会把一整个 `<system-reminder>` 上下文块作为 custom_message 注入，
/// 用户消息里偶尔也会有；那不是用户写的输入
fn is_injected(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("<system-reminder")
        || t.starts_with("<user_info")
        || t.starts_with("<environment")
        || t.starts_with("<session_context")
}

/// 盘符大写、分隔符统一成 `/`，和其他 adapter 的路径视觉一致
fn normalize_cwd(cwd: &str) -> String {
    let s = cwd.replace('\\', "/");
    let b = s.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return format!(
            "{}:/{}",
            (b[0] as char).to_ascii_uppercase(),
            s[2..].trim_start_matches('/')
        );
    }
    s
}

/// 目录名 `--D--bigproject-orrery--` → `D:/bigproject/orrery`（有损，仅兜底）。
/// 编码把 `:` 和 `\` 都变成 `-` 再用 `--` 包住，所以名字里原本带 `-` 的目录解不回来
fn decode_dir_name(path: &Path) -> String {
    let dir = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let inner = dir.trim_start_matches('-').trim_end_matches('-');
    // 盘符后面跟两个 '-'（':' 和 '\` 各变成一个）
    if let Some((drive, rest)) = inner.split_once("--") {
        if drive.len() == 1 && drive.as_bytes()[0].is_ascii_alphabetic() {
            return format!("{}:/{}", drive.to_ascii_uppercase(), rest.replace('-', "/"));
        }
    }
    // Unix 下没有盘符，编码时 `:` 不存在，只有 `\`→`-`；绝对路径补回开头的 `/`
    format!("/{}", inner.replace('-', "/"))
}

/// 文件名 `<ts>_<id>.jsonl` → id。时间戳里没有 `_`，按第一个切开
fn file_id(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let (_, id) = stem.split_once(TS_SEP)?;
    (!id.is_empty()).then(|| id.to_string())
}

/// `2026-10-09T11:10:10.893Z` → Unix 毫秒；格式不符返回 None。
/// 与 `kimi_code::parse_iso8601_ms` 同一套算法（各 adapter 自带一份，避免为一个小函数开公用模块）
fn parse_iso8601_ms(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut rest = &s[19..];
    let mut ms = 0i64;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits: String = frac.chars().take_while(|c| c.is_ascii_digit()).collect();
        ms = format!("{:0<3}", &digits[..digits.len().min(3)])
            .parse()
            .ok()?;
        rest = &frac[digits.len()..];
    }
    let offset_min = match rest {
        "Z" | "z" => 0,
        _ if rest.len() == 6 && (rest.starts_with('+') || rest.starts_with('-')) => {
            let sign = if rest.starts_with('-') { -1 } else { 1 };
            sign * (rest[1..3].parse::<i64>().ok()? * 60 + rest[4..6].parse::<i64>().ok()?)
        }
        _ => return None,
    };
    // days_from_civil（Howard Hinnant）
    let (yy, mm) = if mo <= 2 {
        (y - 1, mo + 9)
    } else {
        (y, mo - 3)
    };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * mm + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let total = ((days * 24 + h) * 60 + mi - offset_min) * 60 + sec;
    u64::try_from(total * 1000 + ms).ok()
}

/* ── 给 resume.rs 用：按 id 反查会话文件的绝对路径 ── */

/// 扫 sessions 根目录，找 header `id` 等于 `id` 的文件。找不到返回 None。
/// 路径由后端解析，不接受前端传路径
pub fn session_file(id: &str) -> Option<PathBuf> {
    let root = sessions_root()?;
    for (_, files) in collect_by_dir(&root) {
        for f in files {
            if head_of(&f).map(|h| h.id == id).unwrap_or(false) {
                return Some(f);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个用例用自己的目录（id 唯一），并行测试互不干扰。
    /// parse_session 只依赖文件本身，不碰 ORRERY_HOME 这种进程级全局状态
    fn write(dir: &str, file: &str, lines: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "orrery-step-{}-{}-{}",
            dir,
            file,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let proj = root.join("sessions").join(dir);
        fs::create_dir_all(&proj).unwrap();
        let f = proj.join(format!("{file}.jsonl"));
        fs::write(&f, lines.join("\n")).unwrap();
        f
    }

    /// 真机形状的首行与模型切换
    const HEAD: &str = r#"{"type":"session","version":3,"id":"01a12010-5077-70de-838d-3f8e8537716b","timestamp":"2026-10-09T09:48:22.520Z","cwd":"D:\\bigproject\\orrery"}"#;
    const MODEL: &str = r#"{"type":"model_change","id":"0f0b0b43","parentId":null,"timestamp":"2026-10-09T09:48:23.072Z","provider":"step","modelId":"step-5-preview"}"#;

    #[test]
    fn project_path_comes_from_the_header_not_the_dir_name() {
        // 目录名有损（中文和 `-` 都分不清），cwd 才是精确的
        let f = write(
            "--D--bigproject-orrery--",
            "2026-10-09T09-48-22-520Z_01a12010-5077-70de-838d-3f8e8537716b",
            &[
                HEAD,
                MODEL,
                r#"{"type":"message","id":"a1","parentId":null,"timestamp":"2026-10-09T09:48:30.000Z","message":{"role":"user","content":[{"type":"text","text":"帮我把会话管起来"}],"timestamp":1791538110000}}"#,
            ],
        );
        let s = parse_session(&f, &[]).unwrap();
        assert_eq!(
            s.id, "01a12010-5077-70de-838d-3f8e8537716b",
            "id 取 header，不取文件名"
        );
        assert_eq!(
            s.project, "D:/bigproject/orrery",
            "盘符大写、分隔符统一成 /"
        );
        assert_eq!(s.model, "step-5-preview");
        assert_eq!(s.title, "帮我把会话管起来");
        assert_eq!(s.harness, "stepcode");
    }

    /// header 没有 cwd 时才退回解目录名。名字里带 `-` 的目录解不回来（`-` 就是分隔符），
    /// 所以这条路径只是兜底，正常会话都有 header cwd
    #[test]
    fn dir_name_is_only_a_fallback() {
        let f = write(
            "--D--code-recipe--",
            "2026-10-09T09-48-22-520Z_01a12011-5077-70de-838d-3f8e8537716b",
            &[
                r#"{"type":"session","version":3,"id":"01a12011-5077-70de-838d-3f8e8537716b","timestamp":"2026-10-09T09:48:22.520Z"}"#,
            ],
        );
        let s = parse_session(&f, &[]).unwrap();
        assert_eq!(s.project, "D:/code/recipe");
        // 名字里本来带 `-` 的目录会被拆开：这是已知的损失，所以 cwd 优先
        let lossy = write(
            "--D--code-recipe-box--",
            "2026-10-09T09-48-22-520Z_01a1201b-5077-70de-838d-3f8e8537716b",
            &[
                r#"{"type":"session","version":3,"id":"01a1201b-5077-70de-838d-3f8e8537716b","timestamp":"2026-10-09T09:48:22.520Z"}"#,
            ],
        );
        assert_eq!(
            parse_session(&lossy, &[]).unwrap().project,
            "D:/code/recipe/box"
        );
    }

    /// `/name` 设置的显示名优先于第一条用户输入
    #[test]
    fn session_name_beats_the_first_user_message() {
        let f = write(
            "--D--code-acme--",
            "2026-10-09T09-48-22-520Z_01a12012-5077-70de-838d-3f8e8537716b",
            &[
                HEAD,
                r#"{"type":"message","id":"a1","parentId":null,"timestamp":"2026-10-09T09:48:30.000Z","message":{"role":"user","content":[{"type":"text","text":"第一句"}],"timestamp":1}}"#,
                r#"{"type":"session_info","id":"n1","parentId":"a1","timestamp":"2026-10-09T09:49:00.000Z","name":"会话管理"}"#,
                r#"{"type":"session_info","id":"n2","parentId":"n1","timestamp":"2026-10-09T09:50:00.000Z","name":"改成这个"}"#,
            ],
        );
        let s = parse_session(&f, &[]).unwrap();
        assert_eq!(s.title, "改成这个", "官方取最后一条 session_info");
    }

    /// 注入的 <system-reminder> 不当标题也不当摘要
    #[test]
    fn injected_context_is_not_used_as_title() {
        let f = write(
            "--D--code-acme--",
            "2026-10-09T09-48-22-520Z_01a12013-5077-70de-838d-3f8e8537716b",
            &[
                HEAD,
                r#"{"type":"custom_message","customType":"ultraloop-discovery","content":"<system-reminder>Ultracode …</system-reminder>","display":false,"id":"c1","parentId":null,"timestamp":"2026-10-09T09:48:25.000Z"}"#,
                r#"{"type":"message","id":"a1","parentId":"c1","timestamp":"2026-10-09T09:48:30.000Z","message":{"role":"user","content":[{"type":"text","text":"真正的提问"}],"timestamp":1}}"#,
            ],
        );
        let s = parse_session(&f, &[]).unwrap();
        assert_eq!(s.title, "真正的提问");
        assert_eq!(s.excerpt, "真正的提问");
    }

    /// 四个用量桶互不重叠，直接相加；reasoning 恒为 0 时 unsplit 也是 0
    #[test]
    fn usage_buckets_add_up_directly() {
        let u = entry_usage(&serde_json::json!({
            "input": 10743, "output": 127, "cacheRead": 1536, "cacheWrite": 0,
            "reasoning": 0, "totalTokens": 12406
        }));
        assert_eq!(
            (u.input, u.output, u.cache_read, u.cache_write),
            (10743, 127, 1536, 0)
        );
        assert_eq!(u.unsplit, 0, "本机 599/599 条分项之和就等于 totalTokens");
        assert_eq!(u.total(), 12406);
        assert_eq!(u.calls, 1);
    }

    /// 供应商把 reasoning 单独记账时，差额进 unsplit，不猜进哪个桶
    #[test]
    fn reasoning_outside_output_lands_in_unsplit() {
        let u = entry_usage(&serde_json::json!({
            "input": 100, "output": 20, "cacheRead": 0, "cacheWrite": 0,
            "reasoning": 30, "totalTokens": 150
        }));
        assert_eq!(u.total(), 150);
        assert_eq!(u.unsplit, 30, "分项之外的 30 不猜，单列");
    }

    /// compaction / branch_summary 自带 usage（生成摘要那次调用），要计入
    #[test]
    fn compaction_usage_is_counted() {
        let f = write(
            "--D--code-acme--",
            "2026-10-09T09-48-22-520Z_01a12014-5077-70de-838d-3f8e8537716b",
            &[
                HEAD,
                r#"{"type":"compaction","id":"k1","parentId":null,"timestamp":"2026-10-09T09:49:00.000Z","summary":"…","tokensBefore":50000,"usage":{"input":900,"output":50,"cacheRead":0,"cacheWrite":0,"totalTokens":950}}"#,
            ],
        );
        let s = parse_session(&f, &[]).unwrap();
        assert_eq!(s.usage.total(), 950);
        assert_eq!(s.usage.calls, 1);
    }

    /// toolResult 里子 agent 的用量会双倍计数（子 agent 有各自的会话文件），必须跳过
    #[test]
    fn subagent_usage_inside_a_tool_result_is_skipped() {
        let f = write(
            "--D--code-acme--",
            "2026-10-09T09-48-22-520Z_01a12015-5077-70de-838d-3f8e8537716b",
            &[
                HEAD,
                r#"{"type":"message","id":"a1","parentId":null,"timestamp":"2026-10-09T09:48:30.000Z","message":{"role":"assistant","content":[{"type":"text","text":"好"}],"model":"step-5-preview","usage":{"input":100,"output":10,"cacheRead":0,"cacheWrite":0,"totalTokens":110}}}"#,
                r#"{"type":"message","id":"t1","parentId":"a1","timestamp":"2026-10-09T09:48:40.000Z","message":{"role":"toolResult","toolCallId":"c1","toolName":"subagent","content":[{"type":"text","text":"done"}],"details":{"results":[{"agent":"general","usage":{"input":9999,"output":999,"cacheRead":0,"cacheWrite":0,"totalTokens":10998}}]},"isError":false}}"#,
            ],
        );
        let s = parse_session(&f, &[]).unwrap();
        assert_eq!(
            s.usage.total(),
            110,
            "子 agent 的 10998 不能进来，否则和折叠的子会话双倍"
        );
    }

    /// 主会话 + 折叠的子 agent：用量相加、subagents 计数
    #[test]
    fn folded_subagents_add_their_usage() {
        let main = write(
            "--D--code-acme--",
            "2026-10-09T09-48-22-520Z_01a12016-5077-70de-838d-3f8e8537716b",
            &[
                HEAD,
                r#"{"type":"message","id":"a1","parentId":null,"timestamp":"2026-10-09T09:48:30.000Z","message":{"role":"user","content":[{"type":"text","text":"主会话"}],"timestamp":1}}"#,
                r#"{"type":"message","id":"a2","parentId":"a1","timestamp":"2026-10-09T09:50:00.000Z","message":{"role":"assistant","content":[{"type":"text","text":"好"}],"model":"step-5-preview","usage":{"input":100,"output":10,"cacheRead":0,"cacheWrite":0,"totalTokens":110}}}"#,
            ],
        );
        let sub = write(
            "--D--code-acme--",
            "2026-10-09T09-49-00-000Z_subagent-11111111-2222-3333-4444-555555555555",
            &[
                r#"{"type":"session","version":3,"id":"subagent-11111111-2222-3333-4444-555555555555","timestamp":"2026-10-09T09:49:00.000Z","cwd":"D:\\code\\acme"}"#,
                r#"{"type":"message","id":"b1","parentId":null,"timestamp":"2026-10-09T09:49:30.000Z","message":{"role":"assistant","content":[{"type":"text","text":"子 agent"}],"model":"step-5-preview","usage":{"input":50,"output":5,"cacheRead":0,"cacheWrite":0,"totalTokens":55}}}"#,
            ],
        );
        let s = parse_session(&main, &[sub]).unwrap();
        assert_eq!(s.usage.total(), 165, "110 + 55");
        assert_eq!(s.subagents, 1);
    }

    /// 时间区间落在主会话内的才算子 agent；区间对不上的单独列出，不硬塞
    #[test]
    fn subagents_fold_by_time_containment_only() {
        let main = write(
            "--D--code-acme--",
            "2026-10-09T09-00-00-000Z_01a12017-5077-70de-838d-3f8e8537716b",
            &[
                r#"{"type":"session","version":3,"id":"01a12017-5077-70de-838d-3f8e8537716b","timestamp":"2026-10-09T09:00:00.000Z","cwd":"D:\\code\\acme"}"#,
                r#"{"type":"message","id":"a1","parentId":null,"timestamp":"2026-10-09T09:30:00.000Z","message":{"role":"user","content":[{"type":"text","text":"x"}],"timestamp":1}}"#,
            ],
        );
        let inside = write(
            "--D--code-acme--",
            "2026-10-09T09-10-00-000Z_subagent-11111111-2222-3333-4444-555555555555",
            &[
                r#"{"type":"session","version":3,"id":"subagent-11111111-2222-3333-4444-555555555555","timestamp":"2026-10-09T09:10:00.000Z","cwd":"D:\\code\\acme"}"#,
                r#"{"type":"message","id":"b1","parentId":null,"timestamp":"2026-10-09T09:20:00.000Z","message":{"role":"user","content":"y"}}"#,
            ],
        );
        // 父会话不在本机：区间完全不重叠
        let orphan = write(
            "--D--code-acme--",
            "2026-10-09T11-10-00-000Z_subagent-99999999-2222-3333-4444-555555555555",
            &[
                r#"{"type":"session","version":3,"id":"subagent-99999999-2222-3333-4444-555555555555","timestamp":"2026-10-09T11:10:00.000Z","cwd":"D:\\code\\acme"}"#,
                r#"{"type":"message","id":"c1","parentId":null,"timestamp":"2026-10-09T11:20:00.000Z","message":{"role":"user","content":"z"}}"#,
            ],
        );
        let (mains, subs) = split_main_and_sub(&[main.clone(), inside.clone(), orphan.clone()]);
        assert_eq!(mains, vec![main.clone()]);
        assert_eq!(subs, vec![inside.clone(), orphan.clone()]);
        let folded = fold_subagents(&mains, &subs);
        assert_eq!(folded[0].1, vec![inside], "区间内的折进去");
        assert_eq!(folded[0].0, main);
        // 硬塞不进去的那条由 list_sessions 单独列出
        let s = parse_session(&orphan, &[]).unwrap();
        assert_eq!(s.subagents, 0);
    }

    /// 并行开多个会话时，取区间最窄的那个父会话
    #[test]
    fn the_tightest_parent_wins() {
        let wide = write(
            "--D--code-acme--",
            "2026-10-09T09-00-00-000Z_01a12018-5077-70de-838d-3f8e8537716b",
            &[
                r#"{"type":"session","version":3,"id":"01a12018-5077-70de-838d-3f8e8537716b","timestamp":"2026-10-09T09:00:00.000Z","cwd":"D:\\code\\acme"}"#,
                r#"{"type":"message","id":"a1","parentId":null,"timestamp":"2026-10-09T09:30:00.000Z","message":{"role":"user","content":"x"}}"#,
            ],
        );
        let tight = write(
            "--D--code-acme--",
            "2026-10-09T09-10-00-000Z_01a12019-5077-70de-838d-3f8e8537716b",
            &[
                r#"{"type":"session","version":3,"id":"01a12019-5077-70de-838d-3f8e8537716b","timestamp":"2026-10-09T09:10:00.000Z","cwd":"D:\\code\\acme"}"#,
                r#"{"type":"message","id":"a1","parentId":null,"timestamp":"2026-10-09T09:25:00.000Z","message":{"role":"user","content":"x"}}"#,
            ],
        );
        let sub = write(
            "--D--code-acme--",
            "2026-10-09T09-15-00-000Z_subagent-11111111-2222-3333-4444-555555555555",
            &[
                r#"{"type":"session","version":3,"id":"subagent-11111111-2222-3333-4444-555555555555","timestamp":"2026-10-09T09:15:00.000Z","cwd":"D:\\code\\acme"}"#,
                r#"{"type":"message","id":"b1","parentId":null,"timestamp":"2026-10-09T09-20:00.000Z","message":{"role":"user","content":"y"}}"#,
            ],
        );
        let folded = fold_subagents(&[wide.clone(), tight.clone()], std::slice::from_ref(&sub));
        assert_eq!(folded[1].1, vec![sub], "09:10–09:25 比 09:00–09:30 更紧");
        assert!(folded[0].1.is_empty());
    }

    /// 文件名和 header 都能认出子 agent
    #[test]
    fn subagent_files_are_recognised() {
        let by_name = write(
            "--D--code-acme--",
            "2026-10-09T09-15-00-000Z_subagent-11111111-2222-3333-4444-555555555555",
            &[
                r#"{"type":"session","version":3,"id":"subagent-11111111-2222-3333-4444-555555555555","timestamp":"2026-10-09T09:15:00.000Z","cwd":"D:\\code\\acme"}"#,
            ],
        );
        let main = write(
            "--D--code-acme--",
            "2026-10-09T09-00-00-000Z_01a1201a-5077-70de-838d-3f8e8537716b",
            &[HEAD],
        );
        assert!(is_subagent_file(&by_name));
        assert!(!is_subagent_file(&main));
    }

    #[test]
    fn iso8601_and_file_id() {
        assert_eq!(
            parse_iso8601_ms("2026-10-09T11:10:10.893Z"),
            Some(1_791_544_210_893)
        );
        assert_eq!(parse_iso8601_ms("not a date"), None);
        let p = Path::new("/x/2026-10-09T11-10-10-893Z_01a1205b-35cd-77c7-93d9-141138d6aea9.jsonl");
        assert_eq!(
            file_id(p).as_deref(),
            Some("01a1205b-35cd-77c7-93d9-141138d6aea9")
        );
        assert_eq!(file_id(Path::new("/x/nosep.jsonl")), None);
    }

    #[test]
    fn injected_context_is_detected() {
        assert!(is_injected("<system-reminder>…"));
        assert!(is_injected("  <user_info>OS</user_info>"));
        assert!(!is_injected("帮我把会话管起来"));
    }
}

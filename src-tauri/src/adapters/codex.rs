//! Codex sessions from `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`
//!
//! 实测（Codex CLI 0.147 → 0.154，79 个 rollout，1.5 GB）：
//! - 首行 `session_meta`：id / cwd / source；`source.subagent` 为子 agent（如 guardian 自动审查），
//!   `parent_thread_id` 指向父会话 → 并入父会话，不单列
//! - 同一 id 可能有多个 rollout（恢复会话新开文件）→ 按 id 合并
//! - 标题在 `~/.codex/session_index.jsonl` 的 `thread_name`，没有则取首条用户输入
//!
//! token 有两本账，取并集而不相加：
//! 1. `token_usage_record`（新版才有）：每个响应一条，按 `response_id` 去重，覆盖上下文压缩调用
//! 2. `event_msg/token_count`：`total_token_usage` 是**进程内**累计，恢复会话后清零（9 个文件实测），
//!    不能取最后一条；按"与上一条 total 不同"去重后累加 `last_token_usage`
//!    （10453 次相邻记录中 99% 满足 total = 上一条 + last；非累计值的 108 条 total 不变，跳过）
//! 规则：记录 1 出现后的时间段用 1，之前的时间段用 2。实测 12 个同时有两本账的文件里
//! 8 个完全一致，其余 4 个账本 1 恰好多 3 次调用 = 文件里的 3 次上下文压缩，账本 2 漏记。
//!
//! 字段语义（OpenAI 风格，input 含缓存命中）：
//! input = input_tokens − cached_input_tokens；cache_read = cached_input_tokens；
//! output = output_tokens（已含 reasoning_output_tokens）。
//! 0.147 alpha 导入的会话只有 total_tokens、分项全 0 → 记为 unsplit，不猜测拆分。

use super::{
    contains, file_sig, for_each_line, format_tokens, storage_absent, system_time_ms, truncate,
    HarnessStorage, SessionSummary, TokenUsage,
};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

fn codex_home() -> Option<PathBuf> {
    let home = super::codex_home()?;
    home.join("sessions").is_dir().then_some(home)
}

/// 单个 rollout 文件的解析结果（按文件缓存）
#[derive(Clone, Default)]
struct Rollout {
    id: String,
    parent: Option<String>,
    is_subagent: bool,
    cwd: String,
    model: Option<String>,
    first_user: Option<String>,
    usage: TokenUsage,
}

pub fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    let Some(home) = codex_home() else {
        return Ok(vec![]);
    };
    let titles = thread_names(&home);

    struct Group {
        usage: TokenUsage,
        bytes: u64,
        mtime_ms: u64,
        path: PathBuf,
        subagents: u32,
        first_user: Option<String>,
        model: Option<String>,
        cwd: String,
    }
    let mut groups: HashMap<String, Group> = HashMap::new();
    let mut children: Vec<(Rollout, u64, u64)> = vec![];

    for path in collect_rollouts(&home.join("sessions")) {
        let Ok(meta) = fs::metadata(&path) else { continue };
        let bytes = meta.len();
        let mtime_ms = meta.modified().map(system_time_ms).unwrap_or(0);
        let Some(r) = parse_cached(&path) else { continue };
        if r.is_subagent && r.parent.is_some() {
            children.push((r, bytes, mtime_ms));
            continue;
        }
        let g = groups.entry(r.id.clone()).or_insert_with(|| Group {
            usage: TokenUsage::default(),
            bytes: 0,
            mtime_ms: 0,
            path: path.clone(),
            subagents: 0,
            first_user: None,
            model: None,
            cwd: String::new(),
        });
        g.usage.add(&r.usage);
        g.bytes += bytes;
        if mtime_ms >= g.mtime_ms {
            g.mtime_ms = mtime_ms;
            g.path = path.clone();
        }
        if g.first_user.is_none() {
            g.first_user = r.first_user.clone();
        }
        if g.model.is_none() {
            g.model = r.model.clone();
        }
        if g.cwd.is_empty() {
            g.cwd = r.cwd.clone();
        }
    }

    // 子 agent 并入父会话；父会话不在本机的就单独列出
    for (r, bytes, mtime_ms) in children {
        let parent = r.parent.clone().unwrap_or_default();
        if let Some(g) = groups.get_mut(&parent) {
            g.usage.add(&r.usage);
            g.bytes += bytes;
            g.subagents += 1;
            g.mtime_ms = g.mtime_ms.max(mtime_ms);
        } else {
            let id = r.id.clone();
            groups.insert(
                id,
                Group {
                    usage: r.usage,
                    bytes,
                    mtime_ms,
                    path: PathBuf::new(),
                    subagents: 0,
                    first_user: r.first_user.clone(),
                    model: r.model.clone(),
                    cwd: r.cwd.clone(),
                },
            );
        }
    }

    Ok(groups
        .into_iter()
        .map(|(id, g)| {
            let title = titles
                .get(&id)
                .map(|t| truncate(t, 48))
                .or_else(|| g.first_user.as_deref().map(|t| truncate(t, 48)))
                .unwrap_or_default();
            SessionSummary {
                harness: "codex".into(),
                title,
                project: g.cwd.replace('\\', "/"),
                model: g.model.unwrap_or_else(|| "—".into()),
                status: "idle".into(),
                updated_ms: g.mtime_ms,
                tokens: format_tokens(g.usage.total()),
                usage: g.usage,
                excerpt: g.first_user.map(|t| truncate(&t, 120)).unwrap_or_default(),
                path: g.path.to_string_lossy().to_string(),
                log: vec![],
                size_bytes: g.bytes,
                subagents: g.subagents,
                id,
            }
        })
        .collect())
}

pub fn storage() -> HarnessStorage {
    let Some(home) = codex_home() else {
        return storage_absent("codex", "~/.codex/sessions/");
    };
    let root = home.join("sessions");
    // 与 list_sessions 同口径：主会话按 id 去重，父会话不在本机的子 agent 单独算一个
    let mut ids = std::collections::HashSet::new();
    let mut subs: Vec<(String, Option<String>)> = vec![];
    let mut bytes = 0;
    for path in collect_rollouts(&root) {
        bytes += fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        // 只读首行判断是否子 agent，不做全量解析
        if let Some(head) = read_head(&path) {
            match head {
                (id, true, parent @ Some(_)) => subs.push((id, parent)),
                (id, _, _) => {
                    ids.insert(id);
                }
            }
        }
    }
    let orphans: std::collections::HashSet<_> = subs
        .into_iter()
        .filter(|(_, parent)| !parent.as_ref().is_some_and(|p| ids.contains(p)))
        .map(|(id, _)| id)
        .collect();
    let session_count = ids.len() + orphans.iter().filter(|id| !ids.contains(*id)).count();
    HarnessStorage {
        harness: "codex".into(),
        connected: true,
        sessions: session_count as u32,
        session_bytes: bytes,
        root_bytes: super::dir_size(&root),
        root: "~/.codex/sessions/".into(),
    }
}

pub(crate) fn collect_rollouts(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, acc: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => walk(&p, acc),
                Ok(t) if t.is_file() => {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                        acc.push(p);
                    }
                }
                _ => {}
            }
        }
    }
    let mut acc = vec![];
    walk(root, &mut acc);
    acc
}

/// id → 最新的 thread_name（后写覆盖前写）
fn thread_names(home: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(file) = fs::File::open(home.join("session_index.jsonl")) else {
        return map;
    };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if let (Some(id), Some(name)) = (
            v.get("id").and_then(|x| x.as_str()),
            v.get("thread_name").and_then(|x| x.as_str()),
        ) {
            if !name.trim().is_empty() {
                map.insert(id.to_string(), name.trim().to_string());
            }
        }
    }
    map
}

/// (id, 是否子 agent, 父会话 id)
pub(crate) fn read_head(path: &Path) -> Option<(String, bool, Option<String>)> {
    let file = fs::File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(file).read_line(&mut line).ok()?;
    let v: serde_json::Value = serde_json::from_str(&line).ok()?;
    let p = v.get("payload")?;
    let id = p.get("id")?.as_str()?.to_string();
    let is_sub = p.pointer("/source/subagent").is_some();
    let parent = p.get("parent_thread_id").and_then(|x| x.as_str()).map(String::from);
    Some((id, is_sub, parent))
}

/* ── 按文件缓存：签名不变不重读 ── */

type RolloutMemo = HashMap<PathBuf, (u64, Rollout)>;

fn parse_cached(path: &Path) -> Option<Rollout> {
    static MEMO: OnceLock<Mutex<RolloutMemo>> = OnceLock::new();
    let memo = MEMO.get_or_init(|| Mutex::new(HashMap::new()));
    let sig = file_sig(&[path.to_path_buf()]);
    if let Ok(map) = memo.lock() {
        if let Some((old, r)) = map.get(path) {
            if *old == sig {
                return Some(r.clone());
            }
        }
    }
    let r = parse_rollout(path)?;
    if let Ok(mut map) = memo.lock() {
        map.insert(path.to_path_buf(), (sig, r.clone()));
    }
    Some(r)
}

/// 真正由用户输入的文本；跳过 Codex 注入的上下文（`<environment_context>`、`# AGENTS.md instructions` 等）
fn user_text(payload: &serde_json::Value) -> Option<String> {
    let is_injected = |t: &str| t.starts_with('<') || t.starts_with("# AGENTS.md");
    let pick = |t: &str| Some(t.trim()).filter(|t| !t.is_empty() && !is_injected(t)).map(String::from);
    match payload.get("type").and_then(|t| t.as_str()) {
        Some("user_message") => payload.get("message").and_then(|m| m.as_str()).and_then(pick),
        Some("message") if payload.get("role").and_then(|r| r.as_str()) == Some("user") => payload
            .get("content")?
            .as_array()?
            .iter()
            .filter(|c| c.get("type").and_then(|t| t.as_str()) == Some("input_text"))
            .find_map(|c| c.get("text").and_then(|t| t.as_str()).and_then(pick)),
        _ => None,
    }
}

/// OpenAI 风格 usage → 互不重叠的 TokenUsage（见模块注释）
fn to_usage(u: &serde_json::Value) -> TokenUsage {
    let n = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    let (input, cached, output, total) = (
        n("input_tokens"),
        n("cached_input_tokens"),
        n("output_tokens"),
        n("total_tokens"),
    );
    if input == 0 && output == 0 && total > 0 {
        return TokenUsage { unsplit: total, calls: 1, ..Default::default() };
    }
    TokenUsage {
        input: input.saturating_sub(cached),
        cache_read: cached,
        cache_write: n("cache_write_input_tokens"),
        output,
        unsplit: 0,
        calls: 1,
    }
}

fn parse_rollout(path: &Path) -> Option<Rollout> {
    let file = fs::File::open(path).ok()?;
    let mut r = Rollout::default();
    let mut first = true;

    // 账本 1：response_id → (timestamp, usage)
    let mut records: HashMap<String, (String, TokenUsage)> = HashMap::new();
    // 账本 2：去重后的 (timestamp, last usage)
    let mut counts: Vec<(String, TokenUsage)> = vec![];
    let mut prev_total: Option<u64> = None;

    for_each_line(file, |line| {
        if first {
            first = false;
            if contains(line, b"\"session_meta\"") {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) {
                    if let Some(p) = v.get("payload") {
                        r.id = p.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        r.cwd = p.get("cwd").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        r.is_subagent = p.pointer("/source/subagent").is_some();
                        r.parent = p
                            .get("parent_thread_id")
                            .and_then(|x| x.as_str())
                            .map(String::from);
                    }
                }
            }
            return true;
        }

        let is_record = contains(line, b"\"token_usage_record\"");
        let is_count = !is_record && contains(line, b"\"token_count\"");
        let want_model = r.model.is_none() && contains(line, b"\"turn_context\"");
        // 用户输入两种形态：新版 response_item(message, role=user)，旧版 event_msg(user_message)
        let want_user = r.first_user.is_none()
            && (contains(line, b"\"user_message\"") || contains(line, b"\"role\":\"user\""));
        if !(is_record || is_count || want_model || want_user) {
            return true;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
            return true;
        };
        let ts = v.get("timestamp").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let payload = v.get("payload");

        if is_record {
            if let (Some(rid), Some(u)) = (
                payload.and_then(|p| p.get("response_id")).and_then(|x| x.as_str()),
                payload.and_then(|p| p.get("usage")),
            ) {
                records.insert(rid.to_string(), (ts, to_usage(u)));
            }
        } else if is_count {
            if let Some(info) = payload.and_then(|p| p.get("info")).filter(|i| !i.is_null()) {
                let total = info
                    .pointer("/total_token_usage/total_tokens")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                // 与上一条累计值相同 = 重复快照，不是新调用
                if prev_total != Some(total) {
                    prev_total = Some(total);
                    if let Some(last) = info.get("last_token_usage") {
                        counts.push((ts, to_usage(last)));
                    }
                }
            }
        } else if want_model && v.get("type").and_then(|t| t.as_str()) == Some("turn_context") {
            r.model = payload
                .and_then(|p| p.get("model"))
                .and_then(|m| m.as_str())
                .filter(|m| !m.is_empty())
                .map(String::from);
        } else if want_user {
            r.first_user = payload.and_then(user_text);
        }
        true
    });

    if r.id.is_empty() {
        return None;
    }

    // 账本 1 覆盖的时间段以它为准，之前的时间段用账本 2
    let records_from = records.values().map(|(ts, _)| ts.as_str()).min().map(String::from);
    for (_, u) in records.values() {
        r.usage.add(u);
    }
    for (ts, u) in &counts {
        if records_from.as_deref().map_or(true, |from| ts.as_str() < from) {
            r.usage.add(u);
        }
    }
    Some(r)
}

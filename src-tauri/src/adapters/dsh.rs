//! DSH (DeepSeek) sessions from `~/.dsh/sessions/<workspace>/session-<uuid>/`
//!
//! 目录布局（@deepseek-ai/dsh 0.1.5 实测）：
//! - `session.v3.jsonl.zstd`   当前格式（事件日志，每次追加一个 zstd 帧）
//! - `session.jsonl.zstd`      旧格式；升级后会迁移成 v3 并继续写 v3
//!   → 两者并存时只读 v3，否则会重复计数（实测 v3 合计与 DSH 自己的
//!   `storages/session_projcache` 完全一致，v0 是迁移前的旧副本）
//! - `storages/workspace.json` 归档列表 `global.archivedSessionIds`
//!
//! 关键事件：
//! - `session`            首行：cwd / createdAt
//! - `session/title`      标题，后写覆盖前写（fallback → provider）
//! - `model/selection`    所选模型
//! - `user/message`       用户输入（取首条文本作摘要）
//! - `assistant/message`  `data.usage` 每步一条：inputTokens（已不含缓存）/ outputTokens（含推理）
//!   / cacheReadTokens / cacheWriteTokens；实测 totalTokens = inputTokens + outputTokens
//!   + cacheReadTokens
//!
//! `storages/session_projcache` 是 DSH 的投影缓存，会落后于日志（实测落后 7 条事件），
//! 所以以日志为准，缓存只在验证时对照。

use super::{
    contains, dir_size, file_sig, for_each_line, format_tokens, memoized, storage_absent,
    system_time_ms, truncate, HarnessStorage, SessionSummary, TokenUsage,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const LOG_V3: &str = "session.v3.jsonl.zstd";
const LOG_V0: &str = "session.jsonl.zstd";

fn dsh_home() -> Option<PathBuf> {
    let home = super::dsh_home()?;
    home.join("sessions").is_dir().then_some(home)
}

pub fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    let Some(home) = dsh_home() else {
        return Ok(vec![]);
    };
    let archived = archived_ids(&home);
    let mut out = vec![];
    for (dir, log) in collect_sessions(&home.join("sessions")) {
        let sig = file_sig(std::slice::from_ref(&log));
        let Some(mut s) = memoized(&log, sig, || parse_session(&dir, &log)) else {
            continue;
        };
        if archived.contains(&s.id) {
            s.status = "done".into();
        }
        s.size_bytes = session_bytes(&home, &dir);
        out.push(s);
    }
    Ok(out)
}

pub fn storage() -> HarnessStorage {
    let Some(home) = dsh_home() else {
        return storage_absent("dsh", "~/.dsh/sessions/");
    };
    let root = home.join("sessions");
    let sessions = collect_sessions(&root);
    HarnessStorage {
        harness: "dsh".into(),
        connected: true,
        sessions: sessions.len() as u32,
        session_bytes: sessions.iter().map(|(dir, _)| session_bytes(&home, dir)).sum(),
        root_bytes: dir_size(&root),
        root: "~/.dsh/sessions/".into(),
    }
}

/// 会话目录 + `storages/session_projcache/sessions/<id>.json`，与删除时移除的范围一致
fn session_bytes(home: &Path, dir: &Path) -> u64 {
    let cache = dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(|id| home.join("storages").join("session_projcache").join("sessions").join(format!("{id}.json")))
        .and_then(|p| fs::metadata(p).ok())
        .map_or(0, |m| m.len());
    dir_size(dir) + cache
}

/// (会话目录, 要读的日志)；v3 优先
fn collect_sessions(root: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut acc = vec![];
    let Ok(workspaces) = fs::read_dir(root) else { return acc };
    for ws in workspaces.flatten() {
        let Ok(entries) = fs::read_dir(ws.path()) else { continue };
        for entry in entries.flatten() {
            let dir = entry.path();
            let is_session = dir
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("session-"));
            if !is_session {
                continue;
            }
            let log = [LOG_V3, LOG_V0].iter().map(|f| dir.join(f)).find(|p| p.is_file());
            if let Some(log) = log {
                acc.push((dir, log));
            }
        }
    }
    acc
}

fn archived_ids(home: &Path) -> HashSet<String> {
    fs::read_to_string(home.join("storages").join("workspace.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| {
            v.pointer("/global/archivedSessionIds")?.as_array().map(|a| {
                a.iter().filter_map(|x| x.as_str().map(String::from)).collect()
            })
        })
        .unwrap_or_default()
}

fn parse_session(dir: &Path, log: &Path) -> Option<SessionSummary> {
    let id = dir.file_name()?.to_str()?.to_string();
    let modified = fs::metadata(log).ok()?.modified().ok()?;
    let file = fs::File::open(log).ok()?;
    let decoder = zstd::stream::read::Decoder::new(file).ok()?;

    let mut usage = TokenUsage::default();
    let mut cwd: Option<String> = None;
    let mut title: Option<String> = None;
    let mut excerpt: Option<String> = None;
    let mut model: Option<String> = None;
    let mut last_time: u64 = 0;

    for_each_line(decoder, |line| {
        let is_usage = contains(line, b"\"assistant/message\"") && contains(line, b"\"usage\"");
        let is_title = contains(line, b"\"session/title\"");
        let is_model = contains(line, b"\"model/selection\"");
        let is_head = cwd.is_none() && contains(line, b"\"type\":\"session\"");
        let is_user = excerpt.is_none() && contains(line, b"\"user/message\"");
        if !(is_usage || is_title || is_model || is_head || is_user) {
            return true;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
            return true;
        };
        if let Some(t) = v.get("time").and_then(|t| t.as_u64()) {
            last_time = last_time.max(t);
        }
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let data = v.get("data");
        match ty {
            "session" => {
                cwd = v.get("cwd").and_then(|c| c.as_str()).map(|c| c.replace('\\', "/"));
            }
            "session/title" => {
                if let Some(t) = data.and_then(|d| d.get("title")).and_then(|t| t.as_str()) {
                    if !t.trim().is_empty() {
                        title = Some(truncate(t.trim(), 48));
                    }
                }
            }
            "model/selection" => {
                if let Some(m) = data.and_then(|d| d.get("model")).and_then(|m| m.as_str()) {
                    model = Some(m.to_string());
                }
            }
            "user/message" => {
                let text = data
                    .and_then(|d| d.get("content"))
                    .and_then(|c| c.as_array())
                    .and_then(|parts| {
                        parts.iter().find_map(|p| {
                            (p.get("type").and_then(|t| t.as_str()) == Some("text"))
                                .then(|| p.get("text").and_then(|t| t.as_str()))
                                .flatten()
                        })
                    });
                if let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) {
                    excerpt = Some(truncate(text, 120));
                }
            }
            "assistant/message" => {
                if let Some(u) = data.and_then(|d| d.get("usage")) {
                    let n = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
                    usage.add(&TokenUsage {
                        input: n("inputTokens"),
                        cache_write: n("cacheWriteTokens"),
                        cache_read: n("cacheReadTokens"),
                        output: n("outputTokens"),
                        unsplit: 0,
                        calls: 1,
                    });
                }
                if model.is_none() {
                    if let Some(m) = data
                        .and_then(|d| d.pointer("/message/source/model"))
                        .and_then(|m| m.as_str())
                    {
                        model = Some(m.to_string());
                    }
                }
            }
            _ => {}
        }
        true
    });

    Some(SessionSummary {
        id,
        harness: "dsh".into(),
        // 空标题留给前端按界面语言兜底
        title: title.unwrap_or_default(),
        project: cwd.unwrap_or_default(),
        model: model.unwrap_or_else(|| "—".into()),
        status: "idle".into(),
        updated_ms: if last_time > 0 { last_time } else { system_time_ms(modified) },
        tokens: format_tokens(usage.total()),
        usage,
        excerpt: excerpt.unwrap_or_default(),
        path: dir.to_string_lossy().to_string(),
        log: vec![],
        size_bytes: 0,
        subagents: 0,
        kind: String::new(),
    })
}

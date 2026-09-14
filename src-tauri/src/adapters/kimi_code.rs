//! Kimi Code sessions from `~/.kimi-code/sessions/<workspace>/session_<uuid>/`
//!
//! 目录布局（Kimi Code 0.42 实测）：
//! - `state.json`                  id / cwd（旧版为 workDir）/ title / createdAt / updatedAt / agents
//! - `agents/main/wire.jsonl`      主 agent 事件流
//! - `agents/agent-N/wire.jsonl`   子 agent 事件流（state.json 里 type = "sub"）
//! - `media/` `logs/` `notify/`    附属文件，计入磁盘占用
//!
//! token：每条 `usage.record` 事件就是一次 API 调用，无流式重复。
//! `usageScope` 实测只有 `turn`（常规步骤）与 `session`（上下文压缩调用），
//! 后者是独立请求而非汇总，两者都计入。
//!
//! 隐私：`state.json.lastPrompt` 可能含用户粘贴的密钥，不读取、不展示。

use super::{
    dir_size, file_sig, format_tokens, memoized, truncate, HarnessStorage, SessionSummary,
    TokenUsage,
};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

fn sessions_root() -> Option<PathBuf> {
    let root = dirs::home_dir()?.join(".kimi-code").join("sessions");
    root.is_dir().then_some(root)
}

pub fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    let Some(root) = sessions_root() else {
        return Ok(vec![]);
    };
    let mut out = vec![];
    for dir in collect_session_dirs(&root) {
        let wires = wire_files(&dir);
        let mut sig_files = vec![dir.join("state.json")];
        sig_files.extend(wires.iter().cloned());
        let sig = file_sig(&sig_files);

        let Some(mut s) = memoized(&dir, sig, || parse_session(&dir, &wires)) else {
            continue;
        };
        s.size_bytes = dir_size(&dir);
        out.push(s);
    }
    Ok(out)
}

pub fn storage() -> HarnessStorage {
    let mut st = HarnessStorage {
        harness: "kimi".into(),
        connected: true,
        sessions: 0,
        session_bytes: 0,
        root_bytes: 0,
        root: "~/.kimi-code/sessions/".into(),
    };
    let Some(root) = sessions_root() else {
        st.connected = false;
        return st;
    };
    for dir in collect_session_dirs(&root) {
        st.sessions += 1;
        st.session_bytes += dir_size(&dir);
    }
    st.root_bytes = dir_size(&root);
    st
}

/// `<workspace>/session_*/`，且必须有 state.json
fn collect_session_dirs(root: &Path) -> Vec<PathBuf> {
    let mut acc = vec![];
    let Ok(workspaces) = fs::read_dir(root) else { return acc };
    for ws in workspaces.flatten() {
        let Ok(entries) = fs::read_dir(ws.path()) else { continue };
        for entry in entries.flatten() {
            let dir = entry.path();
            let is_session = dir
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("session_"));
            if is_session && dir.join("state.json").is_file() {
                acc.push(dir);
            }
        }
    }
    acc
}

/// main 在前，其余 agent 按名排序
fn wire_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir.join("agents")) else {
        return vec![];
    };
    let mut agents: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path().join("wire.jsonl"))
        .filter(|p| p.is_file())
        .collect();
    agents.sort_by_key(|p| {
        let name = p
            .parent()
            .and_then(|a| a.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        (name != "main", name)
    });
    agents
}

fn parse_session(dir: &Path, wires: &[PathBuf]) -> Option<SessionSummary> {
    let raw = fs::read_to_string(dir.join("state.json")).ok()?;
    let state: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let str_of = |k: &str| state.get(k).and_then(|v| v.as_str()).unwrap_or("").trim();

    let id = match str_of("id") {
        "" => dir.file_name()?.to_str()?.to_string(),
        s => s.to_string(),
    };
    // 空标题留给前端按界面语言兜底（"Session {id}" / "会话 {id}" / "セッション {id}"）
    let title_raw = str_of("title");
    let title = truncate(title_raw, 48);
    let excerpt = truncate(title_raw, 120);
    let updated_ms = state
        .get("updatedAt")
        .or_else(|| state.get("createdAt"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let subagents = state
        .get("agents")
        .and_then(|a| a.as_object())
        .map(|m| {
            m.values()
                .filter(|a| a.get("type").and_then(|t| t.as_str()) == Some("sub"))
                .count() as u32
        })
        .unwrap_or(0);

    let mut usage = TokenUsage::default();
    let mut model: Option<String> = None;
    for wire in wires {
        scan_wire(wire, &mut usage, &mut model);
    }

    Some(SessionSummary {
        id,
        harness: "kimi".into(),
        title,
        project: project_dir(dir, &state),
        model: model.unwrap_or_else(|| "—".into()),
        status: if state.get("archived").and_then(|v| v.as_bool()) == Some(true) {
            "done".into()
        } else {
            "idle".into()
        },
        updated_ms,
        tokens: format_tokens(usage.total()),
        usage,
        excerpt,
        path: dir.to_string_lossy().to_string(),
        log: vec![],
        size_bytes: 0,
        subagents,
    })
}

/// 工作目录：新版 state.json 用 `cwd`，旧版用 `workDir`（本机 91 个会话中 71 个是旧版）；
/// 都没有时按所属 workspace 查 `~/.kimi-code/workspaces.json`
fn project_dir(dir: &Path, state: &serde_json::Value) -> String {
    for key in ["cwd", "workDir"] {
        if let Some(s) = state.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            return s.replace('\\', "/");
        }
    }
    let workspace = dir.parent().and_then(|w| w.file_name()).and_then(|n| n.to_str());
    let registry = dirs::home_dir().map(|h| h.join(".kimi-code").join("workspaces.json"));
    if let (Some(ws), Some(reg)) = (workspace, registry) {
        if let Some(root) = fs::read_to_string(reg)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|v| v.pointer(&format!("/workspaces/{ws}/root"))?.as_str().map(String::from))
        {
            return root.replace('\\', "/");
        }
    }
    String::new()
}

fn scan_wire(path: &Path, usage: &mut TokenUsage, model: &mut Option<String>) {
    let Ok(file) = fs::File::open(path) else { return };
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        let is_usage = line.contains("\"type\":\"usage.record\"");
        let need_model = model.is_none() && line.contains("\"type\":\"llm.request\"");
        if !(is_usage || need_model) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if need_model {
            if let Some(m) = v.get("model").and_then(|m| m.as_str()) {
                *model = Some(m.to_string());
            }
        }
        if is_usage {
            let Some(u) = v.get("usage") else { continue };
            let n = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
            usage.add(&TokenUsage {
                input: n("inputOther"),
                cache_write: n("inputCacheCreation"),
                cache_read: n("inputCacheRead"),
                output: n("output"),
                calls: 1,
            });
        }
    }
}

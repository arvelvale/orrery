//! Claude Code sessions from `~/.claude/projects/<project>/<id>.jsonl`
//!
//! 目录布局：
//! - `<project>/<id>.jsonl`            主会话
//! - `<project>/<id>/subagents/*.jsonl` 子 agent 会话（并入主会话，不单列）
//! - `<project>/<id>/tool-results/*`    工具输出
//! - `<project>/memory/`               记忆文件（不属于任何会话）
//!
//! token：流式输出时同一条 assistant 消息按内容块拆成多行，每行都带同一份 usage
//! （实测 4034 行 usage 仅 1558 个 message.id）。按 message.id 去重，保留最后一行
//! （流式过程中 output_tokens 会递增）。

use super::{
    dir_size, file_sig, format_tokens, memoized, system_time_ms, truncate, HarnessStorage,
    SessionSummary, TokenUsage,
};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

fn projects_root() -> Option<PathBuf> {
    let root = super::claude_home()?.join("projects");
    root.is_dir().then_some(root)
}

pub fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    let Some(root) = projects_root() else {
        return Ok(vec![]);
    };
    let mut out = vec![];
    for path in collect_session_files(&root) {
        let subagent_files = subagent_files(&path);
        let mut sig_files = vec![path.clone()];
        sig_files.extend(subagent_files.iter().cloned());
        let sig = file_sig(&sig_files);

        let Some(mut s) = memoized(&path, sig, || parse_session(&path, &subagent_files)) else {
            continue;
        };
        s.size_bytes = session_bytes(&path);
        out.push(s);
    }
    Ok(out)
}

pub fn storage() -> HarnessStorage {
    let mut st = HarnessStorage {
        harness: "cc".into(),
        connected: true,
        sessions: 0,
        session_bytes: 0,
        root_bytes: 0,
        root: "~/.claude/projects/".into(),
    };
    let Some(root) = projects_root() else {
        return st;
    };
    for path in collect_session_files(&root) {
        st.sessions += 1;
        st.session_bytes += session_bytes(&path);
    }
    st.root_bytes = dir_size(&root);
    st
}

/// 只收集 `<project>/*.jsonl` 这一层，子 agent 的 jsonl 不算独立会话
fn collect_session_files(root: &Path) -> Vec<PathBuf> {
    let mut acc = vec![];
    let Ok(projects) = fs::read_dir(root) else { return acc };
    for project in projects.flatten() {
        let dir = project.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                acc.push(path);
            }
        }
    }
    acc
}

fn subagent_files(path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(path.with_extension("").join("subagents")) else {
        return vec![];
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl"))
        .collect();
    files.sort();
    files
}

/// 主文件 + 同名附属目录 + `~/.claude` 下按会话 id 命名的目录（file-history / session-env / tasks）。
/// 与删除时实际移除的范围一致，否则删除后"释放量"会大于列表显示的占用
fn session_bytes(path: &Path) -> u64 {
    let main = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let companion = path.with_extension("");
    let mut total = main + if companion.is_dir() { dir_size(&companion) } else { 0 };
    let id = path.file_stem().and_then(|s| s.to_str());
    // path = <claude>/projects/<project>/<id>.jsonl
    let claude = path.parent().and_then(Path::parent).and_then(Path::parent);
    if let (Some(id), Some(claude)) = (id, claude) {
        for extra in ["file-history", "session-env", "tasks"] {
            let p = claude.join(extra).join(id);
            if p.is_dir() {
                total += dir_size(&p);
            }
        }
    }
    total
}

fn parse_session(path: &Path, subagent_files: &[PathBuf]) -> Option<SessionSummary> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let id = path.file_stem()?.to_str()?.to_string();
    let project_folder = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");

    let main = scan_jsonl(path, true);
    let mut usage = main.usage;
    for f in subagent_files {
        usage.add(&scan_jsonl(f, false).usage);
    }

    // 空标题留给前端按界面语言兜底
    let title = main.title.clone().unwrap_or_default();
    let excerpt = main.excerpt.unwrap_or_default();
    // 目录名编码有损（`:` `\` `-` 都变成 `-`），优先用会话里记录的 cwd
    let project = main
        .cwd
        .unwrap_or_else(|| decode_claude_project_dir(project_folder));

    Some(SessionSummary {
        id,
        harness: "cc".into(),
        title,
        project,
        model: main.model.unwrap_or_else(|| "—".into()),
        status: "idle".into(),
        updated_ms: system_time_ms(modified),
        tokens: format_tokens(usage.total()),
        usage,
        excerpt,
        path: path.to_string_lossy().to_string(),
        log: vec![],
        size_bytes: 0,
        subagents: subagent_files.len() as u32,
        kind: String::new(),
    })
}

#[derive(Default)]
struct Scan {
    title: Option<String>,
    excerpt: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    usage: TokenUsage,
}

/// 逐行读；先用子串预筛再做 JSON 解析——体积大头是附件/工具输出行，直接跳过
fn scan_jsonl(path: &Path, want_meta: bool) -> Scan {
    let mut scan = Scan::default();
    let Ok(file) = fs::File::open(path) else { return scan };
    // message.id → 该调用最后一次出现的 usage
    let mut by_msg: HashMap<String, TokenUsage> = HashMap::new();

    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };

        let has_usage = line.contains("\"usage\"") && line.contains("\"assistant\"");
        let need_cwd = want_meta && scan.cwd.is_none() && line.contains("\"cwd\"");
        let need_title = want_meta && scan.title.is_none() && line.contains("\"type\":\"user\"");
        if !(has_usage || need_cwd || need_title) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };

        if need_cwd {
            if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
                scan.cwd = Some(cwd.replace('\\', "/"));
            }
        }

        if need_title && v.get("type").and_then(|t| t.as_str()) == Some("user") {
            if let Some(text) = extract_text(&v) {
                let text = text.trim();
                if !text.is_empty() && !text.starts_with('<') {
                    scan.title = Some(truncate(text, 48));
                    scan.excerpt = Some(truncate(text, 120));
                }
            }
        }

        if has_usage {
            let Some(msg) = v.get("message") else { continue };
            if want_meta && scan.model.is_none() {
                if let Some(m) = msg.get("model").and_then(|m| m.as_str()) {
                    if m != "<synthetic>" {
                        scan.model = Some(m.to_string());
                    }
                }
            }
            let (Some(id), Some(u)) = (msg.get("id").and_then(|i| i.as_str()), msg.get("usage"))
            else {
                continue;
            };
            let n = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
            by_msg.insert(
                id.to_string(),
                TokenUsage {
                    input: n("input_tokens"),
                    cache_write: n("cache_creation_input_tokens"),
                    cache_read: n("cache_read_input_tokens"),
                    output: n("output_tokens"),
                    unsplit: 0,
                    calls: 1,
                },
            );
        }
    }

    for u in by_msg.values() {
        scan.usage.add(u);
    }
    scan
}

fn extract_text(v: &serde_json::Value) -> Option<String> {
    let content = v.pointer("/message/content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = content.as_array() {
        let mut parts = vec![];
        for item in arr {
            if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    parts.push(t.to_string());
                }
            }
        }
        if !parts.is_empty() {
            return Some(parts.join("\n"));
        }
    }
    None
}

/// Claude 将项目路径编码为目录名：`/` `:` `\` 等变为 `-`（有损，仅作兜底）
fn decode_claude_project_dir(dir: &str) -> String {
    // 启发式：Windows 下 `D:\code\my-app` 编码为 `D--code-my-app`
    let s = dir.replace('-', "/");
    // 恢复盘符形态： /D/ → D:/
    let bytes: Vec<char> = s.chars().collect();
    if bytes.len() >= 3 && bytes[0] == '/' && bytes[2] == '/' && bytes[1].is_ascii_alphabetic() {
        return format!("{}:/{}", bytes[1], s[3..].trim_start_matches('/'));
    }
    s
}

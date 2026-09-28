//! WorkBuddy sessions from `~/.workbuddy/projects/<work-dir>/<sessionId>.jsonl`
//!
//! 布局（2026-09-27 本机实测）。落盘位置和 Claude Code 的 `projects/<slug>/<id>.jsonl` 同构，
//! 但 item 形状是另一套，所以单独一个 adapter：
//! - `~/.workbuddy/projects/<work-dir>/<sessionId>.jsonl`   一行一个 item，追加写入
//! - item 的 `type`：`session-meta` / `message` / `function_call` / `function_call_result`
//!   / `reasoning` / `file-history-snapshot` / `ai-title`
//! - 每个 item 自带 `id`（UUIDv7）、`parentId`、`sessionId`、`timestamp`；
//!   多数 item 还带 `cwd`，所以项目路径**直接可读**，不用像 Claude Code 那样去解有损目录名
//! - `ai-title` 一条会话只出现一次，`aiTitle` 就是会话标题
//! - 用量挂在 `message.usage`（`input_tokens`/`output_tokens`/`total_tokens`，
//!   有时有 `cache_read_input_tokens` / `cache_creation_input_tokens`），
//!   原始记录在 `providerData.rawUsage`（`prompt_tokens`/`completion_tokens`/…）
//! - 正在运行的会话登记在 `~/.workbuddy/sessions/<pid>.json`（pid / sessionId / heartbeat），
//!   与 Claude Code 的 `~/.claude/sessions/<pid>.json` 同一套路
//!
//! token 口径：
//! - **按 `providerData.messageId` 去重**：一次请求会拆成 message / function_call /
//!   reasoning 多个 item，它们共用同一个 messageId 和同一份用量（实测 33 条带用量的 item
//!   对应 33 个不同的 messageId，正常不重复；去重只是防流式重写）
//! - `output` 含 `reasoning_tokens`（在 `completion_tokens_details` 里），与 Codex 同口径
//! - 分项之和与 `total` 对不上时，差额进 `unsplit` 而不是猜拆分比例；
//!   分项加起来反而超过 `total` 则丢掉缓存桶（说明缓存已含在 input，OpenAI 口径）
//!
//! 删除：暂不开放。WorkBuddy 是图形应用，会话由它自己写，Orrery 没有验证过删掉
//! jsonl 后它的历史列表会变成什么样。恢复也不做：WorkBuddy 是 Electron 应用，
//! 没有可供终端调用的 CLI。

use super::cleanup::pid_alive;
use super::{
    file_sig, format_tokens, memoized, truncate, HarnessStorage, SessionSummary, TokenUsage,
};
use serde_json::Value;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

fn projects_root() -> Option<PathBuf> {
    // WorkBuddy 没有自己的 home 环境变量，跟着 Orrery 的 home 走，
    // 这样 ORRERY_HOME 沙盒能一起隔离
    let root = super::home_dir()?.join(".workbuddy").join("projects");
    root.is_dir().then_some(root)
}

/// `~/.workbuddy/sessions/<pid>.json`：{pid, sessionId, lastHeartbeat, ...}
/// 返回还活着的 sessionId 集合
fn live_sessions() -> HashSet<String> {
    let Ok(dir) = fs::read_dir(super::home_dir().map(|h| h.join(".workbuddy").join("sessions")).unwrap_or_default())
    else {
        return HashSet::new();
    };
    dir.flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|e| {
            let raw = fs::read_to_string(e.path()).ok()?;
            let v: Value = serde_json::from_str(&raw).ok()?;
            let pid = v.get("pid").and_then(Value::as_u64)?;
            let sid = v.get("sessionId").and_then(Value::as_str)?.to_string();
            // prewarm 进程没有正在跑的会话，不算
            let kind = v.get("kind").and_then(Value::as_str).unwrap_or("");
            (kind == "interactive" && pid_alive(pid)).then_some(sid)
        })
        .collect()
}

pub fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    let Some(root) = projects_root() else {
        return Ok(vec![]);
    };
    let live = live_sessions();
    let mut out = vec![];
    for path in collect_session_files(&root) {
        let sig = file_sig(std::slice::from_ref(&path));
        let Some(mut s) = memoized(&path, sig, || parse_session(&path)) else {
            continue;
        };
        s.size_bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        // 会话还在跑就是绿色，不要因为历史会话一律 idle 把正在跑的也画成蓝的
        if live.contains(&s.id) {
            s.status = "running".into();
        }
        out.push(s);
    }
    Ok(out)
}

pub fn storage() -> HarnessStorage {
    let mut st = HarnessStorage {
        harness: "workbuddy".into(),
        connected: true,
        sessions: 0,
        session_bytes: 0,
        root_bytes: 0,
        root: "~/.workbuddy/projects/".into(),
    };
    let Some(root) = projects_root() else {
        return st;
    };
    for path in collect_session_files(&root) {
        st.sessions += 1;
        st.session_bytes += fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    }
    st.root_bytes = super::dir_size(&root);
    st
}

fn collect_session_files(root: &Path) -> Vec<PathBuf> {
    let mut acc = vec![];
    let Ok(dirs) = fs::read_dir(root) else { return acc };
    for dir in dirs.flatten() {
        let Ok(entries) = fs::read_dir(dir.path()) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                acc.push(path);
            }
        }
    }
    acc
}

/// WorkBuddy 把盘符写成小写（`d:\…`），分隔符用反斜杠。
/// 显示时统一成 `/` 并把盘符还原成大写，和其他 adapter 的路径视觉一致
fn normalize_cwd(cwd: &str) -> String {
    let s = cwd.replace('\\', "/");
    let b = s.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return format!("{}:/{}", (b[0] as char).to_ascii_uppercase(), s[2..].trim_start_matches('/'));
    }
    s
}

fn parse_session(path: &Path) -> Option<SessionSummary> {
    let file = File::open(path).ok()?;
    let mut usage = TokenUsage::default();
    // 一次请求拆成多个 item，共用同一个 providerData.messageId，按它去重
    let mut seen: HashSet<String> = HashSet::new();
    let mut updated_ms = 0u64;
    let mut model = String::new();
    let mut project = String::new();
    let mut ai_title = String::new();
    let mut user_texts: Vec<String> = vec![];
    let mut excerpt = String::new();
    let mut log: Vec<(String, String)> = vec![];

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let ts = v.get("timestamp").and_then(Value::as_u64).unwrap_or(0);
        updated_ms = updated_ms.max(ts);
        let role = v.get("role").and_then(Value::as_str).unwrap_or("assistant");
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");

        if project.is_empty() {
            if let Some(c) = v.get("cwd").and_then(Value::as_str) {
                project = normalize_cwd(c);
            }
        }
        if kind == "ai-title" && ai_title.is_empty() {
            if let Some(t) = v.get("aiTitle").and_then(Value::as_str) {
                ai_title = t.to_string();
            }
        }
        if let Some(m) = v.pointer("/providerData/model").and_then(Value::as_str) {
            if model.is_empty() {
                model = m.to_string();
            }
        }
        if let Some(text) = item_text(&v) {
            // 注入的上下文块既不当标题也不当摘要
            if !is_injected(&text) {
                if excerpt.is_empty() {
                    excerpt = text.clone();
                }
                if role == "user" {
                    user_texts.push(text);
                }
            }
        }
        if let Some(u) = item_usage(&v) {
            // 同一 messageId 只算一次；没有 messageId 的按 item id 兜底
            let key = v
                .pointer("/providerData/messageId")
                .and_then(Value::as_str)
                .or_else(|| v.get("id").and_then(Value::as_str))
                .unwrap_or("")
                .to_string();
            if key.is_empty() || seen.insert(key) {
                usage.add(&u);
            }
        }
        if log.len() < 6 {
            let cls = if role == "user" { "hi" } else { "ok" };
            log.push((cls.to_string(), line_label(&v, kind, role)));
        }
    }

    if updated_ms == 0 {
        return None;
    }
    // 标题：优先 WorkBuddy 自己生成的 ai-title；拿不到再退回最近一条真实用户输入
    let title = if !ai_title.is_empty() {
        ai_title
    } else {
        user_texts.last().cloned().unwrap_or_else(|| excerpt.clone())
    };
    let excerpt = truncate(&excerpt, 200);

    Some(SessionSummary {
        id: path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string(),
        harness: "workbuddy".into(),
        title: truncate(&title, 80),
        project,
        model: if model.is_empty() { "—".into() } else { model },
        status: "idle".into(),
        updated_ms,
        tokens: format_tokens(usage.total()),
        usage,
        excerpt,
        path: path.to_string_lossy().to_string(),
        log,
        size_bytes: 0,
        subagents: 0,
        kind: String::new(),
    })
}

/// WorkBuddy 会把一整个 `<system-reminder>` 上下文块塞进用户消息，
/// 那不是用户写的输入，不能当标题也不能当摘要
fn is_injected(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("<system-reminder") || t.starts_with("<user_info") || t.starts_with("<environment")
}

fn item_text(v: &Value) -> Option<String> {
    // ai-title 没有 content，标题单独取
    let content = v.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    let arr = content.as_array()?;
    let mut parts = vec![];
    for item in arr {
        let t = item.get("type").and_then(Value::as_str).unwrap_or("");
        if t == "input_text" || t == "output_text" || t == "text" {
            if let Some(s) = item.get("text").and_then(Value::as_str) {
                parts.push(s.to_string());
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn line_label(v: &Value, kind: &str, role: &str) -> String {
    let text = item_text(v).unwrap_or_default();
    // function_call / reasoning 没有 content，用 providerData 里的说明顶一下
    let fallback = v
        .pointer("/providerData/argumentsDisplayText")
        .and_then(Value::as_str)
        .or_else(|| v.pointer("/providerData/reasoning").and_then(Value::as_str))
        .unwrap_or("");
    let text = if text.trim().is_empty() { fallback } else { &text };
    let tag = if kind.is_empty() { role } else { kind };
    truncate(text, 80).replace('\n', " ") + &format!(" [{tag}]")
}

fn item_usage(v: &Value) -> Option<TokenUsage> {
    let norm = v.pointer("/message/usage");
    let raw = v.pointer("/providerData/rawUsage");
    let pd = v.pointer("/providerData/usage");
    if norm.is_none() && raw.is_none() {
        return None;
    }
    // rawUsage 是 OpenAI 形状，message.usage 是规范化后的；两边都试
    let u = |keys: &[&str]| -> u64 {
        for obj in [raw, pd, norm].into_iter().flatten() {
            for k in keys {
                if let Some(n) = obj.get(k).and_then(Value::as_u64) {
                    return n;
                }
            }
        }
        0
    };
    let input = u(&["input_tokens", "inputTokens", "prompt_tokens", "promptTokens"]);
    let output = u(&["output_tokens", "outputTokens", "completion_tokens", "completionTokens"]);
    let cache_read = u(&[
        "cache_read_input_tokens",
        "cacheReadInputTokens",
        "cached_read_tokens",
        "cached_tokens",
        "cachedTokens",
    ]);
    let cache_write = u(&[
        "cache_creation_input_tokens",
        "cacheCreationInputTokens",
        "cache_write_input_tokens",
        "prompt_cache_write_tokens",
        "cachedWriteTokens",
    ]);
    let total = u(&["total_tokens", "totalTokens"]);
    if input + output + cache_read + cache_write + total == 0 {
        return None;
    }

    let mut usage = TokenUsage { input, output, cache_read, cache_write, calls: 1, ..Default::default() };
    let sum = input + output + cache_read + cache_write;
    // 分项之和小于 total：多出来的没有分项可依，单列不猜
    // 分项之和超过 total：说明缓存已含在 input 里（OpenAI 口径），丢掉缓存桶重算，避免重复计数
    if total > 0 && sum > total {
        usage.cache_read = 0;
        usage.cache_write = 0;
        usage.unsplit = total.saturating_sub(input + output);
    } else if total > sum {
        usage.unsplit = total - sum;
    }
    Some(usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个用例用自己的目录（id 唯一），并行测试互不干扰。
    /// parse_session 只依赖文件本身，所以不碰 ORRERY_HOME 这种进程级全局状态
    fn write(dir: &str, id: &str, lines: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("orrery-wb-{}-{}-{}", id, dir, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let proj = root.join(".workbuddy").join("projects").join(dir);
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(root.join(".workbuddy").join("sessions")).unwrap();
        let f = proj.join(format!("{id}.jsonl"));
        fs::write(&f, lines.join("\n")).unwrap();
        root
    }

    /// 仿照实测字段形状的虚构 session-meta
    const META: &str = r#"{"type":"session-meta","id":"00000000-0000-4000-8000-000000000002","sessionId":"00000000-0000-4000-8000-000000000001","timestamp":1790517232377,"meta":{"codebuddy.ai/hostKind":"unopted"}}"#;

    #[test]
    fn project_path_comes_straight_from_cwd() {
        let root = write(
            "d-code-acme",
            "00000000-0000-4000-8000-000000000001",
            &[
                META,
                // 这一行取真机形状：cwd 直接在 item 上，不用解有损目录名
                r#"{"id":"00000000-0000-4000-8000-000000000003","timestamp":1790517219314,"type":"message","role":"user","content":[{"type":"input_text","text":"帮我看下登录流程"}],"sessionId":"00000000-0000-4000-8000-000000000001","cwd":"d:\\code\\acme"}"#,
                r#"{"id":"00000000-0000-4000-8000-000000000004","timestamp":1790517238822,"type":"ai-title","aiTitle":"示例项目登录排查","sessionId":"00000000-0000-4000-8000-000000000001","cwd":"d:\\code\\acme"}"#,
            ],
        );
        let f = root.join(".workbuddy/projects/d-code-acme/00000000-0000-4000-8000-000000000001.jsonl");
        let s = parse_session(&f).unwrap();
        assert_eq!(s.project, "D:/code/acme", "盘符还原大写、分隔符统一成 /");
        assert_eq!(s.title, "示例项目登录排查", "ai-title 优先于用户输入");
        assert_eq!(s.harness, "workbuddy");
        let _ = fs::remove_dir_all(&root);
    }

    /// WorkBuddy 会把 <system-reminder> 上下文块塞进用户消息，不能当标题
    #[test]
    fn injected_user_context_is_not_used_as_title() {
        let root = write(
            "d-code-acme",
            "019fdba8-940e-7f20-bfda-365ecb643e52",
            &[
                r#"{"timestamp":1790517219314,"type":"message","role":"user","content":[{"type":"input_text","text":"<system-reminder data-role=\"user-context\">\n<user_info>\nOS Version: win32\n</user_info>"}],"cwd":"d:\\code\\acme"}"#,
                r#"{"timestamp":1790517220000,"type":"message","role":"user","content":[{"type":"input_text","text":"真正的提问"}],"cwd":"d:\\code\\acme"}"#,
                r#"{"timestamp":1790517238822,"type":"message","role":"assistant","content":[{"type":"output_text","text":"助手回复"}],"cwd":"d:\\code\\acme"}"#,
            ],
        );
        let f = root.join(".workbuddy/projects/d-code-acme/019fdba8-940e-7f20-bfda-365ecb643e52.jsonl");
        let s = parse_session(&f).unwrap();
        assert_eq!(s.title, "真正的提问");
        // 摘要取第一条可见文本，注入块被跳过
        assert_eq!(s.excerpt, "真正的提问");
        let _ = fs::remove_dir_all(&root);
    }

    /// 一次请求拆成 message / function_call / reasoning 多个 item，
    /// 共用同一 messageId 与同一份用量：只能算一次
    #[test]
    fn items_sharing_one_message_id_count_once() {
        let mid = "00000000000040008000000000000005";
        let make = |ty: &str| {
            serde_json::json!({
                "timestamp": 1790517241761u64,
                "type": ty,
                "providerData": {
                    "messageId": mid,
                    "model": "deepseek-v4.1-flash"
                },
                "message": { "usage": {
                    "input_tokens": 34835u64, "output_tokens": 231u64, "total_tokens": 35066u64
                } },
                "cwd": "d:\\code\\acme"
            })
            .to_string()
        };
        let reasoning = make("reasoning");
        let call = make("function_call");
        let message = make("message");
        let root = write(
            "d-code-acme",
            "019fdba9-940e-7f20-bfda-365ecb643e53",
            &[&reasoning, &call, &message],
        );
        let f = root.join(".workbuddy/projects/d-code-acme/019fdba9-940e-7f20-bfda-365ecb643e53.jsonl");
        let s = parse_session(&f).unwrap();
        assert_eq!(s.usage.input, 34835, "三个 item 同一个 messageId 只算一次");
        assert_eq!(s.usage.output, 231);
        assert_eq!(s.usage.calls, 1, "三个 item 是一次 API 调用");
        assert_eq!(s.usage.total(), 35066);
        assert_eq!(s.tokens, "35.1k");
        let _ = fs::remove_dir_all(&root);
    }

    /// total 大于分项之和：差额进 unsplit，不猜比例
    #[test]
    fn usage_remainder_goes_to_unsplit_not_guessed() {
        let u = item_usage(&serde_json::json!({
            "type":"message","role":"assistant",
            "message":{"usage":{"input_tokens":34835,"output_tokens":231,"total_tokens":40000}}
        }))
        .unwrap();
        assert_eq!(u.unsplit, 40000 - 34835 - 231);
        assert_eq!(u.total(), 40000);
    }

    /// 分项之和超过 total：说明缓存已含在 input（OpenAI 口径），丢掉缓存桶
    #[test]
    fn cache_inside_input_is_not_double_counted() {
        let u = item_usage(&serde_json::json!({
            "type":"message","role":"assistant",
            "message":{"usage":{"input_tokens":34835,"output_tokens":231,"total_tokens":35066,"cache_read_input_tokens":34944}}
        }))
        .unwrap();
        assert_eq!(u.cache_read, 0, "34.9k + 34.8k 超过 35k total：缓存已在 input 里");
        assert_eq!(u.total(), 35066);
    }

    /// cache 字段名在不同供应商间不一致，两个位置都读。
    /// 这里用 Anthropic 口径（缓存与 input 分开），所以分项之和就等于 total
    #[test]
    fn raw_usage_field_names_are_all_supported() {
        let u = item_usage(&serde_json::json!({
            "type":"message","role":"assistant",
            "providerData":{"rawUsage":{
                "prompt_tokens":60,"completion_tokens":20,"total_tokens":120,
                "cache_read_input_tokens":40,"cache_creation_input_tokens":0,
                "prompt_cache_hit_tokens":40,"prompt_cache_miss_tokens":60,
                "completion_tokens_details":{"reasoning_tokens":5}
            }}
        }))
        .unwrap();
        assert_eq!(u.input, 60);
        assert_eq!(u.output, 20);
        assert_eq!(u.cache_read, 40);
        assert_eq!(u.unsplit, 0, "分项之和恰好等于 total，没有未知部分");
        assert_eq!(u.total(), 120);
    }

    #[test]
    fn injected_context_is_detected() {
        assert!(is_injected("<system-reminder data-role=\"user-context\">\n…"));
        assert!(is_injected("  <user_info>OS</user_info>"));
        assert!(!is_injected("帮我看下登录流程"));
        assert!(!is_injected(""));
    }

    /// WorkBuddy 自己把盘符写成小写、分隔符用反斜杠
    #[test]
    fn cwd_is_normalized_for_display() {
        assert_eq!(normalize_cwd("d:\\code\\acme"), "D:/code/acme");
        assert_eq!(normalize_cwd("d:\\示例文档\\cards"), "D:/示例文档/cards");
        assert_eq!(normalize_cwd("/home/user/code"), "/home/user/code");
        assert_eq!(normalize_cwd("relative/path"), "relative/path");
    }
}

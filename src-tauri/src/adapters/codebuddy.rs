//! CodeBuddy Code CLI sessions from `~/.codebuddy/projects/<work-dir>/<sessionId>.jsonl`
//!
//! 布局（2.1.4 实测：`@tencent-ai/codebuddy-code`，在隔离 HOME 里跑出真实会话文件）：
//! - `~/.codebuddy/projects/`           每个工作目录一个子目录
//! - `<work-dir>/`                      目录名 = 工作目录把 `/` `\` `:` 换成 `-`（有损）
//! - `<sessionId>.jsonl`               一个会话一个文件，一行一个 history item，追加写入
//! - item 形状：`{type,role,content:[{type:"input_text"|"output_text",text}],
//!   providerData:{agent,usage?},id,timestamp}`
//!
//! token 口径（来源：产物 `dist/codebuddy.js` 的 `SessionStoreImpl`）：
//! - 保存时 `transformItemForSave` 会把 `providerData.usage` 规范化成 `message.usage`，
//!   **只保留** `input_tokens` / `output_tokens` / `total_tokens`，缓存字段被丢掉；
//!   原始的 `providerData.usage` 仍原样留在行里，所以两个位置都可能出现。
//! - `providerData.usage` 的字段名随供应商而变（`inputTokens` 或 `promptTokens`，
//!   `cachedTokens` / `cachedReadTokens` / `cachedWriteTokens` 或 `cachedMissTokens`），
//!   因此这里逐个字段名兼容。
//! - `id` 是消息 id。流式若重复写同一 id，累加前先去重（与 Claude Code 同思路）。
//! - 分项之和与 `total_tokens` 不一致时，差额记进 `unsplit` 而不是猜一个拆分比例；
//!   分项加起来反而超过 total（说明缓存已含在 input 里，OpenAI 口径）则丢掉缓存桶，
//!   只用 input/output，剩下的仍进 `unsplit`。
//!
//! 会话状态：CLI 不落盘任何运行状态，历史会话一律按 idle 展示（与其他 adapter 一致）。
//! 删除：暂不开放。CodeBuddy 的 session 文件由 CLI 自己追加写，Orrery 没有在沙盒里验证过
//! 删掉文件后它的 `--resume` 列表与 `user-state.json` 会变成什么样，所以按只读处理。

use super::{
    file_sig, format_tokens, memoized, truncate, HarnessStorage, SessionSummary, TokenUsage,
};
use serde_json::Value;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

fn projects_root() -> Option<PathBuf> {
    // CLI 用 os.homedir()，没有自己的 home 环境变量；跟着 Orrery 的 home 走，
    // 这样 ORRERY_HOME 沙盒能把它一起隔离
    let root = super::home_dir()?.join(".codebuddy").join("projects");
    root.is_dir().then_some(root)
}

pub fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    let Some(root) = projects_root() else {
        return Ok(vec![]);
    };
    let mut out = vec![];
    for path in collect_session_files(&root) {
        let sig = file_sig(std::slice::from_ref(&path));
        let Some(mut s) = memoized(&path, sig, || parse_session(&path)) else {
            continue;
        };
        s.size_bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        out.push(s);
    }
    Ok(out)
}

pub fn storage() -> HarnessStorage {
    let mut st = HarnessStorage {
        harness: "codebuddy".into(),
        connected: true,
        sessions: 0,
        session_bytes: 0,
        root_bytes: 0,
        root: "~/.codebuddy/projects/".into(),
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

/// 目录名 = 工作目录把 `/` `\` `:` 换成 `-`。有损编码：原路径里的 `-` 无法与分隔符区分，
/// 盘符大小写也不保留（实测落盘是小写盘符）。仅作兜底显示，
/// 解码规则与 Claude Code 的 `decode_claude_project_dir` 同族
fn decode_project_dir(dir: &str) -> String {
    let parts: Vec<&str> = dir.split('-').filter(|s| !s.is_empty()).collect();
    match parts.split_first() {
        // Windows：`d-code-acme` → `D:/code/acme`（盘符还原大写）
        Some((drive, rest)) if drive.len() == 1 && drive.chars().all(|c| c.is_ascii_alphabetic()) => {
            format!("{}:/{}", drive.to_ascii_uppercase(), rest.join("/"))
        }
        // Unix：`home-user-code` → `home/user/code`
        Some(_) => parts.join("/"),
        None => dir.to_string(),
    }
}

fn parse_session(path: &Path) -> Option<SessionSummary> {
    let file = File::open(path).ok()?;
    let slug = path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("");
    let mut usage = TokenUsage::default();
    // 同一 id 可能因流式重复出现，累加前先去重
    let mut seen: HashSet<String> = HashSet::new();
    let mut updated_ms = 0u64;
    let mut model = String::new();
    let mut user_texts: Vec<(bool, String)> = vec![];
    let mut excerpt = String::new();
    let mut log: Vec<(String, String)> = vec![];

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let ts = v.get("timestamp").and_then(Value::as_u64).unwrap_or(0);
        updated_ms = updated_ms.max(ts);
        let role = v.get("role").and_then(Value::as_str).unwrap_or("assistant");
        if let Some(m) = item_text(&v) {
            if role == "user" {
                // skipRun = CLI 自己塞进去的指令，不是用户写的；和它的 --resume 列表同口径
                let skip = v.pointer("/providerData/skipRun").and_then(Value::as_bool).unwrap_or(false);
                user_texts.push((skip, m.clone()));
            }
            if excerpt.is_empty() {
                excerpt = m;
            }
        }
        if let Some(u) = item_usage(&v) {
            let id = v.get("id").and_then(Value::as_str).unwrap_or("");
            if id.is_empty() || seen.insert(id.to_string()) {
                if let Some(m) = v.get("model").and_then(Value::as_str) {
                    if model.is_empty() {
                        model = m.to_string();
                    }
                }
                usage.add(&u);
            }
        }
        if log.len() < 6 {
            let cls = if role == "user" { "hi" } else { "ok" };
            log.push((cls.to_string(), line_label(&v)));
        }
    }

    if updated_ms == 0 {
        return None;
    }
    // 标题取最近一条用户输入（和 CodeBuddy 自己的恢复列表一致），拿不到再退回摘要
    let title = user_texts
        .iter()
        .rev()
        .find(|(skip, t)| !skip && !t.trim().is_empty())
        .or_else(|| user_texts.iter().rev().find(|(_, t)| !t.trim().is_empty()))
        .map(|(_, t)| t.clone())
        .unwrap_or_else(|| excerpt.clone());
    let excerpt = truncate(&excerpt, 200);

    Some(SessionSummary {
        id: path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string(),
        harness: "codebuddy".into(),
        title: truncate(&title, 80),
        project: decode_project_dir(slug),
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

/// 一条 history item 的可见文本。用户消息是 `input_text`，助手消息是 `output_text`
fn item_text(v: &Value) -> Option<String> {
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

/// 情报条里显示的一行摘要：只截一条，不重复罗列
fn line_label(v: &Value) -> String {
    let role = v.get("role").and_then(Value::as_str).unwrap_or("assistant");
    let text = item_text(v).unwrap_or_default();
    truncate(&text, 80).replace('\n', " ") + &format!(" [{role}]")
}

/// 一条 item 的用量。读 `message.usage`（CLI 自己规范化的三个字段）与
/// `providerData.usage`（供应商原始返回）合并；分项与 total 对不上时差额进 unsplit
fn item_usage(v: &Value) -> Option<TokenUsage> {
    let norm = v.pointer("/message/usage");
    let raw = v.pointer("/providerData/usage");
    if norm.is_none() && raw.is_none() {
        return None;
    }
    let u = |keys: &[&str]| -> u64 {
        for obj in [raw, norm].into_iter().flatten() {
            for k in keys {
                if let Some(n) = obj.get(k).and_then(Value::as_u64) {
                    return n;
                }
            }
        }
        0
    };
    let input = u(&["input_tokens", "inputTokens", "promptTokens", "prompt_tokens"]);
    let output = u(&["output_tokens", "outputTokens", "completionTokens", "completion_tokens"]);
    let cache_read = u(&["cachedReadTokens", "cached_read_tokens", "cachedTokens", "cached_tokens"]);
    let cache_write = u(&["cachedWriteTokens", "cached_write_tokens", "cachedMissTokens"]);
    let total = u(&["total_tokens", "totalTokens"]);
    if input + output + cache_read + cache_write + total == 0 {
        return None;
    }

    let mut usage = TokenUsage { input, output, cache_read, cache_write, ..Default::default() };
    let sum = input + output + cache_read + cache_write;
    // 分项之和小于 total：多出来的部分没有分项可依，单列不猜
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

    fn write(dir: &str, id: &str, lines: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("orrery-cb-{}-{}", dir, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let proj = root.join(dir);
        fs::create_dir_all(&proj).unwrap();
        let f = proj.join(format!("{id}.jsonl"));
        fs::write(&f, lines.join("\n")).unwrap();
        f
    }

    /// 沙盒里 CLI 认证失败时仍真实写出的一行：只有用户消息、没有 usage
    const REAL: &str = r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"say hi"}],"providerData":{"agent":"cli"},"id":"a45465263d29433c9ce2d7300ee2c506","timestamp":1790515660056}"#;

    #[test]
    fn real_line_parses_title_project_and_usage_free() {
        let f = write("d-code-acme", "b602c0ad-5818-4c08-a755-9be0b63bda5b", &[REAL]);
        let s = parse_session(&f).unwrap();
        assert_eq!(s.harness, "codebuddy");
        assert_eq!(s.title, "say hi");
        assert_eq!(s.project, "D:/code/acme");
        assert_eq!(s.updated_ms, 1790515660056);
        assert_eq!(s.usage.total(), 0);
        // 0 用量时 format_tokens 给占位符，与前端一致（不显示成 "0 tok"）
        assert_eq!(s.tokens, "—");
        assert_eq!(s.size_bytes, 0, "size 由 list_sessions 填，解析阶段不碰盘");
        let _ = fs::remove_dir_all(f.parent().unwrap().parent().unwrap());
    }

    /// 标题要跟 CodeBuddy 自己的恢复列表同口径：最近一条非 skipRun 的用户消息
    #[test]
    fn title_prefers_latest_user_text_and_skips_cli_injected_runs() {
        let f = write(
            "d-code-acme",
            "019fdba8-940e-7f20-bfda-365ecb643e52",
            &[
                r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"first prompt"}],"providerData":{"agent":"cli"},"id":"1","timestamp":1000}"#,
                r#"{"type":"message","role":"assistant","content":[{"type":"output_text","text":"assistant reply"}],"providerData":{"agent":"cli"},"id":"2","timestamp":2000}"#,
                r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"cli injected"}],"providerData":{"agent":"cli","skipRun":true},"id":"3","timestamp":3000}"#,
                r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"second prompt"}],"providerData":{"agent":"cli"},"id":"4","timestamp":4000}"#,
            ],
        );
        let s = parse_session(&f).unwrap();
        assert_eq!(s.title, "second prompt");
        assert_eq!(s.updated_ms, 4000);
        let _ = fs::remove_dir_all(f.parent().unwrap().parent().unwrap());
    }

    /// message.usage 三个字段齐全且能对上 total：unsplit 为 0
    #[test]
    fn usage_adds_up_without_unsplit() {
        let u = item_usage(&serde_json::json!({
            "type":"message","role":"assistant","id":"m1",
            "message":{"usage":{"input_tokens":100,"output_tokens":20,"total_tokens":120}}
        }))
        .unwrap();
        assert_eq!(u.unsplit, 0);
        assert_eq!(u.total(), 120);
    }

    /// total 大于分项之和（例如缓存被 normalizeUsageFormat 丢掉）：差额进 unsplit，不猜比例
    #[test]
    fn usage_remainder_goes_to_unsplit_not_guessed() {
        let u = item_usage(&serde_json::json!({
            "type":"message","role":"assistant","id":"m2",
            "message":{"usage":{"input_tokens":100,"output_tokens":20,"total_tokens":3000}}
        }))
        .unwrap();
        assert_eq!(u.input, 100);
        assert_eq!(u.output, 20);
        assert_eq!(u.unsplit, 2880);
        assert_eq!(u.total(), 3000);
    }

    /// 分项之和超过 total：说明缓存已含在 input（OpenAI 口径），丢掉缓存桶而不是重复计数
    #[test]
    fn cache_inside_input_is_not_double_counted() {
        let u = item_usage(&serde_json::json!({
            "type":"message","role":"assistant","id":"m3",
            "providerData":{"usage":{"inputTokens":33032,"outputTokens":80,"totalTokens":33112,"cachedTokens":26496}},
            "message":{"usage":{"input_tokens":33032,"output_tokens":80,"total_tokens":33112}}
        }))
        .unwrap();
        assert_eq!(u.cache_read, 0, "33k + 26k 超过 31k total：缓存已经在 input 里");
        assert_eq!(u.unsplit, 0);
        assert_eq!(u.total(), 33112);
    }

    /// 同一 id 重复出现（流式重写）只累加一次
    #[test]
    fn duplicate_message_id_is_counted_once() {
        let line = r#"{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hi"}],"id":"dup","timestamp":5000,"message":{"usage":{"input_tokens":50,"output_tokens":5,"total_tokens":55}}}"#;
        let f = write("d-code-dup", "019fdba8-940e-7f20-bfda-365ecb643e52", &[line, line, line]);
        let mut total = TokenUsage::default();
        let mut seen = HashSet::new();
        for line in BufReader::new(File::open(&f).unwrap()).lines().map_while(Result::ok) {
            let v: Value = serde_json::from_str(&line).unwrap();
            if let Some(u) = item_usage(&v) {
                let id = v.get("id").and_then(Value::as_str).unwrap_or("");
                if seen.insert(id.to_string()) {
                    total.add(&u);
                }
            }
        }
        assert_eq!(total.total(), 55, "三行同一个 id 只算一次");
        let _ = fs::remove_dir_all(f.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn project_dir_decodes_windows_and_unix_shapes() {
        // 实测落盘的目录名是小写盘符
        assert_eq!(decode_project_dir("d-code-acme"), "D:/code/acme");
        assert_eq!(decode_project_dir("home-user-code"), "home/user/code");
        assert_eq!(decode_project_dir(""), "");
    }

    /// 目录名有损：原路径里的 `-` 会被解码成 `/`，盘符大小写也不保留。这是 CLI 自己的
    /// 编码方式，与 Claude Code 的同类问题同级，只用于兜底显示
    #[test]
    fn project_dir_decode_is_documented_lossy() {
        assert_eq!(decode_project_dir("d-code-my-app"), "D:/code/my/app");
    }
}

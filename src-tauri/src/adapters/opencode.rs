//! OpenCode 会话：`~/.local/share/opencode/opencode.db`
//!
//! 1.18 起 OpenCode 把会话存进 SQLite（更早的版本是 JSON 文件）。`session` 表已经把
//! 需要的东西都算好了，不用解析消息流：
//! - `title` 标题、`directory` 工作目录、`model`（JSON，取 `.id`）、`agent`
//! - `parent_id` 非空即子 agent，并入父会话不单列（实测 53 条里 12 条是子 agent，无孤儿）
//! - token 五项：`tokens_input` / `tokens_cache_read` / `tokens_cache_write` /
//!   `tokens_output` / `tokens_reasoning`
//!
//! 口径：`tokens_reasoning` 是**独立**的一桶，不含在 output 里——实测某会话
//! input + output + cache_read + cache_write + reasoning 恰好等于消息里记的 total
//! （25,635,143 + 438,210 + 582,177,840 + 0 + 237,648 = 608,488,841）。按 DSH / Codex
//! 「output 含推理」的口径把它并进 output，五个桶仍互不重叠。
//!
//! 对账：与 `opencode stats` 逐项一致——53 会话、input 108.4M、output 3.4M、
//! cache_read 1853.0M、cache_write 1.2M、$22.72、7270 条消息，零差异。
//!
//! 体积：一个会话在库里的内容 = `message` + `part` + `event` 三张表里 `data` 的字节数
//! （三张表都有 session_id / aggregate_id 索引，单会话最坏 51ms）。全量扫一遍要 2.6s，
//! 所以按会话增量：`time_updated` 没变就用索引里缓存的值。
//!
//! 数据库只以只读方式打开。本适配器暂不接删除入口，Orrery 不直接写别人的
//! 数据库，所以这些会话在界面上不可删除（`cleanup.rs` 里显式挡掉）。

use super::{dir_size, format_tokens, storage_absent, store, truncate, HarnessStorage, SessionSummary, TokenUsage};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const DB: &str = "opencode.db";

/// `$XDG_DATA_HOME/<dir>`，否则 `~/.local/share/<dir>`。
/// OpenCode 的分支们目录布局相同，所以这里按名字取（`custom.rs` 复用）
pub(super) fn family_home(dir: &str) -> Option<PathBuf> {
    let sandboxed = cfg!(debug_assertions) && std::env::var_os("ORRERY_HOME").is_some();
    if !sandboxed {
        if let Some(v) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
            let p = PathBuf::from(v).join(dir);
            if p.is_dir() {
                return Some(p);
            }
        }
    }
    let p = super::home_dir()?.join(".local").join("share").join(dir);
    p.is_dir().then_some(p)
}

fn opencode_home() -> Option<PathBuf> {
    family_home("opencode")
}

/// 普通只读连接保留 WAL 一致性；不能对仍在写入的数据库使用 immutable。
pub(super) fn open(db: &Path) -> Option<Connection> {
    Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

/// 数据库里的一行会话（OpenCode 与登记的同系工具共用）
pub(super) struct Row {
    pub(super) id: String,
    pub(super) parent: Option<String>,
    pub(super) title: String,
    pub(super) directory: String,
    pub(super) model: String,
    pub(super) updated_ms: u64,
    pub(super) usage: TokenUsage,
}

fn read_rows(con: &Connection) -> Result<Vec<Row>, String> {
    let mut stmt = con
        .prepare(
            "SELECT id, parent_id, COALESCE(title,''), COALESCE(directory,''), COALESCE(model,''),
                    COALESCE(time_updated, time_created, 0),
                    COALESCE(tokens_input,0), COALESCE(tokens_cache_write,0), COALESCE(tokens_cache_read,0),
                    COALESCE(tokens_output,0), COALESCE(tokens_reasoning,0)
             FROM session",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Row {
                id: r.get(0)?,
                parent: r.get(1)?,
                title: r.get(2)?,
                directory: r.get::<_, String>(3)?.replace('\\', "/"),
                model: model_id(&r.get::<_, String>(4)?),
                updated_ms: r.get::<_, i64>(5)?.max(0) as u64,
                usage: TokenUsage {
                    input: r.get::<_, i64>(6)?.max(0) as u64,
                    cache_write: r.get::<_, i64>(7)?.max(0) as u64,
                    cache_read: r.get::<_, i64>(8)?.max(0) as u64,
                    // reasoning 并入 output（见模块注释）
                    output: (r.get::<_, i64>(9)?.max(0) + r.get::<_, i64>(10)?.max(0)) as u64,
                    unsplit: 0,
                    calls: 0,
                },
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// `{"id":"glm-5.3-flash","providerID":"opencode-go",…}` → `glm-5.3-flash`
pub(super) fn model_id(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| v.get("id").and_then(|s| s.as_str()).map(String::from))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| if raw.is_empty() { "—".into() } else { raw.to_string() })
}

/// 单个会话在库里占的字节（三张表的 data 长度之和）
pub(super) fn measure(con: &Connection, id: &str) -> u64 {
    let one = |sql: &str| -> u64 {
        con.query_row(sql, [id], |r| r.get::<_, Option<i64>>(0))
            .ok()
            .flatten()
            .unwrap_or(0)
            .max(0) as u64
    };
    one("SELECT SUM(LENGTH(CAST(data AS BLOB))) FROM message WHERE session_id = ?1")
        + one("SELECT SUM(LENGTH(CAST(data AS BLOB))) FROM part WHERE session_id = ?1")
        + one("SELECT SUM(LENGTH(CAST(data AS BLOB))) FROM event WHERE aggregate_id = ?1")
}

/// 体积按会话缓存，`time_updated` 没变就不重新数
pub(super) fn cached_size(con: &Connection, id: &str, updated_ms: u64) -> u64 {
    let cache = &store().sizes;
    if let Some(bytes) = cache.get(id, updated_ms) {
        return bytes;
    }
    let bytes = measure(con, id);
    cache.put(id, updated_ms, bytes);
    store().mark_parsed();
    bytes
}

pub fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    let Some(home) = opencode_home() else {
        return Ok(vec![]);
    };
    let db = home.join(DB);
    if !db.is_file() {
        return Ok(vec![]);
    }
    let Some(con) = open(&db) else {
        return Err(format!("cannot open {} read-only", db.display()));
    };
    // 同一轮的会话元数据与体积来自同一个 SQLite 读快照。
    con.execute_batch("BEGIN").map_err(|e| e.to_string())?;
    summarize("opencode", read_rows(&con)?, &db, |id, updated| cached_size(&con, id, updated))
}

/// 把一批会话行整理成界面用的列表：递归把子 agent 并入根会话，
/// `harness` 决定列表里显示成哪一家（OpenCode 与登记的同系工具共用这套）
pub(super) fn summarize(
    harness: &str,
    rows: Vec<Row>,
    db: &Path,
    mut size: impl FnMut(&str, u64) -> u64,
) -> Result<Vec<SessionSummary>, String> {

    // 子 agent 并入父会话：用量相加、计数 +1、体积一起算进父会话
    let parents: HashMap<&str, Option<&str>> = rows.iter().map(|r| (r.id.as_str(), r.parent.as_deref())).collect();
    let mut roots = HashMap::new();
    for r in &rows {
        let mut root = r.id.as_str();
        let mut seen = std::collections::HashSet::new();
        while let Some(parent) = parents.get(root).copied().flatten().filter(|p| parents.contains_key(p)) {
            if !seen.insert(root) {
                return Err("opencode: cyclic parent_id".into());
            }
            root = parent;
        }
        roots.insert(r.id.clone(), root.to_string());
    }
    let mut extra: HashMap<String, (TokenUsage, u32, u64)> = HashMap::new();
    for r in &rows {
        let root = &roots[&r.id];
        if root == &r.id { continue; }
        let e = extra.entry(root.clone()).or_insert((TokenUsage::default(), 0, 0));
        e.0.add(&r.usage);
        e.1 += 1;
        e.2 += size(&r.id, r.updated_ms);
    }

    let mut out = Vec::new();
    for r in rows {
        // 父会话在本机的子 agent 已经并进去了，不单列
        if roots[&r.id] != r.id {
            continue;
        }
        let mut usage = r.usage;
        let mut subagents = 0;
        let mut bytes = size(&r.id, r.updated_ms);
        if let Some((u, n, b)) = extra.get(&r.id) {
            usage.add(u);
            subagents = *n;
            bytes += b;
        }
        out.push(SessionSummary {
            harness: harness.into(),
            title: truncate(&r.title, 48),
            project: r.directory,
            model: r.model,
            status: "idle".into(),
            updated_ms: r.updated_ms,
            tokens: format_tokens(usage.total()),
            usage,
            excerpt: String::new(),
            path: db.to_string_lossy().to_string(),
            log: vec![],
            size_bytes: bytes,
            subagents,
            // 父会话不在库里的子 agent（实测没有，留着以防版本变化）
            kind: if r.parent.is_some() { "subagent".into() } else { String::new() },
            id: r.id,
        });
    }
    Ok(out)
}

pub fn storage() -> HarnessStorage {
    let Some(home) = opencode_home() else {
        return storage_absent("opencode", "~/.local/share/opencode/");
    };
    let sessions = list_sessions().unwrap_or_default();
    HarnessStorage {
        harness: "opencode".into(),
        connected: true,
        sessions: sessions.len() as u32,
        session_bytes: sessions.iter().map(|s| s.size_bytes).sum(),
        root_bytes: dir_size(&home),
        root: "~/.local/share/opencode/".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, parent: Option<&str>) -> Row {
        Row { id: id.into(), parent: parent.map(String::from), title: String::new(),
            directory: String::new(), model: String::new(), updated_ms: 1,
            usage: TokenUsage { input: 10, output: 2, ..Default::default() } }
    }

    #[test]
    fn nested_and_orphan_agents_keep_all_usage_and_bytes() {
        let rows = vec![row("root", None), row("child", Some("root")),
            row("grandchild", Some("child")), row("orphan", Some("missing")),
            row("orphan-child", Some("orphan"))];
        let sessions = summarize("opencode", rows, Path::new("opencode.db"), |_, _| 7).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions.iter().map(|s| s.usage.total()).sum::<u64>(), 60);
        assert_eq!(sessions[0].subagents, 2);
        assert_eq!(sessions[0].size_bytes, 21);
        assert_eq!(sessions[1].subagents, 1);
        assert_eq!(sessions[1].kind, "subagent");
        assert_eq!(sessions[1].size_bytes, 14);
    }

    #[test]
    fn cyclic_parents_fail_instead_of_silently_dropping_sessions() {
        assert!(summarize("opencode", vec![row("a", Some("b")), row("b", Some("a"))],
            Path::new("opencode.db"), |_, _| 0).is_err());
    }

    #[test]
    fn sqlite_text_is_measured_in_utf8_bytes() {
        let con = Connection::open_in_memory().unwrap();
        con.execute_batch("CREATE TABLE message(session_id TEXT, data TEXT);
            CREATE TABLE part(session_id TEXT, data TEXT);
            CREATE TABLE event(aggregate_id TEXT, data TEXT);
            INSERT INTO message VALUES ('s', '中文');
            INSERT INTO part VALUES ('s', 'abc');
            INSERT INTO event VALUES ('s', '日');").unwrap();
        assert_eq!(measure(&con, "s"), 12);
    }

    #[test]
    fn model_id_takes_the_id_field() {
        assert_eq!(model_id(r#"{"id":"glm-5.3-flash","providerID":"opencode-go"}"#), "glm-5.3-flash");
        // 不是 JSON 就原样用，空的才退化成占位符
        assert_eq!(model_id("claude-opus-5"), "claude-opus-5");
        assert_eq!(model_id(""), "—");
        assert_eq!(model_id("{}"), "{}");
    }
}

//! 自定义 harness：`~/.orrery/harnesses.json`
//!
//! 不少 agent 工具是 OpenCode 的分支，数据目录和表结构同源。与其为每一个单独写
//! 适配器，不如让用户自己登记——尤其是那些还没公开、不该写进开源仓库的工具。
//!
//! 配置格式（不存在这个文件就什么都不做）：
//!
//! ```json
//! [
//!   { "id": "acme", "dir": "acme-code", "db": "acme.db" }
//! ]
//! ```
//!
//! - `id`：会话列表里的 harness 标识，界面用它生成角标
//! - `dir`：`$XDG_DATA_HOME/<dir>` 或 `~/.local/share/<dir>`
//! - `db`：该目录下的 SQLite 文件名
//!
//! 用量口径自动判断：`session` 表有 `tokens_input` 等汇总列就直接读；没有就逐条
//! 累加 `message.data` 里的 `tokens`（`{total, input, output, reasoning, cache:{read,write}}`，
//! 实测 total = 各项之和，五个桶互不重叠）。两种情况都把 reasoning 并进 output，
//! 与 DSH / Codex 口径一致。
//!
//! 一律只读，也不提供删除与终端恢复——我们不写别人的数据库，也不知道对方有没有 CLI。

use super::opencode::{cached_size, family_home, model_id, open, summarize, Row};
use super::{dir_size, HarnessStorage, SessionSummary, TokenUsage};
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Entry {
    pub(crate) id: String,
    dir: String,
    db: String,
}

fn config_path() -> Option<PathBuf> {
    Some(super::data_dir()?.join("harnesses.json"))
}

/// 读登记表。文件不存在、格式不对都当作没有自定义 harness
pub(crate) fn entries() -> Vec<Entry> {
    let Some(raw) = config_path().and_then(|p| std::fs::read(p).ok()) else { return vec![] };
    let parsed: Vec<Entry> = serde_json::from_slice(&raw).unwrap_or_default();
    parsed
        .into_iter()
        // id 会进界面和删除校验，限制成简单标识符
        .filter(|e| {
            !e.id.is_empty()
                && e.id.len() <= 24
                && e.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                && !e.dir.is_empty()
                && !e.db.is_empty()
        })
        .collect()
}

/// 这个 harness 是登记出来的吗（删除与恢复入口据此拒绝）
pub(crate) fn is_custom(harness: &str) -> bool {
    entries().iter().any(|e| e.id == harness)
}

/// session 表里有没有现成的用量汇总列
fn has_token_columns(con: &Connection) -> bool {
    let Ok(mut stmt) = con.prepare("PRAGMA table_info(session)") else { return false };
    // 先收集再判断：query_map 借着 stmt，不能让迭代器活过它
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(1)) else { return false };
    let names: Vec<String> = rows.flatten().collect();
    names.iter().any(|c| c == "tokens_input")
}

/// 逐条消息累加用量，并记下最后出现的模型名
fn aggregate_messages(con: &Connection) -> HashMap<String, (TokenUsage, String)> {
    let mut out: HashMap<String, (TokenUsage, String)> = HashMap::new();
    let Ok(mut stmt) = con.prepare("SELECT session_id, data FROM message ORDER BY time_created") else {
        return out;
    };
    let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) else {
        return out;
    };
    for row in rows.flatten() {
        let (sid, data) = row;
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) else { continue };
        let entry = out.entry(sid).or_default();
        if let Some(m) = v.get("modelID").and_then(|m| m.as_str()).filter(|m| !m.is_empty()) {
            entry.1 = m.to_string();
        }
        let Some(t) = v.get("tokens").filter(|t| t.is_object()) else { continue };
        let n = |key: &str| t.get(key).and_then(serde_json::Value::as_u64).unwrap_or(0);
        let cache = |key: &str| {
            t.get("cache").and_then(|c| c.get(key)).and_then(serde_json::Value::as_u64).unwrap_or(0)
        };
        entry.0.add(&TokenUsage {
            input: n("input"),
            // reasoning 并入 output
            output: n("output") + n("reasoning"),
            cache_read: cache("read"),
            cache_write: cache("write"),
            unsplit: 0,
            calls: 1,
        });
    }
    out
}

fn read_rows(con: &Connection) -> Result<Vec<Row>, String> {
    let from_columns = has_token_columns(con);
    let mut totals = if from_columns { HashMap::new() } else { aggregate_messages(con) };

    let sql = if from_columns {
        "SELECT id, parent_id, COALESCE(title,''), COALESCE(directory,''),
                COALESCE(time_updated, time_created, 0), COALESCE(model,''),
                COALESCE(tokens_input,0), COALESCE(tokens_cache_write,0),
                COALESCE(tokens_cache_read,0), COALESCE(tokens_output,0), COALESCE(tokens_reasoning,0)
         FROM session"
    } else {
        "SELECT id, parent_id, COALESCE(title,''), COALESCE(directory,''),
                COALESCE(time_updated, time_created, 0), '', 0, 0, 0, 0, 0
         FROM session"
    };
    let mut stmt = con.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Row {
                id: r.get(0)?,
                parent: r.get(1)?,
                title: r.get(2)?,
                directory: r.get::<_, String>(3)?.replace('\\', "/"),
                updated_ms: r.get::<_, i64>(4)?.max(0) as u64,
                model: model_id(&r.get::<_, String>(5)?),
                usage: TokenUsage {
                    input: r.get::<_, i64>(6)?.max(0) as u64,
                    cache_write: r.get::<_, i64>(7)?.max(0) as u64,
                    cache_read: r.get::<_, i64>(8)?.max(0) as u64,
                    output: (r.get::<_, i64>(9)?.max(0) + r.get::<_, i64>(10)?.max(0)) as u64,
                    unsplit: 0,
                    calls: 0,
                },
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(rows
        .into_iter()
        .map(|mut row| {
            if let Some((usage, model)) = totals.remove(&row.id) {
                row.usage = usage;
                row.model = model_id(&model);
            }
            row
        })
        .collect())
}

fn list_one(entry: &Entry) -> Result<Vec<SessionSummary>, String> {
    let Some(home) = family_home(&entry.dir) else { return Ok(vec![]) };
    let db = home.join(&entry.db);
    if !db.is_file() {
        return Ok(vec![]);
    }
    let Some(con) = open(&db) else {
        return Err(format!("cannot open {} read-only", db.display()));
    };
    con.execute_batch("BEGIN").map_err(|e| e.to_string())?;
    summarize(&entry.id, read_rows(&con)?, &db, |id, updated| cached_size(&con, id, updated))
}

pub fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    let mut out = Vec::new();
    for entry in entries() {
        match list_one(&entry) {
            Ok(rows) => out.extend(rows),
            // 单个自定义源出错不该拖垮整轮扫描
            Err(e) => eprintln!("[orrery] custom harness {}: {e}", entry.id),
        }
    }
    Ok(out)
}

pub fn storage() -> Vec<HarnessStorage> {
    entries()
        .into_iter()
        .filter_map(|entry| {
            let home = family_home(&entry.dir)?;
            let sessions = list_one(&entry).unwrap_or_default();
            Some(HarnessStorage {
                harness: entry.id.clone(),
                connected: true,
                sessions: sessions.len() as u32,
                session_bytes: sessions.iter().map(|s| s.size_bytes).sum(),
                root_bytes: dir_size(&home),
                root: format!("~/.local/share/{}/", entry.dir),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_simple_ids_are_accepted() {
        let json = br#"[
            {"id":"good","dir":"d","db":"a.db"},
            {"id":"bad id","dir":"d","db":"a.db"},
            {"id":"../escape","dir":"d","db":"a.db"},
            {"id":"empty-dir","dir":"","db":"a.db"}
        ]"#;
        let parsed: Vec<Entry> = serde_json::from_slice(json).unwrap();
        let kept: Vec<String> = parsed
            .into_iter()
            .filter(|e| {
                !e.id.is_empty()
                    && e.id.len() <= 24
                    && e.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                    && !e.dir.is_empty()
                    && !e.db.is_empty()
            })
            .map(|e| e.id)
            .collect();
        assert_eq!(kept, vec!["good"]);
    }

    #[test]
    fn token_columns_are_detected() {
        let con = Connection::open_in_memory().unwrap();
        con.execute_batch("CREATE TABLE session(id TEXT, tokens_input INT)").unwrap();
        assert!(has_token_columns(&con));
        let bare = Connection::open_in_memory().unwrap();
        bare.execute_batch("CREATE TABLE session(id TEXT, title TEXT)").unwrap();
        assert!(!has_token_columns(&bare));
    }

    #[test]
    fn messages_feed_usage_when_the_session_table_has_no_columns() {
        let con = Connection::open_in_memory().unwrap();
        con.execute_batch(
            "CREATE TABLE session(id TEXT, parent_id TEXT, title TEXT, directory TEXT, time_created INT, time_updated INT);
             CREATE TABLE message(session_id TEXT, time_created INT, data TEXT);
             INSERT INTO session VALUES ('s1', NULL, 't', 'D:\\p', 1, 2);
             INSERT INTO message VALUES ('s1', 1, '{\"modelID\":\"m-1\",\"tokens\":{\"input\":5,\"output\":2,\"reasoning\":3,\"cache\":{\"read\":7,\"write\":1}}}');",
        ).unwrap();
        let rows = read_rows(&con).unwrap();
        assert_eq!(rows[0].usage.input, 5);
        assert_eq!(rows[0].usage.output, 5, "reasoning 要并进 output");
        assert_eq!(rows[0].usage.cache_read, 7);
        assert_eq!(rows[0].model, "m-1");
    }
}

//! Z Code sessions from `~/.zcode/cli/`
//!
//! Z Code is a desktop app built on the OpenCode lineage (`session` / `message` /
//! `part` tables), but it keeps a far richer usage ledger than OpenCode does:
//!
//! - `model_usage`: one row per model API call, with retries, cancellation,
//!   time-to-first-token and the provider's raw usage JSON
//! - `turn_usage`: one row per conversation turn
//!
//! **Which ledger.** `model_usage` is the source of truth. `turn_usage` only
//! counts conversation turns and leaves out side calls: on this machine
//! `model_usage` has 60 `main_turn` rows plus 2 `session_title` rows, and those
//! two title-generation calls are exactly the gap between the two tables in the
//! two affected sessions (+235 input, +192 cache read each). Title generation
//! still costs tokens, so it counts.
//!
//! **Token semantics are decided per row, not assumed.** Measured on real data
//! (Zhipu GLM via the bigmodel provider): all 62 rows satisfy
//! `computed_total_tokens = input_tokens + output_tokens`, and the provider's own
//! usage reads `{inputTokens:33032, outputTokens:80, totalTokens:33112,
//! cacheReadTokens:26496}` — cache reads are *inside* `input_tokens`
//! (OpenAI-style). Adding them again would double count. A different provider
//! could report them Anthropic-style (separate from input), so each row uses
//! `computed_total_tokens` as the referee: the five buckets must add up to that
//! row's total exactly. `row_usage` has tests for both shapes.
//!
//! **Disk size** is mostly *outside* the database. For the largest session here
//! the `rollout/` model I/O log alone is 12 MB, plus 1.2 MB of artifacts and
//! 0.8 MB of cached images, against well under 1 MB of rows. So a session's size
//! is its rows in `message`/`part` plus four per-session locations:
//! `rollout/model-io-<id>.jsonl`, `artifacts/<id>/`, `exec/<id>/`, `image-cache/<id>/`.
//!
//! Read-only. Z Code is a desktop app with no CLI on PATH, so there is no resume
//! command, and there is no official delete either — Orrery never writes another
//! tool's database.

use super::opencode::{cached_size, open, summarize, Row};
use super::{dir_size, storage_absent, HarnessStorage, SessionSummary, TokenUsage};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `$ZCODE_HOME/cli`, otherwise `~/.zcode/cli`
fn zcode_home() -> Option<PathBuf> {
    let sandboxed = cfg!(debug_assertions) && std::env::var_os("ORRERY_HOME").is_some();
    if !sandboxed {
        if let Some(v) = std::env::var_os("ZCODE_HOME").filter(|v| !v.is_empty()) {
            let p = PathBuf::from(v).join("cli");
            if p.is_dir() {
                return Some(p);
            }
        }
    }
    let p = super::home_dir()?.join(".zcode").join("cli");
    p.is_dir().then_some(p)
}

fn db_path(home: &Path) -> PathBuf {
    home.join("db").join("db.sqlite")
}

/// One `model_usage` row → non-overlapping buckets whose sum equals the row's total.
///
/// `total` is `computed_total_tokens` (falling back to the provider's total).
/// - Reasoning is added to output only when the total shows it was counted
///   separately; otherwise it is already inside `output_tokens`.
/// - Cache reads/writes are subtracted from input only when the total shows they
///   were inside it (OpenAI-style). Anthropic-style rows keep input as reported.
fn row_usage(input: u64, output: u64, reasoning: u64, cache_write: u64, cache_read: u64, total: u64) -> TokenUsage {
    let total = if total == 0 { input + output } else { total };
    let output = if reasoning > 0 && total == input + output + reasoning { output + reasoning } else { output };
    let caches = cache_read + cache_write;
    let input = if input + output == total && caches <= input { input - caches } else { input };
    TokenUsage { input, output, cache_read, cache_write, unsplit: 0, calls: 1 }
}

/// Per-session usage and the most recent model name, from `model_usage`
fn usage_by_session(con: &Connection) -> Result<HashMap<String, (TokenUsage, String)>, String> {
    let mut stmt = con
        .prepare(
            "SELECT session_id, COALESCE(model_id,''),
                    COALESCE(input_tokens,0), COALESCE(output_tokens,0), COALESCE(reasoning_tokens,0),
                    COALESCE(cache_creation_input_tokens,0), COALESCE(cache_read_input_tokens,0),
                    COALESCE(computed_total_tokens, provider_total_tokens, 0)
             FROM model_usage
             ORDER BY COALESCE(started_at, completed_at, 0)",
        )
        .map_err(|e| e.to_string())?;
    let n = |v: i64| v.max(0) as u64;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                row_usage(
                    n(r.get(2)?),
                    n(r.get(3)?),
                    n(r.get(4)?),
                    n(r.get(5)?),
                    n(r.get(6)?),
                    n(r.get(7)?),
                ),
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out: HashMap<String, (TokenUsage, String)> = HashMap::new();
    for row in rows {
        let (sid, model, usage) = row.map_err(|e| e.to_string())?;
        let entry = out.entry(sid).or_default();
        entry.0.add(&usage);
        // ordered by time, so the last non-empty model wins
        if !model.is_empty() {
            entry.1 = model;
        }
    }
    Ok(out)
}

fn read_rows(con: &Connection) -> Result<Vec<Row>, String> {
    let mut usage = usage_by_session(con)?;
    let mut stmt = con
        .prepare(
            "SELECT id, parent_id, COALESCE(title,''), COALESCE(directory,''),
                    COALESCE(time_updated, time_created, 0)
             FROM session",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?.replace('\\', "/"),
                r.get::<_, i64>(4)?.max(0) as u64,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(rows
        .into_iter()
        .map(|(id, parent, title, directory, updated_ms)| {
            let (usage, model) = usage.remove(&id).unwrap_or_default();
            Row {
                id,
                parent,
                title,
                directory,
                model: if model.is_empty() { "—".into() } else { model },
                updated_ms,
                usage,
            }
        })
        .collect())
}

/// Files a session owns outside the database
fn session_files_bytes(home: &Path, id: &str) -> u64 {
    let rollout = std::fs::metadata(home.join("rollout").join(format!("model-io-{id}.jsonl")))
        .map(|m| m.len())
        .unwrap_or(0);
    rollout
        + ["artifacts", "exec", "image-cache"]
            .iter()
            .map(|d| dir_size(&home.join(d).join(id)))
            .sum::<u64>()
}

fn list(home: &Path) -> Result<Vec<SessionSummary>, String> {
    let db = db_path(home);
    if !db.is_file() {
        return Ok(vec![]);
    }
    let Some(con) = open(&db) else {
        return Err(format!("cannot open {} read-only", db.display()));
    };
    // metadata, usage and row sizes come from one read snapshot
    con.execute_batch("BEGIN").map_err(|e| e.to_string())?;
    summarize("zcode", read_rows(&con)?, &db, |id, updated| {
        // row bytes are cached by `time_updated`; the files are cheap to stat every time
        cached_size(&con, id, updated) + session_files_bytes(home, id)
    })
}

pub fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    match zcode_home() {
        Some(home) => list(&home),
        None => Ok(vec![]),
    }
}

pub fn storage() -> HarnessStorage {
    let Some(home) = zcode_home() else {
        return storage_absent("zcode", "~/.zcode/cli/");
    };
    let sessions = list(&home).unwrap_or_default();
    HarnessStorage {
        harness: "zcode".into(),
        connected: true,
        sessions: sessions.len() as u32,
        session_bytes: sessions.iter().map(|s| s.size_bytes).sum(),
        root_bytes: dir_size(&home),
        root: "~/.zcode/cli/".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum(u: &TokenUsage) -> u64 {
        u.input + u.output + u.cache_read + u.cache_write
    }

    /// A real row from this machine: cache reads are inside input (OpenAI-style)
    #[test]
    fn openai_style_row_does_not_double_count_cache_reads() {
        let u = row_usage(33_032, 80, 0, 0, 26_496, 33_112);
        assert_eq!(u.input, 6_536, "input must exclude the 26,496 cached tokens");
        assert_eq!(u.cache_read, 26_496);
        assert_eq!(u.output, 80);
        assert_eq!(sum(&u), 33_112, "buckets must add up to the row's total");
    }

    /// A provider that reports caches separately from input (Anthropic-style)
    #[test]
    fn anthropic_style_row_keeps_input_as_reported() {
        let u = row_usage(1_000, 200, 0, 300, 5_000, 6_500);
        assert_eq!(u.input, 1_000);
        assert_eq!(sum(&u), 6_500);
    }

    #[test]
    fn reasoning_is_added_only_when_the_total_counts_it_separately() {
        // separate: total = input + output + reasoning
        let separate = row_usage(100, 50, 30, 0, 0, 180);
        assert_eq!(separate.output, 80);
        assert_eq!(sum(&separate), 180);
        // already inside output: total = input + output
        let inside = row_usage(100, 50, 30, 0, 0, 150);
        assert_eq!(inside.output, 50);
        assert_eq!(sum(&inside), 150);
    }

    #[test]
    fn missing_total_falls_back_to_input_plus_output() {
        let u = row_usage(500, 40, 0, 0, 300, 0);
        assert_eq!(sum(&u), 540);
    }

    /// Same shape as the real database: model_usage feeds usage, title-generation calls count
    #[test]
    fn usage_comes_from_every_model_call() {
        let con = Connection::open_in_memory().unwrap();
        con.execute_batch(
            "CREATE TABLE session(id TEXT, parent_id TEXT, title TEXT, directory TEXT,
                                  time_created INT, time_updated INT);
             CREATE TABLE model_usage(session_id TEXT, model_id TEXT, query_source TEXT,
                 input_tokens INT, output_tokens INT, reasoning_tokens INT,
                 cache_creation_input_tokens INT, cache_read_input_tokens INT,
                 computed_total_tokens INT, provider_total_tokens INT, started_at INT, completed_at INT);
             INSERT INTO session VALUES ('sess_a', NULL, 'Title', 'D:\\p', 1, 2);
             INSERT INTO model_usage VALUES ('sess_a','GLM-5.3-Flash','main_turn',33032,80,0,0,26496,33112,33112,1,2);
             INSERT INTO model_usage VALUES ('sess_a','GLM-5.3-Flash','session_title',235,20,0,0,192,255,255,3,4);",
        )
        .unwrap();
        let rows = read_rows(&con).unwrap();
        assert_eq!(rows.len(), 1);
        let u = &rows[0].usage;
        assert_eq!(u.calls, 2, "title generation is a real call and must be counted");
        assert_eq!(u.input + u.output + u.cache_read + u.cache_write, 33_112 + 255);
        assert_eq!(rows[0].model, "GLM-5.3-Flash");
        assert_eq!(rows[0].directory, "D:/p");
    }
}

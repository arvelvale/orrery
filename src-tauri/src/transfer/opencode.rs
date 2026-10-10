//! OpenCode side of session transfer (1.18, measured on real data 2026-10).
//!
//! - Reader: the session's `message` / `part` rows, opened read-only. Part types:
//!   `text` → text (`ignored` ones skipped); `file` → inline image (anything
//!   else, e.g. PDF, is refused); `tool` → call plus result (`error` keeps its
//!   message, an interrupted `running` call keeps only the call); `subtask` →
//!   labelled user text. `reasoning`, `step-start`, `step-finish`, `patch` and
//!   `compaction` are not conversation. After a compaction OpenCode only sends
//!   the summary and what follows, so the reader starts there too.
//! - Writer: an export document handed to the official `opencode import`, run
//!   in the project folder because import assigns the project from the working
//!   directory. Measured:
//!   - every message needs its full set of fields; a missing one fails the
//!     import *after* part of the session was already written, so a failed or
//!     incomplete import is removed again with the official `session delete`
//!   - resuming without `-m` uses the model recorded on the messages, so they
//!     carry the model the user last used in OpenCode, not a made-up one
//!   - ids follow OpenCode's scheme (48-bit time-ordered prefix, ascending for
//!     messages and parts, descending for sessions) so messages keep their order

use super::{
    clean_dir, data_url_image, ConvertedSession, Image, Part, Role, SourceStamps, Transcript, Turn,
};
use crate::adapters::{self, cleanup, opencode as oc};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use uuid::Uuid;

/* ── reader ── */

pub(super) fn read(id: &str) -> Result<Transcript, String> {
    let db = oc::opencode_home().ok_or("session_not_found")?.join(oc::DB);
    let stamps = SourceStamps::opencode_session(&db, id)?;
    let con = oc::open(&db).ok_or("source_read_failed")?;
    // one read snapshot for the session, its messages and its parts
    con.execute_batch("BEGIN")
        .map_err(|e| format!("source_read_failed: {e}"))?;
    let (title, cwd): (String, String) = con
        .query_row(
            "SELECT COALESCE(title,''), COALESCE(directory,'') FROM session WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| format!("source_read_failed: {e}"))?
        .ok_or("session_not_found")?;
    if cwd.is_empty() {
        return Err("source_cwd_missing".into());
    }

    let mut stmt = con
        .prepare("SELECT message_id, data FROM part WHERE session_id = ?1 ORDER BY id")
        .map_err(|e| format!("source_read_failed: {e}"))?;
    let mut parts: HashMap<String, Vec<Value>> = HashMap::new();
    for row in stmt
        .query_map([id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| format!("source_read_failed: {e}"))?
    {
        let (message, data) = row.map_err(|e| format!("source_read_failed: {e}"))?;
        parts
            .entry(message)
            .or_default()
            .push(serde_json::from_str(&data).map_err(|_| "source_json_invalid")?);
    }
    let mut stmt = con
        .prepare("SELECT id, data FROM message WHERE session_id = ?1 ORDER BY time_created, id")
        .map_err(|e| format!("source_read_failed: {e}"))?;
    let messages: Vec<(String, Value)> = stmt
        .query_map([id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| format!("source_read_failed: {e}"))?
        .map(|row| {
            let (mid, data) = row.map_err(|e| format!("source_read_failed: {e}"))?;
            Ok((
                mid,
                serde_json::from_str(&data).map_err(|_| "source_json_invalid")?,
            ))
        })
        .collect::<Result<_, String>>()?;

    let empty = Vec::new();
    let start = compaction_start(&messages, &parts);
    let mut turns: Vec<Turn> = Vec::new();
    for (mid, info) in &messages[start..] {
        let role = match info["role"].as_str() {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            _ => return Err("unsupported_content".into()),
        };
        // same-role parts of one message form one turn
        let record_start = turns.len();
        for part in parts.get(mid).unwrap_or(&empty) {
            for (part_role, p) in map_part(part, role)? {
                let joinable = turns.len() > record_start;
                match turns.last_mut() {
                    Some(last) if joinable && last.role == part_role => last.parts.push(p),
                    _ => turns.push(Turn {
                        role: part_role,
                        parts: vec![p],
                    }),
                }
            }
        }
    }
    let title = Some(title)
        .filter(|t| !t.trim().is_empty())
        .or_else(|| super::title_from_turns(&turns))
        .unwrap_or_else(|| id.to_owned());
    Ok(Transcript {
        source_name: "OpenCode",
        cwd,
        title,
        turns,
        stamps,
    })
}

/// Index of the user message that started the last completed compaction, or 0
fn compaction_start(messages: &[(String, Value)], parts: &HashMap<String, Vec<Value>>) -> usize {
    let has_compaction = |mid: &str| {
        parts
            .get(mid)
            .is_some_and(|ps| ps.iter().any(|p| p["type"] == "compaction"))
    };
    for (i, (mid, info)) in messages.iter().enumerate().rev() {
        let summarised = messages[i + 1..]
            .iter()
            .any(|(_, m)| m["role"] == "assistant" && m["summary"] == true);
        if info["role"] == "user" && has_compaction(mid) && summarised {
            return i;
        }
    }
    0
}

/// One stored part → zero or more transcript parts
fn map_part(p: &Value, role: Role) -> Result<Vec<(Role, Part)>, String> {
    Ok(match p["type"].as_str().unwrap_or("") {
        "text" if p["ignored"] == true => vec![],
        "text" => vec![(
            role,
            Part::Text(p["text"].as_str().ok_or("unsupported_content")?.to_owned()),
        )],
        "file" => vec![(role, file_image(p)?)],
        "tool" => {
            let state = &p["state"];
            let id = p["callID"]
                .as_str()
                .ok_or("tool_call_id_missing")?
                .to_owned();
            let call = Part::ToolCall {
                id: id.clone(),
                name: p["tool"].as_str().ok_or("tool_name_missing")?.to_owned(),
                input: state["input"].to_string(),
            };
            // images a tool returned (the `read` tool on a screenshot) are `file` attachments
            let images = state["attachments"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| {
                    a["url"]
                        .as_str()
                        .and_then(data_url_image)
                        .ok_or_else(|| "unsupported_tool_output_media".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            let output = match state["status"].as_str() {
                Some("completed") => Some(state["output"].as_str().unwrap_or("").to_owned()),
                Some("error") => Some(format!("error: {}", state["error"].as_str().unwrap_or(""))),
                Some("running" | "pending") => None, // interrupted: the call happened, no result exists
                _ => return Err("unsupported_content".into()),
            };
            let mut out = vec![(Role::Assistant, call)];
            if let Some(output) = output {
                out.push((Role::Assistant, Part::ToolResult { id, output, images }));
            }
            out
        }
        "subtask" => vec![(
            Role::User,
            Part::Text(format!(
                "[OpenCode subtask · {}: {}]\n{}",
                p["agent"].as_str().unwrap_or("?"),
                p["description"].as_str().unwrap_or(""),
                p["prompt"].as_str().unwrap_or("")
            )),
        )],
        "reasoning" | "step-start" | "step-finish" | "patch" | "compaction" | "snapshot" => vec![],
        _ => return Err("unsupported_content".into()),
    })
}

/// `data:image/png;base64,…` file part → image; PDFs and links are refused
fn file_image(p: &Value) -> Result<Part, String> {
    let Image { media_type, data } = p["url"]
        .as_str()
        .and_then(data_url_image)
        .ok_or("unsupported_content")?;
    Ok(Part::Image { media_type, data })
}

/* ── writer ── */

const B62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// OpenCode-style ids: `<prefix>_` + 12 hex digits of (ms × 4096 + counter) kept to
/// 48 bits (inverted for sessions, so newer sorts first) + 14 random base62 characters
struct Ids {
    now_ms: u64,
    counter: u64,
}

impl Ids {
    fn next(&mut self, prefix: &str, descending: bool) -> String {
        const MASK: u64 = (1 << 48) - 1;
        self.counter += 1;
        let mut v = self.now_ms.wrapping_mul(0x1000).wrapping_add(self.counter) & MASK;
        if descending {
            v = !v & MASK;
        }
        let random: String = Uuid::new_v4().as_bytes()[..14]
            .iter()
            .map(|b| B62[*b as usize % 62] as char)
            .collect();
        format!("{prefix}_{v:012x}{random}")
    }
}

/// Model and OpenCode version from the user's most recent activity
fn recent_model_and_version(db: &Path) -> Result<(Value, String), String> {
    let con = oc::open(db).ok_or("opencode_model_unknown")?;
    let model: Value = con
        .query_row(
            "SELECT json_extract(data, '$.model') FROM message
             WHERE json_extract(data, '$.role') = 'user' AND json_extract(data, '$.model') IS NOT NULL
             ORDER BY time_created DESC LIMIT 1",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(|m: &Value| m["providerID"].is_string() && m["modelID"].is_string())
        .ok_or("opencode_model_unknown")?;
    let version = con
        .query_row(
            "SELECT version FROM session ORDER BY time_updated DESC LIMIT 1",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_else(|_| "1.18.31".into());
    Ok((model, version))
}

/// The document `opencode import` reads, and its session id
fn export_doc(
    t: &Transcript,
    cwd: &str,
    model: &Value,
    version: &str,
    now_ms: u64,
) -> Result<(String, Value), String> {
    let mut ids = Ids { now_ms, counter: 0 };
    let sid = ids.next("ses", true);
    let mut messages = Vec::new();
    let mut last_user: Option<String> = None;
    let mut time = now_ms;
    for turn in &t.turns {
        time += 1;
        let mid = ids.next("msg", false);
        let mut parts = Vec::new();
        for part in &turn.parts {
            let base = json!({"id": ids.next("prt", false), "sessionID": sid, "messageID": mid});
            let body = match (turn.role, part) {
                (Role::User, Part::Text(text)) => json!({"type": "text", "text": text}),
                (Role::User, Part::Image { media_type, data }) => {
                    let ext = media_type.trim_start_matches("image/");
                    json!({"type": "file", "mime": media_type, "filename": format!("image.{ext}"), "url": format!("data:{media_type};base64,{data}")})
                }
                (Role::Assistant, p) => {
                    json!({"type": "text", "text": super::claude::tool_record_text(p, t.source_name).ok_or("unsupported_assistant_content")?})
                }
                // tool records are assistant-side by construction
                (Role::User, _) => return Err("unsupported_content".into()),
            };
            let mut merged = base;
            merged
                .as_object_mut()
                .expect("object")
                .extend(body.as_object().expect("object").clone());
            parts.push(merged);
        }
        let info = match turn.role {
            Role::User => {
                last_user = Some(mid.clone());
                json!({"id": mid, "sessionID": sid, "role": "user", "time": {"created": time}, "agent": "build", "model": model})
            }
            Role::Assistant => {
                // OpenCode hangs every assistant message off the user message it answers
                let parent = last_user.clone().ok_or("unsupported_content")?;
                json!({"id": mid, "sessionID": sid, "role": "assistant", "parentID": parent,
                       "time": {"created": time, "completed": time},
                       "modelID": model["modelID"], "providerID": model["providerID"],
                       "mode": "build", "agent": "build", "path": {"cwd": cwd, "root": cwd}, "cost": 0,
                       "tokens": {"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}},
                       "finish": "stop"})
            }
        };
        messages.push(json!({"info": info, "parts": parts}));
    }
    let doc = json!({
        "info": {"id": sid, "slug": "orrery-transfer", "projectID": "global", "directory": cwd,
                 "title": t.title, "version": version, "time": {"created": now_ms, "updated": time}},
        "messages": messages,
    });
    Ok((sid, doc))
}

pub(super) fn write(t: &Transcript) -> Result<ConvertedSession, String> {
    let bin = cleanup::opencode_bin().ok_or("opencode_cli_missing")?;
    let home = oc::opencode_home().ok_or("opencode_model_unknown")?;
    let db = home.join(oc::DB);
    let (model, version) = recent_model_and_version(&db)?;
    let cwd = clean_dir(&t.cwd);
    let now_ms = time::OffsetDateTime::now_utc().unix_timestamp_nanos() as u64 / 1_000_000;
    let (sid, doc) = export_doc(t, &cwd, &model, &version, now_ms)?;
    let expected = doc["messages"].as_array().map_or(0, Vec::len) as i64;

    let stage_dir = adapters::data_dir()
        .ok_or("orrery_data_dir_missing")?
        .join("transfer-stage");
    fs::create_dir_all(&stage_dir).map_err(|e| format!("stage_create_failed: {e}"))?;
    let stage = stage_dir.join(format!("opencode-{sid}.json"));
    fs::write(&stage, doc.to_string()).map_err(|e| format!("stage_create_failed: {e}"))?;
    let result = (|| {
        t.stamps.verify_unchanged()?;
        let out = oc_command(&bin, &home, &cwd)
            .arg("import")
            .arg(&stage)
            .arg("--pure")
            .output()
            .map_err(|e| format!("opencode_start_failed: {e}"))?;
        if !out.status.success() || !String::from_utf8_lossy(&out.stdout).contains(&sid) {
            return Err("opencode_import_failed".to_string());
        }
        // import is not atomic: check that every message landed
        let landed = oc::open(&db)
            .and_then(|con| {
                con.query_row(
                    "SELECT COUNT(*) FROM message WHERE session_id = ?1",
                    [&sid],
                    |r| r.get::<_, i64>(0),
                )
                .ok()
            })
            .unwrap_or(-1);
        if landed != expected {
            return Err("opencode_import_incomplete".to_string());
        }
        Ok(())
    })();
    let _ = fs::remove_file(&stage);
    if let Err(e) = result {
        remove_partial(&bin, &home, &cwd, &db, &sid);
        return Err(e);
    }
    Ok(ConvertedSession {
        harness: "opencode".into(),
        id: sid,
        existing: false,
    })
}

/// Undo a half-written import through OpenCode's own delete, if anything landed
fn remove_partial(bin: &Path, home: &Path, cwd: &str, db: &Path, sid: &str) {
    let exists = oc::open(db)
        .and_then(|con| {
            con.query_row("SELECT 1 FROM session WHERE id = ?1", [sid], |_| Ok(()))
                .optional()
                .ok()
                .flatten()
        })
        .is_some();
    if exists {
        let _ = oc_command(bin, home, cwd)
            .args(["session", "delete", sid, "--pure"])
            .output();
    }
}

/// The opencode CLI pointed at the same data folder we read, running in `cwd`
fn oc_command(bin: &Path, home: &Path, cwd: &str) -> Command {
    let mut cmd = Command::new(bin);
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(data) = home.parent() {
        cmd.env("XDG_DATA_HOME", data);
    }
    cleanup::no_window(&mut cmd);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(turns: Vec<Turn>) -> Transcript {
        Transcript {
            source_name: "Codex",
            cwd: "D:/code/recipe-box".into(),
            title: "Import recipes".into(),
            turns,
            stamps: SourceStamps::files(&[]).unwrap(),
        }
    }

    #[test]
    fn ids_follow_opencode_ordering() {
        let mut ids = Ids {
            now_ms: 1_786_864_671_694,
            counter: 0,
        };
        let (a, b) = (ids.next("msg", false), ids.next("msg", false));
        assert!(a < b, "messages must sort in creation order: {a} {b}");
        assert_eq!(a.len(), "msg_".len() + 12 + 14);
        assert!(a[4..].bytes().all(|c| c.is_ascii_alphanumeric()));
        let (s1, s2) = (ids.next("ses", true), ids.next("ses", true));
        assert!(s1 > s2, "newer sessions sort first: {s1} {s2}");
        assert!(super::super::valid_id("opencode", &s1));
    }

    #[test]
    fn export_doc_has_every_field_import_requires() {
        let t = transcript(vec![
            Turn {
                role: Role::User,
                parts: vec![
                    Part::Text("hi".into()),
                    Part::Image {
                        media_type: "image/png".into(),
                        data: "AA==".into(),
                    },
                ],
            },
            Turn {
                role: Role::Assistant,
                parts: vec![Part::ToolCall {
                    id: "c1".into(),
                    name: "exec".into(),
                    input: "ls".into(),
                }],
            },
        ]);
        let model = json!({"providerID": "p", "modelID": "m", "variant": "high"});
        let (sid, doc) = export_doc(
            &t,
            "D:/code/recipe-box",
            &model,
            "1.18.31",
            1_786_864_671_694,
        )
        .unwrap();
        assert_eq!(doc["info"]["id"], sid);
        for key in ["slug", "projectID", "directory", "title", "version", "time"] {
            assert!(!doc["info"][key].is_null(), "session info lacks {key}");
        }
        let messages = doc["messages"].as_array().unwrap();
        let user = &messages[0];
        assert_eq!(user["info"]["model"], model);
        assert_eq!(user["parts"][1]["type"], "file");
        assert_eq!(user["parts"][1]["url"], "data:image/png;base64,AA==");
        let assistant = &messages[1]["info"];
        for key in [
            "parentID",
            "modelID",
            "providerID",
            "mode",
            "agent",
            "path",
            "cost",
            "tokens",
            "finish",
        ] {
            assert!(!assistant[key].is_null(), "assistant message lacks {key}");
        }
        assert_eq!(assistant["parentID"], user["info"]["id"]);
        assert_eq!(
            messages[1]["parts"][0]["text"],
            "[Codex tool call: exec · c1]\nls"
        );
        for part in messages.iter().flat_map(|m| m["parts"].as_array().unwrap()) {
            assert_eq!(part["sessionID"], sid);
        }
    }

    #[test]
    fn an_answer_without_a_question_is_refused() {
        let t = transcript(vec![Turn {
            role: Role::Assistant,
            parts: vec![Part::Text("hello".into())],
        }]);
        assert!(export_doc(
            &t,
            "D:/x",
            &json!({"providerID": "p", "modelID": "m"}),
            "1",
            1
        )
        .is_err());
    }

    #[test]
    fn parts_map_to_the_transcript_or_stop_the_transfer() {
        let tool = json!({"type":"tool","tool":"read","callID":"c1","state":{"status":"completed","input":{"path":"a"},"output":"body"}});
        let mapped = map_part(&tool, Role::Assistant).unwrap();
        assert_eq!(
            mapped[0].1,
            Part::ToolCall {
                id: "c1".into(),
                name: "read".into(),
                input: r#"{"path":"a"}"#.into()
            }
        );
        assert_eq!(
            mapped[1].1,
            Part::ToolResult {
                id: "c1".into(),
                output: "body".into(),
                images: vec![]
            }
        );
        let running = json!({"type":"tool","tool":"bash","callID":"c2","state":{"status":"running","input":{}}});
        assert_eq!(
            map_part(&running, Role::Assistant).unwrap().len(),
            1,
            "an interrupted call has no result"
        );
        let failed = json!({"type":"tool","tool":"bash","callID":"c3","state":{"status":"error","input":{},"error":"boom"}});
        assert_eq!(
            map_part(&failed, Role::Assistant).unwrap()[1].1,
            Part::ToolResult {
                id: "c3".into(),
                output: "error: boom".into(),
                images: vec![]
            }
        );
        for skipped in [
            "reasoning",
            "step-start",
            "step-finish",
            "patch",
            "compaction",
        ] {
            assert!(
                map_part(&json!({"type": skipped}), Role::Assistant)
                    .unwrap()
                    .is_empty(),
                "{skipped}"
            );
        }
        assert!(map_part(
            &json!({"type":"text","text":"x","ignored":true}),
            Role::User
        )
        .unwrap()
        .is_empty());
        let pdf = json!({"type":"file","mime":"application/pdf","url":"data:application/pdf;base64,AA=="});
        assert!(
            map_part(&pdf, Role::User).is_err(),
            "a PDF cannot be carried over"
        );
        assert!(map_part(&json!({"type":"brand-new-part"}), Role::User).is_err());
    }

    /// Every direction that involves OpenCode, with the real OpenCode and Codex
    /// CLIs, on synthetic sessions inside an isolated `ORRERY_HOME`. Run filtered:
    /// `cargo test --lib sandbox_opencode -- --ignored --nocapture`
    #[test]
    #[ignore = "requires the OpenCode and Codex CLIs and an isolated process-wide home"]
    fn sandbox_opencode_both_ways() {
        use super::super::{claude, convert_to, now_rfc3339};
        use crate::adapters::codex as codex_adapter;
        let home = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../.orrery/transfer-e2e")
            .join(Uuid::new_v4().to_string());
        let cwd = home.join("project");
        fs::create_dir_all(&cwd).unwrap();
        let cwd_s = cwd.to_str().unwrap().to_owned();
        std::env::set_var("ORRERY_HOME", &home);
        // keep the CLIs' config, state and cache in the sandbox too
        for (var, dir) in [
            ("XDG_CONFIG_HOME", "cfg"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_CACHE_HOME", "cache"),
        ] {
            std::env::set_var(var, home.join(dir));
        }
        let oc_home = home.join(".local/share/opencode");
        fs::create_dir_all(&oc_home).unwrap();
        let bin = cleanup::opencode_bin().expect("opencode CLI");

        // OpenCode needs a model the user actually used: seed one session with it
        let model = json!({"providerID": "fake", "modelID": "probe"});
        let seed = transcript(vec![
            Turn {
                role: Role::User,
                parts: vec![Part::Text("seed".into())],
            },
            Turn {
                role: Role::Assistant,
                parts: vec![Part::Text("seeded".into())],
            },
        ]);
        let (_, doc) = export_doc(&seed, &cwd_s, &model, "1.18.31", 1_786_000_000_000).unwrap();
        let seed_file = home.join("seed.json");
        fs::write(&seed_file, doc.to_string()).unwrap();
        assert!(oc_command(&bin, &oc_home, &cwd_s)
            .arg("import")
            .arg(&seed_file)
            .arg("--pure")
            .status()
            .unwrap()
            .success());
        let db = oc_home.join(oc::DB);
        let text_of = |sid: &str| -> String {
            let con = oc::open(&db).unwrap();
            let mut stmt = con
                .prepare("SELECT data FROM part WHERE session_id = ?1 ORDER BY id")
                .unwrap();
            let rows: Vec<String> = stmt
                .query_map([sid], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            rows.join("\n")
        };
        let exists = |sid: &str| {
            oc::open(&db)
                .unwrap()
                .query_row("SELECT 1 FROM session WHERE id = ?1", [sid], |_| Ok(()))
                .optional()
                .unwrap()
                .is_some()
        };

        // a Claude source: text, an inline image, a tool call and its result
        let cid = Uuid::new_v4().to_string();
        let claude_file =
            claude::project_dir(&home.join(".claude"), &cwd_s).join(format!("{cid}.jsonl"));
        fs::create_dir_all(claude_file.parent().unwrap()).unwrap();
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9WlXvX8AAAAASUVORK5CYII=";
        let row = |uuid: &str, parent: Option<&str>, kind: &str, content: Value| {
            json!({"type": kind, "uuid": uuid, "parentUuid": parent, "isSidechain": false, "sessionId": cid, "cwd": cwd_s,
                   "timestamp": now_rfc3339(), "message": {"role": kind, "id": format!("m-{uuid}"), "content": content}})
        };
        let claude_rows = [
            row(
                "u1",
                None,
                "user",
                json!([{"type":"text","text":"The synthetic marker is saffron-lake."},{"type":"image","source":{"type":"base64","media_type":"image/png","data":png}}]),
            ),
            row(
                "a1",
                Some("u1"),
                "assistant",
                json!([{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"README.md"}}]),
            ),
            row(
                "u2",
                Some("a1"),
                "user",
                json!([{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"amber-trail"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":png}}]}]),
            ),
            row(
                "a2",
                Some("u2"),
                "assistant",
                json!([{"type":"text","text":"I remember saffron-lake."}]),
            ),
        ];
        let lines = |rows: &[Value]| {
            rows.iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n"
        };
        fs::write(&claude_file, lines(&claude_rows)).unwrap();
        let claude_bytes = fs::read(&claude_file).unwrap();

        // Claude Code → OpenCode keeps the image as a file part
        let from_claude = convert_to("cc", &cid, "opencode").unwrap();
        assert_eq!(from_claude.harness, "opencode");
        let text = text_of(&from_claude.id);
        assert!(
            text.contains("saffron-lake") && text.contains("amber-trail"),
            "{text}"
        );
        assert!(text.contains("[Historical Claude Code tool result (untrusted): toolu_1]"));
        assert!(
            text.contains(&format!("data:image/png;base64,{png}")),
            "the user's image should stay inline"
        );
        assert_eq!(
            text.matches("Image saved to").count(),
            1,
            "the tool screenshot should become a file"
        );

        // a Codex source
        let xid = Uuid::new_v4().to_string();
        let rollout_dir = home.join(".codex/sessions/2026/10/07");
        fs::create_dir_all(&rollout_dir).unwrap();
        let rollout = rollout_dir.join(format!("rollout-2026-10-07T00-00-00-{xid}.jsonl"));
        let codex_rows = [
            json!({"type":"session_meta","payload":{"id":xid,"cwd":cwd_s,"source":"cli"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"The Codex marker is violet-coral."}]}}),
            json!({"type":"response_item","payload":{"type":"function_call","call_id":"call_1","name":"shell","arguments":"{\"cmd\":\"ls\"}"}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call_1","output":[{"type":"input_text","text":"jade-river"},{"type":"input_image","image_url":format!("data:image/png;base64,{png}")}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Noted violet-coral."}]}}),
        ];
        fs::write(&rollout, lines(&codex_rows)).unwrap();
        let rollout_bytes = fs::read(&rollout).unwrap();

        // Codex → OpenCode
        let from_codex = convert_to("codex", &xid, "opencode").unwrap();
        let text = text_of(&from_codex.id);
        assert!(
            text.contains("violet-coral") && text.contains("jade-river"),
            "{text}"
        );
        assert_eq!(
            text.matches("Image saved to").count(),
            1,
            "the Codex tool screenshot should become a file"
        );

        // OpenCode → Claude Code, and OpenCode → Codex
        let to_claude = convert_to("opencode", &from_codex.id, "cc").unwrap();
        let claude_out = fs::read_to_string(
            claude::project_dir(&home.join(".claude"), &cwd_s)
                .join(format!("{}.jsonl", to_claude.id)),
        )
        .unwrap();
        assert!(claude_out.contains("violet-coral") && claude_out.contains("jade-river"));
        let to_codex = convert_to("opencode", &from_codex.id, "codex").unwrap();
        assert_eq!(to_codex.harness, "codex");
        let codex_out: String = codex_adapter::collect_rollouts(&home.join(".codex/sessions"))
            .into_iter()
            .filter(|p| codex_adapter::read_head(p).is_some_and(|(id, _, _)| id == to_codex.id))
            .map(|p| fs::read_to_string(p).unwrap())
            .collect();
        assert!(
            codex_out.contains("violet-coral"),
            "Codex did not receive the history"
        );
        assert_eq!(
            fs::read_dir(home.join(".orrery/transfer-stage"))
                .unwrap()
                .count(),
            0,
            "staging left behind"
        );

        // Codex's importer drops images, so the inline user image becomes a file
        // too; the screenshot reference from the first hop travels as text
        let image_to_codex = convert_to("opencode", &from_claude.id, "codex").unwrap();
        let codex_out: String = codex_adapter::collect_rollouts(&home.join(".codex/sessions"))
            .into_iter()
            .filter(|p| {
                codex_adapter::read_head(p).is_some_and(|(id, _, _)| id == image_to_codex.id)
            })
            .map(|p| fs::read_to_string(p).unwrap())
            .collect();
        assert!(codex_out.contains("saffron-lake"));
        assert_eq!(
            super::super::media::history_image_refs(&codex_out),
            2,
            "{codex_out}"
        );
        assert!(
            !codex_out.contains(png),
            "no image data may reach Codex's importer"
        );
        let media_root = home.join(".orrery/transfer-media");
        let owners: Vec<Value> = fs::read_dir(&media_root)
            .unwrap()
            .flatten()
            .map(|e| {
                serde_json::from_str(&fs::read_to_string(e.path().join("owner.json")).unwrap())
                    .unwrap()
            })
            .collect();
        assert_eq!(
            owners.len(),
            3,
            "one folder per transfer that saved images: {owners:?}"
        );
        for dir in fs::read_dir(&media_root)
            .unwrap()
            .flatten()
            .map(|e| e.path())
        {
            let owner: Value =
                serde_json::from_str(&fs::read_to_string(dir.join("owner.json")).unwrap()).unwrap();
            let pngs = fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
                .count();
            assert_eq!(
                owner["images"].as_u64(),
                Some(pngs as u64),
                "owner.json must match the files in {}",
                dir.display()
            );
        }

        // a failed import leaves half a session behind; it must be removable
        let mut ids = Ids {
            now_ms: 1_786_000_100_000,
            counter: 0,
        };
        let bad_sid = ids.next("ses", true);
        let mut broken = doc.clone();
        broken["info"]["id"] = json!(bad_sid);
        for m in broken["messages"].as_array_mut().unwrap() {
            let mid = ids.next("msg", false);
            m["info"]["id"] = json!(mid);
            m["info"]["sessionID"] = json!(bad_sid);
            m["info"].as_object_mut().unwrap().remove("mode");
            for p in m["parts"].as_array_mut().unwrap() {
                p["id"] = json!(ids.next("prt", false));
                p["sessionID"] = json!(bad_sid);
                p["messageID"] = json!(mid);
            }
        }
        // the assistant must point at the renamed user message
        let first_user = broken["messages"][0]["info"]["id"].clone();
        broken["messages"][1]["info"]["parentID"] = first_user;
        let broken_file = home.join("broken.json");
        fs::write(&broken_file, broken.to_string()).unwrap();
        assert!(!oc_command(&bin, &oc_home, &cwd_s)
            .arg("import")
            .arg(&broken_file)
            .arg("--pure")
            .status()
            .unwrap()
            .success());
        assert!(
            exists(&bad_sid),
            "expected OpenCode to leave a partial session behind"
        );
        remove_partial(&bin, &oc_home, &cwd_s, &db, &bad_sid);
        assert!(!exists(&bad_sid), "the partial session was not removed");

        assert_eq!(
            fs::read(&claude_file).unwrap(),
            claude_bytes,
            "Claude source changed"
        );
        assert_eq!(
            fs::read(&rollout).unwrap(),
            rollout_bytes,
            "Codex source changed"
        );
        let result = json!({"home": home, "project": cwd, "opencode_from_claude": from_claude.id, "opencode_from_codex": from_codex.id,
                            "claude_from_opencode": to_claude.id, "codex_from_opencode": to_codex.id, "codex_with_image": image_to_codex.id});
        fs::write(home.join("opencode-result.json"), result.to_string()).unwrap();
        println!(
            "sandbox_result={}",
            home.join("opencode-result.json").display()
        );
    }

    #[test]
    fn reading_starts_at_the_last_finished_compaction() {
        let msg = |id: &str, role: &str, summary: bool| {
            (id.to_string(), json!({"role": role, "summary": summary}))
        };
        let messages = vec![
            msg("m1", "user", false),
            msg("m2", "assistant", false),
            msg("m3", "user", false),
            msg("m4", "assistant", true),
            msg("m5", "user", false),
        ];
        let mut parts = HashMap::new();
        parts.insert("m3".to_string(), vec![json!({"type":"compaction"})]);
        assert_eq!(compaction_start(&messages, &parts), 2);
        // a compaction whose summary never arrived does not cut the history
        let unfinished = vec![msg("m1", "user", false), msg("m3", "user", false)];
        assert_eq!(compaction_start(&unfinished, &parts), 0);
    }
}

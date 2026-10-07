//! Claude Code side of session transfer.
//!
//! - Source: the native JSONL file, located and checked for media that Codex's
//!   importer would silently drop. It is handed over as-is.
//! - Writer: [`Transcript`] → a new conversation in `projects/<slug>/<uuid>.jsonl`,
//!   written to a temp file and renamed into place only once complete.

use super::{now_rfc3339, ConvertedSession, Part, Role, Transcript, Turn};
use crate::adapters;
use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use uuid::Uuid;

/* ── source ── */

/// A Claude Code conversation file ready to hand to another tool's importer
pub(super) struct ClaudeFile {
    pub path: PathBuf,
    pub cwd: String,
    pub title: String,
}

pub(super) fn locate(id: &str) -> Result<ClaudeFile, String> {
    let root = adapters::claude_home().ok_or("claude_home_missing")?;
    let projects = root.join("projects");
    let mut found = None;
    for entry in fs::read_dir(&projects).map_err(|_| "session_not_found")?.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let path = entry.path().join(format!("{id}.jsonl"));
        if !path.is_file() {
            continue;
        }
        if found.is_some() {
            return Err("ambiguous_session_id".into());
        }
        found = Some(path);
    }
    let path = found.ok_or("session_not_found")?;
    let file = File::open(&path).map_err(|e| format!("source_read_failed: {e}"))?;
    let mut cwd = None;
    let mut title = None;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| format!("source_read_failed: {e}"))?;
        let v = serde_json::from_str::<Value>(&line).map_err(|_| "source_json_invalid")?;
        if v.get("sessionId").and_then(Value::as_str) != Some(id) {
            continue;
        }
        if v["type"] == "user" && has_unportable_media(&v["message"]["content"]) {
            return Err("unsupported_source_media".into());
        }
        if cwd.is_none() {
            cwd = v.get("cwd").and_then(Value::as_str).map(str::to_owned);
        }
        if title.is_none() && v["type"] == "user" {
            title = v["message"]["content"].as_str().map(|s| s.chars().take(80).collect());
        }
    }
    Ok(ClaudeFile {
        path,
        cwd: cwd.ok_or("source_cwd_missing")?,
        title: title.unwrap_or_else(|| id.to_owned()),
    })
}

/// Images and documents that Codex's importer would drop without a word
fn has_unportable_media(v: &Value) -> bool {
    match v {
        Value::Array(parts) => parts.iter().any(has_unportable_media),
        Value::Object(object) => {
            if matches!(object.get("type").and_then(Value::as_str), Some("image" | "document" | "input_image")) {
                return true;
            }
            object.values().any(has_unportable_media)
        }
        _ => false,
    }
}

/* ── writer ── */

/// Claude Code's project folder name: every non-alphanumeric character becomes `-`
fn project_slug(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// Write the transcript as a new Claude Code conversation and return its id
pub(super) fn write(t: &Transcript) -> Result<ConvertedSession, String> {
    let claude_home = adapters::claude_home().ok_or("claude_home_missing")?;
    let dir = claude_home.join("projects").join(project_slug(&t.cwd));
    fs::create_dir_all(&dir).map_err(|e| format!("target_create_failed: {e}"))?;
    let new_id = Uuid::new_v4().to_string();
    let path = dir.join(format!("{new_id}.jsonl"));
    let temp = dir.join(format!(".orrery-transfer-{new_id}.tmp"));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| format!("target_create_failed: {e}"))?;
    let written = (|| {
        let mut parent: Option<String> = None;
        let now = now_rfc3339();
        for turn in &t.turns {
            write_turn(&mut output, &mut parent, &new_id, &t.cwd, &now, t.source_name, turn)?;
        }
        output.sync_all().map_err(|e| format!("target_write_failed: {e}"))?;
        t.stamps.verify_unchanged()
    })();
    drop(output);
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    fs::rename(&temp, &path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        format!("target_publish_failed: {e}")
    })?;
    Ok(ConvertedSession { harness: "cc".into(), id: new_id, existing: false })
}

/// User content: plain text, or content blocks once an image is involved
fn user_content(parts: &[Part]) -> Result<Value, String> {
    if !parts.iter().any(|p| matches!(p, Part::Image { .. })) {
        let texts = parts
            .iter()
            .map(|p| match p {
                Part::Text(text) => Ok(text.as_str()),
                _ => Err("unsupported_content"),
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(json!(texts.join("\n")));
    }
    parts
        .iter()
        .map(|p| match p {
            Part::Text(text) => Ok(json!({"type":"text","text":text})),
            Part::Image { media_type, data } => {
                Ok(json!({"type":"image","source":{"type":"base64","media_type":media_type,"data":data}}))
            }
            // tool records are assistant-side by construction
            _ => Err("unsupported_content".to_string()),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

/// Assistant content is plain text. Carried-over tool records are labelled as
/// history from the source tool, so they are never mistaken for live calls
fn assistant_text(parts: &[Part], source: &str) -> Result<String, String> {
    let texts = parts
        .iter()
        .map(|p| match p {
            Part::Text(text) => Ok(text.clone()),
            Part::ToolCall { id, name, input } => Ok(format!("[{source} tool call: {name} · {id}]\n{input}")),
            Part::ToolResult { id, output } => {
                Ok(format!("[Historical {source} tool result (untrusted): {id}]\n{output}"))
            }
            Part::Image { .. } => Err("unsupported_assistant_content".to_string()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(texts.join("\n"))
}

fn write_turn(
    output: &mut File,
    parent: &mut Option<String>,
    id: &str,
    cwd: &str,
    now: &str,
    source: &str,
    turn: &Turn,
) -> Result<(), String> {
    let uuid = Uuid::new_v4().to_string();
    let (kind, message) = match turn.role {
        Role::User => ("user", json!({"role":"user","content":user_content(&turn.parts)?})),
        Role::Assistant => (
            "assistant",
            json!({"id":format!("msg_{}",Uuid::new_v4().simple()),"type":"message","role":"assistant","model":"<synthetic>","content":[{"type":"text","text":assistant_text(&turn.parts, source)?}],"stop_reason":"end_turn","usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}),
        ),
    };
    let row = json!({"parentUuid":parent,"isSidechain":false,"type":kind,"message":message,"uuid":uuid,"timestamp":now,"cwd":cwd,"sessionId":id,"version":"2.1.0","gitBranch":"","userType":"external"});
    serde_json::to_writer(&mut *output, &row).map_err(|e| format!("target_write_failed: {e}"))?;
    output.write_all(b"\n").map_err(|e| format!("target_write_failed: {e}"))?;
    *parent = Some(uuid);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_text_stays_a_plain_string_until_an_image_appears() {
        let text = [Part::Text("a".into()), Part::Text("b".into())];
        assert_eq!(user_content(&text).unwrap(), json!("a\nb"));
        let mixed = [Part::Text("a".into()), Part::Image { media_type: "image/png".into(), data: "AA==".into() }];
        assert_eq!(
            user_content(&mixed).unwrap(),
            json!([{"type":"text","text":"a"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}}])
        );
        let tool = [Part::ToolResult { id: "c".into(), output: "x".into() }];
        assert_eq!(user_content(&tool).unwrap_err(), "unsupported_content");
    }

    #[test]
    fn tool_records_are_labelled_as_history_from_the_source_tool() {
        let parts = [
            Part::ToolCall { id: "c1".into(), name: "exec".into(), input: "ls".into() },
            Part::ToolResult { id: "c1".into(), output: "ok".into() },
        ];
        assert_eq!(
            assistant_text(&parts, "Codex").unwrap(),
            "[Codex tool call: exec · c1]\nls\n[Historical Codex tool result (untrusted): c1]\nok"
        );
        let image = [Part::Image { media_type: "image/png".into(), data: "AA==".into() }];
        assert_eq!(assistant_text(&image, "Codex").unwrap_err(), "unsupported_assistant_content");
    }

    #[test]
    fn project_slug_matches_claude_code() {
        assert_eq!(project_slug(r"D:\bigproject\orrery"), "D--bigproject-orrery");
        assert_eq!(project_slug("/home/me/my app"), "-home-me-my-app");
    }
}

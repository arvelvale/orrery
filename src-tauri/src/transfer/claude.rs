//! Claude Code side of session transfer.
//!
//! - Reader: the conversation's *active branch*. A session file can fork (a
//!   rewind or an edited prompt starts a new branch), so the reader walks
//!   `parentUuid` back from the last message instead of taking the file in
//!   order. The walk stops at a compact boundary, whose `parentUuid` is empty:
//!   what is left is the compact summary plus everything after it, which is
//!   exactly what Claude Code itself sends to the model.
//! - Codex source: the native file, located and checked for media that Codex's
//!   importer would silently drop, then handed over as-is.
//! - Writer: [`Transcript`] → a new conversation in `projects/<slug>/<uuid>.jsonl`,
//!   written to a temp file and renamed into place only once complete.
//!
//! Only `user` and `assistant` records are conversation. The rest (attachments,
//! titles, mode switches, file-history snapshots, …) is metadata and skipped.

use super::{clean_dir, now_rfc3339, title_from_turns, ConvertedSession, Image, Part, Role, SourceStamps, Transcript, Turn};
use crate::adapters;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/* ── source ── */

/// The one `projects/<p>/<id>.jsonl` holding this session
fn find_file(id: &str) -> Result<PathBuf, String> {
    let root = adapters::claude_home().ok_or("claude_home_missing")?;
    let mut found = None;
    for entry in fs::read_dir(root.join("projects")).map_err(|_| "session_not_found")?.flatten() {
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
    found.ok_or_else(|| "session_not_found".into())
}

fn lines(path: &Path) -> Result<Vec<Value>, String> {
    let file = File::open(path).map_err(|e| format!("source_read_failed: {e}"))?;
    BufReader::new(file)
        .lines()
        .map(|line| {
            let line = line.map_err(|e| format!("source_read_failed: {e}"))?;
            serde_json::from_str::<Value>(&line).map_err(|_| "source_json_invalid".to_string())
        })
        .collect()
}

/// A Claude Code conversation file ready to hand to another tool's importer
pub(super) struct ClaudeFile {
    pub path: PathBuf,
    pub cwd: String,
    pub title: String,
}

pub(super) fn locate(id: &str) -> Result<ClaudeFile, String> {
    let path = find_file(id)?;
    let mut cwd = None;
    let mut title = None;
    for v in lines(&path)? {
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

/* ── reader ── */

pub(super) fn read(id: &str) -> Result<Transcript, String> {
    let path = find_file(id)?;
    let stamps = SourceStamps::files(std::slice::from_ref(&path))?;
    let (cwd, title, turns) = from_records(id, &lines(&path)?)?;
    Ok(Transcript { source_name: "Claude Code", cwd, title, turns, stamps })
}

/// `(cwd, title, turns)` of session `id` from the file's parsed lines
fn from_records(id: &str, records: &[Value]) -> Result<(String, String, Vec<Turn>), String> {
    let mut by_uuid: HashMap<&str, &Value> = HashMap::new();
    let mut leaf = None;
    let (mut custom_title, mut ai_title) = (None, None);
    for v in records {
        if v.get("sessionId").and_then(Value::as_str) != Some(id) {
            continue;
        }
        match v["type"].as_str() {
            Some("custom-title") => custom_title = v["customTitle"].as_str(),
            Some("ai-title") => ai_title = v["aiTitle"].as_str(),
            _ => {}
        }
        let Some(uuid) = v["uuid"].as_str() else { continue };
        by_uuid.insert(uuid, v);
        if matches!(v["type"].as_str(), Some("user" | "assistant")) && v["isSidechain"] != true {
            leaf = Some(uuid);
        }
    }

    // the active branch, newest first, then turned around
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut cursor = leaf;
    while let Some(uuid) = cursor {
        if !seen.insert(uuid) {
            return Err("source_json_invalid".into()); // a parentUuid cycle
        }
        let Some(record) = by_uuid.get(uuid) else { break };
        chain.push(*record);
        cursor = record["parentUuid"].as_str();
    }
    chain.reverse();

    let mut cwd = None;
    let mut turns: Vec<Turn> = Vec::new();
    let mut last_assistant_message: Option<&str> = None;
    for record in chain {
        if record["isSidechain"] == true {
            continue;
        }
        let kind = record["type"].as_str().unwrap_or("");
        if kind != "user" && kind != "assistant" {
            continue;
        }
        if cwd.is_none() {
            cwd = record["cwd"].as_str().map(str::to_owned);
        }
        let content = &record["message"]["content"];
        let parts = if kind == "user" { user_parts(content)? } else { assistant_parts(content)? };
        // one API response is written as several records sharing message.id
        let message_id = record["message"]["id"].as_str();
        let continues = kind == "assistant"
            && message_id.is_some()
            && message_id == last_assistant_message
            && turns.last().is_some_and(|t| t.role == Role::Assistant);
        last_assistant_message = if kind == "assistant" { message_id } else { None };
        // same-role parts of one record form one turn; across records only the
        // pieces of one assistant response are joined
        let record_start = turns.len();
        for (role, part) in parts {
            let joinable = turns.len() > record_start || continues;
            match turns.last_mut() {
                Some(last) if last.role == role && joinable => last.parts.push(part),
                _ => turns.push(Turn { role, parts: vec![part] }),
            }
        }
    }
    let title = custom_title
        .or(ai_title)
        .map(str::to_owned)
        .or_else(|| title_from_turns(&turns))
        .unwrap_or_else(|| id.to_owned());
    Ok((cwd.ok_or("source_cwd_missing")?, title, turns))
}

/// A user record. Tool results ride in user records but belong to the assistant side.
fn user_parts(content: &Value) -> Result<Vec<(Role, Part)>, String> {
    if let Some(text) = content.as_str() {
        return Ok(vec![(Role::User, Part::Text(text.to_owned()))]);
    }
    let blocks = content.as_array().ok_or("unsupported_content")?;
    blocks
        .iter()
        .map(|b| match b["type"].as_str().unwrap_or("") {
            "text" => Ok((Role::User, Part::Text(b["text"].as_str().ok_or("unsupported_content")?.to_owned()))),
            "image" => Ok((Role::User, image(b)?)),
            "tool_result" => {
                let (output, images) = tool_result(&b["content"])?;
                let id = b["tool_use_id"].as_str().ok_or("tool_call_id_missing")?.to_owned();
                Ok((Role::Assistant, Part::ToolResult { id, output, images }))
            }
            _ => Err("unsupported_content".to_string()),
        })
        .collect()
}

fn assistant_parts(content: &Value) -> Result<Vec<(Role, Part)>, String> {
    if let Some(text) = content.as_str() {
        return Ok(vec![(Role::Assistant, Part::Text(text.to_owned()))]);
    }
    let mut out = Vec::new();
    for b in content.as_array().ok_or("unsupported_content")? {
        match b["type"].as_str().unwrap_or("") {
            "text" => out.push((Role::Assistant, Part::Text(b["text"].as_str().ok_or("unsupported_content")?.to_owned()))),
            "tool_use" => out.push((
                Role::Assistant,
                Part::ToolCall {
                    id: b["id"].as_str().ok_or("tool_call_id_missing")?.to_owned(),
                    name: b["name"].as_str().ok_or("tool_name_missing")?.to_owned(),
                    input: b["input"].to_string(),
                },
            )),
            "image" => out.push((Role::Assistant, image(b)?)),
            "thinking" | "redacted_thinking" => {} // hidden reasoning is not exported
            _ => return Err("unsupported_content".into()),
        }
    }
    Ok(out)
}

/// An inline base64 image block; a URL or file reference cannot be carried over
fn base64_image(b: &Value) -> Option<Image> {
    let source = &b["source"];
    let media = source["media_type"].as_str().unwrap_or("");
    let data = source["data"].as_str().unwrap_or("");
    let ok = source["type"] == "base64"
        && matches!(media, "image/png" | "image/jpeg" | "image/gif" | "image/webp")
        && !data.is_empty()
        && data.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=');
    ok.then(|| Image { media_type: media.to_owned(), data: data.to_owned() })
}

fn image(b: &Value) -> Result<Part, String> {
    let Image { media_type, data } = base64_image(b).ok_or("unsupported_image")?;
    Ok(Part::Image { media_type, data })
}

/// Tool result content: its text, and any images the tool returned (screenshots).
/// The images are saved as files later; see `media.rs`.
fn tool_result(content: &Value) -> Result<(String, Vec<Image>), String> {
    match content {
        Value::Null => Ok((String::new(), vec![])),
        Value::String(s) => Ok((s.clone(), vec![])),
        Value::Array(items) => {
            let (mut texts, mut images) = (Vec::new(), Vec::new());
            for i in items {
                match i["type"].as_str().unwrap_or("") {
                    "text" => texts.push(i["text"].as_str().ok_or("unsupported_tool_output")?.to_owned()),
                    "tool_reference" => {
                        texts.push(format!("[tool reference: {}]", i["tool_name"].as_str().unwrap_or("?")))
                    }
                    "image" => images.push(base64_image(i).ok_or("unsupported_tool_output_media")?),
                    _ => return Err("unsupported_tool_output".into()),
                }
            }
            Ok((texts.join("\n"), images))
        }
        _ => Err("unsupported_tool_output".into()),
    }
}

/* ── writer ── */

/// Claude Code's project folder name: every non-alphanumeric character becomes `-`
fn project_slug(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// `<claude home>/projects/<slug>` for a working directory
pub(super) fn project_dir(home: &Path, cwd: &str) -> PathBuf {
    home.join("projects").join(project_slug(&clean_dir(cwd)))
}

/// Write the transcript as a new Claude Code conversation and return its id
pub(super) fn write(t: &Transcript) -> Result<ConvertedSession, String> {
    let claude_home = adapters::claude_home().ok_or("claude_home_missing")?;
    let dir = project_dir(&claude_home, &t.cwd);
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
        render(&mut output, t, &new_id)?;
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

/// The transcript as Claude Code JSONL rows for session `id`
pub(super) fn render(output: &mut File, t: &Transcript, id: &str) -> Result<(), String> {
    let cwd = clean_dir(&t.cwd);
    let mut parent: Option<String> = None;
    let now = now_rfc3339();
    for turn in &t.turns {
        write_turn(output, &mut parent, id, &cwd, &now, t.source_name, turn)?;
    }
    Ok(())
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
pub(super) fn assistant_text(parts: &[Part], source: &str) -> Result<String, String> {
    let texts = parts
        .iter()
        .map(|p| tool_record_text(p, source).ok_or_else(|| "unsupported_assistant_content".to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(texts.join("\n"))
}

/// Text and tool records as history text; images have no text form
pub(super) fn tool_record_text(part: &Part, source: &str) -> Option<String> {
    match part {
        Part::Text(text) => Some(text.clone()),
        Part::ToolCall { id, name, input } => Some(format!("[{source} tool call: {name} · {id}]\n{input}")),
        // images are saved as files before writing; one still here is a bug, not content to drop
        Part::ToolResult { id, output, images } if images.is_empty() => {
            Some(format!("[Historical {source} tool result (untrusted): {id}]\n{output}"))
        }
        Part::ToolResult { .. } => None,
        Part::Image { .. } => None,
    }
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

    const S: &str = "11111111-1111-4111-8111-111111111111";

    fn rec(uuid: &str, parent: Option<&str>, kind: &str, content: Value) -> Value {
        json!({"type": kind, "uuid": uuid, "parentUuid": parent, "sessionId": S, "cwd": "D:/code/acme-web",
               "message": {"role": kind, "id": format!("msg-{uuid}"), "content": content}})
    }

    #[test]
    fn only_the_active_branch_is_read() {
        // u1 → a1 → u2(old prompt) → a2   and an edit u2b → a2b that replaced it
        let records = vec![
            rec("u1", None, "user", json!("start")),
            rec("a1", Some("u1"), "assistant", json!([{"type":"text","text":"ok"}])),
            rec("u2", Some("a1"), "user", json!("abandoned prompt")),
            rec("a2", Some("u2"), "assistant", json!([{"type":"text","text":"abandoned answer"}])),
            rec("u2b", Some("a1"), "user", json!("edited prompt")),
            rec("a2b", Some("u2b"), "assistant", json!([{"type":"text","text":"kept answer"}])),
        ];
        let (cwd, _, turns) = from_records(S, &records).unwrap();
        assert_eq!(cwd, "D:/code/acme-web");
        let text = format!("{turns:?}");
        assert!(text.contains("edited prompt") && text.contains("kept answer"));
        assert!(!text.contains("abandoned"), "a rewound branch leaked into the transfer");
    }

    #[test]
    fn reading_stops_at_a_compact_boundary() {
        let mut boundary = json!({"type":"system","subtype":"compact_boundary","uuid":"b","parentUuid":null,"logicalParentUuid":"a1","sessionId":S,"cwd":"D:/x"});
        boundary["content"] = json!("Conversation compacted");
        let records = vec![
            rec("u1", None, "user", json!("before compaction")),
            rec("a1", Some("u1"), "assistant", json!([{"type":"text","text":"old"}])),
            boundary,
            rec("s", Some("b"), "user", json!("Summary of the earlier conversation")),
            rec("a2", Some("s"), "assistant", json!([{"type":"text","text":"after"}])),
        ];
        let (_, _, turns) = from_records(S, &records).unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].parts, vec![Part::Text("Summary of the earlier conversation".into())]);
    }

    #[test]
    fn tool_results_move_to_the_assistant_side_and_responses_are_joined() {
        let mut a1 = rec("a1", Some("u1"), "assistant", json!([{"type":"thinking","thinking":"hidden"},{"type":"text","text":"reading"}]));
        let mut a1b = rec("a1b", Some("a1"), "assistant", json!([{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"a.md"}}]));
        a1["message"]["id"] = json!("msg-same");
        a1b["message"]["id"] = json!("msg-same");
        let records = vec![
            rec("u1", None, "user", json!("read a.md")),
            a1,
            a1b,
            rec("u2", Some("a1b"), "user", json!([{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"body"}]}])),
        ];
        let (_, title, turns) = from_records(S, &records).unwrap();
        assert_eq!(title, "read a.md");
        assert_eq!(turns.len(), 3, "{turns:?}");
        assert_eq!(turns[1].role, Role::Assistant);
        assert_eq!(
            turns[1].parts,
            vec![
                Part::Text("reading".into()),
                Part::ToolCall { id: "t1".into(), name: "Read".into(), input: r#"{"file_path":"a.md"}"#.into() },
            ],
            "the two records of one response form one turn, thinking dropped"
        );
        assert_eq!(turns[2].role, Role::Assistant, "a tool result must never become a user turn");
        assert_eq!(turns[2].parts, vec![Part::ToolResult { id: "t1".into(), output: "body".into(), images: vec![] }]);
    }

    #[test]
    fn titles_prefer_the_ones_claude_code_shows() {
        let mut records = vec![rec("u1", None, "user", json!("first words"))];
        assert_eq!(from_records(S, &records).unwrap().1, "first words");
        records.push(json!({"type":"ai-title","aiTitle":"Generated title","sessionId":S}));
        assert_eq!(from_records(S, &records).unwrap().1, "Generated title");
        records.push(json!({"type":"custom-title","customTitle":"Named by the user","sessionId":S}));
        assert_eq!(from_records(S, &records).unwrap().1, "Named by the user");
    }

    #[test]
    fn screenshots_travel_but_linked_media_stops_the_transfer() {
        let png = json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}});
        let ok = vec![rec("u1", None, "user", json!([{"type":"text","text":"see"}, png.clone()]))];
        assert!(matches!(from_records(S, &ok).unwrap().2[0].parts[1], Part::Image { .. }));
        let screenshot = vec![
            rec("u1", None, "user", json!("go")),
            rec("u2", Some("u1"), "user", json!([{"type":"tool_result","tool_use_id":"t","content":[{"type":"text","text":"captured"}, png]}])),
        ];
        let turns = from_records(S, &screenshot).unwrap().2;
        assert_eq!(
            turns[1].parts,
            vec![Part::ToolResult {
                id: "t".into(),
                output: "captured".into(),
                images: vec![Image { media_type: "image/png".into(), data: "AA==".into() }],
            }]
        );
        let linked_shot = vec![rec("u1", None, "user", json!([{"type":"tool_result","tool_use_id":"t","content":[{"type":"image","source":{"type":"url","url":"https://x/a.png"}}]}]))];
        assert_eq!(from_records(S, &linked_shot).unwrap_err(), "unsupported_tool_output_media");
        let linked = vec![rec("u1", None, "user", json!([{"type":"image","source":{"type":"url","url":"https://x/a.png"}}]))];
        assert_eq!(from_records(S, &linked).unwrap_err(), "unsupported_image");
        let unknown = vec![rec("u1", None, "user", json!("q")), rec("a1", Some("u1"), "assistant", json!([{"type":"server_tool_use"}]))];
        assert_eq!(from_records(S, &unknown).unwrap_err(), "unsupported_content");
    }

    #[test]
    fn user_text_stays_a_plain_string_until_an_image_appears() {
        let text = [Part::Text("a".into()), Part::Text("b".into())];
        assert_eq!(user_content(&text).unwrap(), json!("a\nb"));
        let mixed = [Part::Text("a".into()), Part::Image { media_type: "image/png".into(), data: "AA==".into() }];
        assert_eq!(
            user_content(&mixed).unwrap(),
            json!([{"type":"text","text":"a"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}}])
        );
        let tool = [Part::ToolResult { id: "c".into(), output: "x".into(), images: vec![] }];
        assert_eq!(user_content(&tool).unwrap_err(), "unsupported_content");
    }

    #[test]
    fn tool_records_are_labelled_as_history_from_the_source_tool() {
        let parts = [
            Part::ToolCall { id: "c1".into(), name: "exec".into(), input: "ls".into() },
            Part::ToolResult { id: "c1".into(), output: "ok".into(), images: vec![] },
        ];
        assert_eq!(
            assistant_text(&parts, "Codex").unwrap(),
            "[Codex tool call: exec · c1]\nls\n[Historical Codex tool result (untrusted): c1]\nok"
        );
        let image = [Part::Image { media_type: "image/png".into(), data: "AA==".into() }];
        assert_eq!(assistant_text(&image, "Codex").unwrap_err(), "unsupported_assistant_content");
    }

    #[test]
    fn project_folder_matches_claude_code_and_ignores_dot_segments() {
        assert_eq!(project_slug(r"D:\bigproject\orrery"), "D--bigproject-orrery");
        assert_eq!(project_slug("/home/me/my app"), "-home-me-my-app");
        // paths in this platform's own form: on macOS/Linux `\` is an ordinary
        // character, not a separator, and no harness there records such a path
        let home = Path::new("H");
        let path = |parts: &[&str]| parts.join(std::path::MAIN_SEPARATOR_STR);
        assert_eq!(
            project_dir(home, &path(&["", "work", "a", "src-tauri", "..", ".orrery", "p"])),
            project_dir(home, &path(&["", "work", "a", ".orrery", "p"]))
        );
        assert_eq!(super::clean_dir(&path(&["", "work", ".", "a"])), path(&["", "work", "a"]));
    }
}

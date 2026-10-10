//! Codex side of session transfer.
//!
//! - Reader: a thread's main rollout files → [`Transcript`]
//! - Writer: Codex's own app-server importer (`externalAgentConfig/import`),
//!   which accepts Claude Code JSONL. Codex creates the rollout itself; importing
//!   the same source version again returns the existing target.

use super::{data_url_image, Image, Part, Role, SourceStamps, Transcript, Turn};
use crate::adapters::{self, cleanup, codex};
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use uuid::Uuid;

use super::ConvertedSession;

/* ── reader ── */

/// A Codex thread as a transcript. Subagent rollouts are not part of it.
pub(super) fn read(id: &str) -> Result<Transcript, String> {
    let codex_home = adapters::codex_home().ok_or("codex_home_missing")?;
    let mut files: Vec<_> = codex::collect_rollouts(&codex_home.join("sessions"))
        .into_iter()
        .filter(|p| codex::read_head(p).is_some_and(|(found, sub, _)| found == id && !sub))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err("session_not_found".into());
    }
    let stamps = SourceStamps::files(&files)?;

    let mut cwd: Option<String> = None;
    for file in &files {
        let mut head = String::new();
        BufReader::new(File::open(file).map_err(|e| format!("source_read_failed: {e}"))?)
            .read_line(&mut head)
            .map_err(|e| format!("source_read_failed: {e}"))?;
        let meta: Value = serde_json::from_str(&head).map_err(|_| "source_json_invalid")?;
        let this_cwd = meta["payload"]["cwd"]
            .as_str()
            .ok_or("source_cwd_missing")?;
        if cwd.as_deref().is_some_and(|old| old != this_cwd) {
            return Err("multiple_project_dirs_unsupported".into());
        }
        cwd = Some(this_cwd.to_owned());
    }
    let cwd = cwd.ok_or("source_cwd_missing")?;

    let mut turns = Vec::new();
    let mut first_user = None;
    for file in &files {
        for line in
            BufReader::new(File::open(file).map_err(|e| format!("source_read_failed: {e}"))?)
                .lines()
        {
            let line = line.map_err(|e| format!("source_read_failed: {e}"))?;
            let v: Value = serde_json::from_str(&line).map_err(|_| "source_json_invalid")?;
            if v["type"] != "response_item" {
                continue;
            }
            if first_user.is_none() {
                // skips the context Codex injects as "user" messages
                first_user = codex::user_text(&v["payload"]);
            }
            if let Some(turn) = turn(&v["payload"])? {
                turns.push(turn);
            }
        }
    }
    let title = codex::thread_names(&codex_home)
        .remove(id)
        .or_else(|| first_user.map(|t| t.chars().take(80).collect()))
        .unwrap_or_else(|| id.to_owned());
    Ok(Transcript {
        source_name: "Codex",
        cwd,
        title,
        turns,
        stamps,
    })
}

/// One `response_item` payload → a turn, `None` for items that are not conversation
fn turn(p: &Value) -> Result<Option<Turn>, String> {
    let turn = match p["type"].as_str().unwrap_or("") {
        "message" => {
            let role = match p["role"].as_str().unwrap_or("") {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                _ => return Ok(None), // developer / system prompts are not conversation
            };
            let items = p["content"].as_array().ok_or("unsupported_content")?;
            let mut parts = Vec::with_capacity(items.len());
            for part in items {
                match part["type"].as_str().unwrap_or("") {
                    "input_text" | "output_text" | "text" => {
                        parts.push(Part::Text(
                            part["text"]
                                .as_str()
                                .ok_or("unsupported_content")?
                                .to_owned(),
                        ));
                    }
                    "input_image" if role == Role::User => parts.push(image(part)?),
                    _ => return Err("unsupported_content".into()),
                }
            }
            if parts.is_empty() {
                return Ok(None);
            }
            Turn { role, parts }
        }
        "function_call" | "custom_tool_call" => {
            let input = p["arguments"]
                .as_str()
                .or_else(|| p["input"].as_str())
                .ok_or("tool_input_missing")?;
            Turn {
                role: Role::Assistant,
                parts: vec![Part::ToolCall {
                    id: p["call_id"]
                        .as_str()
                        .ok_or("tool_call_id_missing")?
                        .to_owned(),
                    name: p["name"].as_str().ok_or("tool_name_missing")?.to_owned(),
                    input: input.to_owned(),
                }],
            }
        }
        "function_call_output" | "custom_tool_call_output" => {
            let (output, images) = tool_output(&p["output"])?;
            Turn {
                role: Role::Assistant,
                parts: vec![Part::ToolResult {
                    id: p["call_id"]
                        .as_str()
                        .ok_or("tool_call_id_missing")?
                        .to_owned(),
                    output,
                    images,
                }],
            }
        }
        "reasoning" => return Ok(None), // hidden reasoning is not exported
        _ => return Err("unsupported_response_item".into()),
    };
    Ok(Some(turn))
}

/// `data:image/png;base64,…` → an image part; anything else is refused
fn image(part: &Value) -> Result<Part, String> {
    let Image { media_type, data } = part["image_url"]
        .as_str()
        .and_then(data_url_image)
        .ok_or("unsupported_image")?;
    Ok(Part::Image { media_type, data })
}

/// Tool output: its text, and any images it returned (screenshots), which are
/// saved as files later; see `media.rs`
pub(super) fn tool_output(output: &Value) -> Result<(String, Vec<Image>), String> {
    if let Some(text) = output.as_str() {
        return Ok((text.to_owned(), vec![]));
    }
    let parts = output.as_array().ok_or("unsupported_tool_output")?;
    let (mut texts, mut images) = (Vec::new(), Vec::new());
    for part in parts {
        match part["type"].as_str().unwrap_or("") {
            "input_text" | "output_text" | "text" => texts.push(
                part["text"]
                    .as_str()
                    .ok_or("unsupported_tool_output")?
                    .to_owned(),
            ),
            "input_image" => images.push(
                part["image_url"]
                    .as_str()
                    .and_then(data_url_image)
                    .ok_or("unsupported_tool_output_media")?,
            ),
            _ => return Err("unsupported_tool_output_media".into()),
        }
    }
    Ok((texts.join("\n"), images))
}

/* ── writer ── */

/// A custom CLAUDE_CONFIG_DIR is not discovered by Codex's importer. Expose
/// one source file in an isolated, short-lived HOME; keep CODEX_HOME real.
struct StagedClaudeSource {
    home: PathBuf,
    file: PathBuf,
}

impl Drop for StagedClaudeSource {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.file);
        let mut dir = self.file.parent();
        while let Some(path) = dir {
            let _ = fs::remove_dir(path); // only empty stage directories
            if path == self.home {
                break;
            }
            dir = path.parent();
        }
    }
}

fn stage_claude_source(source: &Path, id: &str) -> Result<StagedClaudeSource, String> {
    let home = adapters::data_dir()
        .ok_or("orrery_data_dir_missing")?
        .join("transfer-stage")
        // Codex deduplicates by source path. Keep this path stable across
        // attempts, even though Drop removes its contents after each import.
        .join(id);
    let project = source
        .parent()
        .and_then(Path::file_name)
        .ok_or("source_project_missing")?;
    let file = home
        .join(".claude/projects")
        .join(project)
        .join(format!("{id}.jsonl"));
    fs::create_dir_all(file.parent().ok_or("stage_path_invalid")?)
        .map_err(|e| format!("stage_create_failed: {e}"))?;
    if file.exists() {
        fs::remove_file(&file).map_err(|e| format!("stage_cleanup_failed: {e}"))?;
    }
    let stage = StagedClaudeSource { home, file };
    if fs::hard_link(source, &stage.file).is_err() {
        fs::copy(source, &stage.file).map_err(|e| format!("stage_copy_failed: {e}"))?;
    }
    Ok(stage)
}

/// Import one Claude Code JSONL file into Codex through Codex's own importer
pub(super) fn import_claude_file(
    source: &Path,
    cwd: &str,
    title: &str,
    id: &str,
) -> Result<ConvertedSession, String> {
    if !Path::new(cwd).is_dir() {
        return Err("cwd_missing".into());
    }
    let home = adapters::home_dir().ok_or("home_missing")?;
    // Codex currently discovers Claude sessions only under HOME/.claude/projects.
    let default_projects = home.join(".claude/projects");
    let source_projects = source
        .parent()
        .and_then(Path::parent)
        .ok_or("source_path_invalid")?;
    let default_home =
        fs::canonicalize(source_projects).ok() == fs::canonicalize(&default_projects).ok();
    let staged = if default_home {
        None
    } else {
        Some(stage_claude_source(source, id)?)
    };
    let import_path = staged.as_ref().map_or(source, |s| s.file.as_path());
    let import_home = staged.as_ref().map_or(home.as_path(), |s| s.home.as_path());
    run_import(source, import_path, import_home, cwd, title)
}

/// Any other source: render Claude JSONL into a staging HOME, then import that.
/// Codex's importer drops images from Claude files without a word, so a
/// transcript that has any is refused rather than silently thinned.
pub(super) fn import_transcript(t: &Transcript) -> Result<ConvertedSession, String> {
    if t.has_images() {
        return Err("unsupported_source_media".into());
    }
    let id = Uuid::new_v4().to_string();
    let home = adapters::data_dir()
        .ok_or("orrery_data_dir_missing")?
        .join("transfer-stage")
        .join(&id);
    let file =
        super::claude::project_dir(&home.join(".claude"), &t.cwd).join(format!("{id}.jsonl"));
    fs::create_dir_all(file.parent().ok_or("stage_path_invalid")?)
        .map_err(|e| format!("stage_create_failed: {e}"))?;
    // removes the rendered file and its empty folders whatever happens next
    let stage = StagedClaudeSource { home, file };
    let mut output = File::create(&stage.file).map_err(|e| format!("stage_create_failed: {e}"))?;
    super::claude::render(&mut output, t, &id)?;
    output
        .sync_all()
        .map_err(|e| format!("stage_create_failed: {e}"))?;
    drop(output);
    t.stamps.verify_unchanged()?;
    run_import(
        &stage.file,
        &stage.file,
        &stage.home,
        &super::clean_dir(&t.cwd),
        &t.title,
    )
}

/// Drive `codex app-server` through one `externalAgentConfig/import`.
/// `source` is watched for changes; `import_path` under `import_home` is what Codex reads.
fn run_import(
    source: &Path,
    import_path: &Path,
    import_home: &Path,
    cwd: &str,
    title: &str,
) -> Result<ConvertedSession, String> {
    let codex_home = adapters::codex_home().ok_or("codex_home_missing")?;
    fs::create_dir_all(&codex_home).map_err(|e| format!("target_create_failed: {e}"))?;
    let exe = cleanup::codex_bin().ok_or("codex_cli_missing")?;
    let before = fs::metadata(source).map_err(|e| format!("source_read_failed: {e}"))?;
    let mut cmd = Command::new(exe);
    cmd.args(["app-server", "--stdio"])
        .env("CODEX_HOME", &codex_home)
        .env("HOME", import_home)
        .env("USERPROFILE", import_home)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cleanup::no_window(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("codex_start_failed: {e}"))?;
    let stdout = child.stdout.take().ok_or("codex_stdout_missing")?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let result = (|| {
        let mut stdin = child.stdin.take().ok_or("codex_stdin_missing")?;
        send(
            &mut stdin,
            &json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"orrery","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}}),
        )?;
        let init = recv_id(&rx, 1)?;
        if init.get("error").is_some() {
            return Err("codex_initialize_failed".into());
        }
        send(&mut stdin, &json!({"method":"initialized"}))?;
        send(
            &mut stdin,
            &json!({"id":2,"method":"externalAgentConfig/import","params":{"migrationItems":[{"itemType":"SESSIONS","description":"Orrery session transfer","cwd":null,"details":{"sessions":[{"path":import_path,"cwd":cwd,"title":title}]}}],"source":"orrery","providerId":"orrery","migrationSource":"claude"}}),
        )?;
        loop {
            let line = rx
                .recv_timeout(Duration::from_secs(120))
                .map_err(|_| "codex_import_timeout")?;
            let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if msg.get("id").and_then(Value::as_i64) == Some(2) && msg.get("error").is_some() {
                return Err("codex_import_rejected".into());
            }
            if msg.get("method").and_then(Value::as_str)
                != Some("externalAgentConfig/import/completed")
            {
                continue;
            }
            let result = &msg["params"]["itemTypeResults"][0];
            if let Some(reason) = result["failures"][0]["reason"].as_str() {
                return Err(format!("codex_import_failed: {reason}"));
            }
            let after = fs::metadata(source).map_err(|_| "source_changed_during_import")?;
            if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
                return Err("source_changed_during_import".into());
            }
            if let Some(target) = result["successes"][0]["target"].as_str() {
                let target = Uuid::parse_str(target)
                    .map_err(|_| "codex_import_bad_target")?
                    .to_string();
                if !codex_target_exists(&codex_home, &target) {
                    return Err("codex_target_not_found".into());
                }
                return Ok(ConvertedSession {
                    harness: "codex".into(),
                    id: target,
                    existing: false,
                });
            }
            // An empty success/failure pair means a prior import was skipped. Resolve
            // it only when the source has not changed since that completed import.
            send(
                &mut stdin,
                &json!({"id":3,"method":"externalAgentConfig/import/readHistories"}),
            )?;
            let histories = recv_id(&rx, 3)?;
            if histories.get("error").is_some() {
                return Err("codex_history_failed".into());
            }
            let canonical = fs::canonicalize(import_path).map_err(|_| "source_read_failed")?;
            let source_ms = before
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .ok_or("source_mtime_missing")?;
            let prior = histories["result"]["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|history| {
                    history["completedAtMs"]
                        .as_u64()
                        .is_some_and(|ms| ms >= source_ms)
                })
                .flat_map(|history| history["successes"].as_array().into_iter().flatten())
                .filter_map(|entry| {
                    let path = entry["source"].as_str()?;
                    if fs::canonicalize(path).ok()? != canonical {
                        return None;
                    }
                    let target = Uuid::parse_str(entry["target"].as_str()?).ok()?.to_string();
                    codex_target_exists(&codex_home, &target).then_some(target)
                })
                .next_back();
            return prior
                .map(|id| ConvertedSession {
                    harness: "codex".into(),
                    id,
                    existing: true,
                })
                .ok_or("codex_import_no_target".into());
        }
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn codex_target_exists(home: &Path, id: &str) -> bool {
    codex::collect_rollouts(&home.join("sessions"))
        .iter()
        .any(|p| codex::read_head(p).is_some_and(|(found, _, _)| found == id))
}

fn send(w: &mut impl Write, v: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *w, v).map_err(|e| format!("codex_ipc_failed: {e}"))?;
    w.write_all(b"\n")
        .and_then(|_| w.flush())
        .map_err(|e| format!("codex_ipc_failed: {e}"))
}

fn recv_id(rx: &mpsc::Receiver<String>, id: i64) -> Result<Value, String> {
    loop {
        let line = rx
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| "codex_initialize_timeout")?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if v.get("id").and_then(Value::as_i64) == Some(id) {
            return Ok(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9WlXvX8AAAAASUVORK5CYII=";

    #[test]
    fn messages_keep_part_order_and_images_only_from_the_user() {
        let user = json!({"type":"message","role":"user","content":[
            {"type":"input_text","text":"look"},
            {"type":"input_image","image_url":format!("data:image/png;base64,{PNG}")},
            {"type":"input_text","text":"here"}]});
        let t = turn(&user).unwrap().unwrap();
        assert_eq!(t.role, Role::User);
        assert_eq!(
            t.parts,
            vec![
                Part::Text("look".into()),
                Part::Image {
                    media_type: "image/png".into(),
                    data: PNG.into()
                },
                Part::Text("here".into()),
            ]
        );
        let assistant_image = json!({"type":"message","role":"assistant","content":[
            {"type":"input_image","image_url":format!("data:image/png;base64,{PNG}")}]});
        assert_eq!(turn(&assistant_image).unwrap_err(), "unsupported_content");
    }

    #[test]
    fn tool_records_are_assistant_side_and_reasoning_is_dropped() {
        let call =
            json!({"type":"custom_tool_call","call_id":"c1","name":"functions.exec","input":"ls"});
        assert_eq!(
            turn(&call).unwrap().unwrap(),
            Turn {
                role: Role::Assistant,
                parts: vec![Part::ToolCall {
                    id: "c1".into(),
                    name: "functions.exec".into(),
                    input: "ls".into()
                }]
            }
        );
        let result = json!({"type":"function_call_output","call_id":"c1","output":"done"});
        let t = turn(&result).unwrap().unwrap();
        assert_eq!(
            t.role,
            Role::Assistant,
            "a tool result must never become a user turn"
        );
        assert_eq!(
            t.parts,
            vec![Part::ToolResult {
                id: "c1".into(),
                output: "done".into(),
                images: vec![]
            }]
        );
        assert!(turn(&json!({"type":"reasoning","summary":[]}))
            .unwrap()
            .is_none());
        assert!(
            turn(&json!({"type":"message","role":"developer","content":[]}))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unknown_items_and_odd_images_stop_the_transfer() {
        assert_eq!(
            turn(&json!({"type":"web_search_call"})).unwrap_err(),
            "unsupported_response_item"
        );
        for url in [
            "https://example.com/a.png",
            "data:image/svg+xml;base64,PHN2Zz4=",
            "data:image/png;base64,",
        ] {
            let bad = json!({"type":"message","role":"user","content":[{"type":"input_image","image_url":url}]});
            assert_eq!(turn(&bad).unwrap_err(), "unsupported_image", "{url}");
        }
    }
}

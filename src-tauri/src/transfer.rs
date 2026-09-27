//! Native session copies between Claude Code and Codex. Sources are never modified.
//! Claude -> Codex uses Codex's own external-agent importer; Codex -> Claude writes
//! a new Claude JSONL conversation and verifies that the adapter can read it.

use crate::adapters::{self, cleanup, codex};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex};
use std::thread;
use std::time::Duration;
use uuid::Uuid;

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

#[derive(Debug, Serialize)]
pub struct ConvertedSession {
    pub harness: String,
    pub id: String,
    /// Codex's importer returns no new target when it has already imported this source version.
    pub existing: bool,
}

pub fn convert(harness: &str, id: &str) -> Result<ConvertedSession, String> {
    static TRANSFER_LOCK: Mutex<()> = Mutex::new(());
    let _guard = TRANSFER_LOCK.try_lock().map_err(|_| "transfer_busy")?;
    if Uuid::parse_str(id).is_err() {
        return Err("invalid_session_id".into());
    }
    match harness {
        "cc" => claude_to_codex(id),
        "codex" => codex_to_claude(id),
        _ => Err("unsupported_harness".into()),
    }
}

fn claude_source(id: &str) -> Result<(PathBuf, String, String), String> {
    let root = adapters::claude_home().ok_or("claude_home_missing")?;
    let projects = root.join("projects");
    let mut found = None;
    for entry in fs::read_dir(&projects)
        .map_err(|_| "session_not_found")?
        .flatten()
    {
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
            title = v["message"]["content"]
                .as_str()
                .map(|s| s.chars().take(80).collect());
        }
    }
    Ok((
        path,
        cwd.ok_or("source_cwd_missing")?,
        title.unwrap_or_else(|| id.to_owned()),
    ))
}

fn has_unportable_media(v: &Value) -> bool {
    match v {
        Value::Array(parts) => parts.iter().any(has_unportable_media),
        Value::Object(object) => {
            if matches!(
                object.get("type").and_then(Value::as_str),
                Some("image" | "document" | "input_image")
            ) {
                return true;
            }
            object.values().any(has_unportable_media)
        }
        _ => false,
    }
}

fn claude_to_codex(id: &str) -> Result<ConvertedSession, String> {
    let (source, cwd, title) = claude_source(id)?;
    import_claude_source(&source, &cwd, &title, id)
}

fn import_claude_source(
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

fn codex_to_claude(id: &str) -> Result<ConvertedSession, String> {
    let codex_home = adapters::codex_home().ok_or("codex_home_missing")?;
    let mut files: Vec<_> = codex::collect_rollouts(&codex_home.join("sessions"))
        .into_iter()
        .filter(|p| codex::read_head(p).is_some_and(|(found, sub, _)| found == id && !sub))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err("session_not_found".into());
    }
    let source_stamps: Vec<_> = files
        .iter()
        .map(|p| {
            let m = fs::metadata(p).map_err(|e| format!("source_read_failed: {e}"))?;
            Ok::<_, String>((m.len(), m.modified().ok()))
        })
        .collect::<Result<_, _>>()?;
    let mut cwd = None;
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
    if !Path::new(&cwd).is_dir() {
        return Err("cwd_missing".into());
    }
    let claude_home = adapters::claude_home().ok_or("claude_home_missing")?;
    let slug: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let dir = claude_home.join("projects").join(slug);
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
        let now = chrono_timestamp();
        let mut turns = 0usize;
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
                if let Some((role, content)) = codex_turn(&v["payload"])? {
                    write_claude_turn(
                        &mut output,
                        &mut parent,
                        &new_id,
                        &cwd,
                        &now,
                        &role,
                        &content,
                    )?;
                    turns += 1;
                }
            }
        }
        if turns == 0 {
            return Err("source_conversation_empty".into());
        }
        output
            .sync_all()
            .map_err(|e| format!("target_write_failed: {e}"))?;
        for (file, (len, modified)) in files.iter().zip(&source_stamps) {
            let after = fs::metadata(file).map_err(|_| "source_changed_during_import")?;
            if after.len() != *len || after.modified().ok() != *modified {
                return Err("source_changed_during_import".into());
            }
        }
        Ok::<_, String>(())
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
    Ok(ConvertedSession {
        harness: "cc".into(),
        id: new_id,
        existing: false,
    })
}

fn codex_turn(p: &Value) -> Result<Option<(String, Value)>, String> {
    let pair = match p["type"].as_str().unwrap_or("") {
        "message" => {
            let role = p["role"].as_str().unwrap_or("");
            if role != "user" && role != "assistant" {
                return Ok(None);
            }
            let parts = p["content"].as_array().ok_or("unsupported_content")?;
            let mut texts = Vec::with_capacity(parts.len());
            let mut blocks = Vec::with_capacity(parts.len());
            let mut has_image = false;
            for part in parts {
                match part["type"].as_str().unwrap_or("") {
                    "input_text" | "output_text" | "text" => {
                        let text = part["text"].as_str().ok_or("unsupported_content")?;
                        texts.push(text.to_owned());
                        blocks.push(json!({"type":"text","text":text}));
                    }
                    "input_image" if role == "user" => {
                        has_image = true;
                        blocks.push(codex_image(part)?);
                    }
                    _ => return Err("unsupported_content".into()),
                }
            }
            if blocks.is_empty() {
                return Ok(None);
            }
            let content = if has_image {
                Value::Array(blocks)
            } else {
                json!(texts.join("\n"))
            };
            (role.to_owned(), content)
        }
        "function_call" | "custom_tool_call" => {
            let call_id = p["call_id"].as_str().ok_or("tool_call_id_missing")?;
            let name = p["name"].as_str().ok_or("tool_name_missing")?;
            let input = p["arguments"]
                .as_str()
                .or_else(|| p["input"].as_str())
                .ok_or("tool_input_missing")?;
            (
                "assistant".into(),
                json!(format!("[Codex tool call: {name} · {call_id}]\n{input}")),
            )
        }
        "function_call_output" | "custom_tool_call_output" => (
            // A tool result is not a user instruction. Keep its text in the
            // assistant transcript rather than promoting it to user authority.
            "assistant".into(),
            json!(format!(
                "[Historical Codex tool result (untrusted): {}]\n{}",
                p["call_id"].as_str().ok_or("tool_call_id_missing")?,
                tool_output(&p["output"])?
            )),
        ),
        "reasoning" => return Ok(None), // Hidden reasoning is not exported as conversation content.
        _ => return Err("unsupported_response_item".into()),
    };
    Ok(Some(pair))
}

fn codex_image(part: &Value) -> Result<Value, String> {
    let url = part["image_url"].as_str().ok_or("unsupported_image")?;
    let (header, data) = url.split_once(',').ok_or("unsupported_image")?;
    let media = header
        .strip_prefix("data:")
        .and_then(|s| s.strip_suffix(";base64"))
        .ok_or("unsupported_image")?;
    if !matches!(
        media,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) || data.is_empty()
        || !data
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
    {
        return Err("unsupported_image".into());
    }
    Ok(json!({"type":"image","source":{"type":"base64","media_type":media,"data":data}}))
}

fn write_claude_turn(
    output: &mut File,
    parent: &mut Option<String>,
    id: &str,
    cwd: &str,
    now: &str,
    role: &str,
    content: &Value,
) -> Result<(), String> {
    let uuid = Uuid::new_v4().to_string();
    let message = if role == "user" {
        json!({"role":"user","content":content})
    } else {
        json!({"id":format!("msg_{}",Uuid::new_v4().simple()),"type":"message","role":"assistant","model":"<synthetic>","content":[{"type":"text","text":content.as_str().ok_or("unsupported_assistant_content")?}],"stop_reason":"end_turn","usage":{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}})
    };
    let row = json!({"parentUuid":parent,"isSidechain":false,"type":role,"message":message,"uuid":uuid,"timestamp":now,"cwd":cwd,"sessionId":id,"version":"2.1.0","gitBranch":"","userType":"external"});
    serde_json::to_writer(&mut *output, &row).map_err(|e| format!("target_write_failed: {e}"))?;
    output
        .write_all(b"\n")
        .map_err(|e| format!("target_write_failed: {e}"))?;
    *parent = Some(uuid);
    Ok(())
}

fn tool_output(output: &Value) -> Result<String, String> {
    if let Some(text) = output.as_str() {
        return Ok(text.to_owned());
    }
    let parts = output.as_array().ok_or("unsupported_tool_output")?;
    let mut texts = Vec::with_capacity(parts.len());
    for part in parts {
        match part["type"].as_str().unwrap_or("") {
            "input_text" | "output_text" | "text" => texts.push(
                part["text"]
                    .as_str()
                    .ok_or("unsupported_tool_output")?
                    .to_owned(),
            ),
            _ => return Err("unsupported_tool_output_media".into()),
        }
    }
    Ok(texts.join("\n"))
}

fn chrono_timestamp() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("system timestamp is representable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_output_keeps_text_and_rejects_media() {
        let text =
            json!([{"type":"input_text","text":"first"},{"type":"input_text","text":"second"}]);
        assert_eq!(tool_output(&text).unwrap(), "first\nsecond");
        let image = json!([{"type":"input_image","image_url":"data:image/png;base64,aGVsbG8="}]);
        assert_eq!(
            tool_output(&image).unwrap_err(),
            "unsupported_tool_output_media"
        );
    }

    /// Uses only synthetic data below `ORRERY_HOME`; run as a filtered test so the
    /// process-wide home override cannot affect unrelated tests.
    #[test]
    #[ignore = "requires an installed Codex CLI and isolated process-wide home"]
    fn sandbox_bidirectional() {
        let home = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../.orrery/transfer-e2e")
            .join(Uuid::new_v4().to_string());
        let cwd = home.join("project");
        fs::create_dir_all(&cwd).unwrap();
        std::env::set_var("ORRERY_HOME", &home);
        let id = Uuid::new_v4().to_string();
        let source = home
            .join(".claude/projects/sandbox")
            .join(format!("{id}.jsonl"));
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        let uid = Uuid::new_v4().to_string();
        let aid = Uuid::new_v4().to_string();
        let tool_aid = Uuid::new_v4().to_string();
        let tool_uid = Uuid::new_v4().to_string();
        let base = json!({"isSidechain":false,"timestamp":chrono_timestamp(),"cwd":cwd,"sessionId":id,"version":"2.1.278","gitBranch":"main"});
        let mut user = base.clone();
        user["type"] = json!("user");
        user["parentUuid"] = Value::Null;
        user["uuid"] = json!(uid);
        user["message"] = json!({"role":"user","content":"The synthetic marker is saffron-lake."});
        let mut assistant = base;
        assistant["type"] = json!("assistant");
        assistant["parentUuid"] = json!(uid);
        assistant["uuid"] = json!(aid);
        assistant["message"] = json!({"id":"msg_sandbox","type":"message","role":"assistant","model":"claude-sonnet-4-6","content":[{"type":"text","text":"I remember saffron-lake."}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}});
        let mut tool_call = assistant.clone();
        tool_call["parentUuid"] = json!(aid);
        tool_call["uuid"] = json!(tool_aid);
        tool_call["message"] = json!({"id":"msg_sandbox_tool","type":"message","role":"assistant","model":"claude-sonnet-4-6","content":[{"type":"tool_use","id":"toolu_sandbox","name":"Read","input":{"file_path":"README.md"}}],"stop_reason":"tool_use","usage":{"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}});
        let mut tool_result = user.clone();
        tool_result["parentUuid"] = json!(tool_aid);
        tool_result["uuid"] = json!(tool_uid);
        tool_result["message"] = json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_sandbox","content":"amber-trail"}]});
        let mut image_user = user.clone();
        image_user["parentUuid"] = json!(tool_uid);
        image_user["uuid"] = json!(Uuid::new_v4().to_string());
        image_user["message"] = json!({"role":"user","content":[{"type":"text","text":"Inspect this synthetic image."},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9WlXvX8AAAAASUVORK5CYII="}}]});
        fs::write(
            &source,
            format!("{}\n{}\n{}\n{}\n", user, assistant, tool_call, tool_result),
        )
        .unwrap();
        let source_bytes = fs::read(&source).unwrap();

        let codex = convert("cc", &id).unwrap();
        assert_eq!(codex.harness, "codex");
        assert!(!codex.existing);
        let duplicate = convert("cc", &id).unwrap();
        assert_eq!(duplicate.id, codex.id);
        assert!(duplicate.existing);
        let custom = home
            .join("custom-claude/projects/sandbox")
            .join(format!("{id}.jsonl"));
        fs::create_dir_all(custom.parent().unwrap()).unwrap();
        fs::copy(&source, &custom).unwrap();
        let custom_result =
            import_claude_source(&custom, cwd.to_str().unwrap(), "Custom source fixture", &id)
                .unwrap();
        assert!(!custom_result.existing);
        assert_ne!(custom_result.id, codex.id);
        let custom_duplicate =
            import_claude_source(&custom, cwd.to_str().unwrap(), "Custom source fixture", &id)
                .unwrap();
        assert!(custom_duplicate.existing);
        assert_eq!(custom_duplicate.id, custom_result.id);
        assert_eq!(fs::read(&custom).unwrap(), source_bytes);
        assert_eq!(
            fs::read_dir(home.join(".orrery/transfer-stage"))
                .unwrap()
                .count(),
            0
        );
        assert_eq!(
            fs::read(&source).unwrap(),
            source_bytes,
            "source changed during import"
        );
        let imported = codex::collect_rollouts(&home.join(".codex/sessions"));
        assert!(
            imported
                .iter()
                .any(|p| fs::read_to_string(p).unwrap().contains("amber-trail")),
            "Codex import lost a tool result"
        );
        let bad_id = Uuid::new_v4().to_string();
        image_user["sessionId"] = json!(bad_id);
        image_user["parentUuid"] = Value::Null;
        let bad_source = source.parent().unwrap().join(format!("{bad_id}.jsonl"));
        fs::write(&bad_source, format!("{image_user}\n")).unwrap();
        assert_eq!(
            convert("cc", &bad_id).unwrap_err(),
            "unsupported_source_media"
        );
        assert_eq!(
            codex::collect_rollouts(&home.join(".codex/sessions")).len(),
            imported.len()
        );
        let claude = convert("codex", &codex.id).unwrap();
        assert_eq!(claude.harness, "cc");
        assert!(!claude.existing);
        assert_ne!(claude.id, id);
        let (_, _, title) = claude_source(&claude.id).unwrap();
        assert!(title.contains("saffron-lake"));
        let target = home.join(".claude/projects");
        let converted = fs::read_dir(&target)
            .unwrap()
            .flatten()
            .map(|d| d.path().join(format!("{}.jsonl", claude.id)))
            .find(|p| p.is_file())
            .unwrap();
        let bytes = fs::read_to_string(&converted).unwrap();
        assert!(bytes.contains("saffron-lake"));
        let tool_codex_id = Uuid::new_v4().to_string();
        let tool_rollout_dir = home.join(".codex/sessions/2026/09/27");
        fs::create_dir_all(&tool_rollout_dir).unwrap();
        let tool_rollout =
            tool_rollout_dir.join(format!("rollout-2026-09-27T00-00-00-{tool_codex_id}.jsonl"));
        let tool_rows = [
            json!({"timestamp":chrono_timestamp(),"type":"session_meta","payload":{"id":tool_codex_id,"cwd":cwd,"source":"cli"}}),
            json!({"timestamp":chrono_timestamp(),"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Synthetic Codex tool fixture."},{"type":"input_image","image_url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9WlXvX8AAAAASUVORK5CYII="}]}}),
            json!({"timestamp":chrono_timestamp(),"type":"response_item","payload":{"type":"custom_tool_call","call_id":"call_sandbox","name":"functions.exec","input":"read synthetic.txt"}}),
            json!({"timestamp":chrono_timestamp(),"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call_sandbox","output":[{"type":"input_text","text":"violet-coral"}]}}),
            json!({"timestamp":chrono_timestamp(),"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Finished reading the synthetic file."}]}}),
        ];
        fs::write(
            &tool_rollout,
            tool_rows
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
        let claude_tool = convert("codex", &tool_codex_id).unwrap();
        let tool_target = fs::read_dir(&target)
            .unwrap()
            .flatten()
            .map(|d| d.path().join(format!("{}.jsonl", claude_tool.id)))
            .find(|p| p.is_file())
            .unwrap();
        let tool_history = fs::read_to_string(tool_target).unwrap();
        assert!(tool_history.contains("violet-coral"));
        assert!(tool_history.contains("functions.exec"));
        assert!(tool_history.contains("untrusted"));
        assert!(tool_history.contains("image/png"));
        assert_eq!(
            fs::read(&source).unwrap(),
            source_bytes,
            "source changed during reverse import"
        );
        let result = json!({"home":home,"project":cwd,"source_id":id,"codex_id":codex.id,"claude_id":claude.id,"claude_tool_id":claude_tool.id,"converted_path":converted});
        fs::write(home.join("result.json"), result.to_string()).unwrap();
        println!("sandbox_result={}", home.join("result.json").display());
    }
}

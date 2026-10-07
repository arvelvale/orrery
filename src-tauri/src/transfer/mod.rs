//! Native session copies between harnesses. Sources are never modified.
//!
//! Each harness contributes a *reader* (its session → [`Transcript`]) and, when
//! the target tool has a legitimate way in, a *writer* ([`Transcript`] → a new
//! session that tool resumes natively). A new source then reaches every target
//! at once, instead of one hand-written converter per pair.
//!
//! | harness | reader | writer |
//! |---|---|---|
//! | `cc` | native JSONL is handed to Codex as-is (see below) | writes a new Claude JSONL conversation |
//! | `codex` | rollout `response_item`s | Codex's own `externalAgentConfig/import`, which accepts Claude JSONL |
//!
//! Claude Code → Codex skips the [`Transcript`]: Codex's importer reads Claude's
//! native file directly, and re-rendering it could only lose detail.
//!
//! Fidelity bar, kept from the first version: anything that cannot be carried
//! over faithfully (media the target would drop, unknown record types) stops the
//! transfer with an error instead of producing a quietly incomplete copy. Hidden
//! reasoning is never exported.

mod claude;
mod codex;

use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct ConvertedSession {
    pub harness: String,
    pub id: String,
    /// Codex's importer returns no new target when it has already imported this source version.
    pub existing: bool,
}

/// Who said a turn. Tool calls and results are assistant-side: a tool result
/// is not a user instruction and must never be promoted to user authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    User,
    Assistant,
}

/// One piece of a turn, in the order it appeared
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Part {
    Text(String),
    /// Base64 image; readers only emit the formats every writer accepts
    Image { media_type: String, data: String },
    ToolCall { id: String, name: String, input: String },
    ToolResult { id: String, output: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Turn {
    pub role: Role,
    pub parts: Vec<Part>,
}

/// A conversation read out of one harness, ready for any writer
#[derive(Debug)]
pub(crate) struct Transcript {
    /// Shown in the labels of carried-over tool records, e.g. "Codex"
    pub source_name: &'static str,
    pub cwd: String,
    pub turns: Vec<Turn>,
    /// Source files as they were before reading, re-checked before publishing
    pub stamps: SourceStamps,
}

/// Length and mtime of every source file, taken before reading
#[derive(Debug)]
pub(crate) struct SourceStamps(Vec<(PathBuf, u64, Option<SystemTime>)>);

impl SourceStamps {
    pub fn take(files: &[PathBuf]) -> Result<Self, String> {
        files
            .iter()
            .map(|p| {
                let m = fs::metadata(p).map_err(|e| format!("source_read_failed: {e}"))?;
                Ok((p.clone(), m.len(), m.modified().ok()))
            })
            .collect::<Result<_, String>>()
            .map(Self)
    }

    /// The source must not have changed while we were copying it
    pub fn verify_unchanged(&self) -> Result<(), String> {
        for (path, len, modified) in &self.0 {
            let after = fs::metadata(path).map_err(|_| "source_changed_during_import")?;
            if after.len() != *len || after.modified().ok() != *modified {
                return Err("source_changed_during_import".into());
            }
        }
        Ok(())
    }
}

/// Convert to the harness's default partner (Claude Code ⇄ Codex)
pub fn convert(harness: &str, id: &str) -> Result<ConvertedSession, String> {
    let target = match harness {
        "cc" => "codex",
        "codex" => "cc",
        _ => return Err("unsupported_harness".into()),
    };
    convert_to(harness, id, target)
}

pub(crate) fn convert_to(harness: &str, id: &str, target: &str) -> Result<ConvertedSession, String> {
    static TRANSFER_LOCK: Mutex<()> = Mutex::new(());
    let _guard = TRANSFER_LOCK.try_lock().map_err(|_| "transfer_busy")?;
    if Uuid::parse_str(id).is_err() {
        return Err("invalid_session_id".into());
    }
    match (harness, target) {
        ("cc", "codex") => {
            let source = claude::locate(id)?;
            codex::import_claude_file(&source.path, &source.cwd, &source.title, id)
        }
        (_, "cc") => {
            let transcript = read(harness, id)?;
            if !std::path::Path::new(&transcript.cwd).is_dir() {
                return Err("cwd_missing".into());
            }
            claude::write(&transcript)
        }
        _ => Err("unsupported_harness".into()),
    }
}

/// Every reader behind one door, so adding a source is one match arm
fn read(harness: &str, id: &str) -> Result<Transcript, String> {
    let transcript = match harness {
        "codex" => codex::read(id)?,
        _ => return Err("unsupported_harness".into()),
    };
    if transcript.turns.is_empty() {
        return Err("source_conversation_empty".into());
    }
    Ok(transcript)
}

pub(crate) fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("system timestamp is representable")
}

#[cfg(test)]
mod tests;

//! Native session copies between harnesses. Sources are never modified.
//!
//! Each harness contributes a *reader* (its session → [`Transcript`]) and, when
//! the target tool has a legitimate way in, a *writer* ([`Transcript`] → a new
//! session that tool resumes natively). A new source then reaches every target
//! at once, instead of one hand-written converter per pair.
//!
//! | harness | reader | writer |
//! |---|---|---|
//! | `cc` | the active branch of the JSONL conversation | a new Claude JSONL conversation |
//! | `codex` | rollout `response_item`s | Codex's own `externalAgentConfig/import`, which accepts Claude JSONL |
//! | `opencode` | `message` / `part` rows, read-only | OpenCode's own `opencode import` |
//!
//! Claude Code → Codex skips the [`Transcript`]: Codex's importer reads Claude's
//! native file directly, and re-rendering it could only lose detail. Every other
//! source reaches Codex by rendering Claude JSONL into a staging folder first.
//!
//! Fidelity bar: nothing is dropped quietly. Unknown record types and media no
//! target can hold (PDFs, linked images) stop the transfer with an error.
//! Images a target cannot hold inline are saved as files with a reference left
//! in their place (see `media.rs`); the UI says how many before converting.
//! Hidden reasoning is never exported. Tool calls and results from another tool
//! become labelled history text in the target, never live tool records the
//! target might try to replay.

mod claude;
mod codex;
pub(crate) mod media;
mod opencode;

use rusqlite::OptionalExtension;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;
use uuid::Uuid;

/// Harnesses that can take part in a transfer, as source and as target
pub(crate) const HARNESSES: [&str; 3] = ["cc", "codex", "opencode"];

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

/// A base64 image in a format every writer accepts (png, jpeg, gif, webp)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Image {
    pub media_type: String,
    pub data: String,
}

/// `data:image/png;base64,…` → an image; any other URL or format → `None`
pub(crate) fn data_url_image(url: &str) -> Option<Image> {
    let (header, data) = url.split_once(',')?;
    let media = header.strip_prefix("data:")?.strip_suffix(";base64")?;
    let ok = matches!(media, "image/png" | "image/jpeg" | "image/gif" | "image/webp")
        && !data.is_empty()
        && data.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=');
    ok.then(|| Image { media_type: media.to_owned(), data: data.to_owned() })
}

/// One piece of a turn, in the order it appeared
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Part {
    Text(String),
    /// Base64 image; readers only emit the formats every writer accepts
    Image { media_type: String, data: String },
    ToolCall { id: String, name: String, input: String },
    /// `images` are screenshots and the like returned by the tool; they are
    /// saved as files before any writer sees the transcript
    ToolResult { id: String, output: String, images: Vec<Image> },
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
    pub title: String,
    pub turns: Vec<Turn>,
    /// The source as it was before reading, re-checked before publishing
    pub stamps: SourceStamps,
}

impl Transcript {
    pub fn has_images(&self) -> bool {
        self.turns.iter().flat_map(|t| &t.parts).any(|p| matches!(p, Part::Image { .. }))
    }
}

/// A title from the first thing the user said, when the source has no better one
pub(crate) fn title_from_turns(turns: &[Turn]) -> Option<String> {
    turns.iter().filter(|t| t.role == Role::User).flat_map(|t| &t.parts).find_map(|p| match p {
        Part::Text(text) if !text.trim().is_empty() => Some(text.trim().chars().take(80).collect()),
        _ => None,
    })
}

#[derive(Debug)]
enum Check {
    /// Length and mtime of a source file
    File { path: PathBuf, len: u64, modified: Option<SystemTime> },
    /// One OpenCode session. The database file itself changes whenever OpenCode
    /// writes any session, so only this session's own row and messages count.
    OpenCodeSession { db: PathBuf, id: String, updated: i64, messages: i64 },
}

/// What the source looked like before reading
#[derive(Debug)]
pub(crate) struct SourceStamps(Vec<Check>);

impl SourceStamps {
    pub fn files(files: &[PathBuf]) -> Result<Self, String> {
        files
            .iter()
            .map(|p| {
                let m = fs::metadata(p).map_err(|e| format!("source_read_failed: {e}"))?;
                Ok(Check::File { path: p.clone(), len: m.len(), modified: m.modified().ok() })
            })
            .collect::<Result<_, String>>()
            .map(Self)
    }

    pub fn opencode_session(db: &Path, id: &str) -> Result<Self, String> {
        let (updated, messages) = opencode_session_state(db, id)?.ok_or("session_not_found")?;
        Ok(Self(vec![Check::OpenCodeSession { db: db.to_owned(), id: id.to_owned(), updated, messages }]))
    }

    /// The source must not have changed while we were copying it
    pub fn verify_unchanged(&self) -> Result<(), String> {
        for check in &self.0 {
            let same = match check {
                Check::File { path, len, modified } => fs::metadata(path)
                    .is_ok_and(|after| after.len() == *len && after.modified().ok() == *modified),
                Check::OpenCodeSession { db, id, updated, messages } => {
                    opencode_session_state(db, id).ok().flatten() == Some((*updated, *messages))
                }
            };
            if !same {
                return Err("source_changed_during_import".into());
            }
        }
        Ok(())
    }
}

/// `(time_updated, message count)` of one OpenCode session, read-only
fn opencode_session_state(db: &Path, id: &str) -> Result<Option<(i64, i64)>, String> {
    let con = crate::adapters::opencode::open(db).ok_or("source_read_failed")?;
    con.query_row(
        "SELECT COALESCE(time_updated, time_created, 0), (SELECT COUNT(*) FROM message WHERE session_id = ?1)
         FROM session WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map_err(|e| format!("source_read_failed: {e}"))
}

/// Convert to the harness's default partner (Claude Code ⇄ Codex, OpenCode → Claude Code)
pub fn convert(harness: &str, id: &str, target: Option<&str>) -> Result<ConvertedSession, String> {
    let target = match (harness, target) {
        (_, Some(t)) => t,
        ("cc", None) => "codex",
        ("codex" | "opencode", None) => "cc",
        _ => return Err("unsupported_harness".into()),
    };
    convert_to(harness, id, target)
}

fn check_pair(harness: &str, id: &str, target: &str) -> Result<(), String> {
    if !HARNESSES.contains(&harness) || !HARNESSES.contains(&target) || harness == target {
        return Err("unsupported_harness".into());
    }
    if !valid_id(harness, id) {
        return Err("invalid_session_id".into());
    }
    Ok(())
}

/// Claude Code → Codex with no media hands Claude's native file to Codex's
/// importer as-is. With media it takes the transcript path, so the images
/// become files instead of being dropped by that importer.
fn claude_passthrough(harness: &str, id: &str, target: &str) -> Result<Option<claude::ClaudeFile>, String> {
    if (harness, target) != ("cc", "codex") {
        return Ok(None);
    }
    match claude::locate(id) {
        Ok(file) => Ok(Some(file)),
        Err(e) if e == "unsupported_source_media" => Ok(None),
        Err(e) => Err(e),
    }
}

pub(crate) fn convert_to(harness: &str, id: &str, target: &str) -> Result<ConvertedSession, String> {
    static TRANSFER_LOCK: Mutex<()> = Mutex::new(());
    let _guard = TRANSFER_LOCK.try_lock().map_err(|_| "transfer_busy")?;
    check_pair(harness, id, target)?;
    if let Some(source) = claude_passthrough(harness, id, target)? {
        return codex::import_claude_file(&source.path, &source.cwd, &source.title, id);
    }
    let mut transcript = read(harness, id)?;
    if !Path::new(&transcript.cwd).is_dir() {
        return Err("cwd_missing".into());
    }
    let mut media = media::MediaStore::new()?;
    let result = media::externalize(&mut transcript, target, &mut media).and_then(|()| match target {
        "cc" => claude::write(&transcript),
        "codex" => codex::import_transcript(&transcript),
        "opencode" => opencode::write(&transcript),
        _ => Err("unsupported_harness".into()),
    });
    match &result {
        Ok(done) => media.finish(&done.harness, &done.id),
        Err(_) => media.discard(),
    }
    result
}

/// What a transfer would do, without doing it: read-only
#[derive(Debug, Serialize)]
pub struct TransferPreview {
    /// Images that will be saved as files under ~/.orrery/transfer-media
    pub images_to_files: usize,
}

pub fn preview(harness: &str, id: &str, target: &str) -> Result<TransferPreview, String> {
    check_pair(harness, id, target)?;
    if claude_passthrough(harness, id, target)?.is_some() {
        return Ok(TransferPreview { images_to_files: 0 });
    }
    let transcript = read(harness, id)?;
    if !Path::new(&transcript.cwd).is_dir() {
        return Err("cwd_missing".into());
    }
    Ok(TransferPreview { images_to_files: media::count(&transcript, target) })
}

/// Claude Code and Codex use UUIDs; OpenCode uses `ses_` plus 26 alphanumerics
fn valid_id(harness: &str, id: &str) -> bool {
    match harness {
        "opencode" => {
            id.strip_prefix("ses_").is_some_and(|rest| rest.len() == 26 && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
        }
        _ => Uuid::parse_str(id).is_ok(),
    }
}

/// Every reader behind one door, so adding a source is one match arm
fn read(harness: &str, id: &str) -> Result<Transcript, String> {
    let transcript = match harness {
        "cc" => claude::read(id)?,
        "codex" => codex::read(id)?,
        "opencode" => opencode::read(id)?,
        _ => return Err("unsupported_harness".into()),
    };
    if transcript.turns.is_empty() {
        return Err("source_conversation_empty".into());
    }
    Ok(transcript)
}

/// Remove `.` and `..` from a folder path, the way a shell's working directory
/// would read. Purely lexical on purpose: `canonicalize` would also resolve
/// junctions and symlinks, and a harness started inside a linked folder records
/// the link's path, not its target.
pub(crate) fn clean_dir(cwd: &str) -> String {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in Path::new(cwd).components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                // never climb above the root or the drive
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out.to_string_lossy().into_owned()
}

pub(crate) fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("system timestamp is representable")
}

#[cfg(test)]
mod tests;

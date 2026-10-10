//! Images a target cannot hold inline are saved as files, and the transcript
//! keeps a reference to each one in its place. Nothing is dropped silently.
//!
//! | image | Claude Code | OpenCode | Codex |
//! |---|---|---|---|
//! | sent by the user | inline | inline | **file** (Codex's importer drops images) |
//! | in a tool result | **file** | **file** | **file** |
//! | sent by the assistant | **file** | **file** | **file** |
//!
//! Tool results become labelled history text in every target, so their images
//! have no inline place. All three targets can open a saved image on demand:
//! Claude Code's `Read`, Codex's `view_image`, OpenCode's `read` (which returns
//! images as attachments). The model does not see the image until it opens it.
//!
//! Files live in `~/.orrery/transfer-media/<transfer>/`, with `owner.json`
//! naming the session they belong to, so deleting that session through Orrery
//! can take the images with it. They stay outside the project folder on purpose:
//! no stray untracked files in the user's repository.

use super::{Image, Part, Role, Transcript};
use crate::adapters;
use base64::Engine;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const FOLDER: &str = "transfer-media";

/// Does this image have to become a file for this target?
fn needs_file(target: &str, role: Role) -> bool {
    role == Role::Assistant || target == "codex"
}

/// How many images a transfer to `target` would save as files
pub(super) fn count(t: &Transcript, target: &str) -> usize {
    t.turns
        .iter()
        .flat_map(|turn| turn.parts.iter().map(move |p| (turn.role, p)))
        .map(|(role, p)| match p {
            Part::Image { .. } if needs_file(target, role) => 1,
            Part::ToolResult { images, .. } => images.len(),
            _ => 0,
        })
        .sum()
}

/// One transfer's media folder, created only when the first image is saved
pub(super) struct MediaStore {
    dir: PathBuf,
    saved: usize,
}

impl MediaStore {
    pub fn new() -> Result<Self, String> {
        let root = adapters::data_dir()
            .ok_or("orrery_data_dir_missing")?
            .join(FOLDER);
        Ok(Self {
            dir: root.join(Uuid::new_v4().to_string()),
            saved: 0,
        })
    }

    fn save(&mut self, image: &Image) -> Result<PathBuf, String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&image.data)
            .map_err(|_| "unsupported_image".to_string())?;
        let ext = match image.media_type.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => return Err("unsupported_image".into()),
        };
        fs::create_dir_all(&self.dir).map_err(|e| format!("media_write_failed: {e}"))?;
        self.saved += 1;
        let path = self.dir.join(format!("{}.{ext}", self.saved));
        fs::write(&path, bytes).map_err(|e| format!("media_write_failed: {e}"))?;
        Ok(path)
    }

    /// The transfer succeeded: record which session owns these images
    pub fn finish(self, harness: &str, id: &str) {
        if self.saved > 0 {
            let owner = json!({"harness": harness, "id": id, "images": self.saved});
            let _ = fs::write(self.dir.join("owner.json"), owner.to_string());
        }
    }

    /// The transfer failed: leave nothing behind
    pub fn discard(self) {
        if self.saved > 0 {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

/// The line that stands in for a saved image
fn reference(path: &Path) -> String {
    format!(
        "[Image saved to {} - open it with a file-reading tool if you need to see it]",
        path.display()
    )
}

/// Save every image the target cannot hold inline, and leave a reference in its place
pub(super) fn externalize(
    t: &mut Transcript,
    target: &str,
    store: &mut MediaStore,
) -> Result<(), String> {
    for turn in &mut t.turns {
        for part in &mut turn.parts {
            match part {
                Part::Image { media_type, data } if needs_file(target, turn.role) => {
                    let path = store.save(&Image {
                        media_type: media_type.clone(),
                        data: data.clone(),
                    })?;
                    *part = Part::Text(reference(&path));
                }
                Part::ToolResult { output, images, .. } if !images.is_empty() => {
                    for image in images.drain(..) {
                        let path = store.save(&image)?;
                        if !output.is_empty() {
                            output.push('\n');
                        }
                        output.push_str(&reference(&path));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// The session these images belong to was deleted: take them along, to the
/// Recycle Bin or permanently, the same way as the session. Best effort: a
/// leftover folder only costs disk space, never correctness.
pub fn release(harness: &str, id: &str, to_trash: bool) {
    for dir in folders_owned_by(harness, id) {
        let _ = if to_trash {
            trash::delete(&dir).map_err(|e| e.to_string())
        } else {
            fs::remove_dir_all(&dir).map_err(|e| e.to_string())
        };
    }
}

/// Media folders owned by a session
fn folders_owned_by(harness: &str, id: &str) -> Vec<PathBuf> {
    let Some(root) = adapters::data_dir().map(|d| d.join(FOLDER)) else {
        return vec![];
    };
    let Ok(entries) = fs::read_dir(root) else {
        return vec![];
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|dir| {
            fs::read_to_string(dir.join("owner.json"))
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .is_some_and(|o| o["harness"] == harness && o["id"] == id)
        })
        .collect()
}

/// References to saved images in what the model reads back: Codex writes every
/// message twice, as an `event_msg` for its UI and a `response_item` for the
/// model, so only the latter is counted
#[cfg(test)]
pub(crate) fn history_image_refs(rollout: &str) -> usize {
    rollout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["type"] == "response_item")
        .map(|v| v.to_string().matches("Image saved to").count())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::super::{SourceStamps, Turn};
    use super::*;

    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9WlXvX8AAAAASUVORK5CYII=";

    fn image() -> Image {
        Image {
            media_type: "image/png".into(),
            data: PNG.into(),
        }
    }

    fn sample() -> Transcript {
        Transcript {
            source_name: "Claude Code",
            cwd: "D:/code/acme-web".into(),
            title: "t".into(),
            turns: vec![
                Turn {
                    role: Role::User,
                    parts: vec![
                        Part::Text("look".into()),
                        Part::Image {
                            media_type: "image/png".into(),
                            data: PNG.into(),
                        },
                    ],
                },
                Turn {
                    role: Role::Assistant,
                    parts: vec![Part::ToolResult {
                        id: "t1".into(),
                        output: "screenshot taken".into(),
                        images: vec![image(), image()],
                    }],
                },
            ],
            stamps: SourceStamps::files(&[]).unwrap(),
        }
    }

    #[test]
    fn user_images_stay_inline_except_towards_codex() {
        assert_eq!(count(&sample(), "cc"), 2, "only the tool-result images");
        assert_eq!(count(&sample(), "opencode"), 2);
        assert_eq!(count(&sample(), "codex"), 3, "the user's image too");
    }

    #[test]
    fn saved_images_are_real_files_referenced_in_place() {
        let dir = std::env::temp_dir().join(format!("orrery-media-{}", Uuid::new_v4()));
        let mut store = MediaStore {
            dir: dir.clone(),
            saved: 0,
        };
        let mut t = sample();
        externalize(&mut t, "codex", &mut store).unwrap();
        assert!(
            !t.has_images(),
            "nothing image-shaped may reach the Codex writer"
        );
        let Part::Text(user_ref) = &t.turns[0].parts[1] else {
            panic!("user image not replaced")
        };
        assert!(user_ref.contains("1.png"));
        let Part::ToolResult { output, images, .. } = &t.turns[1].parts[0] else {
            panic!()
        };
        assert!(images.is_empty());
        assert!(output.starts_with("screenshot taken\n[Image saved to "));
        assert!(output.contains("2.png") && output.contains("3.png"));
        assert_eq!(
            fs::read(dir.join("2.png")).unwrap(),
            base64::engine::general_purpose::STANDARD
                .decode(PNG)
                .unwrap()
        );
        store.discard();
        assert!(
            !dir.exists(),
            "a failed transfer must not leave its images behind"
        );
    }

    #[test]
    fn broken_base64_is_refused_not_written() {
        let dir = std::env::temp_dir().join(format!("orrery-media-{}", Uuid::new_v4()));
        let mut store = MediaStore {
            dir: dir.clone(),
            saved: 0,
        };
        let bad = Image {
            media_type: "image/png".into(),
            data: "not base64!".into(),
        };
        assert_eq!(store.save(&bad).unwrap_err(), "unsupported_image");
        assert!(!dir.exists());
    }
}

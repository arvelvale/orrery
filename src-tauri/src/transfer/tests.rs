//! End-to-end transfer test. Unit tests for each side live next to the code.

use super::*;
use crate::adapters::codex as codex_adapter;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

#[test]
fn tool_output_keeps_text_and_screenshots_but_refuses_other_media() {
    let text = json!([{"type":"input_text","text":"first"},{"type":"input_text","text":"second"}]);
    assert_eq!(
        codex::tool_output(&text).unwrap(),
        ("first\nsecond".to_string(), vec![])
    );
    let screenshot = json!([{"type":"input_text","text":"shot"},{"type":"input_image","image_url":"data:image/png;base64,aGVsbG8="}]);
    let (output, images) = codex::tool_output(&screenshot).unwrap();
    assert_eq!(output, "shot");
    assert_eq!(
        images,
        vec![Image {
            media_type: "image/png".into(),
            data: "aGVsbG8=".into()
        }]
    );
    let linked = json!([{"type":"input_image","image_url":"https://example.com/a.png"}]);
    assert_eq!(
        codex::tool_output(&linked).unwrap_err(),
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
    let base = json!({"isSidechain":false,"timestamp":now_rfc3339(),"cwd":cwd,"sessionId":id,"version":"2.1.278","gitBranch":"main"});
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

    let codex = convert("cc", &id, None).unwrap();
    assert_eq!(codex.harness, "codex");
    assert!(!codex.existing);
    let duplicate = convert("cc", &id, None).unwrap();
    assert_eq!(duplicate.id, codex.id);
    assert!(duplicate.existing);
    let custom = home
        .join("custom-claude/projects/sandbox")
        .join(format!("{id}.jsonl"));
    fs::create_dir_all(custom.parent().unwrap()).unwrap();
    fs::copy(&source, &custom).unwrap();
    let custom_result =
        codex::import_claude_file(&custom, cwd.to_str().unwrap(), "Custom source fixture", &id)
            .unwrap();
    assert!(!custom_result.existing);
    assert_ne!(custom_result.id, codex.id);
    let custom_duplicate =
        codex::import_claude_file(&custom, cwd.to_str().unwrap(), "Custom source fixture", &id)
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
    let imported = codex_adapter::collect_rollouts(&home.join(".codex/sessions"));
    assert!(
        imported
            .iter()
            .any(|p| fs::read_to_string(p).unwrap().contains("amber-trail")),
        "Codex import lost a tool result"
    );
    // A user image and a tool screenshot, bound for Codex whose importer drops
    // images: both are saved as files and Codex gets a reference to each
    let media_id = Uuid::new_v4().to_string();
    image_user["sessionId"] = json!(media_id);
    image_user["parentUuid"] = Value::Null;
    let mut screenshot = image_user.clone();
    screenshot["parentUuid"] = image_user["uuid"].clone();
    screenshot["uuid"] = json!(Uuid::new_v4().to_string());
    screenshot["message"] = json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_shot","content":[{"type":"text","text":"captured coral-dune"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9WlXvX8AAAAASUVORK5CYII="}}]}]});
    let media_source = source.parent().unwrap().join(format!("{media_id}.jsonl"));
    fs::write(&media_source, format!("{image_user}\n{screenshot}\n")).unwrap();
    let media_bytes = fs::read(&media_source).unwrap();
    let with_media = convert("cc", &media_id, None).unwrap();
    assert_eq!(with_media.harness, "codex");
    let rollout: String = codex_adapter::collect_rollouts(&home.join(".codex/sessions"))
        .into_iter()
        .filter(|p| codex_adapter::read_head(p).is_some_and(|(id, _, _)| id == with_media.id))
        .map(|p| fs::read_to_string(p).unwrap())
        .collect();
    assert!(
        rollout.contains("coral-dune"),
        "Codex lost the screenshot's text"
    );
    assert_eq!(
        media::history_image_refs(&rollout),
        2,
        "both images need a reference in what Codex's model reads"
    );
    let owned: Vec<_> = fs::read_dir(home.join(".orrery/transfer-media"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(owned.len(), 1);
    let owner: Value =
        serde_json::from_str(&fs::read_to_string(owned[0].join("owner.json")).unwrap()).unwrap();
    assert_eq!(
        (owner["harness"].as_str(), owner["id"].as_str()),
        (Some("codex"), Some(with_media.id.as_str()))
    );
    assert!(owned[0].join("1.png").is_file() && owned[0].join("2.png").is_file());
    assert_eq!(
        fs::read(&media_source).unwrap(),
        media_bytes,
        "source changed"
    );
    // deleting that Codex session through Orrery takes its images along
    media::release("codex", &with_media.id, false);
    assert!(!owned[0].exists(), "images outlived their session");
    let claude = convert("codex", &codex.id, None).unwrap();
    assert_eq!(claude.harness, "cc");
    assert!(!claude.existing);
    assert_ne!(claude.id, id);
    let title = claude::locate(&claude.id).unwrap().title;
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
        json!({"timestamp":now_rfc3339(),"type":"session_meta","payload":{"id":tool_codex_id,"cwd":cwd,"source":"cli"}}),
        json!({"timestamp":now_rfc3339(),"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Synthetic Codex tool fixture."},{"type":"input_image","image_url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9WlXvX8AAAAASUVORK5CYII="}]}}),
        json!({"timestamp":now_rfc3339(),"type":"response_item","payload":{"type":"custom_tool_call","call_id":"call_sandbox","name":"functions.exec","input":"read synthetic.txt"}}),
        json!({"timestamp":now_rfc3339(),"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call_sandbox","output":[{"type":"input_text","text":"violet-coral"}]}}),
        json!({"timestamp":now_rfc3339(),"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Finished reading the synthetic file."}]}}),
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
    let claude_tool = convert("codex", &tool_codex_id, None).unwrap();
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

/// Runs every reader over this machine's real sessions and prints what would
/// transfer. Read-only: readers never write, and nothing is converted.
/// `cargo test --lib real_sessions_dry_run -- --ignored --nocapture`
#[test]
#[ignore = "reads the real local sessions"]
fn real_sessions_dry_run() {
    use std::collections::BTreeMap;
    assert!(
        std::env::var_os("ORRERY_HOME").is_none(),
        "meant for the real home"
    );
    let sessions = crate::adapters::list_all_sessions().unwrap();
    for harness in HARNESSES {
        let mut ok = 0;
        let mut with_images = 0;
        let mut turns = 0;
        let mut to_files: BTreeMap<&str, usize> = BTreeMap::new();
        let mut errors: BTreeMap<String, usize> = BTreeMap::new();
        for s in sessions
            .iter()
            .filter(|s| s.harness == harness && s.kind != "subagent")
        {
            match read(harness, &s.id) {
                Ok(t) => {
                    ok += 1;
                    turns += t.turns.len();
                    with_images += usize::from(t.has_images());
                    for target in HARNESSES.iter().filter(|h| **h != harness) {
                        *to_files.entry(target).or_default() += media::count(&t, target);
                    }
                }
                Err(e) => {
                    *errors
                        .entry(e.split(':').next().unwrap_or("").to_owned())
                        .or_default() += 1
                }
            }
        }
        println!("{harness}: {ok} readable ({turns} turns, {with_images} with inline images), refused {errors:?}, images saved as files per target {to_files:?}");
    }
}

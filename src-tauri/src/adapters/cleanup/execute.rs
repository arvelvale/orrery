//! 把 [`Plan`](super::Plan) 真正落地：移回收站 / 永久删除、改各工具自己的索引、
//! 以及 Codex 只能经官方 CLI 清理的那部分数据库。

use super::super::data_dir;
use super::{harness_root, no_window, strip_verbatim, Mode, Plan, Outcome};
use std::fs;
use std::path::{Path, PathBuf};

/// 返回是否实际改动。写前重读；只移除引用这些 id 的条目
pub(super) fn rewrite_index(path: &Path, ids: &[&str], backup_root: Option<&Path>, rel: &str) -> Result<bool, String> {
    let Ok(raw) = fs::read_to_string(path) else { return Ok(false) };
    let updated = if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
        filter_jsonl(&raw, ids)
    } else {
        filter_json(&raw, ids)?
    };
    let Some(updated) = updated else { return Ok(false) };

    if let Some(b) = backup_root {
        let dst = b.join(rel);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("backup: {e}"))?;
        }
        fs::copy(path, &dst).map_err(|e| format!("backup: {e}"))?;
    }
    let tmp = path.with_file_name(format!(
        ".{}.orrery-tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("index")
    ));
    fs::write(&tmp, updated).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(true)
}

/// JSONL：解析每行，任一顶层字符串字段等于目标 id 的行删掉；解析失败的行原样保留
fn filter_jsonl(raw: &str, ids: &[&str]) -> Option<String> {
    let mut changed = false;
    let mut out = String::with_capacity(raw.len());
    for line in raw.split_inclusive('\n') {
        let hit = serde_json::from_str::<serde_json::Value>(line.trim())
            .ok()
            .and_then(|v| v.as_object().cloned())
            .is_some_and(|obj| {
                ["id", "sessionId", "session_id", "thread_id"]
                    .iter()
                    .any(|k| obj.get(*k).and_then(|x| x.as_str()).is_some_and(|s| ids.contains(&s)))
            });
        if hit {
            changed = true;
        } else {
            out.push_str(line);
        }
    }
    changed.then_some(out)
}

/// JSON：递归移除数组里等于 id 的字符串、或 `id` 字段等于 id 的对象；保持原缩进风格
fn filter_json(raw: &str, ids: &[&str]) -> Result<Option<String>, String> {
    let mut v: serde_json::Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    if !prune(&mut v, ids) {
        return Ok(None);
    }
    let pretty = raw.contains("\n ");
    let mut s = if pretty { serde_json::to_string_pretty(&v) } else { serde_json::to_string(&v) }
        .map_err(|e| e.to_string())?;
    if raw.ends_with('\n') {
        s.push('\n');
    }
    Ok(Some(s))
}

fn prune(v: &mut serde_json::Value, ids: &[&str]) -> bool {
    let mut changed = false;
    match v {
        serde_json::Value::Array(items) => {
            let before = items.len();
            items.retain(|item| match item {
                serde_json::Value::String(s) => !ids.contains(&s.as_str()),
                serde_json::Value::Object(o) => !o.get("id").and_then(|x| x.as_str()).is_some_and(|s| ids.contains(&s)),
                _ => true,
            });
            changed |= items.len() != before;
            for item in items.iter_mut() {
                changed |= prune(item, ids);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, child) in map.iter_mut() {
                changed |= prune(child, ids);
            }
        }
        _ => {}
    }
    changed
}

/// 返回成功删除的 thread id
fn run_codex_delete(root: &Path, threads: &[String]) -> Vec<String> {
    let Some(bin) = codex_bin() else { return vec![] };
    threads
        .iter()
        .filter(|id| is_uuid(id))
        .filter(|id| {
            let mut cmd = std::process::Command::new(&bin);
            cmd.args(["delete", "--force", id.as_str()])
                .env("CODEX_HOME", strip_verbatim(root))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            no_window(&mut cmd);
            cmd.status().is_ok_and(|s| s.success())
        })
        .cloned()
        .collect()
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.chars().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() })
}

/// 找 codex 原生可执行文件：`ORRERY_CODEX_BIN` → PATH 里的 codex → npm 全局包里的 vendor 二进制
///
/// npm 装的 codex 在 Windows 上是 `codex.cmd` 批处理壳，直接调它会弹窗且拿不到退出码，
/// 所以要顺着 npm 的目录结构找到真正的二进制
pub(crate) fn codex_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ORRERY_CODEX_BIN").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let exe_name = if cfg!(windows) { "codex.exe" } else { "codex" };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let exe = dir.join(exe_name);
        if exe.is_file() {
            return Some(exe);
        }
        if cfg!(windows) && dir.join("codex.cmd").is_file() {
            let vendor = dir
                .join("node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe");
            if vendor.is_file() {
                return Some(vendor);
            }
        }
    }
    None
}

/* ── 执行 ── */

pub(super) fn execute(p: &Plan, mode: Mode, stamp: &str) -> Outcome {
    let mut o = Outcome {
        harness: p.harness.clone(),
        id: p.id.clone(),
        mode: if mode == Mode::Trash { "trash" } else { "permanent" }.into(),
        bytes: p.bytes,
        ..Default::default()
    };
    if let Some(reason) = &p.blocked {
        o.error = Some(format!("blocked:{reason}"));
        return o;
    }
    if p.harness == "opencode" {
        super::opencode_cli::execute_opencode(p, mode, stamp, &mut o);
        return o;
    }
    let Some(root) = harness_root(&p.harness) else {
        o.error = Some("blocked:not_found".into());
        return o;
    };
    let files: Vec<PathBuf> = p.files.iter().map(PathBuf::from).collect();

    // Codex 永久删除：先交给官方 CLI（它会删 rollout 与数据库记录），剩下的文件再自己处理
    let mut codex_ok: Vec<String> = vec![];
    if p.harness == "codex" && mode == Mode::Permanent {
        codex_ok = run_codex_delete(&root, &p.codex_threads);
    }

    // 1. 文件
    let existing: Vec<PathBuf> = files.iter().filter(|f| f.exists()).cloned().collect();
    let file_result = match mode {
        Mode::Trash => move_to_trash(&existing),
        Mode::Permanent => remove_permanently(&existing),
    };
    if let Err(e) = file_result {
        o.error = Some(format!("files:{e}"));
        o.files_removed = existing.iter().filter(|f| !f.exists()).count();
        return o;
    }
    o.files_removed = files.len();

    // Codex 回收站模式：文件已进回收站，再清数据库
    if p.harness == "codex" && mode == Mode::Trash {
        codex_ok = run_codex_delete(&root, &p.codex_threads);
    }
    if p.harness == "codex" {
        o.codex_cli = Some(if codex_bin().is_none() {
            "missing".into()
        } else if codex_ok.len() == p.codex_threads.len() {
            "ok".into()
        } else {
            "fallback".into()
        });
    }

    // 2. 索引（Codex 的 session_index 若 CLI 已处理，这里重读后无需改动）
    let backup_root = data_dir().map(|h| h.join("backups").join(stamp).join(&p.harness));
    let ids: Vec<&str> = std::iter::once(p.id.as_str()).chain(p.codex_threads.iter().map(|s| s.as_str())).collect();
    for rel in &p.index_files {
        let path = root.join(rel);
        match rewrite_index(&path, &ids, backup_root.as_deref(), rel) {
            Ok(true) => o.index_files_updated.push(rel.clone()),
            Ok(false) => {}
            Err(e) => {
                o.error = Some(format!("index:{rel}:{e}"));
                return o;
            }
        }
    }
    if !o.index_files_updated.is_empty() {
        o.backup_dir = backup_root.map(|b| b.to_string_lossy().to_string());
    }
    o.ok = true;
    o
}

fn move_to_trash(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    // 回收站 API 不接受 `\\?\` 前缀
    let plain: Vec<PathBuf> = paths.iter().map(|p| strip_verbatim(p)).collect();
    trash::delete_all(&plain).map_err(|e| e.to_string())
}

fn remove_permanently(paths: &[PathBuf]) -> Result<(), String> {
    for p in paths {
        let r = if p.is_dir() { fs::remove_dir_all(p) } else { fs::remove_file(p) };
        r.map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonl_filter_keeps_other_lines_and_bytes() {
        let raw = "{\"sessionId\":\"session_a\",\"x\":1}\r\n{\"sessionId\":\"session_b\"}\nnot json\n{\"id\":\"session_a\"}\n";
        let out = filter_jsonl(raw, &["session_a"]).unwrap();
        assert_eq!(out, "{\"sessionId\":\"session_b\"}\nnot json\n");
        assert!(filter_jsonl(raw, &["session_zzz"]).is_none());
    }

    #[test]
    fn json_prune_arrays_and_objects_preserving_key_order() {
        let raw = "{\n  \"unit\": {\"name\": \"workspace\"},\n  \"global\": {\"archivedSessionIds\": [\"s1\", \"s2\"]},\n  \"tables\": {\"workspaces\": {\"w\": {\"path\": \"D:\\\\x\", \"sessionIds\": [\"s1\", \"s3\"]}}}\n}\n";
        let out = filter_json(raw, &["s1"]).unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v.pointer("/global/archivedSessionIds").unwrap(), &serde_json::json!(["s2"]));
        assert_eq!(v.pointer("/tables/workspaces/w/sessionIds").unwrap(), &serde_json::json!(["s3"]));
        assert!(out.find("unit").unwrap() < out.find("global").unwrap(), "key order kept");
        assert!(out.ends_with('\n'));

        let compact = "{\"sessions\":[{\"id\":\"s1\",\"touchedAt\":1},{\"id\":\"s9\",\"touchedAt\":2}]}";
        let out = filter_json(compact, &["s1"]).unwrap().unwrap();
        assert_eq!(out, "{\"sessions\":[{\"id\":\"s9\",\"touchedAt\":2}]}");
    }

    #[test]
    fn uuid_check() {
        assert!(is_uuid("019fdba8-940e-7f20-bfda-365ecb643e52"));
        assert!(!is_uuid("019fdba8-940e-7f20-bfda-365ecb643e5; rm"));
    }
}

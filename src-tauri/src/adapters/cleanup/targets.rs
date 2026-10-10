//! 按 harness 解析一条会话要删的文件，以及该工具自己的文本索引文件。
//!
//! 这里只做"找到哪些路径"，不碰文件。返回的第三个分量里的 thread id 是 Codex 特有的：
//! 官方 CLI 要按 id 逐个调，所以规划和执行都需要它。

use super::{file_mentions, Ctx};
use crate::adapters::stepcode;
use std::fs;
use std::path::{Path, PathBuf};

/// (要删的文件, 会改动的索引文件（相对 harness 根目录）, Codex 要经官方 CLI 清理的 thread id)
pub(super) type Targets = (Vec<PathBuf>, Vec<String>, Vec<String>);

pub(super) fn cc_files(root: &Path, id: &str) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Ok(projects) = fs::read_dir(root.join("projects")) {
        for proj in projects.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
            let main = proj.join(format!("{id}.jsonl"));
            if main.is_file() {
                out.push(main);
                let companion = proj.join(id);
                if companion.is_dir() {
                    out.push(companion);
                }
            }
        }
    }
    if out.is_empty() {
        return out;
    }
    for extra in ["file-history", "session-env", "tasks"] {
        let p = root.join(extra).join(id);
        if p.exists() {
            out.push(p);
        }
    }
    out
}

/// `sessions/<workspace>/<id>/`：各 workspace 下找同名会话目录
fn find_session_dir(root: &Path, id: &str) -> Option<PathBuf> {
    fs::read_dir(root.join("sessions"))
        .ok()?
        .flatten()
        .map(|ws| ws.path().join(id))
        .find(|p| p.is_dir())
}

pub(super) fn kimi_targets(root: &Path, id: &str) -> Targets {
    let Some(dir) = find_session_dir(root, id) else {
        return (vec![], vec![], vec![]);
    };
    let mut index = vec![];
    if file_mentions(&root.join("session_index.jsonl"), id) {
        index.push("session_index.jsonl".into());
    }
    if let Some(ws) = dir
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
    {
        if file_mentions(&root.join("file-history").join(ws), id) {
            index.push(format!("file-history/{ws}"));
        }
    }
    (vec![dir], index, vec![])
}

pub(super) fn dsh_targets(root: &Path, id: &str) -> Targets {
    let Some(dir) = find_session_dir(root, id) else {
        return (vec![], vec![], vec![]);
    };
    let mut files = vec![dir];
    let cache = root
        .join("storages")
        .join("session_projcache")
        .join("sessions")
        .join(format!("{id}.json"));
    if cache.is_file() {
        files.push(cache);
    }
    let mut index = vec![];
    if file_mentions(&root.join("storages").join("workspace.json"), id) {
        index.push("storages/workspace.json".into());
    }
    (files, index, vec![])
}

pub(super) fn codex_targets(root: &Path, id: &str, ctx: &Ctx) -> Targets {
    let mut files = vec![];
    let mut threads = vec![];
    let mut children = vec![];
    for (path, rid, is_sub, parent) in ctx.codex_heads(root) {
        if rid == id {
            files.push(path.clone());
        } else if *is_sub && parent.as_deref() == Some(id) {
            children.push((rid.clone(), path.clone()));
        }
    }
    if !files.is_empty() {
        threads.push(id.to_string());
    }
    for (cid, path) in children {
        files.push(path);
        if !threads.contains(&cid) {
            threads.push(cid);
        }
    }
    let mut index = vec![];
    if file_mentions(&root.join("session_index.jsonl"), id) {
        index.push("session_index.jsonl".into());
    }
    (files, index, threads)
}

/// StepCode：主 jsonl + 折叠进来的子 agent jsonl。没有索引文件要改
pub(super) fn stepcode_targets(root: &Path, id: &str) -> Targets {
    let mut out = vec![];
    // 每个 cwd 目录下找 header id 命中的主会话，以及时间落在它区间内的子 agent
    let Ok(dirs) = fs::read_dir(root) else {
        return (vec![], vec![], vec![]);
    };
    for dir in dirs.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        let Some(main) = files
            .iter()
            .find(|f| stepcode::head_id(f).as_deref() == Some(id))
        else {
            continue;
        };
        out.push(main.clone());
        // 匹配不到父会话的子 agent 会以 kind = "subagent" 单独列出，它自己就是全部目标。
        // 它的区间必然包住自己，不去掉就会把自己再收一遍——同一个文件在计划里出现两次，
        // 体积算双倍，永久删除还会在第二次 remove_file 上直接失败
        if !stepcode::is_subagent_file(main) {
            let (ma, mb) = stepcode::time_span(main);
            for f in files.iter().filter(|f| stepcode::is_subagent_file(f)) {
                let (a, b) = stepcode::time_span(f);
                if a >= ma && b <= mb {
                    out.push(f.clone());
                }
            }
        }
        break;
    }
    // 同一个文件出现两次会让体积算双倍、永久删除第二次直接失败，兜底去重
    out.dedup();
    (out, vec![], vec![])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 列表显示的占用必须等于删除时实际移除的字节：主 jsonl、同名附属目录、
    /// file-history / session-env / tasks 下的同 id 目录都要收进来
    #[test]
    fn cc_files_cover_companion_and_history_dirs() {
        let root = std::env::temp_dir().join(format!("orrery-cc-targets-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let proj = root.join("projects").join("D--code-recipe-box");
        fs::create_dir_all(&proj).unwrap();
        fs::write(proj.join("ses_target_1.jsonl"), b"{}").unwrap();
        fs::create_dir_all(proj.join("ses_target_1")).unwrap();
        fs::write(proj.join("ses_other_2.jsonl"), b"{}").unwrap();
        for (extra, hit) in [
            ("file-history", true),
            ("session-env", true),
            ("tasks", true),
            ("shell-snapshots", false),
        ] {
            let dir = root.join(extra).join("ses_target_1");
            fs::create_dir_all(&dir).unwrap();
            if !hit {
                let _ = fs::remove_dir_all(&dir);
            }
        }
        fs::create_dir_all(root.join("file-history").join("ses_other_2")).unwrap();

        let rel: Vec<String> = cc_files(&root, "ses_target_1")
            .iter()
            .map(|f| {
                f.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(
            rel,
            [
                "projects/D--code-recipe-box/ses_target_1.jsonl",
                "projects/D--code-recipe-box/ses_target_1",
                "file-history/ses_target_1",
                "session-env/ses_target_1",
                "tasks/ses_target_1",
            ]
        );
        // 每条会话只收自己的附属目录，不会把别人的收进来
        let other: Vec<String> = cc_files(&root, "ses_other_2")
            .iter()
            .map(|f| {
                f.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(
            other,
            [
                "projects/D--code-recipe-box/ses_other_2.jsonl",
                "file-history/ses_other_2"
            ]
        );
        // 没有主 jsonl 的 id：附属目录不该被收进来（否则会删到别人的历史）
        fs::create_dir_all(root.join("file-history").join("ses_nofile_3")).unwrap();
        assert!(cc_files(&root, "ses_nofile_3").is_empty());
        assert!(cc_files(&root, "ses_missing_4").is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    /// 子 agent 的 rollout 要连父会话一起删，父 id 也要进 threads（官方 CLI 按 id 逐个调）
    #[test]
    fn codex_targets_collect_children_and_parent_id() {
        let root =
            std::env::temp_dir().join(format!("orrery-codex-targets-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let day = root.join("sessions").join("2026").join("09").join("27");
        fs::create_dir_all(&day).unwrap();
        let sub = root.join("sessions").join("2026").join("09").join("26");
        fs::create_dir_all(&sub).unwrap();
        let parent = "019fdba8-940e-7f20-bfda-365ecb643e52";
        let child = "019fdba9-940e-7f20-bfda-365ecb643e53";
        let stranger = "019fdbaa-940e-7f20-bfda-365ecb643e54";
        // 头部行的真实形状：id 在 payload.id，子 agent 在 payload.source.subagent，
        // 父会话 id 在 payload.parent_thread_id
        let head = |id: &str, is_sub: bool, parent_id: Option<&str>| {
            let mut payload = serde_json::json!({ "id": id, "cwd": "D:/code/recipe-box" });
            if is_sub {
                payload["source"] = serde_json::json!({ "subagent": {} });
            }
            if let Some(p) = parent_id {
                payload["parent_thread_id"] = serde_json::json!(p);
            }
            serde_json::to_string(
                &serde_json::json!({ "type": "session_meta", "payload": payload }),
            )
            .unwrap()
        };
        fs::write(
            day.join(format!("rollout-{parent}.jsonl")),
            head(parent, false, None),
        )
        .unwrap();
        fs::write(
            sub.join(format!("rollout-{child}.jsonl")),
            head(child, true, Some(parent)),
        )
        .unwrap();
        fs::write(
            sub.join(format!("rollout-{stranger}.jsonl")),
            head(stranger, true, Some("nope")),
        )
        .unwrap();

        let ctx = Ctx::with_running(Default::default());
        let (files, index, threads) = codex_targets(&root, parent, &ctx);
        let mut rel: Vec<String> = files
            .iter()
            .map(|f| {
                f.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        rel.sort();
        assert_eq!(
            rel,
            [
                "sessions/2026/09/26/rollout-019fdba9-940e-7f20-bfda-365ecb643e53.jsonl",
                "sessions/2026/09/27/rollout-019fdba8-940e-7f20-bfda-365ecb643e52.jsonl"
            ]
        );
        // 父会话自己也要进 threads，子 agent 跟在后面（官方 CLI 按 id 逐个调）
        assert_eq!(threads, [parent.to_string(), child.to_string()]);
        assert!(index.is_empty(), "没写 session_index 就什么都不改");

        // 会话不存在时不返回文件，但也不该报错
        assert!(
            codex_targets(&root, "019fdbaa-940e-7f20-bfda-365ecb643e99", &ctx)
                .0
                .is_empty()
        );
        let _ = fs::remove_dir_all(&root);
    }
}

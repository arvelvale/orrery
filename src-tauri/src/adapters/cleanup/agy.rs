//! Antigravity（agy）对话的删除目标。
//!
//! 一个对话 = `conversations/<id>.db`（+ `-wal` / `-shm`）+ `brain/<id>/` + `annotations/<id>.pbtxt`，
//! 子对话再多一份同形状的文件。子对话关系只读 agy 的摘要库，**不写**它——
//! 所以删完它的历史列表里可能还留着标题，删除前要在界面上说明。

use crate::adapters::opencode;
use super::targets::Targets;
use super::valid_id;
use std::fs;
use std::path::Path;

/// 对话及其子对话自己的文件。子对话关系只读 agy 的摘要库
pub(super) fn agy_targets(root: &Path, id: &str) -> Targets {
    let conv = root.join("conversations");
    if !conv.join(format!("{id}.db")).is_file() {
        return (vec![], vec![], vec![]);
    }
    let mut ids = vec![id.to_string()];
    if let Some(con) = opencode::open(&root.join("conversation_summaries.db")) {
        if let Ok(mut stmt) = con.prepare("SELECT conversation_id, parent_conversation_id FROM conversation_summaries") {
            let pairs: Vec<(String, String)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))
                .map(|rows| rows.flatten().collect())
                .unwrap_or_default();
            // 逐层展开；出现过的不再加入，防成环
            let mut i = 0;
            while i < ids.len() {
                let parent = ids[i].clone();
                for (child, p) in &pairs {
                    if *p == parent && valid_id(child) && !ids.contains(child) {
                        ids.push(child.clone());
                    }
                }
                i += 1;
            }
        }
    }
    let mut files = vec![];
    for cid in &ids {
        for ext in ["db", "db-wal", "db-shm"] {
            let f = conv.join(format!("{cid}.{ext}"));
            if f.exists() {
                files.push(f);
            }
        }
        for f in [root.join("brain").join(cid), root.join("annotations").join(format!("{cid}.pbtxt"))] {
            if f.exists() {
                files.push(f);
            }
        }
    }
    (files, vec![], vec![])
}

/// agy 正打开这条对话：它对 `presence/<id>.lock` 加了字节范围锁（Windows `LockFileEx`）。
/// 实测文件照样打得开，**读**才会失败，所以要真读一下；没锁的空文件读到 0 字节
pub(super) fn agy_open(root: &Path, id: &str) -> bool {
    use std::io::Read;
    let lock = root.join("presence").join(format!("{id}.lock"));
    if !lock.is_file() {
        return false;
    }
    match fs::File::open(&lock) {
        Ok(mut f) => f.read(&mut [0u8; 1]).is_err(),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对话自己的文件 + 子对话的文件；别的对话、摘要库都不碰
    #[test]
    fn agy_targets_cover_own_files_and_child_conversations_only() {
        let root = std::env::temp_dir().join(format!("orrery-agy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for d in ["conversations", "brain/conv_parent_1/scratch", "brain/conv_child_22", "brain/conv_other_3", "annotations", "presence"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in ["conv_parent_1.db", "conv_parent_1.db-wal", "conv_child_22.db", "conv_other_3.db"] {
            fs::write(root.join("conversations").join(f), b"x").unwrap();
        }
        fs::write(root.join("annotations/conv_parent_1.pbtxt"), b"x").unwrap();
        let con = rusqlite::Connection::open(root.join("conversation_summaries.db")).unwrap();
        con.execute_batch(
            "CREATE TABLE conversation_summaries(conversation_id TEXT, parent_conversation_id TEXT);
             INSERT INTO conversation_summaries VALUES ('conv_parent_1', ''), ('conv_child_22', 'conv_parent_1'), ('conv_other_3', '');",
        )
        .unwrap();
        drop(con);

        let (files, index, _) = agy_targets(&root, "conv_parent_1");
        let names: Vec<String> =
            files.iter().map(|f| f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(
            names,
            [
                "conversations/conv_parent_1.db",
                "conversations/conv_parent_1.db-wal",
                "brain/conv_parent_1",
                "annotations/conv_parent_1.pbtxt",
                "conversations/conv_child_22.db",
                "brain/conv_child_22",
            ]
        );
        assert!(index.is_empty(), "摘要库是 agy 的，不改");
        assert!(agy_targets(&root, "conv_missing_9").0.is_empty());

        // 留下的旧锁文件不算"打开中"；被独占的才算
        fs::write(root.join("presence/conv_parent_1.lock"), b"").unwrap();
        assert!(!agy_open(&root, "conv_parent_1"));
        // 和 agy 同一种锁：Windows 上 File::lock 就是 LockFileEx。只在测试里用，CI 跑 stable
        #[cfg(windows)]
        #[allow(clippy::incompatible_msrv)]
        {
            let held = fs::OpenOptions::new().read(true).write(true).open(root.join("presence/conv_parent_1.lock")).unwrap();
            held.lock().unwrap();
            assert!(agy_open(&root, "conv_parent_1"), "字节范围锁要判成打开中");
            held.unlock().unwrap();
            assert!(!agy_open(&root, "conv_parent_1"));
        }
        let _ = fs::remove_dir_all(&root);
    }
}

//! 持久化解析索引：`~/.orrery/index.json`
//!
//! 解析缓存原本只活在进程内存里，每次开应用都要把四个 harness 的日志重新读一遍
//! （本机 188 个会话实测冷启动 6–7s，其中大头是磁盘 IO）。这里把同一张表落盘：
//! 启动时读回来，扫描时只对 `sig`（长度 + mtime）变了的文件重新解析。
//!
//! 约定：
//! - 索引只是缓存，坏了、缺了、版本对不上都直接丢弃重建，不允许因为它让会话列表出错
//! - `SessionSummary` / `Rollout` 的字段一旦变动就要 bump `VERSION`，否则会读出半截数据
//! - 写入走临时文件 + rename，避免进程被杀时留下半个文件
//! - 只存本机已有的路径：保存前剔除文件已消失的条目（会话被删掉后不留垃圾）

use super::codex::Rollout;
use super::{data_dir, MemoTable, SessionSummary};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// 索引格式版本：`SessionSummary` / `Rollout` 改字段必须 +1
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    /// (文件或目录路径, sig, 解析结果)。用数组而不是 map：JSON 的 map 键只能是字符串，
    /// Windows 路径进 key 会很难看，也容易在反序列化时丢信息
    sessions: Vec<(PathBuf, u64, SessionSummary)>,
    rollouts: Vec<(PathBuf, u64, Rollout)>,
}

/// 进程内的解析缓存 + 落盘状态
pub(crate) struct Store {
    /// Claude Code / Kimi / DSH：一个会话一条
    pub(crate) sessions: MemoTable<SessionSummary>,
    /// Codex：一个 rollout 文件一条（多个 rollout 合并成一个会话，缓存要在合并之前）
    pub(crate) rollouts: MemoTable<Rollout>,
    dirty: AtomicBool,
    /// 本进程真正重新解析过的文件数（缓存命中的不算），进日志方便看索引有没有生效
    parsed: AtomicUsize,
}

impl Store {
    /// 有文件被重新解析：标记待落盘并计数
    pub(crate) fn mark_parsed(&self) {
        self.mark_dirty();
        self.parsed.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub(crate) fn parsed(&self) -> usize {
        self.parsed.load(Ordering::Relaxed)
    }
}

pub(crate) fn path() -> Option<PathBuf> {
    Some(data_dir()?.join("index.json"))
}

/// 读索引建缓存。任何异常（没有文件、版本不符、解析失败）都退化成空表
pub(crate) fn load() -> Store {
    load_from(path().as_deref())
}

fn load_from(file: Option<&Path>) -> Store {
    // 读回来时就把文件已经消失的条目扔掉：会话可能是被别的工具或手工删掉的，
    // 光靠保存时剔除的话，只要没有文件变动就永远不会重写索引，死条目会一直留着
    fn to_map<T>(v: Vec<(PathBuf, u64, T)>, dropped: &mut usize) -> HashMap<PathBuf, (u64, T)> {
        v.into_iter()
            .filter(|(p, _, _)| {
                let alive = p.exists();
                *dropped += usize::from(!alive);
                alive
            })
            .map(|(p, sig, val)| (p, (sig, val)))
            .collect()
    }
    let mut dropped = 0;
    let snap = file
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Snapshot>(&b).ok())
        .filter(|s| s.version == VERSION)
        .unwrap_or(Snapshot { version: VERSION, sessions: vec![], rollouts: vec![] });
    if !snap.sessions.is_empty() || !snap.rollouts.is_empty() {
        eprintln!(
            "[orrery] index: {} sessions + {} rollouts loaded",
            snap.sessions.len(),
            snap.rollouts.len()
        );
    }
    let store = Store {
        sessions: MemoTable::new(to_map(snap.sessions, &mut dropped)),
        rollouts: MemoTable::new(to_map(snap.rollouts, &mut dropped)),
        dirty: AtomicBool::new(false),
        parsed: AtomicUsize::new(0),
    };
    if dropped > 0 {
        eprintln!("[orrery] index: {dropped} stale entries dropped");
        store.mark_dirty();
    }
    store
}

/// 缓存有变动才写盘；顺手剔除文件已经不在的条目
pub(crate) fn save_if_dirty(store: &Store) {
    if !store.dirty.swap(false, Ordering::Relaxed) {
        return;
    }
    let alive = |p: &Path| p.exists();
    let snap = Snapshot {
        version: VERSION,
        sessions: store.sessions.entries(alive),
        rollouts: store.rollouts.entries(alive),
    };
    if let Err(e) = write_to(path().as_deref(), &snap) {
        // 索引只是缓存，写不进去不该影响功能，记一笔就算了
        eprintln!("[orrery] index: save failed: {e}");
        store.mark_dirty();
    }
}

fn write_to(file: Option<&Path>, snap: &Snapshot) -> Result<(), String> {
    let path = file.ok_or("cannot resolve ~/.orrery")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec(snap).map_err(|e| e.to_string())?;
    // 临时文件 + rename：进程中途被杀也不会留下半个索引
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    eprintln!(
        "[orrery] index: saved {} sessions + {} rollouts ({} KB)",
        snap.sessions.len(),
        snap.rollouts.len(),
        bytes.len() / 1024
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_mismatch_is_discarded() {
        let bad = serde_json::json!({ "version": VERSION + 1, "sessions": [], "rollouts": [] });
        let parsed = serde_json::from_value::<Snapshot>(bad).ok().filter(|s| s.version == VERSION);
        assert!(parsed.is_none(), "版本不符必须整份丢弃，不能当成半份索引继续用");
    }

    #[test]
    fn broken_json_loads_as_empty() {
        let dir = std::env::temp_dir().join(format!("orrery-index-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("index.json");
        std::fs::write(&file, b"{not json").unwrap();
        let store = load_from(Some(&file));
        assert!(store.sessions.entries(|_| true).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 存 → 读回来要一模一样；文件已消失的条目不该写进索引
    #[test]
    fn round_trip_keeps_live_entries_and_drops_dead_ones() {
        let dir = std::env::temp_dir().join(format!("orrery-index-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let live = dir.join("live.jsonl");
        let dead = dir.join("deleted.jsonl");
        std::fs::write(&live, b"x").unwrap();

        let mut s = SessionSummary {
            id: "abc".into(),
            harness: "cc".into(),
            title: "标题".into(),
            project: "D:/x".into(),
            model: "claude-opus-5".into(),
            status: "idle".into(),
            updated_ms: 1_700_000_000_000,
            tokens: "1.2k".into(),
            usage: Default::default(),
            excerpt: "摘要".into(),
            path: live.to_string_lossy().into(),
            log: vec![],
            size_bytes: 42,
            subagents: 1,
            kind: String::new(),
        };
        s.usage.input = 1200;

        let store = load_from(None);
        store.sessions.put(&live, 7, &s);
        store.sessions.put(&dead, 9, &s);
        store.mark_dirty();

        let file = dir.join("index.json");
        let snap = Snapshot {
            version: VERSION,
            sessions: store.sessions.entries(|p| p.exists()),
            rollouts: vec![],
        };
        write_to(Some(&file), &snap).unwrap();

        let back = load_from(Some(&file));
        assert!(back.sessions.get(&dead, 9).is_none(), "文件已删除的条目不能留在索引里");
        let got = back.sessions.get(&live, 7).expect("sig 相同应命中缓存");
        assert_eq!((got.id, got.title, got.usage.input, got.size_bytes), ("abc".into(), "标题".into(), 1200, 42));
        assert!(back.sessions.get(&live, 8).is_none(), "sig 变了必须当作未命中，重新解析");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

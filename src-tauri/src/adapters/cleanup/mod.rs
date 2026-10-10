//! 删除会话：移到回收站或永久删除，并清理各 harness 的文本索引
//!
//! 安全约束（改动前先读完）：
//! 1. 路径只由后端按 (harness, id) 解析，从不采信前端传来的路径；每个目标都必须位于该 harness
//!    数据根目录之内（按真实路径比较，防 `..` 与链接逃逸）
//! 2. 最近 [`ACTIVE_WINDOW_MS`] 内有写入、或 Claude Code 登记为运行中的会话拒绝删除
//! 3. 先删文件，全部成功后才改索引；改索引前把原文件备份到 `~/.orrery/backups/<时间>/`，
//!    写入走"同目录临时文件 + 原子替换"，且写前重读，只移除属于这些会话的条目
//! 4. Codex 的 sqlite（`state_5` 的 threads、`thread_history_1` 里的会话内容副本）只通过官方
//!    `codex delete --force <uuid>` 清理，Orrery 不直接写 Codex 数据库。沙盒实测：
//!    - 删除父会话会一并删 rollout、threads 行、thread_history 条目、session_index 行
//!    - 不会删 guardian 子 agent → 逐个子 agent 再调用
//!    - rollout 已先移到回收站时仍能清掉数据库记录 → 回收站模式可行
//!    - id 不在数据库里时报错 → 退回自行删除文件与 session_index 行
//! 5. OpenCode 的会话只在它自己的 SQLite 里，同样只经官方 CLI 删除（1.18.31 沙盒实测，
//!    用真实库的副本）：
//!    - `opencode session delete <id>` 会连同子 agent 一起删（父子行、message、part 全清）
//!    - `opencode export <id>` 只导出这一条，**不含**子 agent → 回收站模式逐条导出
//!    - `opencode import <file>` 能还原：父子 2 条、261 条消息、1195 个 part 的 `data` 解析后
//!      全部相等（只是 JSON 键顺序被重排），会话行逐字段相等；但项目与 `directory` 取自
//!      **运行 import 时的工作目录** → 恢复说明里先 cd
//!    - 库是 `auto_vacuum=0`，删掉的行变成空闲页留给 OpenCode 复用，文件不会立即变小
//!
//!    所以"回收站"对 OpenCode 的含义是：导出 JSON 到 `~/.orrery/exports/` 再删，旁边写一份
//!    RESTORE.txt 给出按顺序执行的恢复命令
//!
//! 各 harness 牵涉的数据（均为本机实测）：
//! | harness | 文件 | 索引 |
//! |---|---|---|
//! | cc    | `projects/<p>/<id>.jsonl`、`projects/<p>/<id>/`、`file-history/<id>/`、`session-env/<id>/`、`tasks/<id>/` | 无（`history.jsonl` 是输入历史，不动） |
//! | kimi  | `sessions/<ws>/<id>/` | `session_index.jsonl` 行、`file-history/<ws>` 的 `sessions[]` |
//! | dsh   | `sessions/<ws>/<id>/`、`storages/session_projcache/sessions/<id>.json` | `storages/workspace.json` 的 `sessionIds` / `archivedSessionIds` |
//! | codex | 该 id 的所有 rollout + 以它为父的子 agent rollout | sqlite（经 `codex delete`）、`session_index.jsonl` 行（兜底） |
//! | opencode | 无（全在 `opencode.db`） | 经 `opencode session delete` |
//! | antigravity | `conversations/<id>.db`(-wal/-shm)、`brain/<id>/`、`annotations/<id>.pbtxt`，及子对话的同类文件 | 无 |
//! | stepcode | `sessions/--<cwd>--/<ts>_<id>.jsonl` 及折叠的子 agent jsonl | 无 |
//!
//! Antigravity（agy，2026-09 版沙盒实测）：`conversation_summaries.db` 是 agy 的库，我们不写。
//! 对话文件删掉后 agy 照常启动；`--conversation <已删 id>` 只提示 not found 并开新对话。
//! 摘要库里那一行 agy 不会自己清（二进制里有 prune 逻辑，但启动、`-p`、交互三种方式都没触发），
//! 所以它的历史列表里可能还留着标题——删除前在对话框里说明。
//! 正在打开的对话：agy 会独占 `presence/<id>.lock`（旧锁文件不会被清，要看的是能否打开，
//! 实测一个 agy 进程恰好锁一个文件）；Unix 上是建议锁、打得开，退回到 10 分钟写入窗口
//!
//! StepCode：官方文档明说"Sessions can be removed by deleting their `.jsonl` files"，
//! 没有需要清理的文本索引，也不写任何别人的数据库。恢复命令见 `resume.rs`
//! （只能走 jsonl 绝对路径，沙盒实测 id 形式查不到）。

//! # 模块划分
//!
//! | 文件 | 职责 |
//! |---|---|
//! | `mod.rs`（本文件） | 类型、入口、规划流程、各 harness 共用的路径/时间工具、跨模块测试 |
//! | `targets.rs` | 按 harness 解析要删的文件与该工具自己的索引文件 |
//! | `agy.rs` | Antigravity 对话及其子对话的目标文件、是否正被打开 |
//! | `opencode_cli.rs` | OpenCode：只读规划 + 经官方 CLI 的导出/删除/恢复说明 |
//! | `execute.rs` | 真正落地：回收站/永久删除、索引改写、Codex 官方 CLI |
//! | `guard.rs` | 跨平台进程查询与"会话是否正在使用"的判定 |
mod agy;
mod execute;
mod guard;
mod opencode_cli;
mod targets;

// 会话转换（`transfer/`）复用这两个能力
pub(crate) use execute::codex_bin;
pub(crate) use guard::no_window;
pub(crate) use guard::pid_alive;
pub(crate) use opencode_cli::opencode_bin;

use super::{
    antigravity, claude_home, codex, codex_home, dir_size, dsh_home, forget_memo, kimi_home,
    system_time_ms,
};
use agy::{agy_open, agy_targets};
use opencode_cli::plan_opencode;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use targets::{cc_files, codex_targets, dsh_targets, kimi_targets, stepcode_targets};

/// 最近这么久内有写入的会话视为可能正在使用
pub const ACTIVE_WINDOW_MS: u64 = 10 * 60 * 1000;

#[derive(Debug, Clone, Deserialize)]
pub struct Target {
    pub harness: String,
    pub id: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Trash,
    Permanent,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Plan {
    pub harness: String,
    pub id: String,
    pub files: Vec<String>,
    pub bytes: u64,
    /// 会改动的索引文件（相对 harness 根目录）
    pub index_files: Vec<String>,
    /// Codex：需要经 `codex delete` 清理数据库的 thread id（父 + 子 agent）
    pub codex_threads: Vec<String>,
    /// OpenCode：要经 CLI 处理的会话 id，根会话在前、子 agent 按层级在后（导入也按这个顺序）
    pub cli_sessions: Vec<String>,
    /// OpenCode：会话记录的工作目录，恢复时要在这里运行 `opencode import`
    #[serde(skip)]
    pub directory: String,
    /// 非空 = 不允许删除，值为原因代码：not_found / active / running / invalid / read_only / cli_missing
    pub blocked: Option<String>,
    /// 不阻止删除的提醒代码：harness_running / codex_cli_missing
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Outcome {
    pub harness: String,
    pub id: String,
    pub ok: bool,
    pub mode: String,
    pub bytes: u64,
    pub files_removed: usize,
    pub index_files_updated: Vec<String>,
    /// Codex：ok / fallback / missing / skipped
    pub codex_cli: Option<String>,
    pub error: Option<String>,
    pub backup_dir: Option<String>,
    /// OpenCode 回收站模式：导出的 JSON 与 RESTORE.txt 所在目录
    pub export_dir: Option<String>,
}

/* ── 入口 ── */

pub fn plan_all(targets: &[Target]) -> Vec<Plan> {
    let ctx = Ctx::new();
    targets.iter().map(|t| plan(t, &ctx)).collect()
}

/// 一批删除共享的只读上下文：进程列表只查一次，Codex rollout 首行只扫一次
/// 一个 codex rollout 的头部信息：文件路径、会话 id、是否子 agent、父会话 id
pub(super) type CodexHead = (PathBuf, String, bool, Option<String>);

pub(super) struct Ctx {
    pub(super) running: HashSet<String>,
    codex_heads: std::cell::OnceCell<Vec<CodexHead>>,
}

impl Ctx {
    pub(super) fn new() -> Self {
        Self::with_running(guard::running_processes())
    }

    pub(super) fn with_running(running: HashSet<String>) -> Self {
        Self {
            running,
            codex_heads: std::cell::OnceCell::new(),
        }
    }

    pub(super) fn codex_heads(&self, root: &Path) -> &[CodexHead] {
        self.codex_heads.get_or_init(|| {
            codex::collect_rollouts(&root.join("sessions"))
                .into_iter()
                .filter_map(|p| codex::read_head(&p).map(|(id, sub, parent)| (p, id, sub, parent)))
                .collect()
        })
    }
}

pub fn delete_all(targets: &[Target], mode: Mode) -> Vec<Outcome> {
    let stamp = backup_stamp();
    let running = guard::running_processes();
    let mut removed: Vec<PathBuf> = vec![];
    let outcomes = targets
        .iter()
        .map(|t| {
            // 执行前重新规划，不信任前端或早先的计划
            // Codex 首行索引每条重建：前一条删除后文件列表已变化；进程列表整批共用
            let p = plan(t, &Ctx::with_running(running.clone()));
            let o = execute::execute(&p, mode, &stamp);
            if o.ok {
                removed.extend(p.files.iter().map(PathBuf::from));
            }
            o
        })
        .collect();
    forget_memo(&removed);
    outcomes
}

/* ── 规划 ── */

fn plan(t: &Target, ctx: &Ctx) -> Plan {
    let mut p = Plan {
        harness: t.harness.clone(),
        id: t.id.clone(),
        ..Default::default()
    };
    if !valid_id(&t.id) {
        p.blocked = Some("invalid".into());
        return p;
    }
    // OpenCode 不碰任何文件，整条走官方 CLI
    if t.harness == "opencode" {
        return plan_opencode(p, ctx);
    }
    // Z Code 与自定义登记的 harness 在各自的 SQLite 里，也没有可用的删除命令。
    // 在查找目录之前挡掉，避免未来的路径解析改动误删整个共享数据库。
    if t.harness == "zcode" || super::custom::is_custom(&t.harness) {
        p.blocked = Some("read_only".into());
        return p;
    }
    let Some(root) = harness_root(&t.harness) else {
        p.blocked = Some("not_found".into());
        return p;
    };
    let (files, index_files, codex_threads) = match t.harness.as_str() {
        "cc" => (cc_files(&root, &t.id), vec![], vec![]),
        "kimi" => kimi_targets(&root, &t.id),
        "dsh" => dsh_targets(&root, &t.id),
        "codex" => codex_targets(&root, &t.id, ctx),
        "antigravity" => agy_targets(&root, &t.id),
        "stepcode" => stepcode_targets(&root, &t.id),
        _ => {
            p.blocked = Some("invalid".into());
            return p;
        }
    };
    // 越界校验：任何目标不在根目录内就整条拒绝
    let Some(canon_root) = canonical(&root) else {
        p.blocked = Some("not_found".into());
        return p;
    };
    if files
        .iter()
        .any(|f| !canonical(f).is_some_and(|c| c.starts_with(&canon_root)))
    {
        p.blocked = Some("invalid".into());
        return p;
    }
    if files.is_empty() && codex_threads.is_empty() {
        p.blocked = Some("not_found".into());
        return p;
    }

    p.bytes = files.iter().map(|f| path_size(f)).sum();
    p.files = files
        .iter()
        .map(|f| f.to_string_lossy().to_string())
        .collect();
    p.index_files = index_files;
    p.codex_threads = codex_threads;

    // SQLite 的 `-shm` 是共享内存索引，只读打开也会刷新它的修改时间（Orrery 自己每次扫描都会碰），
    // 不代表对话有新内容，不参与判断
    let last_write = files
        .iter()
        .filter(|f| !f.to_string_lossy().ends_with("-shm"))
        .map(|f| latest_mtime(f))
        .max()
        .unwrap_or(0);
    let now = system_time_ms(SystemTime::now());
    if now.saturating_sub(last_write) < ACTIVE_WINDOW_MS {
        p.blocked = Some("active".into());
    }
    if t.harness == "cc" && guard::cc_is_running(&root, &t.id) {
        p.blocked = Some("running".into());
    }
    if t.harness == "antigravity" && agy_open(&root, &t.id) {
        p.blocked = Some("open_in_tool".into());
    }

    // 进程名已在 running_processes 里去掉了 .exe 后缀，三个平台比同一个名字
    let proc = match t.harness.as_str() {
        "kimi" => Some("kimi"),
        "codex" => Some("codex"),
        "stepcode" => Some("step"),
        _ => None,
    };
    // agy 的摘要库会留着标题：不阻止删除，但要让用户事先知道
    if t.harness == "antigravity" {
        p.warnings.push("agy_title_kept".into());
    }
    if proc.is_some_and(|name| ctx.running.contains(name)) {
        p.warnings.push("harness_running".into());
    }
    if t.harness == "codex" && execute::codex_bin().is_none() {
        p.warnings.push("codex_cli_missing".into());
    }
    p
}

fn harness_root(harness: &str) -> Option<PathBuf> {
    let root = match harness {
        "cc" => claude_home(),
        "kimi" => kimi_home(),
        "dsh" => dsh_home(),
        "codex" => codex_home(),
        "antigravity" => antigravity::agy_home(),
        "stepcode" => super::stepcode_agent_dir().map(|d| d.join("sessions")),
        _ => None,
    }?;
    root.is_dir().then_some(root)
}

/// 会话 id 只允许字母数字、`-`、`_`；拒绝任何可能构成路径的字符
pub(super) fn valid_id(id: &str) -> bool {
    (8..=80).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/* ── 小工具（ targets / execute 共用）── */

pub(super) fn canonical(p: &Path) -> Option<PathBuf> {
    fs::canonicalize(p).ok().map(|c| strip_verbatim(&c))
}

pub(super) fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
}

pub(super) fn path_size(p: &Path) -> u64 {
    if p.is_dir() {
        dir_size(p)
    } else {
        fs::metadata(p).map(|m| m.len()).unwrap_or(0)
    }
}

/// 文件或目录内最新的修改时间（目录只看两层，足够覆盖会话写入）
pub(super) fn latest_mtime(p: &Path) -> u64 {
    fn walk(p: &Path, depth: u32) -> u64 {
        let own = fs::metadata(p)
            .and_then(|m| m.modified())
            .map(system_time_ms)
            .unwrap_or(0);
        if depth == 0 || !p.is_dir() {
            return own;
        }
        let Ok(entries) = fs::read_dir(p) else {
            return own;
        };
        entries
            .flatten()
            .map(|e| walk(&e.path(), depth - 1))
            .max()
            .unwrap_or(0)
            .max(own)
    }
    walk(p, 3)
}

pub(super) fn file_mentions(path: &Path, id: &str) -> bool {
    fs::read(path).is_ok_and(|b| super::contains(&b, id.as_bytes()))
}

pub(super) fn backup_stamp() -> String {
    let ms = system_time_ms(SystemTime::now());
    format!("{ms}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// StepCode：主 jsonl + 折叠的子 agent jsonl，没有索引文件，也不走官方 CLI
    #[test]
    fn stepcode_targets_collect_the_session_and_its_subagents() {
        let root = std::env::temp_dir().join(format!("orrery-step-targets-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let sessions = root.join("sessions");
        let dir = sessions.join("--D--code-recipe--");
        fs::create_dir_all(&dir).unwrap();
        let parent = "01a12010-5077-70de-838d-3f8e8537716b";
        let child = "subagent-c0aed0ba-ca71-4f4e-a50f-cf26113130d3";
        let stranger = "01a12099-5077-70de-838d-3f8e8537716b";
        let head = |id: &str, ts: &str| {
            serde_json::to_string(&serde_json::json!({
                "type": "session", "version": 3, "id": id, "timestamp": ts, "cwd": "D:\\code\\recipe"
            }))
            .unwrap()
        };
        let tail = |ts: &str| {
            serde_json::to_string(&serde_json::json!({
                "type": "message", "id": "m1", "parentId": null, "timestamp": ts,
                "message": { "role": "user", "content": "x" }
            }))
            .unwrap()
        };
        let write = |name: &str, id: &str, ts: &str, tail_ts: &str| {
            fs::write(
                dir.join(name),
                format!("{}\n{}", head(id, ts), tail(tail_ts)),
            )
            .unwrap();
        };
        write(
            &format!("2026-10-09T09-48-22-520Z_{parent}.jsonl"),
            parent,
            "2026-10-09T09:48:22.520Z",
            "2026-10-09T09:50:00.000Z",
        );
        write(
            &format!("2026-10-09T09-49-00-000Z_{child}.jsonl"),
            child,
            "2026-10-09T09:49:00.000Z",
            "2026-10-09T09:49:30.000Z",
        );
        // 区间完全不重叠的子 agent：父会话不在本机，不该被捎带删掉
        let orphan = "subagent-99999999-2222-3333-4444-555555555555";
        write(
            &format!("2026-10-09T11-49-00-000Z_{orphan}.jsonl"),
            orphan,
            "2026-10-09T11:49:00.000Z",
            "2026-10-09T11:49:30.000Z",
        );
        write(
            &format!("2026-10-09T12-48-22-520Z_{stranger}.jsonl"),
            stranger,
            "2026-10-09T12:48:22.520Z",
            "2026-10-09T12:50:00.000Z",
        );

        let (files, index, threads) = targets::stepcode_targets(&sessions, parent);
        let mut rel: Vec<String> = files
            .iter()
            .map(|f| {
                f.strip_prefix(&sessions)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        rel.sort();
        assert_eq!(
            rel,
            [
                format!("--D--code-recipe--/2026-10-09T09-48-22-520Z_{parent}.jsonl"),
                format!("--D--code-recipe--/2026-10-09T09-49-00-000Z_{child}.jsonl"),
            ],
            "只收主会话和区间落在它里面的子 agent"
        );
        assert!(index.is_empty(), "StepCode 没有要清理的文本索引");
        assert!(threads.is_empty(), "不走官方 CLI");
        assert!(
            targets::stepcode_targets(&sessions, "01a12000-5077-70de-838d-3f8e8537716b")
                .0
                .is_empty()
        );

        // 匹配不到父会话的子 agent 会单独列出来，删它就只删它自己：区间必然包住自己，
        // 漏掉这个判断就会把自己再收一遍——同一个文件在计划里出现两次，体积算双倍，
        // 永久删除还会在第二次 remove_file 上直接失败
        let (files, _, _) = targets::stepcode_targets(&sessions, orphan);
        assert_eq!(
            files,
            [dir.join(format!("2026-10-09T11-49-00-000Z_{orphan}.jsonl"))]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn zcode_plan_is_read_only_without_resolving_files() {
        let p = plan(
            &Target {
                harness: "zcode".into(),
                id: "ses_sandbox_only".into(),
            },
            &Ctx::with_running(HashSet::new()),
        );
        assert_eq!(p.blocked.as_deref(), Some("read_only"));
        assert!(p.files.is_empty());
        assert!(p.index_files.is_empty());
        assert_eq!(p.bytes, 0);
    }

    /// 在 agy 数据目录的**副本**上端到端删一条对话。默认不跑：
    /// ORRERY_HOME 指向沙盒（放 `.gemini/antigravity-cli` 副本），ORRERY_AGY_ID 是对话 id，
    /// `cargo test --lib agy_sandbox -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn agy_sandbox_trash() {
        let home = std::env::var_os("ORRERY_HOME").expect("ORRERY_HOME 必须指向沙盒");
        assert_ne!(
            Some(PathBuf::from(&home)),
            dirs::home_dir(),
            "不能对真实主目录跑"
        );
        let id = std::env::var("ORRERY_AGY_ID").expect("ORRERY_AGY_ID");
        let target = Target {
            harness: "antigravity".into(),
            id,
        };
        let plan = plan_all(std::slice::from_ref(&target)).remove(0);
        println!("{plan:?}");
        assert!(plan.blocked.is_none(), "{:?}", plan.blocked);
        let out = delete_all(&[target], Mode::Trash).remove(0);
        println!("{out:?}");
        assert!(out.ok, "{:?}", out.error);
        for f in &plan.files {
            assert!(!Path::new(f).exists(), "{f} 还在");
        }
    }

    /// 在 OpenCode 库的**副本**上端到端跑回收站模式。默认不跑：
    /// ORRERY_HOME 指向沙盒（放 `.local/share/opencode/opencode.db` 副本），ORRERY_OC_ID 是根会话，
    /// `cargo test opencode_sandbox -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn opencode_sandbox_trash_roundtrip() {
        let home = std::env::var_os("ORRERY_HOME").expect("ORRERY_HOME 必须指向沙盒");
        assert_ne!(
            Some(PathBuf::from(&home)),
            dirs::home_dir(),
            "不能对真实主目录跑"
        );
        let id = std::env::var("ORRERY_OC_ID").expect("ORRERY_OC_ID");
        let target = Target {
            harness: "opencode".into(),
            id,
        };
        let plan = plan_all(std::slice::from_ref(&target)).remove(0);
        println!("{plan:?}");
        assert!(plan.blocked.is_none(), "{:?}", plan.blocked);
        let out = delete_all(&[target], Mode::Trash).remove(0);
        println!("{out:?}");
        assert!(out.ok, "{:?}", out.error);
        let dir = PathBuf::from(out.export_dir.unwrap());
        for sid in &plan.cli_sessions {
            assert!(dir.join(format!("{sid}.json")).is_file(), "{sid} 没导出");
        }
        println!("{}", fs::read_to_string(dir.join("RESTORE.txt")).unwrap());
    }

    #[test]
    fn id_validation_rejects_paths() {
        assert!(valid_id("session_0ed9be17-001b-4642-8b8a-2f71a7df757c"));
        assert!(valid_id("983ae63c-4c5e-483e-8269-d7510506ee68"));
        for bad in [
            "../../etc",
            "a/b/c/d/e/f",
            r"..\..\x",
            "short",
            "abc def ghij",
            "C:secretsxx",
        ] {
            assert!(!valid_id(bad), "{bad}");
        }
    }
}

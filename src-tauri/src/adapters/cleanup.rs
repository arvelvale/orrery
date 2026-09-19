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
//!
//! Antigravity（agy，2026-09 版沙盒实测）：`conversation_summaries.db` 是 agy 的库，我们不写。
//! 对话文件删掉后 agy 照常启动；`--conversation <已删 id>` 只提示 not found 并开新对话。
//! 摘要库里那一行 agy 不会自己清（二进制里有 prune 逻辑，但启动、`-p`、交互三种方式都没触发），
//! 所以它的历史列表里可能还留着标题——删除前在对话框里说明。
//! 正在打开的对话：agy 会独占 `presence/<id>.lock`（旧锁文件不会被清，要看的是能否打开，
//! 实测一个 agy 进程恰好锁一个文件）；Unix 上是建议锁、打得开，退回到 10 分钟写入窗口

use super::{
    antigravity, claude_home, codex, codex_home, data_dir, dir_size, dsh_home, forget_memo, kimi_home, opencode,
    system_time_ms,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
type CodexHead = (PathBuf, String, bool, Option<String>);

struct Ctx {
    running: HashSet<String>,
    codex_heads: std::cell::OnceCell<Vec<CodexHead>>,
}

impl Ctx {
    fn new() -> Self {
        Self::with_running(running_processes())
    }

    fn with_running(running: HashSet<String>) -> Self {
        Self { running, codex_heads: std::cell::OnceCell::new() }
    }

    fn codex_heads(&self, root: &Path) -> &[CodexHead] {
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
    let running = running_processes();
    let mut removed: Vec<PathBuf> = vec![];
    let outcomes = targets
        .iter()
        .map(|t| {
            // 执行前重新规划，不信任前端或早先的计划
            // Codex 首行索引每条重建：前一条删除后文件列表已变化；进程列表整批共用
            let p = plan(t, &Ctx::with_running(running.clone()));
            let o = execute(&p, mode, &stamp);
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
    let mut p = Plan { harness: t.harness.clone(), id: t.id.clone(), ..Default::default() };
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
    if files.iter().any(|f| !canonical(f).is_some_and(|c| c.starts_with(&canon_root))) {
        p.blocked = Some("invalid".into());
        return p;
    }
    if files.is_empty() && codex_threads.is_empty() {
        p.blocked = Some("not_found".into());
        return p;
    }

    p.bytes = files.iter().map(|f| path_size(f)).sum();
    p.files = files.iter().map(|f| f.to_string_lossy().to_string()).collect();
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
    if t.harness == "cc" && cc_is_running(&root, &t.id) {
        p.blocked = Some("running".into());
    }
    if t.harness == "antigravity" && agy_open(&root, &t.id) {
        p.blocked = Some("open_in_tool".into());
    }

    // 进程名已在 running_processes 里去掉了 .exe 后缀，三个平台比同一个名字
    let proc = match t.harness.as_str() {
        "kimi" => Some("kimi"),
        "codex" => Some("codex"),
        _ => None,
    };
    // agy 的摘要库会留着标题：不阻止删除，但要让用户事先知道
    if t.harness == "antigravity" {
        p.warnings.push("agy_title_kept".into());
    }
    if proc.is_some_and(|name| ctx.running.contains(name)) {
        p.warnings.push("harness_running".into());
    }
    if t.harness == "codex" && codex_bin().is_none() {
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
        _ => None,
    }?;
    root.is_dir().then_some(root)
}

/// 会话 id 只允许字母数字、`-`、`_`；拒绝任何可能构成路径的字符
fn valid_id(id: &str) -> bool {
    (8..=80).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn cc_files(root: &Path, id: &str) -> Vec<PathBuf> {
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

fn find_session_dir(root: &Path, id: &str) -> Option<PathBuf> {
    fs::read_dir(root.join("sessions"))
        .ok()?
        .flatten()
        .map(|ws| ws.path().join(id))
        .find(|p| p.is_dir())
}

type Targets = (Vec<PathBuf>, Vec<String>, Vec<String>);

fn kimi_targets(root: &Path, id: &str) -> Targets {
    let Some(dir) = find_session_dir(root, id) else { return (vec![], vec![], vec![]) };
    let mut index = vec![];
    if file_mentions(&root.join("session_index.jsonl"), id) {
        index.push("session_index.jsonl".into());
    }
    if let Some(ws) = dir.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) {
        if file_mentions(&root.join("file-history").join(ws), id) {
            index.push(format!("file-history/{ws}"));
        }
    }
    (vec![dir], index, vec![])
}

fn dsh_targets(root: &Path, id: &str) -> Targets {
    let Some(dir) = find_session_dir(root, id) else { return (vec![], vec![], vec![]) };
    let mut files = vec![dir];
    let cache = root.join("storages").join("session_projcache").join("sessions").join(format!("{id}.json"));
    if cache.is_file() {
        files.push(cache);
    }
    let mut index = vec![];
    if file_mentions(&root.join("storages").join("workspace.json"), id) {
        index.push("storages/workspace.json".into());
    }
    (files, index, vec![])
}

fn codex_targets(root: &Path, id: &str, ctx: &Ctx) -> Targets {
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

/* ── Antigravity ── */

/// 对话及其子对话自己的文件。子对话关系只读 agy 的摘要库
fn agy_targets(root: &Path, id: &str) -> Targets {
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
fn agy_open(root: &Path, id: &str) -> bool {
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

/* ── OpenCode ── */

/// 只读查库：会话在不在、子 agent 有哪些、多大、最近什么时候写过
fn plan_opencode(mut p: Plan, ctx: &Ctx) -> Plan {
    let Some(db) = opencode::opencode_home().map(|h| h.join(opencode::DB)).filter(|d| d.is_file()) else {
        p.blocked = Some("not_found".into());
        return p;
    };
    let Some(con) = opencode::open(&db) else {
        p.blocked = Some("not_found".into());
        return p;
    };
    let Ok(sessions) = opencode_tree(&con, &p.id) else {
        p.blocked = Some("not_found".into());
        return p;
    };
    if sessions.is_empty() {
        p.blocked = Some("not_found".into());
        return p;
    }
    p.directory = sessions[0].2.clone();
    p.bytes = sessions.iter().map(|(id, updated, _)| opencode::cached_size(&con, id, *updated)).sum();
    let last_write = sessions.iter().map(|(_, updated, _)| *updated).max().unwrap_or(0);
    p.cli_sessions = sessions.into_iter().map(|(id, _, _)| id).collect();

    let now = system_time_ms(SystemTime::now());
    if now.saturating_sub(last_write) < ACTIVE_WINDOW_MS {
        p.blocked = Some("active".into());
    } else if opencode_bin().is_none() {
        // 没有 CLI 就没有安全的删法——不退回去自己写库
        p.blocked = Some("cli_missing".into());
    }
    if ctx.running.contains("opencode") {
        p.warnings.push("harness_running".into());
    }
    p
}

/// 根会话及其所有后代：(id, time_updated, directory)，父在前子在后
fn opencode_tree(con: &rusqlite::Connection, root: &str) -> Result<Vec<(String, u64, String)>, String> {
    let mut stmt = con
        .prepare("SELECT id, parent_id, COALESCE(time_updated, time_created, 0), COALESCE(directory,'') FROM session")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, Option<String>, u64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)?.max(0) as u64, r.get(3)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    let Some(first) = rows.iter().find(|r| r.0 == root) else { return Ok(vec![]) };
    let mut out = vec![(first.0.clone(), first.2, first.3.clone())];
    // 逐层展开；`out` 既是结果也是队列，出现过的 id 不再加入，防 parent_id 成环
    let mut i = 0;
    while i < out.len() {
        let parent = out[i].0.clone();
        for r in rows.iter().filter(|r| r.1.as_deref() == Some(parent.as_str())) {
            if !out.iter().any(|o| o.0 == r.0) {
                out.push((r.0.clone(), r.2, r.3.clone()));
            }
        }
        i += 1;
    }
    Ok(out)
}

/// 找 opencode 原生可执行文件：`ORRERY_OPENCODE_BIN` → PATH 里的 opencode →
/// npm 全局包里的二进制（Windows 上 PATH 里只有 `opencode.cmd` 壳，与 codex 同理）
fn opencode_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ORRERY_OPENCODE_BIN").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let exe_name = if cfg!(windows) { "opencode.exe" } else { "opencode" };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let exe = dir.join(exe_name);
        if exe.is_file() {
            return Some(exe);
        }
        if cfg!(windows) && dir.join("opencode.cmd").is_file() {
            let vendor = dir.join("node_modules/opencode-ai/bin/opencode.exe");
            if vendor.is_file() {
                return Some(vendor);
            }
        }
    }
    None
}

/// 调 opencode CLI。`XDG_DATA_HOME` 指向我们规划时读的那个库的上级目录，保证两边是同一个库；
/// `--pure` 不加载第三方插件
fn opencode_cmd(bin: &Path, home: &Path, args: &[&str]) -> std::process::Command {
    let mut cmd = std::process::Command::new(bin);
    cmd.args(args).arg("--pure").stdin(std::process::Stdio::null());
    if let Some(data) = home.parent() {
        cmd.env("XDG_DATA_HOME", strip_verbatim(data)).current_dir(strip_verbatim(home));
    }
    no_window(&mut cmd);
    cmd
}

fn execute_opencode(p: &Plan, mode: Mode, stamp: &str, o: &mut Outcome) {
    let (Some(bin), Some(home)) = (opencode_bin(), opencode::opencode_home()) else {
        o.error = Some("blocked:cli_missing".into());
        return;
    };

    // 回收站模式：每条都导出成功才删，任何一条导不出来就整条放弃
    if mode == Mode::Trash {
        // 每条会话一个目录，同一批删多条时各自的 RESTORE.txt 互不覆盖
        let Some(dir) = data_dir().map(|d| d.join("exports").join("opencode").join(stamp).join(&p.id)) else {
            o.error = Some("export:cannot resolve ~/.orrery".into());
            return;
        };
        if let Err(e) = fs::create_dir_all(&dir) {
            o.error = Some(format!("export:{e}"));
            return;
        }
        for id in &p.cli_sessions {
            let out = opencode_cmd(&bin, &home, &["export", id]).stderr(std::process::Stdio::null()).output();
            let json = match out {
                Ok(out) if out.status.success() => out.stdout,
                Ok(out) => {
                    o.error = Some(format!("export:{id}: exit {}", out.status));
                    return;
                }
                Err(e) => {
                    o.error = Some(format!("export:{id}: {e}"));
                    return;
                }
            };
            // 导出内容必须是这条会话本身，否则宁可不删
            let exported_id = serde_json::from_slice::<serde_json::Value>(&json)
                .ok()
                .and_then(|v| v.pointer("/info/id").and_then(|s| s.as_str()).map(String::from));
            if exported_id.as_deref() != Some(id.as_str()) {
                o.error = Some(format!("export:{id}: unexpected output"));
                return;
            }
            if let Err(e) = fs::write(dir.join(format!("{id}.json")), &json) {
                o.error = Some(format!("export:{e}"));
                return;
            }
        }
        let _ = fs::write(dir.join("RESTORE.txt"), restore_notes(p, &dir));
        o.export_dir = Some(dir.to_string_lossy().to_string());
    }

    // 删根会话，CLI 会连带删子 agent；之后核对，还在的逐条再删（防以后版本不再级联）
    let _ = opencode_cmd(&bin, &home, &["session", "delete", &p.id])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let left = opencode_remaining(&home, &p.cli_sessions);
    for id in &left {
        let _ = opencode_cmd(&bin, &home, &["session", "delete", id])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let left = opencode_remaining(&home, &p.cli_sessions);
    if !left.is_empty() {
        o.error = Some(format!("cli:{} of {} sessions still in OpenCode", left.len(), p.cli_sessions.len()));
        return;
    }
    o.ok = true;
}

/// 这些 id 里还留在 OpenCode 库里的
fn opencode_remaining(home: &Path, ids: &[String]) -> Vec<String> {
    let Some(con) = opencode::open(&home.join(opencode::DB)) else { return ids.to_vec() };
    ids.iter()
        .filter(|id| {
            con.query_row("SELECT 1 FROM session WHERE id = ?1", [id.as_str()], |_| Ok(()))
                .is_ok()
        })
        .cloned()
        .collect()
}

/// 写给人看的恢复步骤：import 要在原工作目录下跑（它按当前目录归项目），父会话先于子 agent
fn restore_notes(p: &Plan, dir: &Path) -> String {
    let mut s = String::from(
        "Restore these OpenCode sessions by running, in order:\n\
         按顺序运行下面几行即可恢复（import 按当前目录归项目，所以先回到原目录）：\n\n",
    );
    s.push_str(&format!("cd \"{}\"\n", p.directory));
    for id in &p.cli_sessions {
        s.push_str(&format!("opencode import \"{}\"\n", dir.join(format!("{id}.json")).display()));
    }
    s
}

/* ── 执行 ── */

fn execute(p: &Plan, mode: Mode, stamp: &str) -> Outcome {
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
        execute_opencode(p, mode, stamp, &mut o);
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

/* ── 索引改写 ── */

/// 返回是否实际改动。写前重读；只移除引用这些 id 的条目
fn rewrite_index(path: &Path, ids: &[&str], backup_root: Option<&Path>, rel: &str) -> Result<bool, String> {
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

/* ── Codex CLI ── */

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
fn codex_bin() -> Option<PathBuf> {
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

/* ── 运行态检测 ── */

/// 当前运行的进程名（小写，去掉 `.exe` 后缀和目录部分）
///
/// Windows 走 `tasklist`，macOS / Linux 走 `ps`。取不到就返回空集合——
/// 结果只用来提示"该工具正在运行"，宁可不提示，也不要因为拿不到进程表就拦住删除
///
/// 平台差异：macOS 的 `comm` 是完整路径（取最后一段）；Linux 的 `comm` 来自内核的
/// `TASK_COMM_LEN`，**截断到 15 个字符**。目前要匹配的 `kimi` / `codex` 都很短，
/// 以后要匹配更长的进程名得改用 `-o args=` 再自己取第一段
fn running_processes() -> HashSet<String> {
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("tasklist");
        c.args(["/FO", "CSV", "/NH"]);
        c
    } else {
        let mut c = std::process::Command::new("ps");
        // -A 全部进程，comm= 只要命令名、不要表头（macOS 与 Linux 都支持）
        c.args(["-A", "-o", "comm="]);
        c
    };
    no_window(&mut cmd);
    let Ok(out) = cmd.output() else { return HashSet::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split(',').next())
        .map(|n| n.trim().trim_matches('"'))
        // macOS 的 comm 是完整路径，取最后一段
        .map(|n| n.rsplit(['/', std::path::MAIN_SEPARATOR]).next().unwrap_or(n))
        .map(|n| n.trim_end_matches(".exe").to_ascii_lowercase())
        .filter(|n| !n.is_empty())
        .collect()
}

/// Claude Code 在 `~/.claude/sessions/<pid>.json` 登记运行中的会话；进程还活着就视为运行中
fn cc_is_running(root: &Path, id: &str) -> bool {
    let Ok(entries) = fs::read_dir(root.join("sessions")) else { return false };
    entries.flatten().any(|e| {
        let Ok(raw) = fs::read_to_string(e.path()) else { return false };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else { return false };
        v.get("sessionId").and_then(|s| s.as_str()) == Some(id)
            && v.get("pid").and_then(|p| p.as_u64()).is_some_and(pid_alive)
    })
}

fn pid_alive(pid: u64) -> bool {
    if cfg!(windows) {
        let mut cmd = std::process::Command::new("tasklist");
        cmd.args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"]);
        no_window(&mut cmd);
        return cmd
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!(",\"{pid}\",")))
            .unwrap_or(false);
    }
    // Unix：进程不存在时 ps 退出码非 0
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .map(|o| o.status.success() && !o.stdout.iter().all(u8::is_ascii_whitespace))
        .unwrap_or(false)
}

#[cfg(windows)]
fn no_window(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}
#[cfg(not(windows))]
fn no_window(_cmd: &mut std::process::Command) {}

/* ── 小工具 ── */

fn canonical(p: &Path) -> Option<PathBuf> {
    fs::canonicalize(p).ok().map(|c| strip_verbatim(&c))
}

fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
}

fn path_size(p: &Path) -> u64 {
    if p.is_dir() { dir_size(p) } else { fs::metadata(p).map(|m| m.len()).unwrap_or(0) }
}

/// 文件或目录内最新的修改时间（目录只看两层，足够覆盖会话写入）
fn latest_mtime(p: &Path) -> u64 {
    fn walk(p: &Path, depth: u32) -> u64 {
        let own = fs::metadata(p).and_then(|m| m.modified()).map(system_time_ms).unwrap_or(0);
        if depth == 0 || !p.is_dir() {
            return own;
        }
        let Ok(entries) = fs::read_dir(p) else { return own };
        entries.flatten().map(|e| walk(&e.path(), depth - 1)).max().unwrap_or(0).max(own)
    }
    walk(p, 3)
}

fn file_mentions(path: &Path, id: &str) -> bool {
    fs::read(path).is_ok_and(|b| super::contains(&b, id.as_bytes()))
}

fn backup_stamp() -> String {
    let ms = system_time_ms(SystemTime::now());
    format!("{ms}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zcode_plan_is_read_only_without_resolving_files() {
        let p = plan(&Target { harness: "zcode".into(), id: "ses_sandbox_only".into() }, &Ctx::with_running(HashSet::new()));
        assert_eq!(p.blocked.as_deref(), Some("read_only"));
        assert!(p.files.is_empty());
        assert!(p.index_files.is_empty());
        assert_eq!(p.bytes, 0);
    }

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

    /// 子 agent 递归展开、父在前；成环的 parent_id 不会死循环
    #[test]
    fn opencode_tree_lists_root_first_then_descendants() {
        let con = rusqlite::Connection::open_in_memory().unwrap();
        con.execute_batch(
            "CREATE TABLE session(id TEXT, parent_id TEXT, time_created INT, time_updated INT, directory TEXT);
             INSERT INTO session VALUES ('root', NULL, 1, 5, 'D:/p');
             INSERT INTO session VALUES ('kid', 'root', 1, 9, 'D:/p');
             INSERT INTO session VALUES ('grandkid', 'kid', 1, 7, 'D:/p');
             INSERT INTO session VALUES ('other', NULL, 1, 3, 'D:/q');
             INSERT INTO session VALUES ('a', 'b', 1, 1, '');
             INSERT INTO session VALUES ('b', 'a', 1, 1, '');",
        )
        .unwrap();
        let ids: Vec<String> = opencode_tree(&con, "root").unwrap().into_iter().map(|r| r.0).collect();
        assert_eq!(ids, ["root", "kid", "grandkid"]);
        assert_eq!(opencode_tree(&con, "a").unwrap().len(), 2, "成环也要停下");
        assert!(opencode_tree(&con, "missing").unwrap().is_empty());
    }

    #[test]
    fn restore_notes_cd_first_and_use_absolute_paths() {
        let p = Plan {
            directory: "D:/code/recipe-box".into(),
            cli_sessions: vec!["ses_root".into(), "ses_kid".into()],
            ..Default::default()
        };
        let dir = Path::new("C:/x/exports");
        let notes = restore_notes(&p, dir);
        let cd = notes.find("cd \"D:/code/recipe-box\"").unwrap();
        let root = notes.find("ses_root.json").unwrap();
        let kid = notes.find("ses_kid.json").unwrap();
        assert!(cd < root && root < kid, "先 cd，再父会话，再子 agent");
        assert!(notes.contains(&dir.join("ses_root.json").display().to_string()));
    }

    /// 在 agy 数据目录的**副本**上端到端删一条对话。默认不跑：
    /// ORRERY_HOME 指向沙盒（放 `.gemini/antigravity-cli` 副本），ORRERY_AGY_ID 是对话 id，
    /// `cargo test --lib agy_sandbox -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn agy_sandbox_trash() {
        let home = std::env::var_os("ORRERY_HOME").expect("ORRERY_HOME 必须指向沙盒");
        assert_ne!(Some(PathBuf::from(&home)), dirs::home_dir(), "不能对真实主目录跑");
        let id = std::env::var("ORRERY_AGY_ID").expect("ORRERY_AGY_ID");
        let target = Target { harness: "antigravity".into(), id };
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
        assert_ne!(Some(PathBuf::from(&home)), dirs::home_dir(), "不能对真实主目录跑");
        let id = std::env::var("ORRERY_OC_ID").expect("ORRERY_OC_ID");
        let target = Target { harness: "opencode".into(), id };
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
        for bad in ["../../etc", "a/b/c/d/e/f", r"..\..\x", "short", "abc def ghij", "C:secretsxx"] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

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

    /// 进程表必须真取到东西。这两个函数是"删除前检查工具是否在运行"的地基，
    /// 换平台后如果命令调错，只会返回空集合/false，不会报错——保护就静默失效了。
    /// CI 在三个平台都跑这两个测试，就是为了让这种失效变成红灯
    #[test]
    fn running_processes_is_not_empty_and_normalized() {
        let procs = running_processes();
        assert!(!procs.is_empty(), "取不到进程表：本平台的进程枚举命令调用有问题");
        for name in &procs {
            assert!(!name.contains(['/', std::path::MAIN_SEPARATOR]), "进程名里不该留路径：{name}");
            assert!(!name.ends_with(".exe"), "进程名里不该留 .exe 后缀：{name}");
            assert_eq!(name, &name.to_ascii_lowercase(), "进程名要统一小写：{name}");
        }
    }

    #[test]
    fn pid_alive_knows_this_process() {
        assert!(pid_alive(std::process::id() as u64), "当前进程必须被判定为存活");
        // 超出各平台 pid 上限，必然不存在
        assert!(!pid_alive(4_294_900_000), "不存在的 pid 不能判成存活");
    }
}

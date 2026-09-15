//! 删除会话：移到回收站或永久删除，并清理各 harness 的文本索引
//!
//! 安全约束（改动前先读完）：
//! 1. 路径只由后端按 (harness, id) 解析，从不采信前端传来的路径；每个目标都必须位于该 harness
//!    数据根目录之内（按真实路径比较，防 `..` 与链接逃逸）
//! 2. 最近 [`ACTIVE_WINDOW_MS`] 内有写入、或 Claude Code 登记为运行中的会话拒绝删除
//! 3. 先删文件，全部成功后才改索引；改索引前把原文件备份到 `~/.openplane/backups/<时间>/`，
//!    写入走"同目录临时文件 + 原子替换"，且写前重读，只移除属于这些会话的条目
//! 4. Codex 的 sqlite（`state_5` 的 threads、`thread_history_1` 里的会话内容副本）只通过官方
//!    `codex delete --force <uuid>` 清理，Openplane 不直接写 Codex 数据库。沙盒实测：
//!    - 删除父会话会一并删 rollout、threads 行、thread_history 条目、session_index 行
//!    - 不会删 guardian 子 agent → 逐个子 agent 再调用
//!    - rollout 已先移到回收站时仍能清掉数据库记录 → 回收站模式可行
//!    - id 不在数据库里时报错 → 退回自行删除文件与 session_index 行
//!
//! 各 harness 牵涉的数据（均为本机实测）：
//! | harness | 文件 | 索引 |
//! |---|---|---|
//! | cc    | `projects/<p>/<id>.jsonl`、`projects/<p>/<id>/`、`file-history/<id>/`、`session-env/<id>/`、`tasks/<id>/` | 无（`history.jsonl` 是输入历史，不动） |
//! | kimi  | `sessions/<ws>/<id>/` | `session_index.jsonl` 行、`file-history/<ws>` 的 `sessions[]` |
//! | dsh   | `sessions/<ws>/<id>/`、`storages/session_projcache/sessions/<id>.json` | `storages/workspace.json` 的 `sessionIds` / `archivedSessionIds` |
//! | codex | 该 id 的所有 rollout + 以它为父的子 agent rollout | sqlite（经 `codex delete`）、`session_index.jsonl` 行（兜底） |

use super::{claude_home, codex, codex_home, dir_size, dsh_home, forget_memo, home_dir, kimi_home, system_time_ms};
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
    /// 非空 = 不允许删除，值为原因代码：not_found / active / running / invalid
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
}

/* ── 入口 ── */

pub fn plan_all(targets: &[Target]) -> Vec<Plan> {
    let ctx = Ctx::new();
    targets.iter().map(|t| plan(t, &ctx)).collect()
}

/// 一批删除共享的只读上下文：进程列表只查一次，Codex rollout 首行只扫一次
struct Ctx {
    running: HashSet<String>,
    codex_heads: std::cell::OnceCell<Vec<(PathBuf, String, bool, Option<String>)>>,
}

impl Ctx {
    fn new() -> Self {
        Self::with_running(running_processes())
    }

    fn with_running(running: HashSet<String>) -> Self {
        Self { running, codex_heads: std::cell::OnceCell::new() }
    }

    fn codex_heads(&self, root: &Path) -> &[(PathBuf, String, bool, Option<String>)] {
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
    let Some(root) = harness_root(&t.harness) else {
        p.blocked = Some("not_found".into());
        return p;
    };
    let (files, index_files, codex_threads) = match t.harness.as_str() {
        "cc" => (cc_files(&root, &t.id), vec![], vec![]),
        "kimi" => kimi_targets(&root, &t.id),
        "dsh" => dsh_targets(&root, &t.id),
        "codex" => codex_targets(&root, &t.id, ctx),
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

    let last_write = files.iter().map(|f| latest_mtime(f)).max().unwrap_or(0);
    let now = system_time_ms(SystemTime::now());
    if now.saturating_sub(last_write) < ACTIVE_WINDOW_MS {
        p.blocked = Some("active".into());
    }
    if t.harness == "cc" && cc_is_running(&root, &t.id) {
        p.blocked = Some("running".into());
    }

    let proc = match t.harness.as_str() {
        "kimi" => Some("kimi.exe"),
        "codex" => Some("codex.exe"),
        _ => None,
    };
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
    let backup_root = home_dir().map(|h| h.join(".openplane").join("backups").join(stamp).join(&p.harness));
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
        ".{}.openplane-tmp",
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

/// 找 codex 原生可执行文件：`OPENPLANE_CODEX_BIN` → PATH 里的 codex.exe → npm 全局安装包里的 vendor 二进制
fn codex_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("OPENPLANE_CODEX_BIN").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let exe = dir.join("codex.exe");
        if exe.is_file() {
            return Some(exe);
        }
        if dir.join("codex.cmd").is_file() {
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

/// 当前运行的进程名（小写）
fn running_processes() -> HashSet<String> {
    let mut cmd = std::process::Command::new("tasklist");
    cmd.args(["/FO", "CSV", "/NH"]);
    no_window(&mut cmd);
    let Ok(out) = cmd.output() else { return HashSet::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split(',').next())
        .map(|n| n.trim_matches('"').to_ascii_lowercase())
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
    let mut cmd = std::process::Command::new("tasklist");
    cmd.args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"]);
    no_window(&mut cmd);
    cmd.output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!(",\"{pid}\",")))
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
}

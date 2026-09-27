//! OpenCode 的会话全在它自己的 SQLite 里，没有任何属于会话的文件可删，
//! 所以规划是只读查库，执行只能走官方 CLI（详见 `cleanup` 模块文档第 5 条）。

use super::super::{data_dir, system_time_ms};
use super::{no_window, strip_verbatim, Ctx, Mode, Plan, Outcome, ACTIVE_WINDOW_MS};
use crate::adapters::opencode;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 只读查库：会话在不在、子 agent 有哪些、多大、最近什么时候写过
pub(super) fn plan_opencode(mut p: Plan, ctx: &Ctx) -> Plan {
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
pub(super) fn opencode_tree(con: &rusqlite::Connection, root: &str) -> Result<Vec<(String, u64, String)>, String> {
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

pub(super) fn execute_opencode(p: &Plan, mode: Mode, stamp: &str, o: &mut Outcome) {
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
pub(super) fn restore_notes(p: &Plan, dir: &Path) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}

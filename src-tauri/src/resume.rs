//! 在终端里恢复一个会话
//!
//! 每个 harness 都有自己的恢复命令（本机实测 2026-09-17）：
//!
//! | harness  | 命令                                  | id 形态 |
//! |----------|---------------------------------------|---------|
//! | cc       | `claude --resume <id>`                | jsonl 文件名，UUID |
//! | codex    | `codex resume <id>`                   | UUID |
//! | kimi     | `kimi --session <id>`                 | `session_<uuid>`（与 kimi 自己 `session_index.jsonl` 里的 `sessionId` 一致） |
//! | dsh      | `dsh --profile <profile> --resume <id>` | `session-<uuid>`（与 dsh 自己投影缓存的文件名一致） |
//! | opencode | `opencode --session <id>`             | `ses_xxx` |
//!
//! DSH 的坑：`--resume` 不是启动器的参数，而是转发给被引导的 profile 应用。实测本机
//! 只装了 `web` profile，而 web 应用没有 `--resume`（它在浏览器界面里选会话）。所以
//! 先看 `~/.dsh/profiles` 下装了什么：有非 web 的（如 `tui`）就在终端里直接恢复那条会话；
//! 只有 web 就退一步启动 `dsh --profile web`，会话在它自己的网页界面里选——
//! 这时返回 `web_fallback:` 前缀，前端要如实说明"打开的是 DSH 网页界面，不是直接恢复"。
//!
//! 在 `~/.orrery/harnesses.json` 里登记的工具不给恢复命令——我们不知道它有没有 CLI，
//! 一律报 `unsupported_harness`。
//!
//! 终端：优先 Windows Terminal（`wt -d <cwd> <程序> <参数…>`），没有就退回
//! `cmd /c start "" cmd /k`。macOS 用 `open -a Terminal`，Linux 试几个常见终端。
//!
//! 安全：id 必须先过 `valid_id`（只允许字母数字和 `-_`），工作目录由后端从扫描结果
//! 解析，不接受前端任意路径；参数一律按数组传给进程，不拼 shell 字符串。
//! 唯一需要拼字符串的是 cmd 回退分支，那里已经保证 id 不含空白和引号。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 会话 id 的白名单校验：与 `adapters::cleanup` 同一套口径
fn valid_id(id: &str) -> bool {
    (8..=80).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// DSH 装了哪个能在终端里跑的 profile。`web` 是浏览器 UI，没有 `--resume`，不算
fn dsh_terminal_profile() -> Option<String> {
    let dir = match std::env::var_os("DSH_HOME") {
        Some(h) => PathBuf::from(h).join("profiles"),
        None => dirs::home_dir()?.join(".dsh").join("profiles"),
    };
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n != "web" && n != "node_modules" && !n.starts_with('.'))
        .collect();
    // tui 是官方的终端 profile，优先它；否则拿字典序第一个，行为可预测
    names.sort();
    names.iter().find(|n| *n == "tui").cloned().or_else(|| names.first().cloned())
}

/// 只装了 web profile 时的退路：至少把 DSH 的网页界面打开
fn dsh_has_web_profile() -> bool {
    let dir = match std::env::var_os("DSH_HOME") {
        Some(h) => PathBuf::from(h).join("profiles"),
        None => match dirs::home_dir() {
            Some(h) => h.join(".dsh").join("profiles"),
            None => return false,
        },
    };
    dir.join("web").is_dir()
}

/// harness → (可执行文件, 参数)
fn command_for(harness: &str, id: &str) -> Option<(&'static str, Vec<String>)> {
    let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    Some(match harness {
        "cc" => ("claude", args(&["--resume", id])),
        "codex" => ("codex", args(&["resume", id])),
        "kimi" => ("kimi", args(&["--session", id])),
        "dsh" => match dsh_terminal_profile() {
            Some(profile) => ("dsh", vec!["--profile".into(), profile, "--resume".into(), id.to_string()]),
            // 只有 web profile：启动网页界面，让用户在里面挑
            None if dsh_has_web_profile() => ("dsh", args(&["--profile", "web"])),
            None => return None,
        },
        "opencode" => ("opencode", args(&["--session", id])),
        _ => return None,
    })
}

/// 在 PATH 里找可执行文件
///
/// Windows 上扩展名的**顺序很要紧**：npm 会同时装 `codex`（给 bash 用的 shim，
/// Windows 执行不了）和 `codex.cmd`。先试无扩展名的话，CreateProcess 会"成功"地
/// 起一个立刻失败的进程——终端一闪而过，而代码这边还以为成功了（实测踩过）。
fn resolve_program(name: &str) -> Option<String> {
    let exts: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat", ""] } else { &[""] };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    None
}

/// 起一个终端，`cd` 到 cwd 后执行 program + args
fn spawn_terminal(cwd: &Path, program: &str, args: &[String]) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        // Windows Terminal：-d 指定工作目录，后面直接跟要执行的命令
        if let Some(wt) = resolve_program("wt") {
            let mut cmd = Command::new(wt);
            cmd.arg("-d").arg(cwd).arg(program).args(args);
            if cmd.spawn().is_ok() {
                return Ok(());
            }
        }
        // 回退：新开一个 cmd 窗口，/k 保留窗口好看报错
        // id 已过白名单校验，program 是 PATH 里解析出的真实路径
        let line = format!("cd /d \"{}\" && \"{}\" {}", cwd.display(), program, args.join(" "));
        Command::new("cmd")
            .args(["/c", "start", "", "cmd", "/k", &line])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("cannot start a terminal: {e}"))
    }
    #[cfg(target_os = "macos")]
    {
        // Terminal.app 只接受一个可执行文件，包一层脚本
        let script = format!("cd {:?} && {:?} {}\n", cwd, program, args.join(" "));
        let sh = std::env::temp_dir().join("orrery-resume.command");
        std::fs::write(&sh, script).map_err(|e| e.to_string())?;
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755));
        Command::new("open")
            .arg("-a")
            .arg("Terminal")
            .arg(&sh)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("cannot start Terminal: {e}"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let line = format!("cd {:?} && {:?} {}; exec $SHELL", cwd, program, args.join(" "));
        for term in ["x-terminal-emulator", "gnome-terminal", "konsole", "xfce4-terminal", "alacritty", "xterm"] {
            if resolve_program(term).is_none() {
                continue;
            }
            let ok = match term {
                "gnome-terminal" => Command::new(term).args(["--", "sh", "-c", &line]).spawn().is_ok(),
                _ => Command::new(term).args(["-e", "sh", "-c", &line]).spawn().is_ok(),
            };
            if ok {
                return Ok(());
            }
        }
        Err("no terminal emulator found".into())
    }
}

/// 在终端里恢复会话。`project` 是会话记录的工作目录
pub fn resume(harness: &str, id: &str, project: &str) -> Result<String, String> {
    if !valid_id(id) {
        return Err("invalid_id".into());
    }
    let Some((name, args)) = command_for(harness, id) else {
        // DSH 单独给个说法：命令本身存在，是本机没装能在终端跑的 profile
        if harness == "dsh" {
            return Err("dsh_no_terminal_profile".into());
        }
        return Err("unsupported_harness".into());
    };
    let Some(program) = resolve_program(name) else {
        // 前端按这个前缀提示"没找到 xxx 命令"
        return Err(format!("cli_missing:{name}"));
    };
    let cwd = PathBuf::from(project);
    if !cwd.is_dir() {
        return Err("cwd_missing".into());
    }
    spawn_terminal(&cwd, &program, &args)?;
    let line = format!("{name} {}", args.join(" "));
    // DSH 走 web 退路时，打开的是网页界面而不是这条会话，前端要说清楚
    if harness == "dsh" && !args.iter().any(|a| a == "--resume") {
        return Ok(format!("web_fallback:{line}"));
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_harness_has_a_command() {
        // dsh 取决于本机装了哪个 profile，单独测
        for h in ["cc", "codex", "kimi", "opencode"] {
            let (program, args) = command_for(h, "session_0ed9be17-001b-4642-8b8a").expect(h);
            assert!(!program.is_empty());
            assert!(args.iter().any(|a| a.contains("0ed9be17")), "{h} 的命令里必须带会话 id");
        }
        assert!(command_for("whatever-tool", "whatever-id").is_none());
    }

    #[test]
    fn ids_with_shell_metacharacters_are_rejected() {
        for bad in ["a", "id with space", "id&&calc", "id\"quote", "id;rm -rf", "../../etc/passwd"] {
            assert!(!valid_id(bad), "{bad} 不该通过校验");
        }
        assert!(valid_id("session_0ed9be17-001b-4642-8b8a-2f71a7df757c"));
        assert!(valid_id("ses_f6b9de073ffeUg7Y"));
    }

    #[test]
    fn unknown_harness_and_bad_id_fail_before_touching_the_system() {
        assert_eq!(resume("cc", "x", "."), Err("invalid_id".into()));
        assert_eq!(resume("nope", "session_0ed9be17-001b", "."), Err("unsupported_harness".into()));
        // 自己登记的工具没有恢复命令
        assert_eq!(resume("some-registered-tool", "ses_ffe5f7731d1234", "."), Err("unsupported_harness".into()));
    }
}

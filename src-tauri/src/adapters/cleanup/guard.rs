//! 删除前的"这个工具是不是正在用"判定。
//!
//! 取不到进程表时一律降级为"没在运行"：这两处的检查只用于提示和拦截，
//! 不该因为命令调不通就把用户拦在删除之外。但反过来说，降级也会让保护静默失效——
//! 下面两个测试在三平台 CI 上都跑，就是为了让这种失效变成红灯。

use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// 当前运行的进程名（小写，去掉 `.exe` 后缀和目录部分）
///
/// Windows 走 `tasklist`，macOS / Linux 走 `ps`。取不到就返回空集合——
/// 结果只用来提示"该工具正在运行"，宁可不提示，也不要因为拿不到进程表就拦住删除
///
/// 平台差异：macOS 的 `comm` 是完整路径（取最后一段）；Linux 的 `comm` 来自内核的
/// `TASK_COMM_LEN`，**截断到 15 个字符**。目前要匹配的 `kimi` / `codex` 都很短，
/// 以后要匹配更长的进程名得改用 `-o args=` 再自己取第一段
pub(super) fn running_processes() -> HashSet<String> {
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
pub(super) fn cc_is_running(root: &Path, id: &str) -> bool {
    let Ok(entries) = fs::read_dir(root.join("sessions")) else { return false };
    entries.flatten().any(|e| {
        let Ok(raw) = fs::read_to_string(e.path()) else { return false };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else { return false };
        v.get("sessionId").and_then(|s| s.as_str()) == Some(id)
            && v.get("pid").and_then(|p| p.as_u64()).is_some_and(pid_alive)
    })
}

/// 进程是否存活。`workbuddy` adapter 要用它判断会话是否正在运行
pub(crate) fn pid_alive(pid: u64) -> bool {
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

/// 子进程不要弹控制台窗口（Windows）。`transfer.rs` 也要用
#[cfg(windows)]
pub(crate) fn no_window(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}
#[cfg(not(windows))]
pub(crate) fn no_window(_cmd: &mut std::process::Command) {}

#[cfg(test)]
mod tests {
    use super::*;

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

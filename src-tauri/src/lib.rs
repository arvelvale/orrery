mod adapters;
// 集成测试要直接调代理的启停与状态
pub mod proxy;

use adapters::{cleanup, HarnessStorage, SessionSummary};
use proxy::ProxyStatus;
use std::path::Path;

// 扫盘命令标 async：Tauri 2 的同步命令跑在主线程，扫大目录会卡住窗口
#[tauri::command(async)]
fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    adapters::list_all_sessions()
}

#[tauri::command(async)]
fn storage_stats() -> Vec<HarnessStorage> {
    adapters::storage_stats()
}

/// 删除前预览：每条会话会删哪些文件、多大、是否被保护
#[tauri::command(async)]
fn plan_delete(targets: Vec<cleanup::Target>) -> Vec<cleanup::Plan> {
    cleanup::plan_all(&targets)
}

/// 执行删除；后端会重新规划，不采信前端传来的路径
#[tauri::command(async)]
fn delete_sessions(targets: Vec<cleanup::Target>, mode: cleanup::Mode) -> Vec<cleanup::Outcome> {
    cleanup::delete_all(&targets, mode)
}

#[tauri::command]
fn get_proxy_status() -> ProxyStatus {
    proxy::status()
}

#[tauri::command]
fn ping_proxy() -> ProxyStatus {
    proxy::ping()
}

#[tauri::command]
fn save_route(harness: String, model: String) -> Result<(), String> {
    proxy::save_route(&harness, &model)
}

#[tauri::command(async)]
fn start_proxy() -> Result<ProxyStatus, String> {
    proxy::start()
}

#[tauri::command(async)]
fn stop_proxy() -> Result<ProxyStatus, String> {
    proxy::stop()
}

#[tauri::command(async)]
fn set_proxy_auto_start(enabled: bool) -> Result<(), String> {
    proxy::set_auto_start(enabled)
}

#[tauri::command]
fn open_path(path: String) -> Result<bool, String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(format!("path not found: {path}"));
    }
    // 传文件路径时打开它所在的目录（三个平台的文件管理器都能接受目录）
    let target = if p.is_file() {
        p.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| p.to_path_buf())
    } else {
        p.to_path_buf()
    };
    let opener = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Ok(std::process::Command::new(opener).arg(target).spawn().is_ok())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|_app| {
            if let Some(dir) = adapters::data_dir() {
                let _ = std::fs::create_dir_all(dir);
            }
            proxy::init();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            storage_stats,
            plan_delete,
            delete_sessions,
            get_proxy_status,
            start_proxy,
            stop_proxy,
            set_proxy_auto_start,
            ping_proxy,
            save_route,
            open_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running Orrery");
}

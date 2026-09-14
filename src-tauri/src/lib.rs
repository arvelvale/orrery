mod adapters;
mod proxy;

use adapters::{HarnessStorage, SessionSummary};
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

#[tauri::command]
fn open_path(path: String) -> Result<bool, String> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(format!("path not found: {path}"));
    }
    #[cfg(target_os = "windows")]
    {
        let target = if p.is_file() {
            p.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| p.to_path_buf())
        } else {
            p.to_path_buf()
        };
        let ok = std::process::Command::new("explorer")
            .arg(target)
            .spawn()
            .is_ok();
        return Ok(ok);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let ok = std::process::Command::new("xdg-open")
            .arg(p)
            .spawn()
            .is_ok();
        Ok(ok)
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|_app| {
            if let Some(home) = dirs::home_dir() {
                let _ = std::fs::create_dir_all(home.join(".openplane"));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            storage_stats,
            get_proxy_status,
            ping_proxy,
            save_route,
            open_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running Openplane");
}

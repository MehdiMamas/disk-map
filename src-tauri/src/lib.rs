pub mod apps;
pub mod hogs;
pub mod permissions;
pub mod scanner;
pub mod update;

use scanner::{Node, ViewNode, PRUNE_FRACTION};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::Emitter;

#[derive(Serialize)]
struct ScanResult {
    root: ViewNode,
    errors: u64,
    permission_denied: u64,
    denied_examples: Vec<String>,
    other_fs_skipped: u64,
    elapsed_ms: u64,
}

#[tauri::command]
fn home_dir() -> String {
    dirs::home_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into())
}

#[tauri::command]
async fn scan(app: tauri::AppHandle, path: String) -> Result<ScanResult, String> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(format!("{path} is not a folder"));
    }
    let started = std::time::Instant::now();
    let r2 = root.clone();
    let out = tauri::async_runtime::spawn_blocking(move || {
        scanner::scan_dir(&r2, &scanner::Options::default(), |p| {
            let _ = app.emit("scan-progress", p);
        })
    })
    .await
    .map_err(|e| e.to_string())?;
    let view = scanner::prune(&out.tree, &root, PRUNE_FRACTION, 1);
    let res = ScanResult {
        root: view,
        errors: out.errors,
        permission_denied: out.permission_denied,
        denied_examples: out.denied_examples,
        other_fs_skipped: out.other_fs_skipped,
        elapsed_ms: started.elapsed().as_millis() as u64,
    };
    SCAN_STORE.lock().unwrap().replace((root, out.tree));
    Ok(res)
}

/// The full scanned tree stays in memory so drill-down never rescans.
static SCAN_STORE: Mutex<Option<(PathBuf, Node)>> = Mutex::new(None);

#[tauri::command]
fn view(path: String) -> Result<ViewNode, String> {
    let guard = SCAN_STORE.lock().unwrap();
    let (root, tree) = guard.as_ref().ok_or("nothing scanned yet")?;
    let p = Path::new(&path);
    let node = scanner::find(tree, root, p).ok_or("folder not in the scan")?;
    Ok(scanner::prune(node, p, PRUNE_FRACTION, 1))
}

#[tauri::command]
fn reveal(path: String) -> Result<(), String> {
    use std::process::Command;
    #[cfg(target_os = "macos")]
    let r = Command::new("open").arg("-R").arg(&path).spawn();
    #[cfg(target_os = "windows")]
    let r = Command::new("explorer").arg(format!("/select,{}", path.replace('/', "\\"))).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = Command::new("xdg-open").arg(Path::new(&path).parent().unwrap_or(Path::new("/"))).spawn();
    r.map(|_| ()).map_err(|e| e.to_string())
}

/// Moves one path to the system Trash (never a permanent delete).
fn trash_path(path: &str) -> Result<(), String> {
    #[allow(unused_mut)]
    let mut ctx = trash::TrashContext::default();
    // NSFileManager avoids driving Finder via AppleScript (no Automation prompt).
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        ctx.set_delete_method(DeleteMethod::NsFileManager);
    }
    ctx.delete(path).map_err(|e| e.to_string())?;
    if let Some((root, tree)) = SCAN_STORE.lock().unwrap().as_mut() {
        scanner::remove(tree, root, Path::new(path));
    }
    Ok(())
}

/// Moves to the system Trash (never a permanent delete) and updates the in-memory tree.
#[tauri::command]
fn move_to_trash(path: String) -> Result<(), String> {
    trash_path(&path)
}

const NOT_MAC: &str = "not supported yet";

#[tauri::command]
async fn list_apps() -> Result<Vec<apps::AppEntry>, String> {
    if !cfg!(target_os = "macos") {
        return Err(NOT_MAC.into());
    }
    tauri::async_runtime::spawn_blocking(|| {
        let home = dirs::home_dir().ok_or("no home folder")?;
        Ok(apps::list_apps(&home))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn app_uninstall_plan(path: String) -> Result<apps::UninstallPlan, String> {
    if !cfg!(target_os = "macos") {
        return Err(NOT_MAC.into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let home = dirs::home_dir().ok_or("no home folder")?;
        apps::plan(Path::new(&path), &home).map(|(_, p)| p)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn uninstall_app(path: String, items: Vec<String>) -> Result<Vec<apps::ItemResult>, String> {
    if !cfg!(target_os = "macos") {
        return Err(NOT_MAC.into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let home = dirs::home_dir().ok_or("no home folder")?;
        apps::uninstall(Path::new(&path), &home, &items, trash_path)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// "granted" | "denied" | "unknown" (always "granted" off macOS).
#[tauri::command]
fn full_disk_access() -> String {
    permissions::probe_all().to_string()
}

#[tauri::command]
fn open_privacy_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    Ok(())
}

#[tauri::command]
fn relaunch(app: tauri::AppHandle) {
    app.restart();
}

#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> Result<update::UpdateInfo, String> {
    let current = app.package_info().version.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let rel = update::fetch_latest()?;
        Ok(update::build_info(&current, &rel, std::env::consts::OS))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    update::open(&url)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![home_dir, scan, view, reveal, move_to_trash, full_disk_access, open_privacy_settings, relaunch, check_update, open_url, list_apps, app_uninstall_plan, uninstall_app])
        .run(tauri::generate_context!())
        .expect("error while running Disk Map");
}


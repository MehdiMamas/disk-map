pub mod apps;
pub mod hogs;
pub mod permissions;
pub mod scanner;
pub mod update;

use scanner::{Node, ViewNode, PRUNE_FRACTION};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{Emitter, Manager};

#[derive(Serialize)]
struct ScanResult {
    root: ViewNode,
    errors: u64,
    permission_denied: u64,
    denied_examples: Vec<String>,
    other_fs_skipped: u64,
    elapsed_ms: u64,
    scanned_at: u64,
}

/// What is written to last-scan.json and read back on launch.
#[derive(Serialize, Deserialize, Clone, Default)]
struct Saved {
    root: PathBuf,
    tree: Node,
    scanned_at: u64,
    errors: u64,
    permission_denied: u64,
    other_fs_skipped: u64,
    #[serde(default)]
    denied_examples: Vec<String>,
}

/// The full scanned tree stays in memory so drill-down never rescans.
static SCAN_STORE: Mutex<Option<Saved>> = Mutex::new(None);
static WRITE_LOCK: Mutex<()> = Mutex::new(());

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn save_file(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("last-scan.json"))
}

/// Write the current scan to disk on a background thread (temp file, then rename).
fn persist(app: &tauri::AppHandle) {
    let Some(file) = save_file(app) else { return };
    let Some(saved) = SCAN_STORE.lock().unwrap().clone() else { return };
    std::thread::spawn(move || {
        let _g = WRITE_LOCK.lock().unwrap();
        let _ = write_saved(&file, &saved);
    });
}

fn write_saved(file: &Path, saved: &Saved) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("json.tmp");
    {
        let f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        serde_json::to_writer(f, saved).map_err(std::io::Error::other)?;
    }
    std::fs::rename(&tmp, file)
}

fn result_of(s: &Saved, elapsed_ms: u64) -> ScanResult {
    ScanResult {
        root: scanner::prune(&s.tree, &s.root, PRUNE_FRACTION, 1),
        errors: s.errors,
        permission_denied: s.permission_denied,
        denied_examples: s.denied_examples.clone(),
        other_fs_skipped: s.other_fs_skipped,
        elapsed_ms,
        scanned_at: s.scanned_at,
    }
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
    let app2 = app.clone();
    let out = tauri::async_runtime::spawn_blocking(move || {
        scanner::scan_dir(&r2, &scanner::Options::default(), |p| {
            let _ = app2.emit("scan-progress", p);
        })
    })
    .await
    .map_err(|e| e.to_string())?;
    let saved = Saved {
        root,
        tree: out.tree,
        scanned_at: now_secs(),
        errors: out.errors,
        permission_denied: out.permission_denied,
        other_fs_skipped: out.other_fs_skipped,
        denied_examples: out.denied_examples,
    };
    let res = result_of(&saved, started.elapsed().as_millis() as u64);
    *SCAN_STORE.lock().unwrap() = Some(saved);
    persist(&app);
    Ok(res)
}

/// Restore the scan from the previous launch, or null when there is none (or it is unreadable).
#[tauri::command]
async fn load_last_scan(app: tauri::AppHandle) -> Option<ScanResult> {
    let file = save_file(&app)?;
    let saved: Saved = tauri::async_runtime::spawn_blocking(move || {
        let f = std::io::BufReader::new(std::fs::File::open(file).ok()?);
        serde_json::from_reader(f).ok()
    })
    .await
    .ok()??;
    let res = result_of(&saved, 0);
    *SCAN_STORE.lock().unwrap() = Some(saved);
    Some(res)
}

/// Scan one folder again and splice it into the stored tree, replacing the old subtree.
#[tauri::command]
async fn rescan(app: tauri::AppHandle, path: String) -> Result<ScanResult, String> {
    let target = PathBuf::from(&path);
    let root = {
        let g = SCAN_STORE.lock().unwrap();
        let s = g.as_ref().ok_or("nothing scanned yet")?;
        if !target.starts_with(&s.root) {
            return Err("folder is not part of the scan".into());
        }
        s.root.clone()
    };
    let started = std::time::Instant::now();
    if !target.is_dir() {
        // The folder is gone: drop it from the scan instead of keeping stale numbers.
        if let Some(s) = SCAN_STORE.lock().unwrap().as_mut() {
            scanner::remove(&mut s.tree, &root, &target);
        }
        persist(&app);
        return Err(format!("{path} no longer exists"));
    }
    let t2 = target.clone();
    let app2 = app.clone();
    let out = tauri::async_runtime::spawn_blocking(move || {
        scanner::scan_dir(&t2, &scanner::Options::default(), |p| {
            let _ = app2.emit("scan-progress", p);
        })
    })
    .await
    .map_err(|e| e.to_string())?;
    let (errors, denied) = (out.errors, out.permission_denied);
    {
        let mut g = SCAN_STORE.lock().unwrap();
        let s = g.as_mut().ok_or("nothing scanned yet")?;
        if s.root != root || !scanner::replace(&mut s.tree, &root, &target, out.tree) {
            return Err("the scan changed while this folder was scanned; scan again".into());
        }
        s.scanned_at = now_secs();
        if target == root {
            s.errors = errors;
            s.permission_denied = denied;
            s.other_fs_skipped = out.other_fs_skipped;
            s.denied_examples = out.denied_examples;
        }
    }
    persist(&app);
    let g = SCAN_STORE.lock().unwrap();
    let s = g.as_ref().ok_or("nothing scanned yet")?;
    let node = scanner::find(&s.tree, &s.root, &target).ok_or("folder not in the scan")?;
    let mut res = result_of(s, started.elapsed().as_millis() as u64);
    res.root = scanner::prune(node, &target, PRUNE_FRACTION, 1);
    res.errors = errors;
    res.permission_denied = denied;
    Ok(res)
}

#[tauri::command]
fn view(path: String) -> Result<ViewNode, String> {
    let guard = SCAN_STORE.lock().unwrap();
    let s = guard.as_ref().ok_or("nothing scanned yet")?;
    let p = Path::new(&path);
    let node = scanner::find(&s.tree, &s.root, p).ok_or("folder not in the scan")?;
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
    if let Some(s) = SCAN_STORE.lock().unwrap().as_mut() {
        scanner::remove(&mut s.tree, &s.root.clone(), Path::new(path));
    }
    Ok(())
}

/// Moves to the system Trash (never a permanent delete) and updates the in-memory tree.
#[tauri::command]
fn move_to_trash(app: tauri::AppHandle, path: String) -> Result<(), String> {
    trash_path(&path)?;
    persist(&app);
    Ok(())
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
async fn uninstall_app(app: tauri::AppHandle, path: String, items: Vec<String>) -> Result<Vec<apps::ItemResult>, String> {
    if !cfg!(target_os = "macos") {
        return Err(NOT_MAC.into());
    }
    let res = tauri::async_runtime::spawn_blocking(move || {
        let home = dirs::home_dir().ok_or("no home folder")?;
        apps::uninstall(Path::new(&path), &home, &items, trash_path)
    })
    .await
    .map_err(|e| e.to_string())?;
    persist(&app);
    res
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
        .invoke_handler(tauri::generate_handler![home_dir, scan, view, load_last_scan, rescan, reveal, move_to_trash, full_disk_access, open_privacy_settings, relaunch, check_update, open_url, list_apps, app_uninstall_plan, uninstall_app])
        .run(tauri::generate_context!())
        .expect("error while running Disk Map");
}


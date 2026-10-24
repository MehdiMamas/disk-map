pub mod optimize;
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

/// Pure: turns osascript's stderr into a message the user can act on.
fn finder_error(stderr: &str) -> String {
    if stderr.contains("(-128)") {
        "Cancelled: the admin password is needed to remove this app".into()
    } else if stderr.contains("(-1743)") {
        "Allow Disk Map to control Finder in System Settings \u{2192} Privacy & Security \u{2192} Automation, then try again".into()
    } else {
        stderr.trim().to_string()
    }
}

/// Asks Finder to delete the path, so macOS shows its admin password prompt for
/// root-owned apps. Still goes to the Trash (Put Back works).
#[cfg(target_os = "macos")]
fn trash_with_finder(path: &str) -> Result<(), String> {
    let script = "on run argv\n tell application \"Finder\" to delete (POSIX file (item 1 of argv) as alias)\nend run";
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(finder_error(&String::from_utf8_lossy(&out.stderr)));
    }
    if let Some(s) = SCAN_STORE.lock().unwrap().as_mut() {
        scanner::remove(&mut s.tree, &s.root.clone(), Path::new(path));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn trash_with_finder(_path: &str) -> Result<(), String> {
    Err("macOS needs an admin password to remove this app; drag it to the Trash in Finder".into())
}

/// Moves to the system Trash (never a permanent delete) and updates the in-memory tree.
#[tauri::command]
fn move_to_trash(app: tauri::AppHandle, path: String) -> Result<(), String> {
    trash_path(&path)?;
    persist(&app);
    Ok(())
}

const NOT_MAC: &str = "not supported yet";

/// The Apps list plus where its sizes came from: `from_scan` is the saved scan (root, time) when it answered any size.
#[derive(Serialize)]
struct AppsList {
    apps: Vec<apps::AppEntry>,
    from_scan: Option<(String, u64)>,
}

/// Size of `p` from the tree if it is there, else None (outside the root, or folded into "other").
fn scan_size(saved: &Saved, p: &Path) -> Option<u64> {
    scanner::find(&saved.tree, &saved.root, p).map(|n| n.size)
}

#[tauri::command]
async fn list_apps(fresh: bool) -> Result<AppsList, String> {
    if !cfg!(target_os = "macos") {
        return Err(NOT_MAC.into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let home = dirs::home_dir().ok_or("no home folder")?;
        let used = std::sync::atomic::AtomicBool::new(false);
        let size = |p: &Path| -> u64 {
            if !fresh {
                if let Some(s) = SCAN_STORE.lock().unwrap().as_ref().and_then(|s| scan_size(s, p)) {
                    used.store(true, std::sync::atomic::Ordering::Relaxed);
                    return s;
                }
            }
            apps::path_size(p)
        };
        let apps = apps::list_apps(&home, &size);
        let from_scan = if used.load(std::sync::atomic::Ordering::Relaxed) {
            SCAN_STORE.lock().unwrap().as_ref().map(|s| (s.root.to_string_lossy().into_owned(), s.scanned_at))
        } else {
            None
        };
        Ok(AppsList { apps, from_scan })
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
        apps::uninstall(Path::new(&path), &home, &items, trash_path, trash_with_finder)
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
        .invoke_handler(tauri::generate_handler![home_dir, scan, view, load_last_scan, rescan, reveal, move_to_trash, full_disk_access, open_privacy_settings, relaunch, check_update, open_url, list_apps, app_uninstall_plan, uninstall_app, optimize::optimize_overview, optimize::optimize_installed, optimize::optimize_processes, optimize::optimize_set_startup, optimize::optimize_uninstall, optimize::optimize_open])
        .run(tauri::generate_context!())
        .expect("error while running Disk Map");
}


#[cfg(test)]
mod tests {
    #[test]
    fn scan_size_uses_tree_and_falls_back_outside() {
        let leaf = |n: &str, size| Node { name: n.into(), size, is_dir: false, files: 1, children: vec![] };
        let dir = |n: &str, size, c| Node { name: n.into(), size, is_dir: true, files: 1, children: c };
        let tree = dir("", 300, vec![dir("Applications", 300, vec![dir("Fake.app", 300, vec![leaf("bin", 300)])])]);
        let saved = Saved { root: PathBuf::from("/nonexistent-root"), tree, ..Default::default() };
        // Not on disk at all, yet answered from the tree.
        assert_eq!(scan_size(&saved, Path::new("/nonexistent-root/Applications/Fake.app")), Some(300));
        // Missing node (folded into "other") and a path outside the root both give None, so the caller measures.
        assert_eq!(scan_size(&saved, Path::new("/nonexistent-root/Applications/Small.plist")), None);
        assert_eq!(scan_size(&saved, Path::new("/elsewhere/Fake.app")), None);
    }

    use super::*;

    #[test]
    fn saved_scan_round_trips_through_the_file() {
        let dir = std::env::temp_dir().join(format!("disk-map-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub/big.bin"), vec![1u8; 2 << 20]).unwrap();
        let out = scanner::scan_dir(&src, &scanner::Options::default(), |_| {});
        let saved = Saved { root: src.clone(), tree: out.tree, scanned_at: 42, errors: 1, ..Default::default() };
        let file = dir.join("data/last-scan.json");
        write_saved(&file, &saved).unwrap();
        assert!(!file.with_extension("json.tmp").exists());
        let back: Saved = serde_json::from_reader(std::fs::File::open(&file).unwrap()).unwrap();
        assert_eq!(back.root, src);
        assert_eq!(back.scanned_at, 42);
        assert_eq!(back.tree.size, saved.tree.size);
        assert!(scanner::find(&back.tree, &back.root, &src.join("sub/big.bin")).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod finder_tests {
    use super::finder_error;
    #[test]
    fn maps_osascript_errors() {
        assert!(finder_error("execution error: User canceled. (-128)").starts_with("Cancelled"));
        assert!(finder_error("Not authorized to send Apple events to Finder. (-1743)").contains("Automation"));
        assert_eq!(finder_error("boom\n"), "boom");
    }
}

//! Uninstall a macOS app: the bundle plus its leftover data in ~/Library, all moved to the Trash.

use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct AppInfo {
    pub path: PathBuf,
    pub name: String,
    pub bundle_id: String,
    pub bundle_name: String,
    pub display_name: String,
    pub executable: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Leftover {
    pub path: String,
    pub size: u64,
    pub kind: String,
    pub default_checked: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UninstallPlan {
    pub name: String,
    pub bundle_id: String,
    pub app_size: u64,
    pub leftovers: Vec<Leftover>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppEntry {
    pub path: String,
    pub name: String,
    pub app_size: u64,
    pub data_size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemResult {
    pub path: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// (folder under ~/Library, label shown to the user)
const LIB_DIRS: &[(&str, &str)] = &[
    ("Application Support", "Application Support"),
    ("Caches", "Caches"),
    ("Preferences", "Preferences"),
    ("Containers", "Sandbox container"),
    ("Group Containers", "Group container"),
    ("Saved Application State", "Saved window state"),
    ("Logs", "Logs"),
    ("HTTPStorages", "HTTP storage"),
    ("WebKit", "WebKit data"),
    ("Cookies", "Cookies"),
    ("LaunchAgents", "Launch agent"),
    ("Application Scripts", "Application scripts"),
];

pub const DOT_FOLDER_KIND: &str = "settings and extensions folder, may be shared";

fn str_key(d: &plist::Dictionary, k: &str) -> String {
    d.get(k).and_then(|v| v.as_string()).unwrap_or("").trim().to_string()
}

/// Reads Contents/Info.plist (XML or binary).
pub fn app_info(app_path: &Path) -> Result<AppInfo, String> {
    validate_bundle_path(app_path)?;
    let v = plist::Value::from_file(app_path.join("Contents/Info.plist")).map_err(|e| format!("can't read Info.plist: {e}"))?;
    let d = v.as_dictionary().ok_or("Info.plist is not a dictionary")?;
    let stem = app_path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let bundle_name = str_key(d, "CFBundleName");
    let display_name = str_key(d, "CFBundleDisplayName");
    let name = [&display_name, &bundle_name, &stem].into_iter().find(|s| !s.is_empty()).cloned().unwrap_or_default();
    Ok(AppInfo {
        path: app_path.to_path_buf(),
        name,
        bundle_id: str_key(d, "CFBundleIdentifier"),
        bundle_name,
        display_name,
        executable: str_key(d, "CFBundleExecutable"),
    })
}

/// Path must be a .app bundle with Contents/Info.plist, not under /System.
pub fn validate_bundle_path(p: &Path) -> Result<(), String> {
    if p.extension().and_then(|e| e.to_str()) != Some("app") {
        return Err("not an app bundle (path must end in .app)".into());
    }
    if p.starts_with("/System") {
        return Err("apps under /System are part of macOS and can't be removed".into());
    }
    if !p.join("Contents/Info.plist").is_file() {
        return Err("not an app bundle (no Contents/Info.plist)".into());
    }
    Ok(())
}

/// Refuses Apple apps and system locations.
pub fn check_safe(info: &AppInfo) -> Result<(), String> {
    validate_bundle_path(&info.path)?;
    if info.bundle_id.to_lowercase().starts_with("com.apple.") {
        return Err(format!("{} is an Apple app and can't be uninstalled here", info.name));
    }
    Ok(())
}

fn regex_escape(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        if "\\.^$|?*+()[]{}".contains(c) {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

pub fn is_running(app_path: &Path) -> bool {
    let pat = format!("{}/Contents/MacOS/", regex_escape(&app_path.to_string_lossy()));
    std::process::Command::new("pgrep")
        .arg("-f")
        .arg("--")
        .arg(pat)
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

pub fn check_not_running(info: &AppInfo) -> Result<(), String> {
    if is_running(&info.path) {
        return Err(format!("Quit {} first", info.name));
    }
    Ok(())
}

/// Pure: does a Library entry name belong to this app? Never a loose substring match.
pub fn name_matches(entry: &str, info: &AppInfo, group_container: bool) -> bool {
    let e = entry.to_lowercase();
    let id = info.bundle_id.to_lowercase();
    if group_container {
        return !id.is_empty() && e.ends_with(&format!(".{id}"));
    }
    let exact = [&info.bundle_id, &info.bundle_name, &info.display_name, &info.executable];
    if exact.iter().any(|n| !n.is_empty() && e == n.to_lowercase()) {
        return true;
    }
    !id.is_empty() && e.starts_with(&format!("{id}."))
}

/// Pure: indices of the names in `names` that belong to the app.
pub fn match_names(names: &[String], info: &AppInfo, group_container: bool) -> Vec<usize> {
    names.iter().enumerate().filter(|(_, n)| name_matches(n, info, group_container)).map(|(i, _)| i).collect()
}

/// Pure: candidate home dot-folder names (e.g. ".cursor").
pub fn dot_folder_names(info: &AppInfo) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for n in [&info.bundle_name, &info.executable, &info.display_name] {
        let n = n.trim().to_lowercase();
        if n.is_empty() || n.contains('/') {
            continue;
        }
        let d = format!(".{n}");
        if !v.contains(&d) {
            v.push(d);
        }
    }
    v
}

/// Physical size (blocks * 512 on unix), no symlink following.
pub fn path_size(p: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else { return 0 };
    #[cfg(unix)]
    let own = {
        use std::os::unix::fs::MetadataExt;
        m.blocks() * 512
    };
    #[cfg(not(unix))]
    let own = m.len();
    if m.is_dir() {
        let mut t = own;
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                t += path_size(&e.path());
            }
        }
        t
    } else {
        own
    }
}

pub fn find_leftovers(info: &AppInfo, home: &Path, size: &(dyn Fn(&Path) -> u64 + Sync)) -> Vec<Leftover> {
    let mut out = Vec::new();
    let lib = home.join("Library");
    for (dir, kind) in LIB_DIRS {
        let base = lib.join(dir);
        let Ok(rd) = std::fs::read_dir(&base) else { continue };
        let entries: Vec<String> = rd.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
        for i in match_names(&entries, info, *dir == "Group Containers") {
            let p = base.join(&entries[i]);
            out.push(Leftover { size: size(&p), path: p.to_string_lossy().into(), kind: (*kind).into(), default_checked: true });
        }
    }
    for d in dot_folder_names(info) {
        let p = home.join(&d);
        if p.is_dir() {
            out.push(Leftover { size: size(&p), path: p.to_string_lossy().into(), kind: DOT_FOLDER_KIND.into(), default_checked: false });
        }
    }
    out
}

/// Validates (bundle, not Apple, not running) and builds the plan.
pub fn plan(app_path: &Path, home: &Path) -> Result<(AppInfo, UninstallPlan), String> {
    let info = app_info(app_path)?;
    check_safe(&info)?;
    check_not_running(&info)?;
    let leftovers = find_leftovers(&info, home, &path_size);
    let p = UninstallPlan { name: info.name.clone(), bundle_id: info.bundle_id.clone(), app_size: path_size(app_path), leftovers };
    Ok((info, p))
}

/// Pure: items to trash = the app itself or plan paths; anything else is dropped.
pub fn accepted_items(app_path: &Path, plan: &UninstallPlan, requested: &[String]) -> Vec<String> {
    let allowed: HashSet<&str> = plan.leftovers.iter().map(|l| l.path.as_str()).collect();
    let app = app_path.to_string_lossy();
    let mut seen = HashSet::new();
    requested
        .iter()
        .filter(|r| (r.as_str() == app || allowed.contains(r.as_str())) && seen.insert((*r).clone()))
        .cloned()
        .collect()
}

/// Trashes the chosen items. When the normal trash fails for the app bundle itself
/// (root-owned, installed by a pkg), `admin_trash` is tried, which lets macOS ask for the
/// admin password. Leftovers only use `trash` and are processed whatever happens to the app.
pub fn uninstall(
    app_path: &Path,
    home: &Path,
    items: &[String],
    trash: impl Fn(&str) -> Result<(), String>,
    admin_trash: impl Fn(&str) -> Result<(), String>,
) -> Result<Vec<ItemResult>, String> {
    let (_, p) = plan(app_path, home)?;
    let app_str = app_path.to_string_lossy().to_string();
    Ok(accepted_items(app_path, &p, items)
        .into_iter()
        .map(|path| {
            let r = match trash(&path) {
                Err(_) if path == app_str => admin_trash(&path),
                r => r,
            };
            match r {
                Ok(()) => ItemResult { path, ok: true, error: None },
                Err(e) => ItemResult { path, ok: false, error: Some(e) },
            }
        })
        .collect())
}

/// `size` measures one path; the caller may answer from a stored scan instead of the disk.
pub fn list_apps(home: &Path, size: &(dyn Fn(&Path) -> u64 + Sync)) -> Vec<AppEntry> {
    let is_app = |p: &Path| p.extension().and_then(|x| x.to_str()) == Some("app");
    let mut apps: Vec<PathBuf> = Vec::new();
    for root in [PathBuf::from("/Applications"), home.join("Applications")] {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if is_app(&p) {
                apps.push(p);
            } else if p.is_dir() {
                if let Ok(sub) = std::fs::read_dir(&p) {
                    apps.extend(sub.flatten().map(|s| s.path()).filter(|s| is_app(s)));
                }
            }
        }
    }
    let mut out: Vec<AppEntry> = Vec::new();
    for chunk in apps.chunks(8) {
        std::thread::scope(|s| {
            let hs: Vec<_> = chunk
                .iter()
                .map(|p| {
                    s.spawn(move || {
                        let info = app_info(p).ok()?;
                        check_safe(&info).ok()?;
                        let data_size = find_leftovers(&info, home, size).iter().filter(|l| l.default_checked).map(|l| l.size).sum();
                        Some(AppEntry { path: p.to_string_lossy().into(), name: info.name, app_size: size(p), data_size })
                    })
                })
                .collect();
            out.extend(hs.into_iter().filter_map(|h| h.join().ok().flatten()));
        });
    }
    out.sort_by(|a, b| (b.app_size + b.data_size).cmp(&(a.app_size + a.data_size)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn info() -> AppInfo {
        AppInfo {
            path: "/Applications/Cursor.app".into(),
            name: "Cursor".into(),
            bundle_id: "com.todesktop.230313mzl4w4u92".into(),
            bundle_name: "Cursor".into(),
            display_name: "".into(),
            executable: "Cursor".into(),
        }
    }
    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn matches_exact_and_id_prefix_only() {
        let names = s(&[
            "Cursor",                                  // 0 exact name
            "cursor",                                  // 1 case-insensitive
            "Cursor Helper",                           // 2 no (loose)
            "CursorOther",                             // 3 no
            "com.todesktop.230313mzl4w4u92.plist",     // 4 id + .
            "com.todesktop.230313mzl4w4u92.ShipIt",    // 5
            "com.todesktop.230313mzl4w4u92",           // 6 exact id
            "com.todesktop.230313mzl4w4u92x",          // 7 no
            "Slack",                                   // 8 no
        ]);
        assert_eq!(match_names(&names, &info(), false), vec![0, 1, 4, 5, 6]);
    }

    #[test]
    fn group_containers_end_with_dot_id() {
        let names = s(&["ABCDE12345.com.todesktop.230313mzl4w4u92", "group.com.todesktop.230313mzl4w4u92", "com.todesktop.230313mzl4w4u92", "Cursor"]);
        assert_eq!(match_names(&names, &info(), true), vec![0, 1]);
    }

    #[test]
    fn dot_folders_are_lowercase_and_deduped() {
        assert_eq!(dot_folder_names(&info()), vec![".cursor".to_string()]);
    }

    fn make_app(root: &Path, name: &str, id: &str) -> PathBuf {
        let app = root.join(format!("{name}.app"));
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::write(
            app.join("Contents/Info.plist"),
            format!("<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{id}</string><key>CFBundleName</key><string>{name}</string><key>CFBundleExecutable</key><string>{name}</string></dict></plist>"),
        )
        .unwrap();
        fs::write(app.join("Contents/MacOS").join(name), b"x").unwrap();
        app
    }

    #[test]
    fn refuses_non_app_paths() {
        let t = tempfile::tempdir().unwrap();
        assert!(validate_bundle_path(&t.path().join("Foo")).is_err());
        let noplist = t.path().join("Foo.app");
        fs::create_dir_all(&noplist).unwrap();
        assert!(validate_bundle_path(&noplist).unwrap_err().contains("Info.plist"));
        assert!(validate_bundle_path(Path::new("/System/Applications/Calculator.app")).unwrap_err().contains("/System"));
    }

    #[test]
    fn refuses_apple_apps() {
        let t = tempfile::tempdir().unwrap();
        let app = make_app(t.path(), "Fake", "com.apple.Fake");
        let info = app_info(&app).unwrap();
        assert!(check_safe(&info).unwrap_err().contains("Apple"));
        assert!(plan(&app, t.path()).is_err());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn refuses_running_app() {
        let t = tempfile::tempdir().unwrap();
        let app = make_app(t.path(), "RunMe", "com.example.runme");
        // Copy `sleep` into the bundle and run it so pgrep sees <app>/Contents/MacOS/.
        let exe = app.join("Contents/MacOS/RunMe");
        fs::copy("/bin/sleep", &exe).unwrap();
        let mut child = std::process::Command::new(&exe).arg("30").spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let r = plan(&app, t.path());
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(r.err().unwrap(), "Quit RunMe first");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn plan_finds_leftovers_and_skips_decoy_and_uninstall_filters_items() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let app = make_app(t.path(), "DiskMapTestApp", "com.example.diskmaptest");
        for d in [
            "Library/Application Support/DiskMapTestApp",
            "Library/Caches/com.example.diskmaptest",
            "Library/Caches/DiskMapTestAppOther",
            ".diskmaptestapp",
        ] {
            fs::create_dir_all(home.join(d)).unwrap();
            fs::write(home.join(d).join("f"), b"data").unwrap();
        }
        fs::create_dir_all(home.join("Library/Preferences")).unwrap();
        fs::write(home.join("Library/Preferences/com.example.diskmaptest.plist"), b"p").unwrap();

        let (_, p) = plan(&app, &home).unwrap();
        let paths: Vec<&str> = p.leftovers.iter().map(|l| l.path.rsplit('/').next().unwrap()).collect();
        assert!(paths.contains(&"DiskMapTestApp") && paths.contains(&"com.example.diskmaptest") && paths.contains(&"com.example.diskmaptest.plist"));
        assert!(!paths.contains(&"DiskMapTestAppOther"));
        let dot = p.leftovers.iter().find(|l| l.path.ends_with(".diskmaptestapp")).unwrap();
        assert!(!dot.default_checked && dot.kind == DOT_FOLDER_KIND);
        assert!(p.leftovers.iter().filter(|l| l.default_checked).count() == 3);

        // Items outside the plan are dropped; the app itself is accepted.
        let decoy = home.join("Library/Caches/DiskMapTestAppOther").to_string_lossy().to_string();
        let ok = home.join("Library/Caches/com.example.diskmaptest").to_string_lossy().to_string();
        let app_s = app.to_string_lossy().to_string();
        let got = accepted_items(&app, &p, &[decoy, "/etc/passwd".into(), ok.clone(), app_s.clone(), ok.clone()]);
        assert_eq!(got, vec![ok, app_s]);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn app_trash_failure_falls_back_to_admin_trash() {
        use std::cell::RefCell;
        let t = tempfile::tempdir().unwrap();
        let app = make_app(t.path(), "Locked", "com.example.locked");
        let app_s = app.to_string_lossy().to_string();
        let left = t.path().join("Library/Caches/com.example.locked");
        fs::create_dir_all(&left).unwrap();
        let left_s = left.to_string_lossy().to_string();
        let items = vec![app_s.clone(), left_s.clone()];
        let deny_app = |p: &str| if p == app_s { Err("denied".to_string()) } else { Ok(()) };

        // Fallback succeeds: app ok, fallback called for the app only, leftover still trashed.
        let calls = RefCell::new(Vec::new());
        let r = uninstall(&app, t.path(), &items, deny_app, |p| { calls.borrow_mut().push(p.to_string()); Ok(()) }).unwrap();
        assert_eq!(*calls.borrow(), vec![app_s.clone()]);
        assert!(r.iter().all(|x| x.ok), "{r:?}");
        assert_eq!(r.len(), 2);

        // Cancelled: clear message kept, leftover still processed.
        let msg = "Cancelled: the admin password is needed to remove this app";
        let r = uninstall(&app, t.path(), &items, deny_app, |_| Err(msg.to_string())).unwrap();
        assert!(!r[0].ok);
        assert_eq!(r[0].error.as_deref(), Some(msg));
        assert!(r[1].ok);
    }

    #[test]
    fn reads_binary_plist() {
        let t = tempfile::tempdir().unwrap();
        let app = t.path().join("Bin.app");
        fs::create_dir_all(app.join("Contents")).unwrap();
        let mut d = plist::Dictionary::new();
        d.insert("CFBundleIdentifier".into(), "com.example.bin".into());
        d.insert("CFBundleDisplayName".into(), "Bin Pretty".into());
        plist::Value::Dictionary(d).to_file_binary(app.join("Contents/Info.plist")).unwrap();
        let i = app_info(&app).unwrap();
        assert_eq!((i.bundle_id.as_str(), i.name.as_str()), ("com.example.bin", "Bin Pretty"));
    }

    /// Manual: `cargo test e2e_fake_app -- --ignored --nocapture` (touches real ~/Library and the Trash).
    #[test]
    #[ignore]
    fn e2e_fake_app() {
        let home = dirs::home_dir().unwrap();
        let app = home.join("Applications/DiskMapTestApp.app");
        let _ = fs::remove_dir_all(&app);
        make_app(&home.join("Applications"), "DiskMapTestApp", "com.example.diskmaptest");
        let mk = |d: &str| {
            fs::create_dir_all(home.join(d)).unwrap();
            fs::write(home.join(d).join("f"), vec![1u8; 5000]).unwrap();
        };
        mk("Library/Application Support/DiskMapTestApp");
        mk("Library/Caches/com.example.diskmaptest");
        mk("Library/Caches/DiskMapTestAppOther");
        mk(".diskmaptestapp");
        fs::write(home.join("Library/Preferences/com.example.diskmaptest.plist"), b"p").unwrap();
        let (_, p) = plan(&app, &home).unwrap();
        for l in &p.leftovers {
            println!("PLAN {} | {} | {} | checked={}", l.path, l.kind, l.size, l.default_checked);
        }
        let mut items: Vec<String> = p.leftovers.iter().filter(|l| l.default_checked).map(|l| l.path.clone()).collect();
        items.push(app.to_string_lossy().into());
        let r = uninstall(&app, &home, &items, crate::trash_path, crate::trash_with_finder).unwrap();
        for x in &r {
            println!("RESULT {} ok={} {:?}", x.path, x.ok, x.error);
            assert!(x.ok);
        }
        assert!(!app.exists());
        assert!(!home.join("Library/Caches/com.example.diskmaptest").exists());
        assert!(home.join(".diskmaptestapp").exists());
        assert!(home.join("Library/Caches/DiskMapTestAppOther").exists());
    }

    /// Manual dry run: prints the plan only, never trashes.
    #[test]
    #[ignore]
    fn dry_run_cursor() {
        let home = dirs::home_dir().unwrap();
        let (_, p) = plan(Path::new("/Applications/Cursor.app"), &home).unwrap();
        println!("APP {} {} {}", p.name, p.bundle_id, p.app_size);
        for l in &p.leftovers {
            println!("PLAN {} | {} | {} | checked={}", l.path, l.kind, l.size, l.default_checked);
        }
    }
}

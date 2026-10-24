//! The Optimize tab: health checks, suggestions and reversible Windows tweaks.
//! Nothing here deletes files or kills processes. Tweaks (see tweaks.rs) write only a fixed
//! list of registry values, service start types and the power plan, and the old value is
//! saved to an undo journal first. Startup apps use the StartupApproved value Windows itself uses.
#![allow(dead_code)]

use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::Manager;

pub mod health;
pub mod hw;
pub mod installed;
pub mod journal;
pub mod macsys;
pub mod net;
pub mod procs;
pub mod restore;
pub mod rules;
pub mod startup;
pub mod tweaks;

/// A button the UI may show. Only these kinds exist, all reversible or just "open something".
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct Action {
    /// "toggle_startup" | "open_settings" | "task_manager" | "reveal"
    pub kind: String,
    pub label: String,
    pub arg: String,
}

impl Action {
    pub fn new(kind: &str, label: &str, arg: &str) -> Self {
        Action { kind: kind.into(), label: label.into(), arg: arg.into() }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct StartupItem {
    pub id: String,
    pub name: String,
    pub command: String,
    pub source: String,
    pub enabled: bool,
    pub can_toggle: bool,
    /// Why we suggest turning it off (only ever a suggestion).
    pub reason: Option<String>,
    /// Shown when it cannot be toggled here.
    pub note: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct DiskInfo {
    pub mount: String,
    pub total: u64,
    pub free: u64,
}

/// Everything the pure rules look at.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub disks: Vec<DiskInfo>,
    pub ram_total: u64,
    pub ram_used: u64,
    pub uptime_secs: u64,
    pub pending_restart: Option<bool>,
    pub temp_bytes: u64,
    pub temp_path: String,
    pub startup: Vec<StartupItem>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct HealthCard {
    pub id: String,
    pub title: String,
    pub value: String,
    /// "ok" | "warn" | "critical"
    pub level: String,
    pub detail: String,
    pub action: Option<Action>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub id: String,
    /// "critical" | "warn" | "info"
    pub severity: String,
    pub title: String,
    pub detail: String,
    pub action: Option<Action>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct InstalledApp {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub size: Option<u64>,
    /// yyyy-mm-dd from the install date the app registered ("Last updated").
    pub updated: Option<String>,
    pub can_uninstall: bool,
    pub tag: Option<String>,
    #[serde(skip)]
    pub uninstall: String,
}

#[derive(Serialize)]
pub struct Overview {
    pub cards: Vec<HealthCard>,
    pub suggestions: Vec<Suggestion>,
    pub startup: Vec<StartupItem>,
}

pub fn human(n: u64) -> String {
    let u = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1000.0 && i < u.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i > 0 && v < 10.0 { format!("{:.1} {}", v, u[i]) } else { format!("{:.0} {}", v, u[i]) }
}

pub fn build_overview() -> Overview {
    let snap = health::snapshot();
    Overview { cards: health::cards(&snap), suggestions: rules::suggest(&snap), startup: snap.startup.clone() }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize)]
pub struct ProfileInfo {
    pub id: String,
    pub name: String,
    pub blurb: String,
}

#[derive(Serialize)]
pub struct LastProfileInfo {
    pub name: String,
    pub at: u64,
}

#[derive(Serialize)]
pub struct TweaksReport {
    pub windows: bool,
    pub mac: bool,
    pub admin: bool,
    pub tweaks: Vec<tweaks::TweakView>,
    pub profiles: Vec<ProfileInfo>,
    pub last_profile: Option<LastProfileInfo>,
    pub changed_count: usize,
    /// Problems from the last action (things that could not be changed or restored).
    pub notes: Vec<String>,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn jpath(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join("optimize-journal.json"))
}

fn report(j: &journal::Journal, notes: Vec<String>) -> TweaksReport {
    let sys = tweaks::Sys;
    let mut ids: Vec<&str> = j.entries.iter().map(|e| e.tweak.as_str()).collect();
    ids.sort();
    ids.dedup();
    TweaksReport {
        windows: cfg!(windows),
        mac: cfg!(target_os = "macos"),
        admin: tweaks::Backend::is_admin(&sys),
        tweaks: if cfg!(any(windows, target_os = "macos")) { tweaks::views(&sys, j) } else { Vec::new() },
        profiles: tweaks::profiles_here().iter().map(|p| ProfileInfo { id: p.id.into(), name: p.name.into(), blurb: p.blurb.into() }).collect(),
        last_profile: j.last_profile.as_ref().map(|l| LastProfileInfo { name: l.name.clone(), at: l.at }),
        changed_count: ids.len(),
        notes,
    }
}

/// Loads the journal, runs `f` with a way to save it, and returns the new state.
fn session(app: &tauri::AppHandle, f: impl FnOnce(&mut journal::Journal, &mut dyn FnMut(&journal::Journal) -> Result<(), String>) -> Result<Vec<String>, String>) -> Result<TweaksReport, String> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = jpath(app)?;
    let mut j = journal::Journal::load(&path)?;
    let mut save = |j: &journal::Journal| j.save(&path);
    let res = f(&mut j, &mut save);
    let notes = res?;
    Ok(report(&j, notes))
}

#[tauri::command]
pub async fn optimize_tweaks(app: tauri::AppHandle) -> Result<TweaksReport, String> {
    blocking(move || session(&app, |_, _| Ok(Vec::new()))).await?
}

#[tauri::command]
pub async fn optimize_set_tweak(app: tauri::AppHandle, id: String, target: String) -> Result<TweaksReport, String> {
    blocking(move || {
        session(&app, |j, save| {
            let tw = tweaks::find(&id).ok_or("Unknown setting.")?;
            tweaks::apply(&tweaks::Sys, j, &tw, &target, now(), save)?;
            Ok(Vec::new())
        })
    })
    .await?
}

#[tauri::command]
pub async fn optimize_undo_tweak(app: tauri::AppHandle, id: String) -> Result<TweaksReport, String> {
    blocking(move || {
        session(&app, |j, save| {
            let tw = tweaks::find(&id).ok_or("Unknown setting.")?;
            tweaks::undo_tweak(&tweaks::Sys, j, &tw, save)?;
            Ok(Vec::new())
        })
    })
    .await?
}

#[tauri::command]
pub async fn optimize_restore_all(app: tauri::AppHandle) -> Result<TweaksReport, String> {
    blocking(move || session(&app, |j, save| tweaks::restore_all(&tweaks::Sys, j, save))).await?
}

/// Read-only: what applying this profile would change.
#[tauri::command]
pub async fn optimize_profile_plan(profile: String) -> Result<Vec<tweaks::PlanItem>, String> {
    blocking(move || tweaks::plan_profile(&tweaks::Sys, &profile)).await?
}

#[tauri::command]
pub async fn optimize_apply_profile(app: tauri::AppHandle, profile: String) -> Result<TweaksReport, String> {
    blocking(move || session(&app, |j, save| tweaks::apply_profile(&tweaks::Sys, j, &profile, now(), save))).await?
}

#[tauri::command]
pub async fn optimize_overview() -> Result<Overview, String> {
    blocking(build_overview).await
}

#[tauri::command]
pub async fn optimize_installed() -> Result<Vec<InstalledApp>, String> {
    blocking(installed::list).await?
}

#[tauri::command]
pub async fn optimize_processes() -> Result<procs::ProcReport, String> {
    blocking(procs::collect).await
}

#[tauri::command]
pub async fn optimize_set_startup(id: String, enabled: bool) -> Result<(), String> {
    blocking(move || startup::set_enabled(&id, enabled)).await?
}

#[tauri::command]
pub async fn optimize_uninstall(id: String) -> Result<(), String> {
    blocking(move || installed::launch_uninstaller(&id)).await?
}

/// kind: storage | startup | power | update | loginitems | taskmanager
#[tauri::command]
pub async fn optimize_open(kind: String) -> Result<(), String> {
    blocking(move || open_target(&kind)).await?
}

/// Maps a fixed list of names to what we open. Anything else is refused.
pub fn settings_target(kind: &str, mac: bool) -> Option<&'static str> {
    if mac {
        match kind {
            "loginitems" | "startup" => Some("x-apple.systempreferences:com.apple.LoginItems-Settings.extension"),
            "storage" => Some("x-apple.systempreferences:com.apple.settings.Storage"),
            _ => None,
        }
    } else {
        match kind {
            "storage" => Some("ms-settings:storagesense"),
            "startup" => Some("ms-settings:startupapps"),
            "power" => Some("ms-settings:powersleep"),
            "update" => Some("ms-settings:windowsupdate"),
            _ => None,
        }
    }
}

fn open_target(kind: &str) -> Result<(), String> {
    use std::process::Command;
    let mac = cfg!(target_os = "macos");
    let res = if kind == "taskmanager" {
        if cfg!(windows) {
            Command::new("taskmgr").spawn()
        } else if mac {
            Command::new("open").args(["-a", "Activity Monitor"]).spawn()
        } else {
            return Err("Not available on this system.".into());
        }
    } else {
        let t = settings_target(kind, mac).ok_or("Unknown settings page.")?;
        if cfg!(windows) {
            Command::new("explorer").arg(t).spawn()
        } else if mac {
            Command::new("open").arg(t).spawn()
        } else {
            return Err("Not available on this system.".into());
        }
    };
    res.map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_whitelist() {
        assert_eq!(settings_target("storage", false), Some("ms-settings:storagesense"));
        assert_eq!(settings_target("startup", false), Some("ms-settings:startupapps"));
        assert_eq!(settings_target("loginitems", true).map(|s| s.starts_with("x-apple")), Some(true));
        assert_eq!(settings_target("rm -rf", false), None);
        assert_eq!(settings_target("power", true), None);
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(1_500_000_000), "1.5 GB");
        assert_eq!(human(250_000_000_000), "250 GB");
    }
}

// ---------- Network and hardware pages ----------

#[derive(Serialize)]
pub struct NetAdapterView {
    pub name: String,
    pub dhcp: bool,
    pub servers: Vec<String>,
    /// True when we changed this adapter's DNS and can undo it.
    pub changed: bool,
}

#[derive(Serialize)]
pub struct NetReport {
    pub windows: bool,
    pub mac: bool,
    pub admin: bool,
    pub adapters: Vec<NetAdapterView>,
}

fn dns_tweak_id(adapter: &str) -> String {
    format!("dns:{adapter}")
}

fn net_report(app: &tauri::AppHandle) -> Result<NetReport, String> {
    let path = jpath(app)?;
    let j = journal::Journal::load(&path)?;
    let adapters = net::list_adapters()?
        .into_iter()
        .map(|a| {
            let changed = j.has(&dns_tweak_id(&a.name));
            NetAdapterView { name: a.name, dhcp: a.dhcp, servers: a.servers, changed }
        })
        .collect();
    Ok(NetReport { windows: cfg!(windows), mac: cfg!(target_os = "macos"), admin: tweaks::Backend::is_admin(&tweaks::Sys), adapters })
}

#[tauri::command]
pub async fn optimize_net(app: tauri::AppHandle) -> Result<NetReport, String> {
    blocking(move || net_report(&app)).await?
}

/// provider: cloudflare | google | quad9 | auto. The old setting is journaled first.
#[tauri::command]
pub async fn optimize_set_dns(app: tauri::AppHandle, adapter: String, provider: String) -> Result<NetReport, String> {
    blocking(move || {
        if cfg!(windows) && !tweaks::Backend::is_admin(&tweaks::Sys) {
            return Err(tweaks::ADMIN_MSG.to_string());
        }
        let want = net::provider_val(&provider)?;
        session(&app, |j, save| {
            let target = journal::Target::Dns { adapter: adapter.clone() };
            let b = &tweaks::Sys;
            let cur = tweaks::Backend::read(b, &target)?;
            if cur == want {
                return Ok(Vec::new());
            }
            let seq = j.push(&dns_tweak_id(&adapter), now(), vec![journal::Change { target: target.clone(), old: cur }]);
            if let Err(e) = save(j) {
                j.remove(seq);
                return Err(e);
            }
            if let Err(e) = tweaks::Backend::write(b, &target, &want) {
                j.remove(seq);
                let _ = save(j);
                return Err(e);
            }
            Ok(Vec::new())
        })?;
        net_report(&app)
    })
    .await?
}

#[tauri::command]
pub async fn optimize_undo_dns(app: tauri::AppHandle, adapter: String) -> Result<NetReport, String> {
    blocking(move || {
        if cfg!(windows) && !tweaks::Backend::is_admin(&tweaks::Sys) {
            return Err(tweaks::ADMIN_MSG.to_string());
        }
        session(&app, |j, save| {
            for e in j.for_tweak(&dns_tweak_id(&adapter)) {
                for c in e.changes.iter().rev() {
                    tweaks::Backend::write(&tweaks::Sys, &c.target, &c.old)?;
                }
                j.remove(e.seq);
                save(j)?;
            }
            Ok(Vec::new())
        })?;
        net_report(&app)
    })
    .await?
}

#[tauri::command]
pub async fn optimize_flush_dns() -> Result<(), String> {
    blocking(net::flush_dns).await?
}

#[tauri::command]
pub async fn optimize_ping(host: String) -> Result<net::PingResult, String> {
    blocking(move || net::ping(&host)).await?
}

#[tauri::command]
pub async fn optimize_hardware() -> Result<hw::Hardware, String> {
    blocking(hw::read).await
}

/// Login Items (macOS, read-only). Err carries a note to show instead of the list.
#[tauri::command]
pub async fn optimize_login_items() -> Result<Vec<String>, String> {
    blocking(startup::login_items).await
}

/// Restarts the Dock and Finder so Dock and Finder settings show. Only runs when the user clicks.
#[tauri::command]
pub async fn optimize_relaunch_shell() -> Result<(), String> {
    blocking(|| {
        #[cfg(target_os = "macos")]
        {
            for app in ["Dock", "Finder"] {
                let _ = std::process::Command::new("/usr/bin/killall").arg(app).output();
            }
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("Only needed on macOS.".to_string())
        }
    })
    .await
}

/// Makes a restore point (Windows) or a Time Machine snapshot (macOS) once per run, before the first change. Slow (10-30 s).
#[tauri::command]
pub async fn ensure_restore_point() -> Result<restore::RestoreResult, String> {
    blocking(restore::ensure).await
}

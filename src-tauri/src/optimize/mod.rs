//! The Optimize tab: read-only health checks, suggestions and a few reversible actions.
//! Nothing here deletes files, kills processes or edits the registry beyond the
//! StartupApproved value that Windows itself uses for the "Startup apps" switch.
#![allow(dead_code)]

use serde::Serialize;

pub mod health;
pub mod installed;
pub mod procs;
pub mod rules;
pub mod startup;

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

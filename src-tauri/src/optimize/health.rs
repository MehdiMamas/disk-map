//! Reads the machine's state into a Snapshot and turns it into health cards. Read-only.
#![allow(dead_code)]
use super::{human, rules, startup, Action, DiskInfo, HealthCard, Snapshot};

fn is_fixed_mount(mount: &str, removable: bool) -> bool {
    if removable {
        return false;
    }
    if cfg!(target_os = "macos") {
        return mount == "/" || (mount.starts_with("/Volumes/") && !mount.starts_with("/System") && !mount.starts_with("/private"));
    }
    true
}

fn disks() -> Vec<DiskInfo> {
    let list = sysinfo::Disks::new_with_refreshed_list();
    let mut out: Vec<DiskInfo> = Vec::new();
    for d in list.list() {
        let mount = d.mount_point().to_string_lossy().into_owned();
        if d.total_space() == 0 || !is_fixed_mount(&mount, d.is_removable()) || out.iter().any(|x| x.mount == mount) {
            continue;
        }
        out.push(DiskInfo { mount, total: d.total_space(), free: d.available_space() });
    }
    out
}

#[cfg(windows)]
fn pending_restart() -> Option<bool> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;
    let h = RegKey::predef(HKEY_LOCAL_MACHINE);
    let a = r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired";
    let b = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending";
    Some(h.open_subkey_with_flags(a, KEY_READ).is_ok() || h.open_subkey_with_flags(b, KEY_READ).is_ok())
}

#[cfg(not(windows))]
fn pending_restart() -> Option<bool> {
    None
}

pub fn snapshot() -> Snapshot {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_memory();
    let temp = std::env::temp_dir();
    let mut items = startup::list();
    for it in items.iter_mut() {
        it.reason = rules::startup_reason(&it.name, &it.command).map(String::from);
    }
    Snapshot {
        disks: disks(),
        ram_total: sys.total_memory(),
        ram_used: sys.used_memory(),
        uptime_secs: System::uptime(),
        pending_restart: pending_restart(),
        temp_bytes: crate::apps::path_size(&temp),
        temp_path: temp.to_string_lossy().into_owned(),
        startup: items,
    }
}

fn card(id: &str, title: &str, value: String, level: &str, detail: &str, action: Option<Action>) -> HealthCard {
    HealthCard { id: id.into(), title: title.into(), value, level: level.into(), detail: detail.into(), action }
}

pub fn cards(s: &Snapshot) -> Vec<HealthCard> {
    let mut v = Vec::new();
    for d in &s.disks {
        let pct = d.free as f64 * 100.0 / d.total.max(1) as f64;
        let lvl = rules::disk_level(pct);
        v.push(card(
            &format!("disk:{}", d.mount),
            &format!("Free space on {}", d.mount),
            format!("{} free ({:.0}%)", human(d.free), pct),
            lvl,
            &format!("{} in total. Warns under {:.0}% free, critical under {:.0}%.", human(d.total), rules::DISK_WARN_PCT, rules::DISK_CRIT_PCT),
            if lvl == "ok" { None } else { Some(Action::new("open_settings", "Open storage settings", "storage")) },
        ));
    }
    let rp = rules::ram_pct(s);
    v.push(card(
        "ram",
        "Memory",
        format!("{:.0}% in use", rp),
        if rp > rules::RAM_WARN_PCT { "warn" } else { "ok" },
        &format!("{} of {} used.", human(s.ram_used), human(s.ram_total)),
        if rp > rules::RAM_WARN_PCT { Some(Action::new("task_manager", "Open task monitor", "")) } else { None },
    ));
    let days = s.uptime_secs / 86_400;
    let long = s.uptime_secs > rules::UPTIME_WARN_DAYS * 86_400;
    v.push(card(
        "uptime",
        "Time since restart",
        if days >= 1 { format!("{days} days") } else { "under a day".into() },
        if long { "warn" } else { "ok" },
        if long { "Restart to clear memory and finish updates." } else { "Nothing to do." },
        None,
    ));
    if let Some(p) = s.pending_restart {
        v.push(card(
            "pending-restart",
            "Pending restart",
            if p { "Restart needed".into() } else { "None".into() },
            if p { "warn" } else { "ok" },
            if p { "Windows is waiting for a restart to finish an update or install." } else { "No restart is waiting." },
            if p { Some(Action::new("open_settings", "Open Windows Update", "update")) } else { None },
        ));
    }
    let enabled = s.startup.iter().filter(|i| i.enabled).count();
    v.push(card(
        "startup",
        "Startup apps",
        format!("{enabled} enabled"),
        if enabled > rules::STARTUP_MANY { "warn" } else { "ok" },
        &format!("More than {} can slow down starting the computer.", rules::STARTUP_MANY),
        None,
    ));
    if !s.temp_path.is_empty() {
        let big = s.temp_bytes > rules::TEMP_BIG_BYTES;
        v.push(card(
            "temp",
            "Temporary files",
            human(s.temp_bytes),
            if big { "warn" } else { "ok" },
            "Your user temp folder. Apps normally clean it themselves.",
            Some(Action::new("reveal", "Show folder", &s.temp_path)),
        ));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_levels() {
        let s = Snapshot {
            disks: vec![DiskInfo { mount: "C:\\".into(), total: 100, free: 5 }],
            ram_total: 100,
            ram_used: 90,
            uptime_secs: 15 * 86_400,
            pending_restart: Some(true),
            temp_bytes: 10,
            temp_path: "C:\\t".into(),
            ..Default::default()
        };
        let c = cards(&s);
        let lvl = |id: &str| c.iter().find(|x| x.id == id).unwrap().level.clone();
        assert_eq!(lvl("disk:C:\\"), "critical");
        assert_eq!(lvl("ram"), "warn");
        assert_eq!(lvl("uptime"), "warn");
        assert_eq!(lvl("pending-restart"), "warn");
        assert_eq!(lvl("startup"), "ok");
        assert_eq!(lvl("temp"), "ok");
    }

    #[test]
    fn pending_card_hidden_when_unknown() {
        assert!(cards(&Snapshot::default()).iter().all(|c| c.id != "pending-restart"));
    }

    #[test]
    fn fixed_mounts() {
        assert!(!is_fixed_mount("E:\\", true));
        assert!(is_fixed_mount("C:\\", false));
    }
}

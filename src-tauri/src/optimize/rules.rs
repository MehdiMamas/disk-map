//! Rule tables and the pure `suggest()` function. Everything here is a suggestion with a reason,
//! never an automatic action. All thresholds are our own heuristics, not Microsoft or Apple limits.
use super::{human, Action, Snapshot, Suggestion};

// Our own heuristics.
pub const DISK_WARN_PCT: f64 = 15.0; // free space below this: warn
pub const DISK_CRIT_PCT: f64 = 10.0; // free space below this: critical
pub const RAM_WARN_PCT: f64 = 85.0; // memory in use above this: warn
pub const UPTIME_WARN_DAYS: u64 = 14; // up longer than this: suggest a restart
pub const STARTUP_MANY: usize = 10; // more enabled startup items than this: suggest a review
pub const TEMP_BIG_BYTES: u64 = 1_000_000_000; // user temp folder bigger than this: mention it

/// Startup items matching any of these are never suggested for turning off. This wins over SUGGEST.
pub static NEVER: &[&str] = &[
    // security
    "defender", "securityhealth", "mcafee", "norton", "avast", "avg", "bitdefender", "kaspersky", "eset",
    "malwarebytes",
    // audio
    "realtek", "rtkaud", "waves", "nahimic",
    // graphics and input
    "nvidia", "radeon", "amd", "intel graphics", "igfx", "synaptics", "logitech", "touchpad",
    // anything that calls itself a driver
    "driver",
];

pub struct Rule {
    pub reason: &'static str,
    /// Case-insensitive substrings of the item name or command.
    pub pats: &'static [&'static str],
}

pub static SUGGEST: &[Rule] = &[
    Rule { reason: "Updater: it can check for updates when you open the app instead.", pats: &["adobe", "jusched", "java update", "googleupdate", "google update"] },
    Rule {
        reason: "Game launcher or chat app: it can start when you open it, not at every boot.",
        pats: &["steam", "epic games", "epicgameslauncher", "eadesktop", "ea app", "ubisoft", "upc.exe", "battle.net", "spotify", "discord", "msteams", "ms-teams", "teams", "skype"],
    },
    Rule {
        reason: "Computer-maker helper: usually optional once the PC is set up.",
        pats: &["hp support assistant", "hpsupportassistant", "supportassist", "lenovo vantage", "lenovovantage"],
    },
    Rule { reason: "Only if you don't use OneDrive sync.", pats: &["onedrive"] },
];

pub struct TrialRule {
    pub pats: &'static [&'static str],
    /// Only tag when the name also contains "trial" (keeps real security software untagged).
    pub need_trial: bool,
}

pub static TRIALWARE: &[TrialRule] = &[
    TrialRule { pats: &["wildtangent", "candy crush", "booking.com", "dropbox promotion"], need_trial: false },
    TrialRule { pats: &["expressvpn", "mcafee", "norton"], need_trial: true },
];

pub const TRIAL_TAG: &str = "Often preinstalled; remove if unused";

pub fn never_suggest(name: &str, command: &str) -> bool {
    let h = format!("{} {}", name, command).to_lowercase();
    NEVER.iter().any(|p| h.contains(p))
}

/// Why this startup item could be turned off, or None. The never-list beats every suggestion.
pub fn startup_reason(name: &str, command: &str) -> Option<&'static str> {
    if never_suggest(name, command) {
        return None;
    }
    let h = format!("{} {}", name, command).to_lowercase();
    SUGGEST.iter().find(|r| r.pats.iter().any(|p| h.contains(p))).map(|r| r.reason)
}

pub fn trial_tag(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    TRIALWARE
        .iter()
        .any(|r| (!r.need_trial || n.contains("trial")) && r.pats.iter().any(|p| n.contains(p)))
        .then_some(TRIAL_TAG)
}

fn rank(sev: &str) -> u8 {
    match sev {
        "critical" => 0,
        "warn" => 1,
        _ => 2,
    }
}

pub fn disk_level(free_pct: f64) -> &'static str {
    if free_pct < DISK_CRIT_PCT { "critical" } else if free_pct < DISK_WARN_PCT { "warn" } else { "ok" }
}

pub fn ram_pct(s: &Snapshot) -> f64 {
    if s.ram_total == 0 { 0.0 } else { s.ram_used as f64 * 100.0 / s.ram_total as f64 }
}

/// Ranked suggestions, most urgent first.
pub fn suggest(s: &Snapshot) -> Vec<Suggestion> {
    let mut out: Vec<Suggestion> = Vec::new();
    for d in &s.disks {
        if d.total == 0 {
            continue;
        }
        let pct = d.free as f64 * 100.0 / d.total as f64;
        let lvl = disk_level(pct);
        if lvl != "ok" {
            out.push(Suggestion {
                id: format!("disk:{}", d.mount),
                severity: lvl.into(),
                title: format!("{} is running low on space", d.mount),
                detail: format!("Only {} of {} is free ({:.0}%). Look at the biggest folders in the Disk tab or open Storage settings.", human(d.free), human(d.total), pct),
                action: Some(Action::new("open_settings", "Open storage settings", "storage")),
            });
        }
    }
    if ram_pct(s) > RAM_WARN_PCT {
        out.push(Suggestion {
            id: "ram".into(),
            severity: "warn".into(),
            title: "Memory is nearly full".into(),
            detail: format!("{:.0}% of memory is in use. Close apps you are not using, or see which ones use the most below.", ram_pct(s)),
            action: Some(Action::new("task_manager", "Open task monitor", "")),
        });
    }
    if s.pending_restart == Some(true) {
        out.push(Suggestion {
            id: "pending-restart".into(),
            severity: "warn".into(),
            title: "Windows is waiting for a restart".into(),
            detail: "An update or install needs a restart to finish. Restart when you are ready.".into(),
            action: Some(Action::new("open_settings", "Open Windows Update", "update")),
        });
    }
    if s.uptime_secs > UPTIME_WARN_DAYS * 86_400 {
        out.push(Suggestion {
            id: "uptime".into(),
            severity: "info".into(),
            title: "Restart to clear memory and finish updates".into(),
            detail: format!("This computer has been on for {} days.", s.uptime_secs / 86_400),
            action: None,
        });
    }
    let enabled = s.startup.iter().filter(|i| i.enabled).count();
    if enabled > STARTUP_MANY {
        out.push(Suggestion {
            id: "startup-many".into(),
            severity: "info".into(),
            title: format!("{enabled} apps start with your computer"),
            detail: "Fewer startup apps usually means a faster start. Review the list below.".into(),
            action: Some(Action::new("open_settings", "Open startup settings", "startup")),
        });
    }
    if s.temp_bytes > TEMP_BIG_BYTES && !s.temp_path.is_empty() {
        out.push(Suggestion {
            id: "temp".into(),
            severity: "info".into(),
            title: "Your temporary folder is large".into(),
            detail: format!("{} of temporary files. Apps usually clean these themselves; you can look inside and decide.", human(s.temp_bytes)),
            action: Some(Action::new("reveal", "Show folder", &s.temp_path)),
        });
    }
    for it in &s.startup {
        if let (true, true, Some(r)) = (it.enabled, it.can_toggle, it.reason.as_ref()) {
            if never_suggest(&it.name, &it.command) {
                continue;
            }
            out.push(Suggestion {
                id: format!("startup:{}", it.id),
                severity: "info".into(),
                title: format!("Consider turning off {} at startup", it.name),
                detail: format!("Suggestion: {r} You can turn it back on any time."),
                action: Some(Action::new("toggle_startup", "Turn off", &it.id)),
            });
        }
    }
    out.sort_by_key(|x| rank(&x.severity));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::{DiskInfo, StartupItem};

    fn item(name: &str, cmd: &str) -> StartupItem {
        StartupItem {
            id: format!("HKCU|Run|{name}"),
            name: name.into(),
            command: cmd.into(),
            enabled: true,
            can_toggle: true,
            reason: startup_reason(name, cmd).map(String::from),
            ..Default::default()
        }
    }

    #[test]
    fn suggests_known_launchers() {
        assert!(startup_reason("Steam", r"C:\Program Files (x86)\Steam\steam.exe -silent").is_some());
        assert!(startup_reason("Discord", "Update.exe --processStart Discord.exe").is_some());
        assert!(startup_reason("OneDrive", "OneDrive.exe /background").unwrap().contains("OneDrive"));
        assert!(startup_reason("Notepad", "notepad.exe").is_none());
    }

    #[test]
    fn never_list_beats_suggest() {
        // "adobe" and "steam" would match, but "driver" / security names win.
        assert!(startup_reason("Adobe Driver Updater", "adobe.exe").is_none());
        assert!(startup_reason("Steam Controller Driver", "steam.exe").is_none());
        assert!(startup_reason("Avast Update", r"C:\avast\updater.exe adobe").is_none());
        assert!(startup_reason("SecurityHealth", "SecurityHealthSystray.exe").is_none());
        assert!(startup_reason("RtkAudUService", "RtkAudUService64.exe").is_none());
        let s = Snapshot { startup: vec![item("Logitech Options", "steam.exe")], ..Default::default() };
        assert!(suggest(&s).iter().all(|x| x.action.as_ref().map(|a| a.kind.as_str()) != Some("toggle_startup")));
    }

    #[test]
    fn disk_thresholds() {
        let mk = |free: u64| Snapshot { disks: vec![DiskInfo { mount: "C:\\".into(), total: 100, free }], ..Default::default() };
        assert!(suggest(&mk(20)).is_empty());
        assert!(suggest(&mk(15)).is_empty());
        assert_eq!(suggest(&mk(14))[0].severity, "warn");
        assert_eq!(suggest(&mk(10))[0].severity, "warn");
        assert_eq!(suggest(&mk(9))[0].severity, "critical");
    }

    #[test]
    fn ram_uptime_restart_thresholds() {
        let s = Snapshot { ram_total: 100, ram_used: 85, uptime_secs: 14 * 86_400, ..Default::default() };
        assert!(suggest(&s).is_empty());
        let s = Snapshot { ram_total: 100, ram_used: 86, uptime_secs: 14 * 86_400 + 1, pending_restart: Some(true), ..Default::default() };
        let ids: Vec<_> = suggest(&s).into_iter().map(|x| x.id).collect();
        assert_eq!(ids, vec!["ram", "pending-restart", "uptime"]);
    }

    #[test]
    fn startup_count_and_temp_thresholds() {
        let ten: Vec<_> = (0..10).map(|i| item(&format!("App{i}"), "x.exe")).collect();
        assert!(suggest(&Snapshot { startup: ten.clone(), ..Default::default() }).is_empty());
        let mut eleven = ten;
        eleven.push(item("App10", "x.exe"));
        assert_eq!(suggest(&Snapshot { startup: eleven, ..Default::default() })[0].id, "startup-many");
        let t = Snapshot { temp_bytes: TEMP_BIG_BYTES + 1, temp_path: "C:\\t".into(), ..Default::default() };
        assert_eq!(suggest(&t)[0].action.as_ref().unwrap().kind, "reveal");
        let t = Snapshot { temp_bytes: TEMP_BIG_BYTES, temp_path: "C:\\t".into(), ..Default::default() };
        assert!(suggest(&t).is_empty());
    }

    #[test]
    fn critical_sorts_first_and_disabled_items_are_skipped() {
        let mut off = item("Spotify", "spotify.exe");
        off.enabled = false;
        let s = Snapshot {
            disks: vec![DiskInfo { mount: "C:\\".into(), total: 100, free: 5 }],
            startup: vec![item("Spotify", "spotify.exe"), off],
            ..Default::default()
        };
        let v = suggest(&s);
        assert_eq!(v[0].severity, "critical");
        assert_eq!(v.iter().filter(|x| x.id.starts_with("startup:")).count(), 1);
        assert!(v.iter().any(|x| x.detail.starts_with("Suggestion:")));
    }

    #[test]
    fn trialware_tags_are_conservative() {
        assert!(trial_tag("WildTangent Games App").is_some());
        assert!(trial_tag("McAfee LiveSafe - Trial").is_some());
        assert!(trial_tag("McAfee LiveSafe").is_none());
        assert!(trial_tag("ExpressVPN").is_none());
        assert!(trial_tag("Visual Studio Code").is_none());
    }
}

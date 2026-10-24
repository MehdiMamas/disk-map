//! Installed apps on Windows, read from the Uninstall registry keys. Read-only; the only action is
//! launching the vendor's own uninstaller after the user confirms. QuietUninstallString is never used.
#![allow(dead_code)]
use super::{rules, InstalledApp};
use std::collections::HashSet;

/// One Uninstall key as read from the registry, before filtering.
#[derive(Clone, Debug, Default)]
pub struct RawEntry {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub version: String,
    /// EstimatedSize, in KB.
    pub size_kb: Option<u64>,
    pub install_date: String,
    pub uninstall: String,
    pub system_component: bool,
    pub has_parent: bool,
    pub release_type: String,
}

fn is_kb_name(name: &str) -> bool {
    let n = name.trim_start();
    let b = n.as_bytes();
    b.len() > 2 && b[..2].eq_ignore_ascii_case(b"kb") && b[2].is_ascii_digit()
}

fn format_date(d: &str) -> Option<String> {
    let d = d.trim();
    if d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()) {
        Some(format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..8]))
    } else {
        None
    }
}

/// Drops entries that are not real apps, removes duplicates, sorts biggest first.
pub fn filter(raw: Vec<RawEntry>) -> Vec<InstalledApp> {
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut out: Vec<InstalledApp> = Vec::new();
    for r in raw {
        let name = r.name.trim().to_string();
        if name.is_empty() || r.system_component || r.has_parent || is_kb_name(&name) {
            continue;
        }
        let rt = r.release_type.to_lowercase();
        if matches!(rt.as_str(), "update" | "hotfix" | "security update") {
            continue;
        }
        if !seen.insert((name.to_lowercase(), r.version.trim().to_string())) {
            continue;
        }
        let cmd = uninstall_command(&r.uninstall);
        out.push(InstalledApp {
            id: r.id,
            tag: rules::trial_tag(&name).map(String::from),
            name,
            publisher: r.publisher.trim().to_string(),
            version: r.version.trim().to_string(),
            size: r.size_kb.filter(|k| *k > 0).map(|k| k * 1024),
            updated: format_date(&r.install_date),
            can_uninstall: !cmd.is_empty(),
            uninstall: cmd,
        });
    }
    out.sort_by(|a, b| b.size.unwrap_or(0).cmp(&a.size.unwrap_or(0)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

/// Windows Installer stores `MsiExec.exe /I{GUID}` (install/repair); uninstalling needs `/X`.
pub fn fix_msi(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    if !lower.contains("msiexec") {
        return s.to_string();
    }
    match lower.find("/i{") {
        Some(i) => format!("{}/X{}", &s[..i], &s[i + 2..]),
        None => s.to_string(),
    }
}

pub fn uninstall_command(s: &str) -> String {
    fix_msi(s.trim())
}

pub fn list() -> Result<Vec<InstalledApp>, String> {
    imp::list()
}

pub fn launch_uninstaller(id: &str) -> Result<(), String> {
    imp::launch_uninstaller(id)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::process::CommandExt;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;

    const BASES: [(&str, &str); 3] = [
        ("HKLM", r"Software\Microsoft\Windows\CurrentVersion\Uninstall"),
        ("HKLM", r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
        ("HKCU", r"Software\Microsoft\Windows\CurrentVersion\Uninstall"),
    ];
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn hive(n: &str) -> Option<RegKey> {
        match n {
            "HKLM" => Some(RegKey::predef(HKEY_LOCAL_MACHINE)),
            "HKCU" => Some(RegKey::predef(HKEY_CURRENT_USER)),
            _ => None,
        }
    }

    pub fn list() -> Result<Vec<InstalledApp>, String> {
        let mut raw = Vec::new();
        for (hn, base) in BASES {
            let Some(h) = hive(hn) else { continue };
            let Ok(root) = h.open_subkey_with_flags(base, KEY_READ) else { continue };
            let names: Vec<String> = root.enum_keys().filter_map(|k| k.ok()).collect();
            for k in names {
                let Ok(key) = root.open_subkey_with_flags(&k, KEY_READ) else { continue };
                let s = |n: &str| key.get_value::<String, _>(n).unwrap_or_default();
                raw.push(RawEntry {
                    id: format!("{hn}|{base}\\{k}"),
                    name: s("DisplayName"),
                    publisher: s("Publisher"),
                    version: s("DisplayVersion"),
                    size_kb: key.get_value::<u32, _>("EstimatedSize").ok().map(u64::from),
                    install_date: s("InstallDate"),
                    uninstall: s("UninstallString"),
                    system_component: key.get_value::<u32, _>("SystemComponent").map(|v| v == 1).unwrap_or(false),
                    has_parent: !s("ParentKeyName").is_empty(),
                    release_type: s("ReleaseType"),
                });
            }
        }
        Ok(filter(raw))
    }

    pub fn launch_uninstaller(id: &str) -> Result<(), String> {
        let (hn, path) = id.split_once('|').ok_or("Unknown app.")?;
        let ok = BASES.iter().any(|(h, b)| *h == hn && path.len() > b.len() + 1 && path.starts_with(*b) && path.as_bytes()[b.len()] == b'\\');
        if !ok || path.contains("..") {
            return Err("Unknown app.".into());
        }
        let key = hive(hn).ok_or("Unknown app.")?.open_subkey_with_flags(path, KEY_READ).map_err(|_| "That app is no longer installed. Refresh the list.".to_string())?;
        // Always re-read the command here; the UI never sends one.
        let cmd = uninstall_command(&key.get_value::<String, _>("UninstallString").unwrap_or_default());
        if cmd.is_empty() {
            return Err("This app has no uninstaller. Use Windows Settings > Apps.".into());
        }
        // cmd /C "<command>": the outer quotes keep a quoted program path intact.
        std::process::Command::new("cmd")
            .raw_arg(format!("/C \"{cmd}\""))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub fn list() -> Result<Vec<InstalledApp>, String> {
        Ok(Vec::new())
    }
    pub fn launch_uninstaller(_id: &str) -> Result<(), String> {
        Err("Not available on this system.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str) -> RawEntry {
        RawEntry { id: name.into(), name: name.into(), uninstall: "x.exe /u".into(), ..Default::default() }
    }

    #[test]
    fn filter_skips_non_apps() {
        let mut parent = e("Child");
        parent.has_parent = true;
        let mut sys = e("Sys");
        sys.system_component = true;
        let mut upd = e("Some Update");
        upd.release_type = "Security Update".into();
        let mut hot = e("Fix");
        hot.release_type = "Hotfix".into();
        let raw = vec![e(""), parent, sys, upd, hot, e("KB5001234"), e("kb123 thing"), e("Good App")];
        let v = filter(raw);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "Good App");
    }

    #[test]
    fn filter_dedupes_sorts_and_converts() {
        let mut a = e("Big");
        a.size_kb = Some(2048);
        a.install_date = "20240131".into();
        let mut a2 = e("big");
        a2.size_kb = Some(1);
        let mut b = e("Small");
        b.size_kb = Some(1);
        let c = e("NoSize");
        let v = filter(vec![c, b, a, a2]);
        let names: Vec<_> = v.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["Big", "Small", "NoSize"]);
        assert_eq!(v[0].size, Some(2048 * 1024));
        assert_eq!(v[0].updated.as_deref(), Some("2024-01-31"));
        assert_eq!(v[2].size, None);
    }

    #[test]
    fn same_name_other_version_is_kept() {
        let mut a = e("App");
        a.version = "1".into();
        let mut b = e("App");
        b.version = "2".into();
        assert_eq!(filter(vec![a, b]).len(), 2);
    }

    #[test]
    fn msi_install_becomes_uninstall() {
        assert_eq!(fix_msi("MsiExec.exe /I{ABC-1}"), "MsiExec.exe /X{ABC-1}");
        assert_eq!(fix_msi("msiexec /i{ABC} /qn"), "msiexec /X{ABC} /qn");
        assert_eq!(fix_msi("MsiExec.exe /X{ABC}"), "MsiExec.exe /X{ABC}");
        assert_eq!(fix_msi(r#""C:\p\unins000.exe" /i{x}"#), r#""C:\p\unins000.exe" /i{x}"#);
        assert_eq!(uninstall_command("  MsiExec.exe /I{G}  "), "MsiExec.exe /X{G}");
    }

    #[test]
    fn trial_entries_get_a_tag() {
        let v = filter(vec![e("WildTangent Games"), e("Firefox")]);
        let t = v.iter().find(|x| x.name == "WildTangent Games").unwrap();
        assert_eq!(t.tag.as_deref(), Some("Often preinstalled; remove if unused"));
        assert!(v.iter().find(|x| x.name == "Firefox").unwrap().tag.is_none());
    }
}

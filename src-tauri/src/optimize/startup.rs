//! Startup apps. Windows: Run keys and Startup folders, with the on/off state Windows keeps in
//! StartupApproved (the same value the Settings "Startup apps" switch writes). macOS: launch agents.
#![allow(dead_code)]
use super::StartupItem;

/// StartupApproved value: 12 bytes. First byte with its low bit clear (02, 06) = enabled,
/// low bit set (03) = disabled. A missing value means enabled.
pub fn parse_approved(bytes: &[u8]) -> bool {
    bytes.first().map(|b| b & 1 == 0).unwrap_or(true)
}

/// Enabled: 02 followed by zeros. Disabled: 03, 3 zero bytes, then the current FILETIME (8 bytes LE).
pub fn encode_approved(enabled: bool, filetime: u64) -> [u8; 12] {
    let mut v = [0u8; 12];
    if enabled {
        v[0] = 0x02;
    } else {
        v[0] = 0x03;
        v[4..12].copy_from_slice(&filetime.to_le_bytes());
    }
    v
}

/// Windows FILETIME: 100 ns ticks since 1601-01-01.
pub fn filetime_now() -> u64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    (secs + 11_644_473_600) * 10_000_000
}

/// Ids look like `HKCU|Run|Name`; the name may contain `|`.
pub fn parse_id(id: &str) -> Option<(&str, &str, &str)> {
    let mut p = id.splitn(3, '|');
    let (a, b, c) = (p.next()?, p.next()?, p.next()?);
    if a.is_empty() || b.is_empty() || c.is_empty() { None } else { Some((a, b, c)) }
}

/// Reads `launchctl print-disabled` output: lines like `"com.foo.bar" => disabled`.
pub fn parse_disabled(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once("=>")?;
            if v.trim().trim_end_matches(',') != "disabled" {
                return None;
            }
            Some(k.trim().trim_matches('"').to_string())
        })
        .collect()
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::io::ErrorKind;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, REG_BINARY};
    use winreg::{RegKey, RegValue};

    const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

    fn hive(name: &str) -> Option<RegKey> {
        match name {
            "HKCU" => Some(RegKey::predef(HKEY_CURRENT_USER)),
            "HKLM" => Some(RegKey::predef(HKEY_LOCAL_MACHINE)),
            _ => None,
        }
    }

    fn approved_enabled(h: &RegKey, kind: &str, name: &str) -> bool {
        h.open_subkey_with_flags(format!(r"{APPROVED}\{kind}"), KEY_READ)
            .and_then(|k| k.get_raw_value(name))
            .map(|v| parse_approved(&v.bytes))
            .unwrap_or(true)
    }

    pub fn list() -> Vec<StartupItem> {
        let mut out = Vec::new();
        let run = r"Software\Microsoft\Windows\CurrentVersion\Run";
        let run32 = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
        let sources = [("HKCU", "Run", run, "Run key (this user)"), ("HKLM", "Run", run, "Run key (all users)"), ("HKLM", "Run32", run32, "Run key (32-bit apps)")];
        for (hn, kind, path, label) in sources {
            let Some(h) = hive(hn) else { continue };
            let Ok(key) = h.open_subkey_with_flags(path, KEY_READ) else { continue };
            let names: Vec<String> = key.enum_values().filter_map(|r| r.ok()).map(|(n, _)| n).collect();
            for n in names {
                let cmd: String = key.get_value(&n).unwrap_or_default();
                let can = hn == "HKCU" || is_admin_hint();
                out.push(StartupItem {
                    id: format!("{hn}|{kind}|{n}"),
                    name: n.clone(),
                    command: cmd,
                    source: label.into(),
                    enabled: approved_enabled(&h, kind, &n),
                    can_toggle: true,
                    note: if can { None } else { Some("Changing this one needs Disk Map to run as administrator.".into()) },
                    reason: None,
                });
            }
        }
        let folders = [("HKCU", dirs::data_dir().map(|d| d.join(r"Microsoft\Windows\Start Menu\Programs\Startup")), "Startup folder (this user)"),
            ("HKLM", std::env::var_os("ProgramData").map(|d| std::path::PathBuf::from(d).join(r"Microsoft\Windows\Start Menu\Programs\Startup")), "Startup folder (all users)")];
        for (hn, dir, label) in folders {
            let (Some(h), Some(dir)) = (hive(hn), dir) else { continue };
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let fname = e.file_name().to_string_lossy().into_owned();
                if fname.eq_ignore_ascii_case("desktop.ini") {
                    continue;
                }
                let name = fname.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or_else(|| fname.clone());
                out.push(StartupItem {
                    id: format!("{hn}|StartupFolder|{fname}"),
                    name,
                    command: e.path().to_string_lossy().into_owned(),
                    source: label.into(),
                    enabled: approved_enabled(&h, "StartupFolder", &fname),
                    can_toggle: true,
                    note: if hn == "HKCU" || is_admin_hint() { None } else { Some("Changing this one needs Disk Map to run as administrator.".into()) },
                    reason: None,
                });
            }
        }
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out
    }

    /// Whether writing under HKLM is likely to work (tries to open a key for writing; creates nothing).
    fn is_admin_hint() -> bool {
        RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(APPROVED, KEY_SET_VALUE).is_ok()
    }

    pub fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
        let (hn, kind, name) = parse_id(id).ok_or("Unknown startup item.")?;
        if !matches!(kind, "Run" | "Run32" | "StartupFolder") {
            return Err("Unknown startup item.".into());
        }
        let h = hive(hn).ok_or("Unknown startup item.")?;
        // Only change items that really exist in the list (never create values for arbitrary names).
        if !list().iter().any(|i| i.id == id) {
            return Err("That startup item no longer exists. Refresh the list.".into());
        }
        let (key, _) = h
            .create_subkey_with_flags(format!(r"{APPROVED}\{kind}"), KEY_SET_VALUE)
            .map_err(denied)?;
        let bytes = encode_approved(enabled, filetime_now()).to_vec();
        let val = RegValue { bytes: std::borrow::Cow::Owned(bytes), vtype: REG_BINARY };
        key.set_raw_value(name, &val).map_err(denied)
    }

    fn denied(e: std::io::Error) -> String {
        if e.kind() == ErrorKind::PermissionDenied {
            "Run Disk Map as administrator to change this item.".into()
        } else {
            e.to_string()
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn uid() -> Result<String, String> {
        let o = Command::new("id").arg("-u").output().map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
    }

    fn user_agents() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join("Library/LaunchAgents"))
    }

    fn label_of(p: &Path) -> Option<String> {
        let v = plist::Value::from_file(p).ok()?;
        v.as_dictionary()?.get("Label")?.as_string().map(String::from)
    }

    fn command_of(p: &Path) -> String {
        let Ok(v) = plist::Value::from_file(p) else { return String::new() };
        let Some(d) = v.as_dictionary() else { return String::new() };
        if let Some(s) = d.get("Program").and_then(|x| x.as_string()) {
            return s.to_string();
        }
        d.get("ProgramArguments")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_string()).collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
    }

    pub fn list() -> Vec<StartupItem> {
        let disabled = uid()
            .ok()
            .and_then(|u| Command::new("launchctl").args(["print-disabled", &format!("gui/{u}")]).output().ok())
            .map(|o| parse_disabled(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default();
        let mut out = Vec::new();
        let mut dirs_: Vec<(PathBuf, bool, &str)> = Vec::new();
        if let Some(d) = user_agents() {
            dirs_.push((d, true, "Launch agent (this user)"));
        }
        dirs_.push((PathBuf::from("/Library/LaunchAgents"), false, "Launch agent (all users)"));
        dirs_.push((PathBuf::from("/Library/LaunchDaemons"), false, "Launch daemon (system)"));
        for (dir, toggle, label) in dirs_ {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("plist") {
                    continue;
                }
                let Some(lab) = label_of(&p) else { continue };
                out.push(StartupItem {
                    id: format!("mac|{}|{}", p.to_string_lossy(), lab),
                    name: lab.clone(),
                    command: command_of(&p),
                    source: label.into(),
                    enabled: !disabled.contains(&lab),
                    can_toggle: toggle,
                    note: if toggle { None } else { Some("Managed by macOS settings. Use Login Items to change it.".into()) },
                    reason: None,
                });
            }
        }
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out
    }

    pub fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
        let rest = id.strip_prefix("mac|").ok_or("Unknown startup item.")?;
        let (path, label) = rest.rsplit_once('|').ok_or("Unknown startup item.")?;
        let p = PathBuf::from(path);
        let base = user_agents().ok_or("No home folder.")?;
        // Only plists directly inside ~/Library/LaunchAgents, and the label must match the file.
        if p.parent() != Some(base.as_path()) || label_of(&p).as_deref() != Some(label) {
            return Err("This item can't be changed here.".into());
        }
        let domain = format!("gui/{}", uid()?);
        let target = format!("{domain}/{label}");
        let run = |args: &[&str]| Command::new("launchctl").args(args).output().map_err(|e| e.to_string());
        if enabled {
            run(&["enable", &target])?;
            let o = run(&["bootstrap", &domain, path])?;
            // Already loaded is fine.
            if !o.status.success() && !String::from_utf8_lossy(&o.stderr).contains("already") && o.status.code() != Some(5) {
                return Err(String::from_utf8_lossy(&o.stderr).trim().to_string());
            }
        } else {
            let o = run(&["disable", &target])?;
            if !o.status.success() {
                return Err(String::from_utf8_lossy(&o.stderr).trim().to_string());
            }
            // Stop it now; it may not be loaded, which is fine.
            let _ = run(&["bootout", &domain, path]);
        }
        Ok(())
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use super::*;
    pub fn list() -> Vec<StartupItem> {
        Vec::new()
    }
    pub fn set_enabled(_id: &str, _enabled: bool) -> Result<(), String> {
        Err("Not available on this system.".into())
    }
}

pub fn list() -> Vec<StartupItem> {
    imp::list()
}

pub fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
    imp::set_enabled(id, enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_parse() {
        assert!(parse_approved(&[]));
        assert!(parse_approved(&[0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(parse_approved(&[0x06, 0, 0, 0]));
        assert!(!parse_approved(&[0x03, 0, 0, 0]));
        assert!(!parse_approved(&[0x01]));
    }

    #[test]
    fn approved_encode_roundtrip() {
        let on = encode_approved(true, 123);
        assert_eq!(on, [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(parse_approved(&on));
        let off = encode_approved(false, 0x0102030405060708);
        assert_eq!(off[0], 3);
        assert_eq!(&off[1..4], &[0, 0, 0]);
        assert_eq!(&off[4..], &0x0102030405060708u64.to_le_bytes());
        assert!(!parse_approved(&off));
    }

    #[test]
    fn id_parse() {
        assert_eq!(parse_id("HKCU|Run|My|App"), Some(("HKCU", "Run", "My|App")));
        assert_eq!(parse_id("HKCU|Run"), None);
        assert_eq!(parse_id("||"), None);
    }

    #[test]
    fn launchctl_disabled_parse() {
        let out = "disabled services = {\n\t\"com.a.b\" => disabled\n\t\"com.c.d\" => enabled\n\t\"com.e.f\" => disabled,\n}";
        assert_eq!(parse_disabled(out), vec!["com.a.b".to_string(), "com.e.f".to_string()]);
    }

    #[test]
    fn filetime_is_after_2020() {
        assert!(filetime_now() > 132_000_000_000_000_000);
    }
}

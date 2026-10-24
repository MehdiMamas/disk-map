//! macOS Full Disk Access detection.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    Readable,
    Denied,
    Missing,
}

/// Decides the access state from the outcome of each probed folder.
pub fn decide(probes: &[Probe]) -> &'static str {
    if probes.contains(&Probe::Readable) {
        "granted"
    } else if probes.contains(&Probe::Denied) {
        "denied"
    } else {
        "unknown"
    }
}

#[cfg(target_os = "macos")]
pub fn probe_all() -> &'static str {
    let home = dirs::home_dir().unwrap_or_default();
    let candidates = [
        home.join("Library/Safari"),
        home.join("Library/Mail"),
        home.join("Library/Messages"),
        home.join("Library/Containers/com.apple.stocks"),
        std::path::PathBuf::from("/Library/Application Support/com.apple.TCC"),
    ];
    let mut probes = Vec::new();
    for c in &candidates {
        match std::fs::read_dir(c) {
            Ok(_) => return "granted",
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => probes.push(Probe::Denied),
            Err(_) => probes.push(Probe::Missing),
        }
    }
    decide(&probes)
}

#[cfg(not(target_os = "macos"))]
pub fn probe_all() -> &'static str {
    "granted"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_readable_is_granted() {
        assert_eq!(decide(&[Probe::Denied, Probe::Readable, Probe::Missing]), "granted");
    }
    #[test]
    fn denied_without_readable_is_denied() {
        assert_eq!(decide(&[Probe::Missing, Probe::Denied]), "denied");
    }
    #[test]
    fn all_missing_is_unknown() {
        assert_eq!(decide(&[Probe::Missing, Probe::Missing]), "unknown");
    }
    #[test]
    fn empty_is_unknown() {
        assert_eq!(decide(&[]), "unknown");
    }
}

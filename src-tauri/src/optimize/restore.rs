//! A Windows System Restore point, made once per app run before the first system change.
//! We never turn System Protection on; if it is off we report that and let the user decide.
use serde::Serialize;
use std::sync::Mutex;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct RestoreResult {
    /// "created" | "already_recent" | "failed" | "unsupported"
    pub status: String,
    /// Unix seconds of the restore point (created / already_recent).
    pub time: Option<i64>,
    /// Plain-words reason (failed).
    pub reason: Option<String>,
}

impl RestoreResult {
    fn ok(status: &str, time: i64) -> Self {
        RestoreResult { status: status.into(), time: Some(time), reason: None }
    }
    fn failed(reason: String) -> Self {
        RestoreResult { status: "failed".into(), time: None, reason: Some(reason) }
    }
}

const DAY: i64 = 24 * 3600;

/// Turns the raw error text into a plain sentence.
pub fn explain_error(raw: &str) -> String {
    let l = raw.to_lowercase();
    if l.contains("access is denied") || l.contains("administrator") || l.contains("elevat") || l.contains("privilege") {
        "Disk Map is not running as administrator, so Windows will not create a restore point.".into()
    } else if l.contains("system protection") || l.contains("not enabled") || l.contains("turned off") || l.contains("disabled") || l.contains("restore is off") {
        "System Protection is turned off for drive C:, so Windows cannot create a restore point. Disk Map does not change that setting.".into()
    } else {
        let t = raw.trim();
        let t: String = t.chars().take(300).collect();
        if t.is_empty() { "Windows did not create a restore point.".into() } else { format!("Windows did not create a restore point: {t}") }
    }
}

/// Parses the script output. Lines: `BEFORE:<unix|none>`, `AFTER:<unix|none>`, `ERR:<text>`.
/// A point newer than the one before means created; a point under 24 hours old means Windows
/// skipped ours because it throttles to one per day, which is fine.
pub fn parse_output(out: &str, now: i64) -> RestoreResult {
    let mut before: Option<i64> = None;
    let mut after: Option<i64> = None;
    let mut err = String::new();
    for line in out.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("BEFORE:") {
            before = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("AFTER:") {
            after = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("ERR:") {
            if err.is_empty() { err = v.trim().to_string(); }
        }
    }
    if let Some(a) = after {
        if before != Some(a) {
            return RestoreResult::ok("created", a);
        }
        if now - a < DAY {
            return RestoreResult::ok("already_recent", a);
        }
    }
    if !err.is_empty() {
        return RestoreResult::failed(explain_error(&err));
    }
    RestoreResult::failed("Windows did not create a restore point. System Protection may be turned off for drive C:.".into())
}

static CACHE: Mutex<Option<RestoreResult>> = Mutex::new(None);

#[cfg(windows)]
fn run_script() -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    let script = "$ErrorActionPreference='Stop'; \
function Newest { try { $r = Get-ComputerRestorePoint | Sort-Object SequenceNumber -Descending | Select-Object -First 1; if ($r) { [int64]([DateTimeOffset][Management.ManagementDateTimeConverter]::ToDateTime($r.CreationTime)).ToUnixTimeSeconds() } else { 'none' } } catch { 'none' } }; \
Write-Output ('BEFORE:' + (Newest)); \
try { Checkpoint-Computer -Description 'Before Disk Map changes' -RestorePointType MODIFY_SETTINGS -WarningAction SilentlyContinue } catch { Write-Output ('ERR:' + ($_.Exception.Message -replace '\s+',' ')) }; \
Write-Output ('AFTER:' + (Newest))";
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(0x0800_0000)
        .output()
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(windows)]
pub fn ensure() -> RestoreResult {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = g.as_ref() {
        return r.clone();
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let r = match run_script() {
        Ok(out) => parse_output(&out, now),
        Err(e) => RestoreResult::failed(explain_error(&e)),
    };
    if r.status != "failed" {
        *g = Some(r.clone());
    }
    r
}

#[cfg(not(windows))]
pub fn ensure() -> RestoreResult {
    let _ = &CACHE;
    RestoreResult { status: "unsupported".into(), time: None, reason: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_point_is_created() {
        let r = parse_output("BEFORE:100\nAFTER:200\n", 300);
        assert_eq!(r, RestoreResult::ok("created", 200));
    }

    #[test]
    fn first_point_ever_is_created() {
        let r = parse_output("BEFORE:none\nAFTER:200\n", 300);
        assert_eq!(r.status, "created");
    }

    #[test]
    fn throttled_recent_point_is_ok() {
        let r = parse_output("BEFORE:1000\nAFTER:1000\n", 1000 + 3600);
        assert_eq!(r, RestoreResult::ok("already_recent", 1000));
    }

    #[test]
    fn old_point_without_new_one_fails() {
        let r = parse_output("BEFORE:1000\nAFTER:1000\n", 1000 + 3 * DAY);
        assert_eq!(r.status, "failed");
    }

    #[test]
    fn admin_error_is_explained() {
        let r = parse_output("BEFORE:none\nERR:Access is denied.\nAFTER:none\n", 5);
        assert_eq!(r.status, "failed");
        assert!(r.reason.unwrap().contains("administrator"));
    }

    #[test]
    fn protection_off_is_explained() {
        let r = parse_output("BEFORE:none\nERR:System Restore is turned off\nAFTER:none\n", 5);
        assert!(r.reason.unwrap().contains("System Protection"));
    }

    #[test]
    fn unknown_error_is_trimmed_and_kept() {
        let r = parse_output("ERR:  something odd  \n", 5);
        assert!(r.reason.unwrap().contains("something odd"));
    }

    #[test]
    fn empty_output_fails() {
        assert_eq!(parse_output("", 5).status, "failed");
    }
}

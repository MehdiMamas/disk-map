use semver::Version;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const API_URL: &str = "https://api.github.com/repos/MehdiMamas/disk-map/releases/latest";
const URL_PREFIX: &str = "https://github.com/MehdiMamas/disk-map/";

#[derive(Deserialize, Debug)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
}

#[derive(Deserialize, Debug)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub newer: bool,
    pub url: String,
}

pub fn parse_version(s: &str) -> Option<Version> {
    Version::parse(s.trim().trim_start_matches('v')).ok()
}

/// True only when `latest` is a stable release strictly above `current`.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(c), Some(l)) => l.pre.is_empty() && l > c,
        _ => false,
    }
}

/// The installer asset for the given OS ("macos", "windows", ...), else the release page.
pub fn pick_url(rel: &Release, os: &str) -> String {
    let suffix = match os {
        "macos" => Some("_universal.dmg"),
        "windows" => Some("-setup.exe"),
        _ => None,
    };
    suffix
        .and_then(|s| rel.assets.iter().find(|a| a.name.ends_with(s)))
        .map(|a| a.browser_download_url.clone())
        .unwrap_or_else(|| rel.html_url.clone())
}

pub fn build_info(current: &str, rel: &Release, os: &str) -> UpdateInfo {
    UpdateInfo {
        current: current.to_string(),
        latest: rel.tag_name.trim_start_matches('v').to_string(),
        newer: is_newer(current, &rel.tag_name),
        url: pick_url(rel, os),
    }
}

pub fn fetch_latest() -> Result<Release, String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build();
    agent
        .get(API_URL)
        .set("User-Agent", "disk-map-update-check")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .into_json::<Release>()
        .map_err(|e| e.to_string())
}

pub fn url_allowed(url: &str) -> bool {
    url.starts_with(URL_PREFIX) && !url.chars().any(|c| c.is_control() || " \"&|^<>%`".contains(c))
}

pub fn open(url: &str) -> Result<(), String> {
    if !url_allowed(url) {
        return Err("refusing to open a URL outside the Disk Map repository".into());
    }
    use std::process::Command;
    #[cfg(target_os = "macos")]
    let r = Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let r = Command::new("cmd").args(["/c", "start", "", url]).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = Command::new("xdg-open").arg(url).spawn();
    r.map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(tag: &str, names: &[&str]) -> Release {
        Release {
            tag_name: tag.into(),
            html_url: "https://github.com/MehdiMamas/disk-map/releases/tag/x".into(),
            assets: names
                .iter()
                .map(|n| Asset { name: n.to_string(), browser_download_url: format!("https://github.com/MehdiMamas/disk-map/releases/download/x/{n}") })
                .collect(),
        }
    }

    #[test]
    fn compare() {
        assert!(is_newer("0.1.0", "0.1.1"));
        assert!(is_newer("0.1.0", "v0.2.0"));
        assert!(!is_newer("0.1.1", "0.1.1"));
        assert!(!is_newer("0.1.1", "v0.1.1"));
        assert!(!is_newer("0.2.0", "0.1.9"));
        assert!(!is_newer("0.1.0", "garbage"));
    }

    #[test]
    fn prerelease_ignored() {
        assert!(!is_newer("0.1.0", "v0.2.0-beta.1"));
    }

    #[test]
    fn asset_pick() {
        let r = rel("v0.1.1", &["Disk Map_0.1.1_universal.dmg", "Disk Map_0.1.1_x64-setup.exe"]);
        assert!(pick_url(&r, "macos").ends_with("_universal.dmg"));
        assert!(pick_url(&r, "windows").ends_with("-setup.exe"));
        assert_eq!(pick_url(&r, "linux"), r.html_url);
    }

    #[test]
    fn missing_asset_falls_back() {
        let r = rel("v0.1.1", &["other.txt"]);
        assert_eq!(pick_url(&r, "macos"), r.html_url);
        let info = build_info("0.1.0", &r, "macos");
        assert_eq!(info, UpdateInfo { current: "0.1.0".into(), latest: "0.1.1".into(), newer: true, url: r.html_url.clone() });
    }

    #[test]
    fn open_url_refuses_foreign() {
        assert!(open("https://evil.example/").is_err());
        assert!(open("https://github.com/MehdiMamas/disk-map-evil/x").is_err());
        assert!(open("https://github.com/other/disk-map/").is_err());
        assert!(open("file:///etc/passwd").is_err());
        assert!(!url_allowed("https://github.com/MehdiMamas/disk-map/x y"));
        assert!(!url_allowed("https://github.com/MehdiMamas/disk-map/x&calc"));
        assert!(url_allowed("https://github.com/MehdiMamas/disk-map/releases/latest"));
    }

    #[test]
    #[ignore]
    fn live_latest_release() {
        let r = fetch_latest().expect("fetch");
        assert!(parse_version(&r.tag_name).is_some(), "tag {}", r.tag_name);
        let info = build_info("0.0.9", &r, "macos");
        println!("{info:?}");
        assert!(info.newer);
        assert!(info.url.ends_with("_universal.dmg"), "{}", info.url);
    }
}

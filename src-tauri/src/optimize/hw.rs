//! Hardware page: read-only facts about this computer.

use super::net::{json_items, run_powershell};
use serde::Serialize;

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct Gpu {
    pub name: String,
    pub driver: String,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct PhysDisk {
    pub name: String,
    /// "SSD", "HDD" or "Unknown".
    pub kind: String,
    pub size: u64,
}

#[derive(Serialize)]
pub struct Volume {
    pub mount: String,
    pub total: u64,
    pub free: u64,
}

#[derive(Serialize)]
pub struct Hardware {
    pub windows: bool,
    pub mac: bool,
    pub cpu: String,
    pub physical_cores: Option<u32>,
    pub logical_cores: u32,
    pub base_mhz: Option<u32>,
    pub ram_total: u64,
    pub ram_speed_mhz: Option<u32>,
    pub gpus: Vec<Gpu>,
    pub disks: Vec<PhysDisk>,
    pub volumes: Vec<Volume>,
    pub os: String,
    pub uptime_secs: u64,
}

fn text(v: &serde_json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string()
}

fn number(v: &serde_json::Value, k: &str) -> Option<u64> {
    v.get(k).and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.trim().parse().ok())))
}

pub fn parse_gpus(json: &str) -> Result<Vec<Gpu>, String> {
    Ok(json_items(json)?
        .iter()
        .map(|v| Gpu { name: text(v, "Name"), driver: text(v, "DriverVersion") })
        .filter(|g| !g.name.is_empty())
        .collect())
}

pub fn parse_disks(json: &str) -> Result<Vec<PhysDisk>, String> {
    Ok(json_items(json)?
        .iter()
        .map(|v| {
            let kind = match v.get("MediaType") {
                Some(serde_json::Value::String(s)) if s.eq_ignore_ascii_case("ssd") => "SSD",
                Some(serde_json::Value::String(s)) if s.eq_ignore_ascii_case("hdd") => "HDD",
                Some(serde_json::Value::Number(n)) if n.as_i64() == Some(4) => "SSD",
                Some(serde_json::Value::Number(n)) if n.as_i64() == Some(3) => "HDD",
                _ => "Unknown",
            };
            PhysDisk { name: text(v, "FriendlyName"), kind: kind.into(), size: number(v, "Size").unwrap_or(0) }
        })
        .filter(|d| !d.name.is_empty())
        .collect())
}

/// Highest memory speed (MHz) among the installed modules.
pub fn parse_ram_speed(json: &str) -> Result<Option<u32>, String> {
    Ok(json_items(json)?.iter().filter_map(|v| number(v, "Speed")).max().map(|n| n as u32))
}

/// ProductName says "Windows 10" on Windows 11, so the build number decides.
pub fn windows_name(product: &str, display: &str, build: &str) -> String {
    let b: u32 = build.trim().parse().unwrap_or(0);
    let mut name = product.to_string();
    if b >= 22000 {
        name = name.replace("Windows 10", "Windows 11");
        if !name.contains("Windows 11") {
            name = "Windows 11".into();
        }
    }
    let mut out = name;
    if !display.is_empty() {
        out.push(' ');
        out.push_str(display);
    }
    if b > 0 {
        out.push_str(&format!(" (build {b})"));
    }
    out
}

/// `sw_vers` output to "macOS 14.5 (23F79)".
pub fn parse_sw_vers(out: &str) -> String {
    let mut name = String::new();
    let mut ver = String::new();
    let mut build = String::new();
    for l in out.lines() {
        if let Some((k, v)) = l.split_once(':') {
            match k.trim() {
                "ProductName" => name = v.trim().to_string(),
                "ProductVersion" => ver = v.trim().to_string(),
                "BuildVersion" => build = v.trim().to_string(),
                _ => {}
            }
        }
    }
    if name.is_empty() {
        name = "macOS".into();
    }
    let mut o = name;
    if !ver.is_empty() {
        o.push(' ');
        o.push_str(&ver);
    }
    if !build.is_empty() {
        o.push_str(&format!(" ({build})"));
    }
    o
}

/// GPU names from `system_profiler SPDisplaysDataType -json`.
pub fn parse_mac_gpus(json: &str) -> Vec<Gpu> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    v.get("SPDisplaysDataType")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().map(|g| Gpu { name: text(g, "sppci_model"), driver: String::new() }).filter(|g| !g.name.is_empty()).collect())
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn mac_cmd(prog: &str, args: &[&str]) -> String {
    std::process::Command::new(prog).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

#[cfg(windows)]
fn os_string() -> String {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    let key = winreg::RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", KEY_READ);
    match key {
        Ok(k) => {
            let g = |n: &str| k.get_value::<String, _>(n).unwrap_or_default();
            windows_name(&g("ProductName"), &g("DisplayVersion"), &g("CurrentBuild"))
        }
        Err(_) => "Windows".into(),
    }
}
#[cfg(target_os = "macos")]
fn os_string() -> String {
    let t = mac_cmd("/usr/bin/sw_vers", &[]);
    if t.trim().is_empty() { "macOS".into() } else { parse_sw_vers(&t) }
}
#[cfg(not(any(windows, target_os = "macos")))]
fn os_string() -> String {
    sysinfo::System::long_os_version().unwrap_or_else(|| "macOS".into())
}

pub fn read() -> Hardware {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();
    let cpus = sys.cpus();
    let mut cpu = cpus.first().map(|c| c.brand().trim().to_string()).unwrap_or_default();
    let mut physical = System::physical_core_count().map(|n| n as u32);
    let mut base = cpus.first().map(|c| c.frequency() as u32).filter(|f| *f > 0);
    let mut ram_speed = None;
    let mut gpus = Vec::new();
    let mut disks = Vec::new();

    if cfg!(windows) {
        if let Ok(t) = run_powershell("Get-CimInstance Win32_Processor | Select-Object Name,NumberOfCores,MaxClockSpeed | ConvertTo-Json -Compress") {
            if let Ok(items) = json_items(&t) {
                if let Some(v) = items.first() {
                    let n = text(v, "Name");
                    if !n.is_empty() {
                        cpu = n;
                    }
                    let cores: u64 = items.iter().filter_map(|x| number(x, "NumberOfCores")).sum();
                    if cores > 0 {
                        physical = Some(cores as u32);
                    }
                    if let Some(m) = number(v, "MaxClockSpeed") {
                        base = Some(m as u32);
                    }
                }
            }
        }
        if let Ok(t) = run_powershell("Get-CimInstance Win32_PhysicalMemory | Select-Object Speed | ConvertTo-Json -Compress") {
            ram_speed = parse_ram_speed(&t).ok().flatten();
        }
        if let Ok(t) = run_powershell("Get-CimInstance Win32_VideoController | Select-Object Name,DriverVersion | ConvertTo-Json -Compress") {
            gpus = parse_gpus(&t).unwrap_or_default();
        }
        if let Ok(t) = run_powershell("Get-PhysicalDisk | Select-Object FriendlyName,@{n='MediaType';e={[string]$_.MediaType}},Size | ConvertTo-Json -Compress") {
            disks = parse_disks(&t).unwrap_or_default();
        }
    }

    #[cfg(target_os = "macos")]
    {
        let brand = mac_cmd("/usr/sbin/sysctl", &["-n", "machdep.cpu.brand_string"]);
        if !brand.trim().is_empty() {
            cpu = brand.trim().to_string();
        }
        if let Ok(n) = mac_cmd("/usr/sbin/sysctl", &["-n", "hw.physicalcpu"]).trim().parse::<u32>() {
            physical = Some(n);
        }
        gpus = parse_mac_gpus(&mac_cmd("/usr/sbin/system_profiler", &["SPDisplaysDataType", "-json"]));
    }

    let volumes = sysinfo::Disks::new_with_refreshed_list()
        .iter()
        .filter(|d| d.total_space() > 0)
        .map(|d| Volume { mount: d.mount_point().to_string_lossy().to_string(), total: d.total_space(), free: d.available_space() })
        .collect();

    Hardware {
        windows: cfg!(windows),
        mac: cfg!(target_os = "macos"),
        cpu,
        physical_cores: physical,
        logical_cores: cpus.len() as u32,
        base_mhz: base,
        ram_total: sys.total_memory(),
        ram_speed_mhz: ram_speed,
        gpus,
        disks,
        volumes,
        os: os_string(),
        uptime_secs: System::uptime(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_single_object_and_array() {
        let one = r#"{"Name":"NVIDIA RTX","DriverVersion":"31.0.1"}"#;
        assert_eq!(parse_gpus(one).unwrap(), vec![Gpu { name: "NVIDIA RTX".into(), driver: "31.0.1".into() }]);
        let two = r#"[{"Name":"A","DriverVersion":"1"},{"Name":"B","DriverVersion":"2"}]"#;
        assert_eq!(parse_gpus(two).unwrap().len(), 2);
        assert!(parse_gpus("").unwrap().is_empty());
    }

    #[test]
    fn disk_kinds() {
        let j = r#"[{"FriendlyName":"Samsung","MediaType":"SSD","Size":500107862016},{"FriendlyName":"WD","MediaType":3,"Size":"1000"},{"FriendlyName":"X","MediaType":"Unspecified","Size":1}]"#;
        let d = parse_disks(j).unwrap();
        assert_eq!((d[0].kind.as_str(), d[0].size), ("SSD", 500107862016));
        assert_eq!(d[1].kind, "HDD");
        assert_eq!(d[1].size, 1000);
        assert_eq!(d[2].kind, "Unknown");
        assert_eq!(parse_disks(r#"{"FriendlyName":"One","MediaType":"HDD","Size":5}"#).unwrap().len(), 1);
    }

    #[test]
    fn ram_speed() {
        assert_eq!(parse_ram_speed(r#"{"Speed":3200}"#).unwrap(), Some(3200));
        assert_eq!(parse_ram_speed(r#"[{"Speed":2666},{"Speed":3200}]"#).unwrap(), Some(3200));
        assert_eq!(parse_ram_speed("").unwrap(), None);
    }

    #[test]
    fn mac_facts() {
        let sw = "ProductName:		macOS
ProductVersion:		14.5
BuildVersion:		23F79
";
        assert_eq!(parse_sw_vers(sw), "macOS 14.5 (23F79)");
        assert_eq!(parse_sw_vers(""), "macOS");
        let j = r#"{"SPDisplaysDataType":[{"sppci_model":"Apple M2 Pro","_name":"x"},{"_name":"none"}]}"#;
        assert_eq!(parse_mac_gpus(j), vec![Gpu { name: "Apple M2 Pro".into(), driver: String::new() }]);
        assert!(parse_mac_gpus("junk").is_empty());
    }

    #[test]
    fn win11_by_build() {
        assert_eq!(windows_name("Windows 10 Pro", "24H2", "26100"), "Windows 11 Pro 24H2 (build 26100)");
        assert_eq!(windows_name("Windows 10 Pro", "22H2", "19045"), "Windows 10 Pro 22H2 (build 19045)");
    }
}

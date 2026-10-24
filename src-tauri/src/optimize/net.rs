//! Network page: DNS servers per adapter, flush DNS, ping.
//! Parsing and validation are pure functions so they can be tested anywhere.

use super::journal::Val;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub dhcp: bool,
    pub servers: Vec<String>,
}

#[derive(Serialize, Debug, Default, PartialEq)]
pub struct PingResult {
    pub sent: Option<u32>,
    pub received: Option<u32>,
    pub loss_pct: Option<u32>,
    pub min_ms: Option<u32>,
    pub max_ms: Option<u32>,
    pub avg_ms: Option<u32>,
    /// Last lines of the raw output, shown when parsing found nothing.
    pub raw: String,
}

/// PowerShell's ConvertTo-Json gives a bare object for one item and an array for several.
pub fn json_items(text: &str) -> Result<Vec<serde_json::Value>, String> {
    let t = text.trim().trim_start_matches('\u{feff}');
    if t.is_empty() {
        return Ok(Vec::new());
    }
    match serde_json::from_str::<serde_json::Value>(t).map_err(|e| e.to_string())? {
        serde_json::Value::Array(a) => Ok(a),
        serde_json::Value::Null => Ok(Vec::new()),
        v => Ok(vec![v]),
    }
}

#[derive(Deserialize)]
struct RawDns {
    #[serde(rename = "InterfaceAlias")]
    alias: String,
    #[serde(rename = "ServerAddresses", default)]
    servers: Option<serde_json::Value>,
}

/// Parses `Get-DnsClientServerAddress` output. `dhcp_aliases` lists adapters that use DHCP.
pub fn parse_dns(json: &str, dhcp_aliases: &[String]) -> Result<Vec<Adapter>, String> {
    let mut out = Vec::new();
    for v in json_items(json)? {
        let r: RawDns = serde_json::from_value(v).map_err(|e| e.to_string())?;
        let servers: Vec<String> = match r.servers {
            Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
            Some(serde_json::Value::String(s)) if !s.is_empty() => vec![s],
            _ => Vec::new(),
        };
        let dhcp = dhcp_aliases.iter().any(|a| a == &r.alias);
        out.push(Adapter { name: r.alias, dhcp, servers });
    }
    Ok(out)
}

/// Aliases from `Get-NetIPInterface` output (Dhcp as 1 / 0) where DHCP is on.
pub fn parse_dhcp_aliases(json: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for v in json_items(json)? {
        let alias = v.get("InterfaceAlias").and_then(|x| x.as_str());
        let dhcp = match v.get("Dhcp") {
            Some(serde_json::Value::Number(n)) => n.as_i64() == Some(1),
            Some(serde_json::Value::String(s)) => s.eq_ignore_ascii_case("enabled"),
            Some(serde_json::Value::Bool(b)) => *b,
            _ => false,
        };
        if let (Some(a), true) = (alias, dhcp) {
            out.push(a.to_string());
        }
    }
    Ok(out)
}

/// Accepts IPv4 / IPv6 literals and hostnames of [A-Za-z0-9.-], at most 253 characters, no leading '-'.
pub fn valid_host(h: &str) -> bool {
    if h.is_empty() || h.len() > 253 || h.starts_with('-') {
        return false;
    }
    if h.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    h.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

fn num_after(line: &str, key: &str) -> Option<u32> {
    let i = line.find(key)? + key.len();
    let digits: String = line[i..].chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Parses English `ping` output (Windows style).
pub fn parse_ping(out: &str) -> PingResult {
    let mut r = PingResult::default();
    for line in out.lines() {
        let l = line.trim();
        if l.starts_with("Packets:") {
            r.sent = num_after(l, "Sent");
            r.received = num_after(l, "Received");
            r.loss_pct = num_after(l, "(");
        } else if l.starts_with("Minimum") {
            r.min_ms = num_after(l, "Minimum");
            r.max_ms = num_after(l, "Maximum");
            r.avg_ms = num_after(l, "Average");
        }
    }
    if r.sent.is_none() && r.avg_ms.is_none() {
        let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
        let start = lines.len().saturating_sub(4);
        r.raw = lines[start..].join("\n");
    }
    r
}

/// Servers for a DNS provider choice. `None` means automatic (DHCP).
pub fn provider_servers(id: &str) -> Result<Option<Vec<&'static str>>, String> {
    Ok(match id {
        "cloudflare" => Some(vec!["1.1.1.1", "1.0.0.1"]),
        "google" => Some(vec!["8.8.8.8", "8.8.4.4"]),
        "quad9" => Some(vec!["9.9.9.9", "149.112.112.112"]),
        "auto" => None,
        _ => return Err("Unknown DNS provider.".into()),
    })
}

/// The journal value for a provider choice.
pub fn provider_val(id: &str) -> Result<Val, String> {
    Ok(Val::Str(match provider_servers(id)? {
        Some(s) => s.join(","),
        None => "dhcp".into(),
    }))
}

/// The `netsh` argument lists that set a journal value on an adapter.
pub fn netsh_plan(adapter: &str, v: &Val) -> Result<Vec<Vec<String>>, String> {
    let Val::Str(s) = v else { return Err("Cannot set DNS to this value.".into()) };
    let name = format!("name={adapter}");
    let base = |extra: &[&str]| -> Vec<String> {
        let mut a: Vec<String> = vec!["interface".into(), "ipv4".into()];
        a.extend(extra.iter().map(|x| x.to_string()));
        a
    };
    if s == "dhcp" {
        return Ok(vec![base(&["set", "dnsservers", &name, "source=dhcp"])]);
    }
    let ips: Vec<&str> = s.split(',').filter(|x| !x.is_empty()).collect();
    if ips.is_empty() || ips.len() > 4 || !ips.iter().all(|i| i.parse::<std::net::Ipv4Addr>().is_ok()) {
        return Err("Not a valid DNS server list.".into());
    }
    let mut plan = vec![base(&["set", "dnsservers", &name, "static", ips[0], "primary"])];
    for (i, ip) in ips.iter().enumerate().skip(1) {
        let idx = format!("index={}", i + 1);
        plan.push(base(&["add", "dnsservers", &name, ip, &idx]));
    }
    Ok(plan)
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    pub fn run(program: &str, args: &[String]) -> Result<String, String> {
        let out = Command::new(program).args(args).creation_flags(0x0800_0000).output().map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        let all = format!("{text}{err}").to_lowercase();
        if out.status.success() {
            Ok(text)
        } else if all.contains("access is denied") || all.contains("requires elevation") {
            Err(super::super::tweaks::ADMIN_MSG.into())
        } else {
            Err(format!("{program} failed: {}", text.trim()))
        }
    }

    pub fn powershell(script: &str) -> Result<String, String> {
        let args: Vec<String> = ["-NoProfile", "-NonInteractive", "-Command", script].iter().map(|s| s.to_string()).collect();
        run("powershell", &args)
    }

    pub fn adapters() -> Result<Vec<Adapter>, String> {
        let dns = powershell("Get-DnsClientServerAddress -AddressFamily IPv4 | Select-Object InterfaceAlias,ServerAddresses | ConvertTo-Json -Compress")?;
        let ifs = powershell("Get-NetIPInterface -AddressFamily IPv4 | Where-Object ConnectionState -eq 'Connected' | Select-Object InterfaceAlias,@{n='Dhcp';e={[int]$_.Dhcp}} | ConvertTo-Json -Compress")?;
        let up: Vec<String> = json_items(&ifs)?.iter().filter_map(|v| v.get("InterfaceAlias").and_then(|x| x.as_str()).map(String::from)).collect();
        let dhcp = parse_dhcp_aliases(&ifs)?;
        let mut list = parse_dns(&dns, &dhcp)?;
        list.retain(|a| up.contains(&a.name) && !a.name.to_lowercase().contains("loopback"));
        Ok(list)
    }
}

/// Adapters that are up, from the system. Windows only.
#[cfg(windows)]
pub fn list_adapters() -> Result<Vec<Adapter>, String> {
    win::adapters()
}
#[cfg(not(windows))]
pub fn list_adapters() -> Result<Vec<Adapter>, String> {
    Ok(Vec::new())
}

/// Reads an adapter's DNS setting as a journal value: Str("dhcp") or Str("ip,ip").
pub fn dns_read(adapter: &str) -> Result<Val, String> {
    let list = list_adapters()?;
    let a = list.iter().find(|a| a.name == adapter).ok_or("Unknown network adapter.")?;
    Ok(Val::Str(if a.dhcp { "dhcp".into() } else { a.servers.join(",") }))
}

/// Writes an adapter's DNS setting. The adapter must be one of the listed ones.
#[cfg(windows)]
pub fn dns_write(adapter: &str, v: &Val) -> Result<(), String> {
    if !list_adapters()?.iter().any(|a| a.name == adapter) {
        return Err("Unknown network adapter.".into());
    }
    for args in netsh_plan(adapter, v)? {
        win::run("netsh", &args)?;
    }
    Ok(())
}
#[cfg(not(windows))]
pub fn dns_write(_adapter: &str, _v: &Val) -> Result<(), String> {
    Err("Windows only for now.".into())
}

#[cfg(windows)]
pub fn flush_dns() -> Result<(), String> {
    win::run("ipconfig", &["/flushdns".to_string()]).map(|_| ())
}
#[cfg(not(windows))]
pub fn flush_dns() -> Result<(), String> {
    Err("Windows only for now.".into())
}

#[cfg(windows)]
pub fn ping(host: &str) -> Result<PingResult, String> {
    use std::os::windows::process::CommandExt;
    if !valid_host(host) {
        return Err("That is not a valid host name or address.".into());
    }
    let out = std::process::Command::new("ping").args(["-n", "4", host]).creation_flags(0x0800_0000).output().map_err(|e| e.to_string())?;
    Ok(parse_ping(&String::from_utf8_lossy(&out.stdout)))
}
#[cfg(not(windows))]
pub fn ping(host: &str) -> Result<PingResult, String> {
    if !valid_host(host) {
        return Err("That is not a valid host name or address.".into());
    }
    Err("Windows only for now.".into())
}

#[cfg(windows)]
pub fn run_powershell(script: &str) -> Result<String, String> {
    win::powershell(script)
}
#[cfg(not(windows))]
pub fn run_powershell(_script: &str) -> Result<String, String> {
    Err("Windows only for now.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_validation() {
        for ok in ["1.1.1.1", "example.com", "::1", "my-host.local"] {
            assert!(valid_host(ok), "{ok}");
        }
        for bad in ["a & calc", "-t", "x;y", "", "a b", "-n", "a|b"] {
            assert!(!valid_host(bad), "{bad}");
        }
        assert!(!valid_host(&"a".repeat(254)));
    }

    #[test]
    fn ping_parse() {
        let o = "Pinging 1.1.1.1 with 32 bytes of data:\r\nReply from 1.1.1.1: bytes=32 time=12ms TTL=58\r\n\r\nPing statistics for 1.1.1.1:\r\n    Packets: Sent = 4, Received = 3, Lost = 1 (25% loss),\r\nApproximate round trip times in milli-seconds:\r\n    Minimum = 11ms, Maximum = 14ms, Average = 12ms\r\n";
        let r = parse_ping(o);
        assert_eq!((r.sent, r.received, r.loss_pct), (Some(4), Some(3), Some(25)));
        assert_eq!((r.min_ms, r.max_ms, r.avg_ms), (Some(11), Some(14), Some(12)));
        let bad = parse_ping("Ping request could not find host zzz. Please check the name and try again.");
        assert!(bad.avg_ms.is_none() && bad.raw.contains("could not find"));
    }

    #[test]
    fn dns_parse_object_and_array() {
        let one = r#"{"InterfaceAlias":"Wi-Fi","ServerAddresses":["1.1.1.1","1.0.0.1"]}"#;
        let a = parse_dns(one, &[]).unwrap();
        assert_eq!(a.len(), 1);
        assert!(!a[0].dhcp);
        assert_eq!(a[0].servers, vec!["1.1.1.1", "1.0.0.1"]);
        let many = r#"[{"InterfaceAlias":"Wi-Fi","ServerAddresses":["192.168.1.1"]},{"InterfaceAlias":"Ethernet","ServerAddresses":"8.8.8.8"}]"#;
        let a = parse_dns(many, &["Wi-Fi".to_string()]).unwrap();
        assert_eq!(a.len(), 2);
        assert!(a[0].dhcp);
        assert_eq!(a[1].servers, vec!["8.8.8.8"]);
        assert!(parse_dns("", &[]).unwrap().is_empty());
    }

    #[test]
    fn dhcp_parse() {
        let j = r#"[{"InterfaceAlias":"Wi-Fi","Dhcp":1},{"InterfaceAlias":"Eth","Dhcp":0}]"#;
        assert_eq!(parse_dhcp_aliases(j).unwrap(), vec!["Wi-Fi"]);
        assert_eq!(parse_dhcp_aliases(r#"{"InterfaceAlias":"X","Dhcp":1}"#).unwrap(), vec!["X"]);
    }

    #[test]
    fn netsh_args() {
        let p = netsh_plan("Wi-Fi", &Val::Str("1.1.1.1,1.0.0.1".into())).unwrap();
        assert_eq!(p.len(), 2);
        assert!(p[0].contains(&"static".to_string()) && p[0].contains(&"name=Wi-Fi".to_string()));
        assert!(p[1].contains(&"index=2".to_string()));
        let d = netsh_plan("Wi-Fi", &Val::Str("dhcp".into())).unwrap();
        assert!(d[0].contains(&"source=dhcp".to_string()));
        assert!(netsh_plan("Wi-Fi", &Val::Str("1.1.1.1;calc".into())).is_err());
        assert_eq!(provider_val("auto").unwrap(), Val::Str("dhcp".into()));
        assert!(provider_val("x").is_err());
    }
}

//! Reversible Windows tweaks (power plan, privacy switches, a few services) and the
//! Work / Gaming / Balanced profiles built from them.
//!
//! The engine is pure: it talks to the system through the `Backend` trait, so the rules
//! (journal before write, rollback, restore order) are unit-tested with a mock on any OS.
//! Rules that never bend: the old value is journaled before every write, a failed write
//! leaves nothing behind, and no process is ever stopped.
use super::journal::{Change, Entry, Journal, LastProfile, Target, Val};
use serde::Serialize;

pub const ADMIN_MSG: &str = "Restart Disk Map as administrator to change this";
const WIN_ONLY: &str = "Windows only for now.";

pub trait Backend {
    fn read(&self, t: &Target) -> Result<Val, String>;
    fn write(&self, t: &Target, v: &Val) -> Result<(), String>;
    fn is_admin(&self) -> bool;
    /// Ids of the network interfaces (for the per-interface Nagle setting).
    fn interfaces(&self) -> Vec<String>;
}

/// (id, label, GUID) of the power plans we offer.
pub const PLANS: &[(&str, &str, &str)] = &[
    ("balanced", "Balanced", "381b4222-f694-41f0-9685-ff5bb260df2e"),
    ("high", "High performance", "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c"),
    ("ultimate", "Ultimate Performance", "e9a42b02-d5df-448d-aa00-03f14749eb61"),
];

#[derive(Clone)]
pub struct Item {
    pub target: Target,
    /// Value when our optimization is applied.
    pub on: Val,
    /// Value when it is not (what Windows normally has).
    pub off: Val,
}

pub enum Kind {
    Toggle(Vec<Item>),
    Plan,
    /// TcpAckFrequency and TCPNoDelay on every network interface.
    Nagle,
}

pub struct Tweak {
    pub id: &'static str,
    pub group: &'static str,
    pub name: &'static str,
    pub desc: &'static str,
    pub admin: bool,
    pub restart: bool,
    pub kind: Kind,
}

fn dw(n: u32) -> Val {
    Val::Dword(n)
}
fn st(s: &str) -> Val {
    Val::Str(s.to_string())
}
fn reg(hive: &str, path: &str, name: &str, on: Val, off: Val) -> Item {
    Item { target: Target::Reg { hive: hive.into(), path: path.into(), name: name.into() }, on, off }
}
fn svc(name: &str, off: &str) -> Item {
    svc_to(name, "disabled", off)
}
fn svc_to(name: &str, on: &str, off: &str) -> Item {
    Item { target: Target::Service { name: name.into() }, on: st(on), off: st(off) }
}
fn regkey(hive: &str, path: &str, on: Val, off: Val) -> Item {
    Item { target: Target::RegKey { hive: hive.into(), path: path.into() }, on, off }
}
fn bin(n: u8) -> Val {
    let mut v = vec![0u8; 12];
    v[0] = n;
    Val::Bin(v)
}

fn bcd(name: &str, on: Val, off: Val) -> Item {
    Item { target: Target::Bcd { name: name.into() }, on, off }
}
fn approved_run(name: &str) -> Item {
    reg("HKCU", r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run", name, bin(3), bin(2))
}

const SYSPROF: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile";
const EDGE: &str = r"SOFTWARE\Policies\Microsoft\Edge";
const CONSENT: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore";
const ADV: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const CLASSIC: &str = r"Software\Classes\CLSID\{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}\InprocServer32";
const NAGLE_IF: &str = r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces";

const CDM: &str = r"Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager";

fn toggle(id: &'static str, group: &'static str, name: &'static str, desc: &'static str, admin: bool, restart: bool, items: Vec<Item>) -> Tweak {
    Tweak { id, group, name, desc, admin, restart, kind: Kind::Toggle(items) }
}

pub fn catalog() -> Vec<Tweak> {
    let mut tips = Vec::new();
    for n in ["SubscribedContent-338388Enabled", "SubscribedContent-338389Enabled", "SubscribedContent-353694Enabled", "SubscribedContent-353696Enabled", "SubscribedContent-338393Enabled", "SubscribedContent-310093Enabled", "SystemPaneSuggestionsEnabled"] {
        tips.push(reg("HKCU", CDM, n, dw(0), dw(1)));
    }
    vec![
        Tweak { id: "power-plan", group: "performance", name: "Power plan", desc: "How hard the PC runs. High performance is faster but uses more power.", admin: false, restart: false, kind: Kind::Plan },
        toggle("visual-fx", "performance", "Visual effects: best performance", "Turns off window animations and shadows so the desktop feels quicker.", false, true, vec![reg("HKCU", r"Software\Microsoft\Windows\CurrentVersion\Explorer\VisualEffects", "VisualFXSetting", dw(2), dw(0))]),
        toggle("game-mode", "performance", "Game Mode", "Lets Windows give a running game more of the PC.", false, false, vec![reg("HKCU", r"Software\Microsoft\GameBar", "AutoGameModeEnabled", dw(1), dw(0))]),
        toggle(
            "gamedvr",
            "performance",
            "Game recording off",
            "Stops Windows recording game clips in the background.",
            false,
            false,
            vec![reg("HKCU", r"System\GameConfigStore", "GameDVR_Enabled", dw(0), dw(1)), reg("HKCU", r"Software\Microsoft\Windows\CurrentVersion\GameDVR", "AppCaptureEnabled", dw(0), dw(1))],
        ),
        toggle("background-apps", "performance", "Background apps off", "Stops store apps running when you are not using them.", false, false, vec![reg("HKCU", r"Software\Microsoft\Windows\CurrentVersion\BackgroundAccessApplications", "GlobalUserDisabled", dw(1), dw(0))]),
        toggle(
            "mouse-accel",
            "performance",
            "Mouse acceleration off",
            "Makes the pointer move the same distance every time, which suits games.",
            false,
            true,
            vec![
                reg("HKCU", r"Control Panel\Mouse", "MouseSpeed", st("0"), st("1")),
                reg("HKCU", r"Control Panel\Mouse", "MouseThreshold1", st("0"), st("6")),
                reg("HKCU", r"Control Panel\Mouse", "MouseThreshold2", st("0"), st("10")),
            ],
        ),
        toggle("hw-gpu-scheduling", "performance", "Hardware GPU scheduling", "Lets the graphics card manage its own memory. Helps some games.", true, true, vec![reg("HKLM", r"SYSTEM\CurrentControlSet\Control\GraphicsDrivers", "HwSchMode", dw(2), dw(1))]),
        toggle("sysmain", "performance", "SysMain service off", "Stops the background preloading service. Can help on slow hard drives.", true, false, vec![svc("SysMain", "auto")]),
        toggle("search-indexing", "performance", "Windows Search indexing off", "Stops file indexing. Searching in Start gets slower.", true, false, vec![svc("WSearch", "delayed-auto")]),
        toggle("advertising-id", "privacy", "Advertising ID off", "Stops apps using an ID to show you tailored ads.", false, false, vec![reg("HKCU", r"Software\Microsoft\Windows\CurrentVersion\AdvertisingInfo", "Enabled", dw(0), dw(1))]),
        toggle("tips-ads", "privacy", "Tips, suggestions and Start ads off", "Removes suggested apps and tips from Start and the lock screen.", false, false, tips),
        toggle("bing", "privacy", "Web results in Start search off", "Start search looks only at your PC.", false, false, vec![reg("HKCU", r"Software\Policies\Microsoft\Windows\Explorer", "DisableSearchBoxSuggestions", dw(1), Val::Absent)]),
        toggle("copilot", "privacy", "Copilot off", "Hides the Copilot assistant.", false, false, vec![reg("HKCU", r"Software\Policies\Microsoft\Windows\WindowsCopilot", "TurnOffWindowsCopilot", dw(1), Val::Absent)]),
        toggle(
            "activity-history",
            "privacy",
            "Activity history off",
            "Stops Windows keeping a timeline of what you opened.",
            true,
            false,
            vec![reg("HKLM", r"SOFTWARE\Policies\Microsoft\Windows\System", "PublishUserActivities", dw(0), Val::Absent), reg("HKLM", r"SOFTWARE\Policies\Microsoft\Windows\System", "UploadUserActivities", dw(0), Val::Absent)],
        ),
        toggle("telemetry", "privacy", "Telemetry: minimum", "Sends Microsoft only the least diagnostic data allowed.", true, false, vec![reg("HKLM", r"SOFTWARE\Policies\Microsoft\Windows\DataCollection", "AllowTelemetry", dw(0), Val::Absent)]),
        toggle("diagtrack", "privacy", "Connected User Experiences service off", "Stops the service that sends diagnostic data.", true, false, vec![svc("DiagTrack", "auto")]),

        toggle(
            "net-throttling",
            "performance",
            "Network throttling off",
            "Removes the limit Windows puts on network traffic while media plays, and gives apps more CPU time.",
            true,
            false,
            vec![reg("HKLM", SYSPROF, "NetworkThrottlingIndex", dw(0xffff_ffff), dw(10)), reg("HKLM", SYSPROF, "SystemResponsiveness", dw(10), dw(20))],
        ),
        Tweak { id: "nagle", group: "performance", name: "Nagle off (lower online game lag)", desc: "Sends small network packets at once instead of grouping them. Can lower lag in online games.", admin: true, restart: false, kind: Kind::Nagle },
        toggle(
            "hpet",
            "performance",
            "Platform clock tweaks (HPET)",
            "Stops forcing the slow platform clock and the dynamic tick. Can smooth frame times on some PCs; on others it changes nothing.",
            true,
            true,
            vec![bcd("useplatformclock", st("No"), Val::Absent), bcd("disabledynamictick", st("Yes"), Val::Absent)],
        ),
        toggle("fast-startup", "performance", "Fast Startup off", "Makes Shut down a full shutdown, which avoids odd issues after updates and driver changes.", true, false, vec![reg("HKLM", r"SYSTEM\CurrentControlSet\Control\Session Manager\Power", "HiberbootEnabled", dw(0), dw(1))]),
        toggle("hibernate-off", "performance", "Hibernation off", "Deletes the hibernation file to free disk space. Hibernate and Fast Startup stop working.", true, false, vec![Item { target: Target::Hibernate, on: st("off"), off: st("on") }]),
        toggle("menu-delay", "performance", "Faster menus", "Menus open after 100 ms instead of 400 ms.", false, true, vec![reg("HKCU", r"Control Panel\Desktop", "MenuShowDelay", st("100"), st("400"))]),
        toggle("startup-delay", "performance", "No delay for startup apps", "Starts your startup apps right after sign-in instead of waiting a few seconds.", false, true, vec![reg("HKCU", r"Software\Microsoft\Windows\CurrentVersion\Explorer\Serialize", "StartupDelayInMSec", dw(0), Val::Absent)]),
        toggle(
            "xbox-services",
            "performance",
            "Xbox services to Manual",
            "Stops four Xbox helper services starting at boot. Leave on if you use the Xbox app or Game Pass.",
            true,
            false,
            vec![svc_to("XblAuthManager", "demand", "demand"), svc_to("XblGameSave", "demand", "demand"), svc_to("XboxNetApiSvc", "demand", "demand"), svc_to("XboxGipSvc", "demand", "demand")],
        ),
        toggle("spooler-manual", "performance", "Print Spooler to Manual", "Starts printing support only when needed. Only if you never print.", true, false, vec![svc_to("Spooler", "demand", "auto")]),
        toggle("fax-manual", "performance", "Fax to Manual", "Starts the fax service only when needed. Only if you never fax.", true, false, vec![svc_to("Fax", "demand", "demand")]),
        toggle("cortana", "privacy", "Cortana off", "Turns off the Cortana assistant through policy.", true, false, vec![reg("HKLM", r"SOFTWARE\Policies\Microsoft\Windows\Windows Search", "AllowCortana", dw(0), Val::Absent)]),
        toggle(
            "office-telemetry",
            "privacy",
            "Office telemetry off",
            "Stops Office sending usage data and feedback prompts.",
            false,
            false,
            vec![
                reg("HKCU", r"Software\Policies\Microsoft\office\16.0\common", "sendtelemetry", dw(3), Val::Absent),
                reg("HKCU", r"Software\Policies\Microsoft\office\16.0\common", "qmenable", dw(0), Val::Absent),
                reg("HKCU", r"Software\Policies\Microsoft\office\16.0\common\feedback", "enabled", dw(0), Val::Absent),
            ],
        ),
        toggle(
            "edge-telemetry",
            "privacy",
            "Edge: less data and clutter",
            "Turns off Edge diagnostic data, shopping assistant, sidebar and startup boost.",
            true,
            false,
            vec![
                reg("HKLM", EDGE, "DiagnosticData", dw(0), Val::Absent),
                reg("HKLM", EDGE, "EdgeShoppingAssistantEnabled", dw(0), Val::Absent),
                reg("HKLM", EDGE, "HubsSidebarEnabled", dw(0), Val::Absent),
                reg("HKLM", EDGE, "StartupBoostEnabled", dw(0), Val::Absent),
            ],
        ),
        toggle("chrome-telemetry", "privacy", "Chrome usage reports off", "Stops Chrome sending usage statistics and crash reports.", true, false, vec![reg("HKLM", r"SOFTWARE\Policies\Google\Chrome", "MetricsReportingEnabled", dw(0), Val::Absent)]),
        toggle("firefox-telemetry", "privacy", "Firefox telemetry off", "Stops Firefox sending telemetry.", true, false, vec![reg("HKLM", r"SOFTWARE\Policies\Mozilla\Firefox", "DisableTelemetry", dw(1), Val::Absent)]),
        toggle("feedback-prompts", "privacy", "Feedback prompts off", "Windows stops asking you to rate things.", false, false, vec![reg("HKCU", r"Software\Microsoft\Siuf\Rules", "NumberOfSIUFInPeriod", dw(0), Val::Absent)]),
        toggle("location-off", "privacy", "Location access off", "Blocks apps from using your location.", false, false, vec![reg("HKCU", &format!(r"{CONSENT}\location"), "Value", st("Deny"), st("Allow"))]),
        toggle("app-diagnostics-off", "privacy", "App diagnostics access off", "Blocks apps from reading what other apps are doing.", false, false, vec![reg("HKCU", &format!(r"{CONSENT}\appDiagnostics"), "Value", st("Deny"), st("Allow"))]),
        toggle(
            "inking-typing",
            "privacy",
            "Inking and typing personalization off",
            "Stops Windows learning from what you type and write.",
            false,
            false,
            vec![reg("HKCU", r"Software\Microsoft\InputPersonalization", "RestrictImplicitTextCollection", dw(1), dw(0)), reg("HKCU", r"Software\Microsoft\InputPersonalization", "RestrictImplicitInkCollection", dw(1), dw(0))],
        ),
        toggle("tailored-experiences", "privacy", "Tailored experiences off", "Stops Windows using your diagnostic data to suggest things.", false, false, vec![reg("HKCU", r"Software\Microsoft\Windows\CurrentVersion\Privacy", "TailoredExperiencesWithDiagnosticDataEnabled", dw(0), dw(1))]),
        toggle("classic-menu", "windows", "Classic right-click menu", "Brings back the full Windows 10 right-click menu. Takes effect after Explorer restarts or you sign out.", false, true, vec![regkey("HKCU", CLASSIC, st(""), Val::Absent)]),
        toggle("widgets-off", "windows", "Widgets button off", "Hides the Widgets button on the taskbar.", false, false, vec![reg("HKCU", ADV, "TaskbarDa", dw(0), dw(1))]),
        toggle("chat-off", "windows", "Chat icon off", "Hides the Chat button on the taskbar.", false, false, vec![reg("HKCU", ADV, "TaskbarMn", dw(0), dw(1))]),
        toggle("taskbar-left", "windows", "Taskbar to the left", "Moves the Start button and icons to the left.", false, false, vec![reg("HKCU", ADV, "TaskbarAl", dw(0), dw(1))]),
        toggle("search-icon", "windows", "Search box as an icon", "Shrinks the taskbar search box to an icon.", false, false, vec![reg("HKCU", r"Software\Microsoft\Windows\CurrentVersion\Search", "SearchboxTaskbarMode", dw(1), dw(2))]),
        toggle("file-extensions", "windows", "Show file extensions", "Shows .exe, .pdf and so on after file names.", false, false, vec![reg("HKCU", ADV, "HideFileExt", dw(0), dw(1))]),
        toggle("hidden-files", "windows", "Show hidden files", "Shows files and folders marked hidden.", false, false, vec![reg("HKCU", ADV, "Hidden", dw(1), dw(2))]),
        toggle("onedrive-startup", "windows", "OneDrive does not start with Windows", "OneDrive stays installed and can be started by hand.", false, false, vec![approved_run("OneDrive")]),
        toggle("utc-clock", "windows", "Hardware clock in UTC", "Only for dual boot with Linux, so both systems agree on the time.", true, true, vec![reg("HKLM", r"SYSTEM\CurrentControlSet\Control\TimeZoneInformation", "RealTimeIsUniversal", dw(1), Val::Absent)]),
    ]
}

pub fn find(id: &str) -> Option<Tweak> {
    catalog().into_iter().find(|t| t.id == id)
}

// ---------- pure helpers ----------

pub enum RawKind {
    Dword,
    Sz,
    Bin,
    Other,
}

/// Turns the raw bytes of a registry value into a `Val`. A value of the wrong type or
/// size is `Other`, which we never overwrite.
pub fn parse_raw(kind: RawKind, bytes: &[u8]) -> Val {
    match kind {
        RawKind::Dword if bytes.len() == 4 => Val::Dword(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
        RawKind::Sz if bytes.len() % 2 == 0 => {
            let w: Vec<u16> = bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            Val::Str(String::from_utf16_lossy(&w).trim_end_matches('\0').to_string())
        }
        RawKind::Bin => Val::Bin(bytes.to_vec()),
        _ => Val::Other,
    }
}

/// Start type of a service in the words `sc config` uses.
pub fn service_start_name(start: u32, delayed: bool) -> &'static str {
    match start {
        0 => "boot",
        1 => "system",
        2 if delayed => "delayed-auto",
        2 => "auto",
        3 => "demand",
        4 => "disabled",
        _ => "unknown",
    }
}

pub fn is_guid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36 && b.iter().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

/// First GUID in a line of powercfg output, lowercased.
pub fn find_guid(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || c == '(' || c == ')').find(|w| is_guid(w)).map(|w| w.to_lowercase())
}

/// The items of a toggle. Nagle has one pair per network interface, found at run time.
fn items_of(b: &dyn Backend, tw: &Tweak) -> Vec<Item> {
    match &tw.kind {
        Kind::Toggle(items) => items.clone(),
        Kind::Nagle => {
            let mut v = Vec::new();
            for id in b.interfaces() {
                let path = format!(r"{NAGLE_IF}\{id}");
                v.push(reg("HKLM", &path, "TcpAckFrequency", dw(1), Val::Absent));
                v.push(reg("HKLM", &path, "TCPNoDelay", dw(1), Val::Absent));
            }
            v
        }
        Kind::Plan => Vec::new(),
    }
}

/// Same value? Startup-approved binaries only compare the enabled/disabled bit.
fn same(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Bin(x), Val::Bin(y)) => x.first().map(|v| v & 1) == y.first().map(|v| v & 1),
        _ => a == b,
    }
}

fn desired(b: &dyn Backend, tw: &Tweak, target: &str) -> Result<Vec<(Target, Val)>, String> {
    match &tw.kind {
        Kind::Toggle(_) | Kind::Nagle => {
            let items = items_of(b, tw);
            let on = match target {
                "on" => true,
                "off" => false,
                _ => return Err(format!("Unknown setting '{target}'.")),
            };
            if items.is_empty() {
                return Err("Not available on this PC.".into());
            }
            Ok(items.iter().map(|i| (i.target.clone(), if on { i.on.clone() } else { i.off.clone() })).collect())
        }
        Kind::Plan => {
            let p = PLANS.iter().find(|p| p.0 == target).ok_or_else(|| format!("Unknown power plan '{target}'."))?;
            Ok(vec![(Target::Power, Val::Str(p.2.to_string()))])
        }
    }
}

/// "on" | "off" for a toggle, a plan id or "other" for the power plan, or "unavailable".
pub fn state(b: &dyn Backend, tw: &Tweak) -> String {
    match &tw.kind {
        Kind::Plan => match b.read(&Target::Power) {
            Ok(Val::Str(g)) => PLANS.iter().find(|p| p.2 == g.to_lowercase()).map(|p| p.0.to_string()).unwrap_or_else(|| "other".into()),
            _ => "unavailable".into(),
        },
        Kind::Toggle(_) | Kind::Nagle => {
            let items = items_of(b, tw);
            if items.is_empty() {
                return "unavailable".into();
            }
            let mut all_on = true;
            for i in &items {
                match b.read(&i.target) {
                    Ok(Val::Absent) if matches!(i.target, Target::Service { .. }) => return "unavailable".into(),
                    Ok(v) => {
                        if !same(&v, &i.on) {
                            all_on = false;
                        }
                    }
                    Err(_) => return "unavailable".into(),
                }
            }
            if all_on { "on".into() } else { "off".into() }
        }
    }
}

// ---------- changing things ----------

type Persist<'a> = &'a mut dyn FnMut(&Journal) -> Result<(), String>;

/// Applies one tweak. Returns Ok(false) when nothing needed to change.
/// The old values are journaled and saved before the first write; if a write fails,
/// the earlier writes are put back and the journal entry is dropped.
pub fn apply(b: &dyn Backend, j: &mut Journal, tw: &Tweak, target: &str, now: u64, persist: Persist) -> Result<bool, String> {
    if tw.admin && !b.is_admin() {
        return Err(ADMIN_MSG.into());
    }
    let want = desired(b, tw, target)?;
    let mut changes: Vec<Change> = Vec::new();
    let mut writes: Vec<(Target, Val)> = Vec::new();
    for (t, v) in want {
        let cur = b.read(&t)?;
        if cur == Val::Other {
            return Err(format!("{}: this setting has an unexpected value, so it was left alone.", tw.name));
        }
        if matches!(t, Target::Service { .. }) && cur == Val::Absent {
            return Err(format!("{}: not available on this PC.", tw.name));
        }
        if same(&cur, &v) {
            continue;
        }
        changes.push(Change { target: t.clone(), old: cur });
        writes.push((t, v));
    }
    if changes.is_empty() {
        return Ok(false);
    }
    let seq = j.push(tw.id, now, changes.clone());
    if let Err(e) = persist(j) {
        j.remove(seq);
        return Err(e);
    }
    for (i, (t, v)) in writes.iter().enumerate() {
        if let Err(e) = b.write(t, v) {
            for c in changes[..i].iter().rev() {
                let _ = b.write(&c.target, &c.old);
            }
            j.remove(seq);
            let _ = persist(j);
            return Err(e);
        }
    }
    Ok(true)
}

/// Puts one entry's old values back, last change first.
fn replay(b: &dyn Backend, e: &Entry) -> Result<(), String> {
    for c in e.changes.iter().rev() {
        if c.old == Val::Other {
            continue;
        }
        b.write(&c.target, &c.old)?;
    }
    Ok(())
}

/// Undoes everything we changed for one tweak, newest change first.
pub fn undo_tweak(b: &dyn Backend, j: &mut Journal, tw: &Tweak, persist: Persist) -> Result<(), String> {
    if tw.admin && !b.is_admin() {
        return Err(ADMIN_MSG.into());
    }
    for e in j.for_tweak(tw.id) {
        replay(b, &e)?;
        j.remove(e.seq);
        persist(j)?;
    }
    Ok(())
}

/// Replays the whole journal, newest first. Entries that fail stay in the journal and are
/// reported in the returned notes.
pub fn restore_all(b: &dyn Backend, j: &mut Journal, persist: Persist) -> Result<Vec<String>, String> {
    let mut notes = Vec::new();
    for e in j.newest_first() {
        let name = find(&e.tweak).map(|t| t.name).unwrap_or("A setting");
        match replay(b, &e) {
            Ok(()) => {
                j.remove(e.seq);
                persist(j)?;
            }
            Err(err) => notes.push(format!("{name}: {err}")),
        }
    }
    if notes.is_empty() {
        j.last_profile = None;
    }
    persist(j)?;
    Ok(notes)
}

// ---------- profiles ----------

pub struct ProfileDef {
    pub id: &'static str,
    pub name: &'static str,
    pub blurb: &'static str,
    /// (tweak id, target). Anything not listed is left as it is.
    pub items: &'static [(&'static str, &'static str)],
}

pub const PROFILES: &[ProfileDef] = &[
    ProfileDef {
        id: "gaming",
        name: "Gaming",
        blurb: "Most speed for games: faster power plan, Game Mode, less in the background.",
        items: &[("power-plan", "high"), ("game-mode", "on"), ("gamedvr", "on"), ("visual-fx", "on"), ("background-apps", "on"), ("mouse-accel", "on"), ("advertising-id", "on"), ("tips-ads", "on"), ("bing", "on"), ("copilot", "on"), ("net-throttling", "on"), ("nagle", "on"), ("menu-delay", "on")],
    },
    ProfileDef {
        id: "work",
        name: "Work",
        blurb: "Steady and quiet for office work: normal power, search kept, fewer distractions.",
        items: &[("power-plan", "balanced"), ("gamedvr", "on"), ("tips-ads", "on"), ("bing", "on"), ("copilot", "on"), ("background-apps", "off"), ("search-indexing", "off"), ("visual-fx", "off"), ("office-telemetry", "on"), ("edge-telemetry", "on"), ("chrome-telemetry", "on"), ("firefox-telemetry", "on")],
    },
    ProfileDef {
        id: "balanced",
        name: "Balanced",
        blurb: "A light touch: normal power, Game Mode on, no ads in Start. Nothing else changes.",
        items: &[("power-plan", "balanced"), ("game-mode", "on"), ("tips-ads", "on"), ("file-extensions", "on"), ("feedback-prompts", "on")],
    },
];

pub fn profile(id: &str) -> Option<&'static ProfileDef> {
    PROFILES.iter().find(|p| p.id == id)
}

fn label(tw: &Tweak, v: &str) -> String {
    match &tw.kind {
        Kind::Plan => match v {
            "other" => "Custom plan".into(),
            "unavailable" => "Unavailable".into(),
            _ => PLANS.iter().find(|p| p.0 == v).map(|p| p.1.to_string()).unwrap_or_else(|| v.to_string()),
        },
        Kind::Toggle(_) | Kind::Nagle => match v {
            "on" => "Applied".into(),
            "off" => "Not applied".into(),
            _ => "Unavailable".into(),
        },
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct PlanItem {
    pub id: String,
    pub name: String,
    pub from: String,
    pub to: String,
    pub admin: bool,
    /// Why this one will not be changed (None = it will be).
    pub skip: Option<String>,
}

pub fn plan_profile(b: &dyn Backend, profile_id: &str) -> Result<Vec<PlanItem>, String> {
    let p = profile(profile_id).ok_or("Unknown profile.")?;
    let admin = b.is_admin();
    let mut out = Vec::new();
    for (id, target) in p.items {
        let tw = find(id).ok_or_else(|| format!("Unknown tweak '{id}'."))?;
        let cur = state(b, &tw);
        let skip = if tw.admin && !admin {
            Some("Needs administrator".to_string())
        } else if cur == "unavailable" {
            Some("Not available on this PC".to_string())
        } else if cur == *target {
            Some("Already set".to_string())
        } else {
            None
        };
        out.push(PlanItem { id: id.to_string(), name: tw.name.to_string(), from: label(&tw, &cur), to: label(&tw, target), admin: tw.admin, skip });
    }
    Ok(out)
}

/// Applies the switches of a profile that can be changed. Returns notes for any that failed.
pub fn apply_profile(b: &dyn Backend, j: &mut Journal, profile_id: &str, now: u64, persist: Persist) -> Result<Vec<String>, String> {
    let p = profile(profile_id).ok_or("Unknown profile.")?;
    let plan = plan_profile(b, profile_id)?;
    let mut notes = Vec::new();
    for (item, (id, target)) in plan.iter().zip(p.items.iter()) {
        if item.skip.is_some() {
            continue;
        }
        let tw = find(id).ok_or_else(|| format!("Unknown tweak '{id}'."))?;
        if let Err(e) = apply(b, j, &tw, target, now, persist) {
            notes.push(format!("{}: {}", tw.name, e));
        }
    }
    j.last_profile = Some(LastProfile { id: p.id.to_string(), name: p.name.to_string(), at: now });
    persist(j)?;
    Ok(notes)
}

// ---------- what the UI gets ----------

#[derive(Serialize, Clone, Debug)]
pub struct Opt {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct TweakView {
    pub id: String,
    pub group: String,
    pub name: String,
    pub desc: String,
    pub admin: bool,
    pub restart: bool,
    /// "toggle" | "choice"
    pub kind: String,
    pub state: String,
    pub options: Vec<Opt>,
    /// Changed by us, so it can be undone.
    pub changed: bool,
}

pub fn views(b: &dyn Backend, j: &Journal) -> Vec<TweakView> {
    catalog()
        .iter()
        .map(|t| TweakView {
            id: t.id.into(),
            group: t.group.into(),
            name: t.name.into(),
            desc: if t.id == "hibernate-off" { hiber_desc(t.desc) } else { t.desc.into() },
            admin: t.admin,
            restart: t.restart,
            kind: if matches!(t.kind, Kind::Plan) { "choice".into() } else { "toggle".into() },
            state: state(b, t),
            options: if matches!(t.kind, Kind::Plan) { PLANS.iter().map(|p| Opt { id: p.0.into(), name: p.1.into() }).collect() } else { Vec::new() },
            changed: j.has(t.id),
        })
        .collect()
}

/// "1.5 GB" style size.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

fn hiber_desc(base: &str) -> String {
    match std::fs::metadata(r"C:\hiberfil.sys") {
        Ok(m) if m.len() > 0 => format!("{base} hiberfil.sys is {} now.", format_size(m.len())),
        _ => base.to_string(),
    }
}

/// Value of one line of `bcdedit /enum {current}` output, e.g. "useplatformclock   Yes".
/// Absent when the line is missing. "yes"/"no" come back as "Yes"/"No".
pub fn parse_bcd(text: &str, name: &str) -> Val {
    for line in text.lines() {
        let mut it = line.trim().splitn(2, char::is_whitespace);
        if it.next().map(|k| k.eq_ignore_ascii_case(name)) == Some(true) {
            let v = it.next().unwrap_or("").trim();
            return match v.to_lowercase().as_str() {
                "yes" => Val::Str("Yes".into()),
                "no" => Val::Str("No".into()),
                _ => Val::Str(v.to_string()),
            };
        }
    }
    Val::Absent
}

// ---------- the real system ----------

pub struct Sys;

impl Backend for Sys {
    fn read(&self, t: &Target) -> Result<Val, String> {
        sys_read(t)
    }
    fn write(&self, t: &Target, v: &Val) -> Result<(), String> {
        sys_write(t, v)
    }
    fn is_admin(&self) -> bool {
        sys_admin()
    }
    fn interfaces(&self) -> Vec<String> {
        sys_interfaces()
    }
}

#[cfg(not(windows))]
fn sys_interfaces() -> Vec<String> {
    Vec::new()
}
#[cfg(windows)]
fn sys_interfaces() -> Vec<String> {
    win::interfaces()
}

#[cfg(not(windows))]
fn sys_read(_t: &Target) -> Result<Val, String> {
    Err(WIN_ONLY.into())
}
#[cfg(not(windows))]
fn sys_write(_t: &Target, _v: &Val) -> Result<(), String> {
    Err(WIN_ONLY.into())
}
#[cfg(not(windows))]
fn sys_admin() -> bool {
    false
}
#[cfg(windows)]
fn sys_read(t: &Target) -> Result<Val, String> {
    win::read(t)
}
#[cfg(windows)]
fn sys_write(t: &Target, v: &Val) -> Result<(), String> {
    win::write(t, v)
}
#[cfg(windows)]
fn sys_admin() -> bool {
    win::is_admin()
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::io::ErrorKind;
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, KEY_WRITE, REG_BINARY, REG_DWORD, REG_SZ};
    use winreg::{RegKey, RegValue};

    fn hive(name: &str) -> RegKey {
        RegKey::predef(if name == "HKLM" { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER })
    }

    fn io_err(e: std::io::Error) -> String {
        if e.kind() == ErrorKind::PermissionDenied {
            ADMIN_MSG.to_string()
        } else {
            e.to_string()
        }
    }

    pub fn is_admin() -> bool {
        hive("HKLM").open_subkey_with_flags("SOFTWARE", KEY_SET_VALUE).is_ok()
    }

    fn reg_read(h: &str, path: &str, name: &str) -> Result<Val, String> {
        let key = match hive(h).open_subkey_with_flags(path, KEY_READ) {
            Ok(k) => k,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Val::Absent),
            Err(e) => return Err(e.to_string()),
        };
        match key.get_raw_value(name) {
            Ok(v) => {
                let kind = if v.vtype == REG_DWORD {
                    RawKind::Dword
                } else if v.vtype == REG_SZ {
                    RawKind::Sz
                } else if v.vtype == REG_BINARY {
                    RawKind::Bin
                } else {
                    RawKind::Other
                };
                Ok(parse_raw(kind, &v.bytes))
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Val::Absent),
            Err(e) => Err(e.to_string()),
        }
    }

    fn reg_write(h: &str, path: &str, name: &str, v: &Val) -> Result<(), String> {
        match v {
            Val::Absent => {
                let key = match hive(h).open_subkey_with_flags(path, KEY_SET_VALUE) {
                    Ok(k) => k,
                    Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
                    Err(e) => return Err(io_err(e)),
                };
                match key.delete_value(name) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(io_err(e)),
                }
            }
            Val::Dword(n) => {
                let (key, _) = hive(h).create_subkey_with_flags(path, KEY_WRITE).map_err(io_err)?;
                key.set_value(name, n).map_err(io_err)
            }
            Val::Str(s) => {
                let (key, _) = hive(h).create_subkey_with_flags(path, KEY_WRITE).map_err(io_err)?;
                let s: String = s.clone();
                key.set_value(name, &s).map_err(io_err)
            }
            Val::Bin(b) => {
                let (key, _) = hive(h).create_subkey_with_flags(path, KEY_WRITE).map_err(io_err)?;
                key.set_raw_value(name, &RegValue { bytes: b.clone().into(), vtype: REG_BINARY }).map_err(io_err)
            }
            Val::Other => Err("Cannot write this kind of value.".into()),
        }
    }

    fn key_read(h: &str, path: &str) -> Result<Val, String> {
        match hive(h).open_subkey_with_flags(path, KEY_READ) {
            Ok(k) => Ok(Val::Str(k.get_value::<String, _>("").unwrap_or_default())),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Val::Absent),
            Err(e) => Err(e.to_string()),
        }
    }

    fn key_write(h: &str, path: &str, v: &Val) -> Result<(), String> {
        match v {
            Val::Absent => {
                // Only the key we made is removed; its parent goes too if that leaves it empty.
                match hive(h).delete_subkey(path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
                    Err(e) => return Err(io_err(e)),
                }
                if let Some((parent, _)) = path.rsplit_once('\\') {
                    let _ = hive(h).delete_subkey(parent);
                }
                Ok(())
            }
            Val::Str(s) => {
                let (key, _) = hive(h).create_subkey_with_flags(path, KEY_WRITE).map_err(io_err)?;
                key.set_value("", &s.clone()).map_err(io_err)
            }
            _ => Err("Cannot write this kind of value.".into()),
        }
    }

    pub fn interfaces() -> Vec<String> {
        hive("HKLM").open_subkey_with_flags(NAGLE_IF, KEY_READ).map(|k| k.enum_keys().filter_map(|n| n.ok()).collect()).unwrap_or_default()
    }

    fn hibernate_read() -> Result<Val, String> {
        match reg_read("HKLM", r"SYSTEM\CurrentControlSet\Control\Power", "HibernateEnabled")? {
            Val::Dword(0) => Ok(Val::Str("off".into())),
            Val::Dword(_) => Ok(Val::Str("on".into())),
            Val::Absent => Ok(Val::Str("off".into())),
            _ => Ok(Val::Other),
        }
    }

    fn run(program: &str, args: &[&str]) -> Result<String, String> {
        let out = Command::new(program).args(args).creation_flags(0x0800_0000).output().map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        if out.status.success() {
            Ok(text)
        } else if text.to_lowercase().contains("access is denied") || String::from_utf8_lossy(&out.stderr).to_lowercase().contains("access is denied") {
            Err(ADMIN_MSG.into())
        } else {
            Err(format!("{program} failed: {}", text.trim()))
        }
    }

    fn service_read(name: &str) -> Result<Val, String> {
        let path = format!(r"SYSTEM\CurrentControlSet\Services\{name}");
        let key = match hive("HKLM").open_subkey_with_flags(&path, KEY_READ) {
            Ok(k) => k,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Val::Absent),
            Err(e) => return Err(e.to_string()),
        };
        let start: u32 = match key.get_value("Start") {
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Val::Absent),
            Err(e) => return Err(e.to_string()),
        };
        let delayed: u32 = key.get_value("DelayedAutostart").unwrap_or(0);
        Ok(Val::Str(service_start_name(start, delayed != 0).to_string()))
    }

    fn service_write(name: &str, v: &Val) -> Result<(), String> {
        let Val::Str(mode) = v else { return Err("Cannot set this service.".into()) };
        if !["auto", "delayed-auto", "demand", "disabled"].contains(&mode.as_str()) {
            return Err(format!("Cannot set a service to '{mode}'."));
        }
        // Only the start type is changed. The service is never stopped.
        run("sc", &["config", name, "start=", mode]).map(|_| ())
    }

    fn power_ensure(guid: &str) -> Result<(), String> {
        let list = run("powercfg", &["/list"])?;
        if list.to_lowercase().contains(guid) {
            return Ok(());
        }
        run("powercfg", &["/duplicatescheme", guid, guid]).map(|_| ())
    }

    pub fn read(t: &Target) -> Result<Val, String> {
        match t {
            Target::Reg { hive, path, name } => reg_read(hive, path, name),
            Target::Service { name } => service_read(name),
            Target::Power => {
                let out = run("powercfg", &["/getactivescheme"])?;
                find_guid(&out).map(Val::Str).ok_or_else(|| "Could not read the power plan.".to_string())
            }
            Target::RegKey { hive, path } => key_read(hive, path),
            Target::Bcd { name } => {
                let out = run("bcdedit", &["/enum", "{current}"])?;
                Ok(parse_bcd(&out, name))
            }
            Target::Hibernate => hibernate_read(),
            Target::Dns { adapter } => crate::optimize::net::dns_read(adapter),
        }
    }

    pub fn write(t: &Target, v: &Val) -> Result<(), String> {
        match t {
            Target::Reg { hive, path, name } => reg_write(hive, path, name, v),
            Target::Service { name } => service_write(name, v),
            Target::Power => {
                let Val::Str(g) = v else { return Err("Cannot set the power plan.".into()) };
                if !is_guid(g) {
                    return Err("Not a power plan.".into());
                }
                let g = g.to_lowercase();
                power_ensure(&g)?;
                run("powercfg", &["/setactive", &g]).map(|_| ())
            }
            Target::RegKey { hive, path } => key_write(hive, path, v),
            Target::Bcd { name } => match v {
                Val::Absent => run("bcdedit", &["/deletevalue", name]).map(|_| ()),
                Val::Str(s) if s == "Yes" || s == "No" => run("bcdedit", &["/set", name, s]).map(|_| ()),
                _ => Err("Cannot set this boot option.".into()),
            },
            Target::Hibernate => match v {
                Val::Str(s) if s == "on" || s == "off" => run("powercfg", &["/hibernate", s]).map(|_| ()),
                _ => Err("Cannot set hibernation.".into()),
            },
            Target::Dns { adapter } => crate::optimize::net::dns_write(adapter, v),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    struct Mock {
        vals: RefCell<HashMap<String, Val>>,
        log: RefCell<Vec<String>>,
        admin: bool,
        fail_on: RefCell<Option<String>>,
    }

    fn key(t: &Target) -> String {
        format!("{:?}", t)
    }

    impl Mock {
        fn new(admin: bool) -> Mock {
            Mock { vals: RefCell::new(HashMap::new()), log: RefCell::new(Vec::new()), admin, fail_on: RefCell::new(None) }
        }
        fn set(&self, t: &Target, v: Val) {
            self.vals.borrow_mut().insert(key(t), v);
        }
        fn get(&self, t: &Target) -> Val {
            self.vals.borrow().get(&key(t)).cloned().unwrap_or(Val::Absent)
        }
    }

    impl Backend for Mock {
        fn read(&self, t: &Target) -> Result<Val, String> {
            Ok(self.get(t))
        }
        fn write(&self, t: &Target, v: &Val) -> Result<(), String> {
            if self.fail_on.borrow().as_deref() == Some(key(t).as_str()) {
                return Err("boom".into());
            }
            self.log.borrow_mut().push(key(t));
            self.vals.borrow_mut().insert(key(t), v.clone());
            Ok(())
        }
        fn is_admin(&self) -> bool {
            self.admin
        }
        fn interfaces(&self) -> Vec<String> {
            vec!["{aaa}".into(), "{bbb}".into()]
        }
    }

    fn ok_persist() -> impl FnMut(&Journal) -> Result<(), String> {
        |_| Ok(())
    }

    fn first_target(tw: &Tweak, i: usize) -> Target {
        match &tw.kind {
            Kind::Toggle(items) => items[i].target.clone(),
            Kind::Plan => Target::Power,
            Kind::Nagle => Target::Power,
        }
    }

    #[test]
    fn bcd_parser() {
        let t = "Windows Boot Loader\n-------\nidentifier  {current}\nuseplatformclock        yes\ndisabledynamictick      No\n";
        assert_eq!(parse_bcd(t, "useplatformclock"), Val::Str("Yes".into()));
        assert_eq!(parse_bcd(t, "disabledynamictick"), Val::Str("No".into()));
        assert_eq!(parse_bcd(t, "tscsyncpolicy"), Val::Absent);
    }

    #[test]
    fn sizes() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536 * 1024 * 1024), "1.5 GB");
    }

    #[test]
    fn ids_unique() {
        let mut ids: Vec<&str> = catalog().iter().map(|t| t.id).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }

    #[test]
    fn nagle_has_item_per_interface_and_undoes() {
        let b = Mock::new(true);
        let tw = find("nagle").unwrap();
        assert_eq!(state(&b, &tw), "off");
        let mut j = Journal::default();
        assert!(apply(&b, &mut j, &tw, "on", 1, &mut ok_persist()).unwrap());
        assert_eq!(j.entries[0].changes.len(), 4);
        assert_eq!(state(&b, &tw), "on");
        undo_tweak(&b, &mut j, &tw, &mut ok_persist()).unwrap();
        assert_eq!(state(&b, &tw), "off");
    }

    #[test]
    fn startup_approved_compares_enabled_bit_only() {
        assert!(same(&bin(3), &Val::Bin(vec![3, 0, 0, 0, 9, 9, 9, 9, 9, 9, 9, 9])));
        assert!(!same(&bin(3), &bin(2)));
        assert!(same(&bin(2), &bin(6)));
    }

    #[test]
    fn every_profile_references_existing_tweaks() {
        let ids: Vec<&str> = catalog().iter().map(|t| t.id).collect();
        let mut uniq = ids.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), ids.len(), "duplicate tweak id");
        for p in PROFILES {
            assert!(!p.items.is_empty());
            for (id, target) in p.items {
                let tw = find(id).unwrap_or_else(|| panic!("profile {} uses unknown tweak {}", p.id, id));
                assert!(desired(&Mock::new(true), &tw, target).is_ok(), "profile {} has a bad target {} for {}", p.id, target, id);
            }
        }
        assert!(profile("gaming").is_some() && profile("work").is_some() && profile("balanced").is_some());
    }

    #[test]
    fn plans_are_guids() {
        for p in PLANS {
            assert!(is_guid(p.2));
        }
    }

    #[test]
    fn parse_raw_present_absent_wrong_type() {
        assert_eq!(parse_raw(RawKind::Dword, &[2, 0, 0, 0]), Val::Dword(2));
        assert_eq!(parse_raw(RawKind::Dword, &[1, 2]), Val::Other);
        assert_eq!(parse_raw(RawKind::Other, &[2, 0, 0, 0]), Val::Other);
        let s: Vec<u8> = "10\0".encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        assert_eq!(parse_raw(RawKind::Sz, &s), Val::Str("10".into()));
        assert_eq!(parse_raw(RawKind::Sz, &[1, 2, 3]), Val::Other);
        // absent is decided by the backend and compares equal to nothing else
        assert_ne!(Val::Absent, Val::Dword(0));
    }

    #[test]
    fn wrong_type_value_is_left_alone() {
        let m = Mock::new(false);
        let tw = find("game-mode").unwrap();
        m.set(&first_target(&tw, 0), Val::Other);
        let mut j = Journal::default();
        assert!(apply(&m, &mut j, &tw, "on", 1, &mut ok_persist()).is_err());
        assert!(m.log.borrow().is_empty());
        assert!(j.entries.is_empty());
    }

    #[test]
    fn guid_and_service_helpers() {
        let out = "Power Scheme GUID: 381B4222-F694-41F0-9685-FF5BB260DF2E  (Balanced)";
        assert_eq!(find_guid(out).as_deref(), Some("381b4222-f694-41f0-9685-ff5bb260df2e"));
        assert_eq!(find_guid("nothing here"), None);
        assert_eq!(service_start_name(2, false), "auto");
        assert_eq!(service_start_name(2, true), "delayed-auto");
        assert_eq!(service_start_name(3, false), "demand");
        assert_eq!(service_start_name(4, false), "disabled");
    }

    #[test]
    fn state_reads() {
        let m = Mock::new(false);
        let tw = find("game-mode").unwrap();
        assert_eq!(state(&m, &tw), "off");
        m.set(&first_target(&tw, 0), Val::Dword(1));
        assert_eq!(state(&m, &tw), "on");
        // a service that does not exist is unavailable
        assert_eq!(state(&m, &find("sysmain").unwrap()), "unavailable");
        let plan = find("power-plan").unwrap();
        m.set(&Target::Power, Val::Str(PLANS[1].2.into()));
        assert_eq!(state(&m, &plan), "high");
        m.set(&Target::Power, Val::Str("11111111-1111-1111-1111-111111111111".into()));
        assert_eq!(state(&m, &plan), "other");
    }

    #[test]
    fn apply_journals_then_undo_restores() {
        let m = Mock::new(false);
        let tw = find("gamedvr").unwrap();
        m.set(&first_target(&tw, 0), Val::Dword(1));
        // second value is absent on purpose
        let mut j = Journal::default();
        assert_eq!(apply(&m, &mut j, &tw, "on", 5, &mut ok_persist()), Ok(true));
        assert_eq!(m.get(&first_target(&tw, 0)), Val::Dword(0));
        assert_eq!(m.get(&first_target(&tw, 1)), Val::Dword(0));
        assert!(j.has("gamedvr"));
        // applying again changes nothing and adds no entry
        assert_eq!(apply(&m, &mut j, &tw, "on", 6, &mut ok_persist()), Ok(false));
        assert_eq!(j.entries.len(), 1);
        undo_tweak(&m, &mut j, &tw, &mut ok_persist()).unwrap();
        assert_eq!(m.get(&first_target(&tw, 0)), Val::Dword(1));
        assert_eq!(m.get(&first_target(&tw, 1)), Val::Absent);
        assert!(!j.has("gamedvr"));
    }

    #[test]
    fn failed_write_leaves_nothing_behind() {
        let m = Mock::new(false);
        let tw = find("gamedvr").unwrap();
        m.set(&first_target(&tw, 0), Val::Dword(1));
        m.set(&first_target(&tw, 1), Val::Dword(1));
        *m.fail_on.borrow_mut() = Some(key(&first_target(&tw, 1)));
        let mut j = Journal::default();
        assert!(apply(&m, &mut j, &tw, "on", 1, &mut ok_persist()).is_err());
        assert_eq!(m.get(&first_target(&tw, 0)), Val::Dword(1));
        assert_eq!(m.get(&first_target(&tw, 1)), Val::Dword(1));
        assert!(j.entries.is_empty());
    }

    #[test]
    fn failed_journal_save_writes_nothing() {
        let m = Mock::new(false);
        let tw = find("game-mode").unwrap();
        let mut j = Journal::default();
        let mut bad = |_: &Journal| -> Result<(), String> { Err("disk full".into()) };
        assert!(apply(&m, &mut j, &tw, "on", 1, &mut bad).is_err());
        assert!(m.log.borrow().is_empty());
        assert!(j.entries.is_empty());
    }

    #[test]
    fn admin_tweaks_refused_without_admin() {
        let m = Mock::new(false);
        let tw = find("hw-gpu-scheduling").unwrap();
        let mut j = Journal::default();
        assert_eq!(apply(&m, &mut j, &tw, "on", 1, &mut ok_persist()), Err(ADMIN_MSG.to_string()));
        assert!(m.log.borrow().is_empty());
        let plan = plan_profile(&m, "gaming").unwrap();
        assert!(plan.iter().filter(|p| p.admin).all(|p| p.skip.is_some()));
        let work = plan_profile(&m, "work").unwrap();
        let idx = work.iter().find(|p| p.id == "search-indexing").unwrap();
        assert_eq!(idx.skip.as_deref(), Some("Needs administrator"));
    }

    #[test]
    fn restore_all_goes_newest_first() {
        let m = Mock::new(false);
        let a = find("game-mode").unwrap();
        let b = find("gamedvr").unwrap();
        let mut j = Journal::default();
        apply(&m, &mut j, &a, "on", 1, &mut ok_persist()).unwrap();
        apply(&m, &mut j, &b, "on", 2, &mut ok_persist()).unwrap();
        m.log.borrow_mut().clear();
        let notes = restore_all(&m, &mut j, &mut ok_persist()).unwrap();
        assert!(notes.is_empty());
        let log = m.log.borrow().clone();
        // b changed two values (written in order 0,1); undoing replays them 1,0, and b comes before a
        assert_eq!(log, vec![key(&first_target(&b, 1)), key(&first_target(&b, 0)), key(&first_target(&a, 0))]);
        assert!(j.entries.is_empty() && j.last_profile.is_none());
        assert_eq!(m.get(&first_target(&a, 0)), Val::Absent);
    }

    #[test]
    fn apply_profile_changes_and_records_it() {
        let m = Mock::new(false);
        m.set(&Target::Power, Val::Str(PLANS[0].2.into()));
        let mut j = Journal::default();
        let notes = apply_profile(&m, &mut j, "gaming", 99, &mut ok_persist()).unwrap();
        assert!(notes.is_empty());
        assert_eq!(m.get(&Target::Power), Val::Str(PLANS[1].2.into()));
        assert_eq!(j.last_profile.as_ref().map(|p| (p.id.as_str(), p.at)), Some(("gaming", 99)));
        restore_all(&m, &mut j, &mut ok_persist()).unwrap();
        assert_eq!(m.get(&Target::Power), Val::Str(PLANS[0].2.into()));
    }
}

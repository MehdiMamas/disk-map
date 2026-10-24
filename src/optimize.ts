import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";

interface Action { kind: string; label: string; arg: string }
interface Card { id: string; title: string; value: string; level: string; detail: string; action: Action | null }
interface Suggestion { id: string; severity: string; title: string; detail: string; action: Action | null }
interface StartupItem { id: string; name: string; command: string; source: string; enabled: boolean; can_toggle: boolean; reason: string | null; note: string | null }
interface Overview { cards: Card[]; suggestions: Suggestion[]; startup: StartupItem[] }
interface InstalledApp { id: string; name: string; publisher: string; version: string; size: number | null; updated: string | null; can_uninstall: boolean; tag: string | null }
interface ProcGroup { name: string; cpu: number; mem: number; count: number }
interface ProcReport { by_cpu: ProcGroup[]; by_mem: ProcGroup[] }
interface Tweak { id: string; group: string; name: string; desc: string; admin: boolean; restart: boolean; kind: string; state: string; options: { id: string; name: string }[]; changed: boolean; relaunch: boolean }
interface ProfileInfo { id: string; name: string; blurb: string }
interface TweaksReport { windows: boolean; mac: boolean; admin: boolean; tweaks: Tweak[]; profiles: ProfileInfo[]; last_profile: { name: string; at: number } | null; changed_count: number; notes: string[] }
interface PlanItem { id: string; name: string; from: string; to: string; admin: boolean; skip: string | null }

export interface OptimizeHooks {
  isMac: boolean;
  fmt: (n: number) => string;
  /** macOS only: opens the existing installed-apps panel. */
  openMacApps: () => void;
}

const ADMIN_TIP = "Restart Disk Map as administrator to change this";
const SECTIONS: [string, string][] = [
  ["overview", "Overview"], ["profiles", "Profiles"], ["performance", "Performance"], ["privacy", "Privacy"], ["windows", "Windows"],
  ["startup", "Startup"], ["apps", "Apps"], ["processes", "Processes"], ["network", "Network"], ["hardware", "Hardware"],
];

const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]!));
const when = (t: number) => new Date(t * 1000).toLocaleString();

let hooks: OptimizeHooks;
let root: HTMLElement;
let overview: Overview | null = null;
let installed: InstalledApp[] | null = null;
let procs: ProcReport | null = null;
let tw: TweaksReport | null = null;
let section = "overview";
let loaded = false;
let loading = false;
let filter = "";
let msg = "";

interface NetAdapter { name: string; dhcp: boolean; servers: string[]; changed: boolean }
interface NetReport { windows: boolean; mac: boolean; admin: boolean; adapters: NetAdapter[] }
interface PingResult { sent: number | null; received: number | null; loss_pct: number | null; min_ms: number | null; max_ms: number | null; avg_ms: number | null; raw: string }
interface Hardware {
  windows: boolean; mac: boolean; cpu: string; physical_cores: number | null; logical_cores: number; base_mhz: number | null;
  ram_total: number; ram_speed_mhz: number | null; gpus: { name: string; driver: string }[];
  disks: { name: string; kind: string; size: number }[]; volumes: { mount: string; total: number; free: number }[];
  os: string; uptime_secs: number;
}
const PROVIDERS: [string, string, string][] = [
  ["cloudflare", "Cloudflare", "1.1.1.1,1.0.0.1"], ["google", "Google", "8.8.8.8,8.8.4.4"],
  ["quad9", "Quad9", "9.9.9.9,149.112.112.112"], ["auto", "Automatic (DHCP)", ""],
];
let net: NetReport | null = null;
let netBusy = false;
let host = "";
let pinging = false;
let pingRes: PingResult | null = null;
let pingErr = "";
let hw: Hardware | null = null;
let hwBusy = false;
let flushNote = "";
interface RestoreResult { status: string; time: number | null; reason: string | null }
let restoreOk = false; // a restore point exists or the user chose to go on without one (this run)
let restoring = false;
let restoreNote = "";
let loginItems: { items: string[]; note: string } | null = null;
let loginBusy = false;
let relaunchNote = "";

const sectionLabel = (id: string, label: string) => (id === "windows" && hooks.isMac ? "Interface" : label);

/** Makes a restore point (Windows) or a Time Machine snapshot (Mac) before the first change of this run. Returns false if the user cancelled. */
async function beforeChange(): Promise<boolean> {
  if (restoreOk) return true;
  restoring = true;
  render();
  let r: RestoreResult;
  try { r = await invoke<RestoreResult>("ensure_restore_point"); }
  catch (e) { r = { status: "failed", time: null, reason: String(e) }; }
  restoring = false;
  if (r.status === "created" || r.status === "already_recent") {
    restoreOk = true;
    const t = r.time ? when(r.time) : "";
    restoreNote = hooks.isMac
      ? (r.status === "created" ? `Snapshot created ${t}` : `Using snapshot from ${t}`)
      : (r.status === "created" ? `Restore point created ${t}` : `Using restore point from ${t}`);
    render();
    return true;
  }
  if (r.status === "unsupported") { restoreOk = true; render(); return true; }
  const go = await askNoRestore(r.reason ?? (hooks.isMac ? "macOS did not create a Time Machine snapshot." : "Windows did not create a restore point."));
  if (go) restoreOk = true;
  render();
  return go;
}

function askNoRestore(reason: string): Promise<boolean> {
  return new Promise((resolve) => {
    const ov = document.createElement("div");
    ov.className = "overlay";
    const mac = hooks.isMac;
    ov.innerHTML = `<div class="card"><h2>${mac ? "No snapshot" : "No restore point"}</h2><p class="rp-reason"></p>
      <small>${mac ? "A Time Machine snapshot keeps a copy of your files from before the change." : "A restore point lets Windows roll the whole computer back."} Disk Map also keeps its own Undo for every change.</small>
      <div class="row"><button class="rp-cancel">Cancel</button><button class="primary rp-go">${mac ? "Continue without snapshot" : "Continue without restore point"}</button></div></div>`;
    ov.querySelector<HTMLElement>(".rp-reason")!.textContent = reason;
    document.body.appendChild(ov);
    const done = (v: boolean) => { ov.remove(); resolve(v); };
    ov.querySelector<HTMLElement>(".rp-cancel")!.addEventListener("click", () => done(false));
    ov.querySelector<HTMLElement>(".rp-go")!.addEventListener("click", () => done(true));
  });
}

export function initOptimize(el: HTMLElement, h: OptimizeHooks) {
  root = el;
  hooks = h;
}

/** Called each time the tab is shown. Loads once; later visits reuse what is already there. */
export function showOptimize() {
  if (!loaded && !loading) void loadAll();
  else render();
}

async function loadAll() {
  loading = true;
  render();
  const [o, i, p, t] = await Promise.allSettled([
    invoke<Overview>("optimize_overview"),
    hooks.isMac ? Promise.resolve([] as InstalledApp[]) : invoke<InstalledApp[]>("optimize_installed"),
    invoke<ProcReport>("optimize_processes"),
    invoke<TweaksReport>("optimize_tweaks"),
  ]);
  const errs: string[] = [];
  if (o.status === "fulfilled") overview = o.value; else errs.push(String(o.reason));
  if (i.status === "fulfilled") installed = i.value; else errs.push(String(i.reason));
  if (p.status === "fulfilled") procs = p.value; else errs.push(String(p.reason));
  if (t.status === "fulfilled") tw = t.value; else errs.push(String(t.reason));
  msg = errs.join(" ");
  loaded = true;
  loading = false;
  render();
}

function refreshing() {
  overview = null; installed = null; procs = null; tw = null; loaded = false; net = null; hw = null; loginItems = null; relaunchNote = "";
  void loadAll();
}

function actionBtn(a: Action | null, extra = ""): string {
  return a ? `<button data-k="${esc(a.kind)}" data-a="${esc(a.arg)}"${extra}>${esc(a.label)}</button>` : "";
}

function winOnly(): string {
  return `<p class="opt-none">Windows only for now.</p>`;
}

function noWindows(): boolean {
  return tw !== null && !tw.windows && !tw.mac;
}

function overviewHtml(): string {
  const o = overview;
  if (!o) return "";
  const cards = o.cards.map((c) => `<div class="opt-card ${esc(c.level)}"><small>${esc(c.title)}</small><b>${esc(c.value)}</b><span>${esc(c.detail)}</span>${actionBtn(c.action)}</div>`).join("");
  const top = o.suggestions.slice(0, 3);
  const sugg = top.length
    ? top.map((s) => `<li class="opt-sug ${esc(s.severity)}"><div class="opt-main"><b>${esc(s.title)}</b><br><small>${esc(s.detail)}</small></div>${actionBtn(s.action)}</li>`).join("")
    : `<li class="opt-none">Nothing to suggest right now.</li>`;
  return `<div class="opt-cards compact">${cards}</div><h3>Top suggestions</h3><ul class="opt-list">${sugg}</ul>`;
}

function profilesHtml(): string {
  if (noWindows()) return winOnly();
  if (!tw) return "";
  const cards = tw.profiles.map((p) => `<div class="opt-card prof"><b>${esc(p.name)}</b><span>${esc(p.blurb)}</span><button data-prof="${esc(p.id)}">Apply…</button></div>`).join("");
  const last = tw.last_profile ? `Last profile applied: ${esc(tw.last_profile.name)}, ${esc(when(tw.last_profile.at))}.` : "No profile applied yet.";
  return `<div class="opt-cards">${cards}</div>
    <div class="opt-bar"><button id="opt-restore"${tw.changed_count ? "" : " disabled"}>Restore all changes</button><small>${last}</small></div>
    ${tw.admin ? "" : `<p class="opt-none">Not running as administrator: switches marked with a shield are skipped.</p>`}`;
}

function tweakRow(t: Tweak): string {
  const locked = t.admin && !tw!.admin;
  const na = t.state === "unavailable";
  const dis = locked || na;
  const tip = locked ? ADMIN_TIP : na ? "Not available on this PC" : "";
  const badges = `${t.relaunch ? `<span class="badge">Needs Dock/Finder relaunch</span>` : ""}${t.admin ? `<span class="badge">Admin</span>` : ""}${t.restart ? `<span class="badge">Restart needed</span>` : ""}`;
  let ctl: string;
  if (t.kind === "choice") {
    const opts = t.options.map((o) => `<option value="${esc(o.id)}"${o.id === t.state ? " selected" : ""}>${esc(o.name)}</option>`).join("");
    const extra = t.state === "other" || na ? `<option value="" selected>${na ? "Unavailable" : "Custom plan"}</option>` : "";
    ctl = `<select data-tw="${esc(t.id)}"${dis ? " disabled" : ""}>${extra}${opts}</select>`;
  } else {
    ctl = `<label class="sw"><input type="checkbox" data-tw="${esc(t.id)}"${t.state === "on" ? " checked" : ""}${dis ? " disabled" : ""}><span></span></label>`;
  }
  const undo = t.changed ? `<button data-undo="${esc(t.id)}"${locked ? " disabled" : ""}>Undo</button>` : "";
  return `<li title="${esc(tip)}"><div class="opt-main"><b>${esc(t.name)}</b>${badges}<br><small>${esc(t.desc)}</small></div>${undo}${ctl}</li>`;
}

function tweaksHtml(group: string): string {
  if (noWindows()) return winOnly();
  if (!tw) return "";
  return `<ul class="opt-list tweaks">${tw.tweaks.filter((t) => t.group === group).map(tweakRow).join("")}</ul>`;
}

function startupHtml(): string {
  const o = overview;
  const su = o ? o.startup.map((s, i) => `<li><div class="opt-main"><b>${esc(s.name)}</b>${s.reason ? ` <span class="tag">${esc(s.reason)}</span>` : ""}<br><small>${esc(s.source)}${s.note ? " · " + esc(s.note) : ""}</small></div>
      <button data-su="${i}"${s.can_toggle ? "" : " disabled"}>${s.enabled ? "Turn off" : "Turn on"}</button></li>`).join("") : "";
  return `<ul class="opt-list">${su || `<li class="opt-none">None found.</li>`}</ul>${loginItemsHtml()}`;
}

function loginItemsHtml(): string {
  if (!hooks.isMac) return "";
  if (!loginItems) {
    if (!loginBusy) void loadLoginItems();
    return `<p class="opt-load">Reading Login Items...</p>`;
  }
  const li = loginItems;
  const rows = li.items.map((n) => `<li><div class="opt-main"><b>${esc(n)}</b><br><small>Login Item, change it in System Settings</small></div></li>`).join("");
  return `<h3>Login Items</h3>${li.note ? `<p class="opt-none">${esc(li.note)}</p>` : ""}<ul class="opt-list">${rows || (li.note ? "" : `<li class="opt-none">None found.</li>`)}</ul>`;
}

async function loadLoginItems() {
  loginBusy = true;
  try { loginItems = { items: await invoke<string[]>("optimize_login_items"), note: "" }; }
  catch (e) { loginItems = { items: [], note: String(e) }; }
  loginBusy = false;
  render();
}

async function relaunchShell() {
  try { await invoke("optimize_relaunch_shell"); relaunchNote = "Dock and Finder restarted."; msg = ""; } catch (e) { msg = String(e); }
  render();
}

function appsHtml(): string {
  if (hooks.isMac) return `<p>Apps and their leftover data are listed in the Apps panel.</p><button data-mac="1">Open installed apps…</button>`;
  const q = filter.toLowerCase();
  const apps = (installed ?? []).filter((a) => !q || a.name.toLowerCase().includes(q) || a.publisher.toLowerCase().includes(q));
  return `<input id="opt-filter" placeholder="Search apps" value="${esc(filter)}" spellcheck="false">
    <ul class="opt-list">${apps.map((a) => `<li><div class="opt-main"><b>${esc(a.name)}</b>${a.tag ? ` <span class="tag">${esc(a.tag)}</span>` : ""}<br><small>${esc([a.publisher, a.version, a.updated].filter(Boolean).join(" · "))}</small></div>
      <span class="sz">${a.size ? hooks.fmt(a.size) : ""}</span><button class="danger" data-un="${esc(a.id)}"${a.can_uninstall ? "" : " disabled"}>Uninstall…</button></li>`).join("") || `<li class="opt-none">No apps found.</li>`}</ul>`;
}

function processesHtml(): string {
  const plist = (v: ProcGroup[], mem: boolean) => v.map((p) => `<li><span>${esc(p.name)}${p.count > 1 ? ` ×${p.count}` : ""}</span><span class="sz">${mem ? hooks.fmt(p.mem) : p.cpu.toFixed(1) + "%"}</span></li>`).join("");
  return `<div class="opt-procs"><div><small>By CPU</small><ul class="opt-list">${procs ? plist(procs.by_cpu, false) : ""}</ul></div>
    <div><small>By memory</small><ul class="opt-list">${procs ? plist(procs.by_mem, true) : ""}</ul></div></div>
    <button data-k="task_manager" data-a="">${hooks.isMac ? "Open Activity Monitor" : "Open Task Manager"}</button>`;
}

function currentProvider(a: NetAdapter): string {
  if (a.dhcp) return "auto";
  const list = a.servers.join(",");
  return PROVIDERS.find((p) => p[2] === list)?.[0] ?? "";
}

function networkHtml(): string {
  if (!net) {
    if (!netBusy) void loadNet();
    return `<p class="opt-load">Reading network adapters...</p>`;
  }
  if (!net.windows && !net.mac) return winOnly();
  const tip = net.windows && !net.admin ? ADMIN_TIP : "";
  const dis = tip ? ` disabled title="${esc(tip)}"` : "";
  const rows = net.adapters.map((a, i) => {
    const cur = currentProvider(a);
    const opts = (cur ? "" : `<option value="" selected>Custom</option>`) + PROVIDERS.map((p) => `<option value="${p[0]}"${p[0] === cur ? " selected" : ""}>${p[1]}</option>`).join("");
    return `<li><span class="opt-main"><b>${esc(a.name)}</b><br><small>${a.dhcp ? "Automatic (DHCP)" : "Set by hand"}: ${a.servers.length ? esc(a.servers.join(", ")) : "none"}</small></span>
      <select data-dns="${i}"${dis}>${opts}</select>
      ${a.changed ? `<button data-dnsundo="${i}"${dis}>Undo</button>` : ""}</li>`;
  }).join("") || `<li class="opt-none">No connected network adapters found.</li>`;
  let pr = "";
  if (pingErr) pr = `<p class="err">${esc(pingErr)}</p>`;
  else if (pingRes) {
    const r = pingRes;
    pr = r.avg_ms !== null || r.loss_pct !== null
      ? `<p>Average ${r.avg_ms ?? "?"} ms (min ${r.min_ms ?? "?"}, max ${r.max_ms ?? "?"}), ${r.loss_pct ?? "?"}% lost${r.sent !== null ? ` (${r.received ?? 0} of ${r.sent} replies)` : ""}</p>`
      : `<pre class="opt-raw">${esc(r.raw)}</pre>`;
  }
  return `<ul class="opt-list">${rows}</ul>
    <p class="opt-none">Switching DNS changes where this PC looks up website names. Every change can be undone.</p>
    <div class="opt-bar"><button id="opt-flush"${net.mac ? " disabled" : ""}>Flush DNS cache</button>${net.mac ? `<small>Needs admin: run <code>sudo dscacheutil -flushcache; sudo killall -HUP mDNSResponder</code> in Terminal</small>` : ""}${flushNote ? `<span class="tag">${esc(flushNote)}</span>` : ""}</div>
    <div class="opt-bar"><input id="opt-host" placeholder="Host to ping, e.g. 1.1.1.1" value="${esc(host)}" spellcheck="false"><button id="opt-ping"${pinging ? " disabled" : ""}>${pinging ? "Pinging..." : "Ping"}</button></div>${pr}`;
}

async function loadNet() {
  netBusy = true;
  try { net = await invoke<NetReport>("optimize_net"); } catch (e) { msg = String(e); net = { windows: false, mac: false, admin: false, adapters: [] }; }
  netBusy = false;
  render();
}

async function setDns(i: number, provider: string) {
  const a = net?.adapters[i];
  if (!a || !provider) return;
  if (!(await beforeChange())) { render(); return; }
  try { net = await invoke<NetReport>("optimize_set_dns", { adapter: a.name, provider }); msg = ""; } catch (e) { msg = String(e); }
  render();
}

async function undoDns(i: number) {
  const a = net?.adapters[i];
  if (!a) return;
  if (!(await beforeChange())) { render(); return; }
  try { net = await invoke<NetReport>("optimize_undo_dns", { adapter: a.name }); msg = ""; } catch (e) { msg = String(e); }
  render();
}

async function flushDns() {
  flushNote = "";
  try { await invoke("optimize_flush_dns"); flushNote = "DNS cache flushed."; msg = ""; } catch (e) { msg = String(e); }
  render();
}

async function doPing() {
  pinging = true; pingRes = null; pingErr = "";
  render();
  try { pingRes = await invoke<PingResult>("optimize_ping", { host: host.trim() }); } catch (e) { pingErr = String(e); }
  pinging = false;
  render();
}

function span(s: number): string {
  const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
  return d ? `${d}d ${h}h ${m}m` : h ? `${h}h ${m}m` : `${m}m`;
}

function hardwareHtml(): string {
  if (!hw) {
    if (!hwBusy) void loadHw();
    return `<p class="opt-load">Reading hardware details...</p>`;
  }
  const h = hw;
  const row = (k: string, v: string) => `<li><span class="opt-main"><b>${k}</b></span><span>${esc(v)}</span></li>`;
  const cores = `${h.physical_cores ?? "?"} physical, ${h.logical_cores} logical`;
  const clock = h.base_mhz ? `, ${(h.base_mhz / 1000).toFixed(2)} GHz` : "";
  const ram = hooks.fmt(h.ram_total) + (h.ram_speed_mhz ? `, ${h.ram_speed_mhz} MHz` : "");
  const gpus = h.gpus.length ? h.gpus.map((g) => row("Graphics", g.name + (g.driver ? ` (driver ${g.driver})` : ""))).join("") : (h.windows || h.mac ? row("Graphics", "Not found") : "");
  const disks = h.disks.map((d) => row("Drive", `${d.name}, ${d.kind}, ${hooks.fmt(d.size)}`)).join("");
  const vols = h.volumes.map((v) => row("Volume " + v.mount, `${hooks.fmt(v.free)} free of ${hooks.fmt(v.total)}`)).join("");
  return `<ul class="opt-list">${row("Processor", h.cpu || "Unknown")}${row("Cores", cores + clock)}${row("Memory", ram)}${gpus}${disks}${vols}${row("System", h.os)}${row("Uptime", span(h.uptime_secs))}</ul>`;
}

async function loadHw() {
  hwBusy = true;
  try { hw = await invoke<Hardware>("optimize_hardware"); } catch (e) { msg = String(e); }
  hwBusy = false;
  render();
}

function render() {
  if (loading && !overview) { root.innerHTML = `<p class="opt-load">Checking your computer…</p>`; return; }
  const nav = SECTIONS.map(([id, label]) => `<button data-sec="${id}"${id === section ? ' class="on"' : ""}>${sectionLabel(id, label)}</button>`).join("");
  const notes = tw && tw.notes.length ? `<p class="err">${tw.notes.map(esc).join("<br>")}</p>` : "";
  let body: string;
  if (section === "overview") body = overviewHtml();
  else if (section === "profiles") body = profilesHtml();
  else if (section === "performance") body = tweaksHtml("performance");
  else if (section === "privacy") body = tweaksHtml("privacy");
  else if (section === "windows") body = tweaksHtml(hooks.isMac ? "interface" : "windows");
  else if (section === "startup") body = startupHtml();
  else if (section === "apps") body = appsHtml();
  else if (section === "network") body = networkHtml();
  else if (section === "hardware") body = hardwareHtml();
  else body = processesHtml();
  const secDef = SECTIONS.find((x) => x[0] === section)!;
  const title = sectionLabel(secDef[0], secDef[1]);
  const needRelaunch = !!tw && tw.mac && tw.tweaks.some((t) => t.relaunch && t.changed);
  const relaunchBtn = needRelaunch ? `<button id="opt-relaunch">Relaunch Dock and Finder to apply</button>` : "";
  root.innerHTML = `<div class="opt-wrap"><nav class="opt-nav">${nav}</nav><div class="opt-body">
    <div class="opt-bar"><h3>${title}</h3><button id="opt-refresh"${loading ? " disabled" : ""}>${loading ? "Refreshing…" : "Refresh"}</button>${relaunchBtn}${relaunchNote ? `<span class="tag">${esc(relaunchNote)}</span>` : ""}${restoring ? `<span class="opt-restoring">${hooks.isMac ? "Creating a Time Machine snapshot…" : "Creating a restore point…"}</span>` : ""}${msg ? `<span class="err">${esc(msg)}</span>` : ""}</div>${restoreNote ? `<p class="opt-restorenote">${esc(restoreNote)}</p>` : ""}${notes}${body}</div></div>`;
  wire();
}

async function setTweak(id: string, target: string) {
  relaunchNote = "";
  if (!(await beforeChange())) { render(); return; }
  try { tw = await invoke<TweaksReport>("optimize_set_tweak", { id, target }); msg = ""; }
  catch (e) {
    msg = String(e);
    try { tw = await invoke<TweaksReport>("optimize_tweaks"); } catch { /* keep what we have */ }
  }
  render();
}

async function undoTweak(id: string) {
  try { tw = await invoke<TweaksReport>("optimize_undo_tweak", { id }); msg = ""; } catch (e) { msg = String(e); }
  render();
}

async function restoreAll() {
  const ok = await confirm("Put every setting that Disk Map changed back to what it was before?", { title: "Restore all changes", kind: "warning" });
  if (!ok) return;
  try { tw = await invoke<TweaksReport>("optimize_restore_all"); msg = ""; } catch (e) { msg = String(e); }
  render();
}

async function openProfile(id: string) {
  const info = tw?.profiles.find((p) => p.id === id);
  let plan: PlanItem[];
  try { plan = await invoke<PlanItem[]>("optimize_profile_plan", { profile: id }); } catch (e) { msg = String(e); render(); return; }
  const todo = plan.filter((x) => !x.skip).length;
  const rows = plan.map((x) => `<li class="${x.skip ? "skip" : ""}"><span>${x.admin ? `<span title="Needs administrator">&#128737;</span> ` : ""}<b>${esc(x.name)}</b></span><span>${esc(x.from)} &rarr; ${esc(x.to)}${x.skip ? ` <small>(skipped: ${esc(x.skip)})</small>` : ""}</span></li>`).join("");
  const ov = document.createElement("div");
  ov.className = "overlay";
  ov.innerHTML = `<div class="card"><h2>Apply ${esc(info?.name ?? id)}?</h2><small>${esc(info?.blurb ?? "")}</small>
    <ul class="opt-plan">${rows}</ul><small>${todo ? `${todo} change${todo === 1 ? "" : "s"}. Each one can be undone.` : "Nothing to change."}</small>
    <div class="row"><button class="pf-cancel">Cancel</button><button class="primary pf-ok"${todo ? "" : " disabled"}>Apply</button></div></div>`;
  document.body.appendChild(ov);
  const close = () => ov.remove();
  ov.querySelector<HTMLElement>(".pf-cancel")!.addEventListener("click", close);
  ov.addEventListener("click", (e) => { if (e.target === ov) close(); });
  ov.querySelector<HTMLElement>(".pf-ok")!.addEventListener("click", async () => {
    close();
    if (!(await beforeChange())) return;
    try { tw = await invoke<TweaksReport>("optimize_apply_profile", { profile: id }); msg = ""; } catch (e) { msg = String(e); }
    render();
  });
}

async function run(kind: string, arg: string) {
  try {
    if (kind === "open_settings") await invoke("optimize_open", { kind: arg });
    else if (kind === "task_manager") await invoke("optimize_open", { kind: "taskmanager" });
    else if (kind === "reveal") await invoke("reveal", { path: arg });
    else if (kind === "toggle_startup") {
      const it = overview?.startup.find((s) => s.id === arg);
      if (it) await toggle(it);
    }
  } catch (e) { msg = String(e); render(); }
}

async function toggle(it: StartupItem) {
  if (!(await beforeChange())) { render(); return; }
  const want = !it.enabled;
  try {
    await invoke("optimize_set_startup", { id: it.id, enabled: want });
    it.enabled = want; // update in place, no reload
    msg = "";
    // Keep the startup card and suggestion list consistent without measuring everything again.
    if (overview) {
      const n = overview.startup.filter((s) => s.enabled).length;
      const c = overview.cards.find((x) => x.id === "startup");
      if (c) c.value = `${n} enabled`;
      overview.suggestions = overview.suggestions.filter((s) => s.id !== "startup:" + it.id);
    }
  } catch (e) { msg = String(e); }
  render();
}

function wire() {
  root.querySelector<HTMLElement>("#opt-refresh")?.addEventListener("click", refreshing);
  root.querySelectorAll<HTMLElement>("button[data-sec]").forEach((b) => b.addEventListener("click", () => { section = b.dataset.sec!; render(); }));
  root.querySelectorAll<HTMLElement>("button[data-prof]").forEach((b) => b.addEventListener("click", () => void openProfile(b.dataset.prof!)));
  root.querySelector<HTMLElement>("#opt-relaunch")?.addEventListener("click", () => void relaunchShell());
  root.querySelector<HTMLElement>("#opt-restore")?.addEventListener("click", () => void restoreAll());
  root.querySelectorAll<HTMLElement>("button[data-undo]").forEach((b) => b.addEventListener("click", () => void undoTweak(b.dataset.undo!)));
  root.querySelectorAll<HTMLInputElement>("input[data-tw]").forEach((i) => i.addEventListener("change", () => void setTweak(i.dataset.tw!, i.checked ? "on" : "off")));
  root.querySelectorAll<HTMLSelectElement>("select[data-tw]").forEach((i) => i.addEventListener("change", () => { if (i.value) void setTweak(i.dataset.tw!, i.value); }));
  root.querySelectorAll<HTMLElement>("button[data-k]").forEach((b) => b.addEventListener("click", () => void run(b.dataset.k!, b.dataset.a ?? "")));
  root.querySelectorAll<HTMLElement>("button[data-su]").forEach((b) => b.addEventListener("click", () => {
    const it = overview?.startup[Number(b.dataset.su)];
    if (it) void toggle(it);
  }));
  root.querySelectorAll<HTMLSelectElement>("select[data-dns]").forEach((i) => i.addEventListener("change", () => void setDns(Number(i.dataset.dns), i.value)));
  root.querySelectorAll<HTMLElement>("button[data-dnsundo]").forEach((b) => b.addEventListener("click", () => void undoDns(Number(b.dataset.dnsundo))));
  root.querySelector<HTMLElement>("#opt-flush")?.addEventListener("click", () => void flushDns());
  root.querySelector<HTMLElement>("#opt-ping")?.addEventListener("click", () => void doPing());
  const hi = root.querySelector<HTMLInputElement>("#opt-host");
  if (hi) {
    hi.addEventListener("input", () => { host = hi.value; });
    hi.addEventListener("keydown", (e) => { if (e.key === "Enter" && !pinging) void doPing(); });
  }
  root.querySelector<HTMLElement>("button[data-mac]")?.addEventListener("click", () => hooks.openMacApps());
  root.querySelectorAll<HTMLElement>("button[data-un]").forEach((b) => b.addEventListener("click", async () => {
    const a = installed?.find((x) => x.id === b.dataset.un);
    if (!a) return;
    const ok = await confirm(`Run the uninstaller for ${a.name}? Its own uninstall program opens; nothing is removed until you finish it there.`, { title: "Uninstall", kind: "warning" });
    if (!ok) return;
    try { await invoke("optimize_uninstall", { id: a.id }); msg = ""; } catch (e) { msg = String(e); }
    render();
  }));
  const f = root.querySelector<HTMLInputElement>("#opt-filter");
  if (f) f.addEventListener("input", () => {
    filter = f.value;
    const pos = f.selectionStart ?? filter.length;
    render();
    const n = root.querySelector<HTMLInputElement>("#opt-filter");
    n?.focus();
    n?.setSelectionRange(pos, pos);
  });
}

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
interface Tweak { id: string; group: string; name: string; desc: string; admin: boolean; restart: boolean; kind: string; state: string; options: { id: string; name: string }[]; changed: boolean }
interface ProfileInfo { id: string; name: string; blurb: string }
interface TweaksReport { windows: boolean; admin: boolean; tweaks: Tweak[]; profiles: ProfileInfo[]; last_profile: { name: string; at: number } | null; changed_count: number; notes: string[] }
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
  ["startup", "Startup"], ["apps", "Apps"], ["processes", "Processes"],
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
  overview = null; installed = null; procs = null; tw = null; loaded = false;
  void loadAll();
}

function actionBtn(a: Action | null, extra = ""): string {
  return a ? `<button data-k="${esc(a.kind)}" data-a="${esc(a.arg)}"${extra}>${esc(a.label)}</button>` : "";
}

function winOnly(): string {
  return `<p class="opt-none">Windows only for now.</p>`;
}

function noWindows(): boolean {
  return hooks.isMac || (tw !== null && !tw.windows);
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
  const badges = `${t.admin ? `<span class="badge">Admin</span>` : ""}${t.restart ? `<span class="badge">Restart needed</span>` : ""}`;
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
  return `<ul class="opt-list">${su || `<li class="opt-none">None found.</li>`}</ul>`;
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

function render() {
  if (loading && !overview) { root.innerHTML = `<p class="opt-load">Checking your computer…</p>`; return; }
  const nav = SECTIONS.map(([id, label]) => `<button data-sec="${id}"${id === section ? ' class="on"' : ""}>${label}</button>`).join("");
  const notes = tw && tw.notes.length ? `<p class="err">${tw.notes.map(esc).join("<br>")}</p>` : "";
  let body: string;
  if (section === "overview") body = overviewHtml();
  else if (section === "profiles") body = profilesHtml();
  else if (section === "performance") body = tweaksHtml("performance");
  else if (section === "privacy") body = tweaksHtml("privacy");
  else if (section === "windows") body = tweaksHtml("windows");
  else if (section === "startup") body = startupHtml();
  else if (section === "apps") body = appsHtml();
  else body = processesHtml();
  const title = SECTIONS.find((x) => x[0] === section)![1];
  root.innerHTML = `<div class="opt-wrap"><nav class="opt-nav">${nav}</nav><div class="opt-body">
    <div class="opt-bar"><h3>${title}</h3><button id="opt-refresh"${loading ? " disabled" : ""}>${loading ? "Refreshing…" : "Refresh"}</button>${msg ? `<span class="err">${esc(msg)}</span>` : ""}</div>${notes}${body}</div></div>`;
  wire();
}

async function setTweak(id: string, target: string) {
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
  root.querySelector<HTMLElement>("#opt-restore")?.addEventListener("click", () => void restoreAll());
  root.querySelectorAll<HTMLElement>("button[data-undo]").forEach((b) => b.addEventListener("click", () => void undoTweak(b.dataset.undo!)));
  root.querySelectorAll<HTMLInputElement>("input[data-tw]").forEach((i) => i.addEventListener("change", () => void setTweak(i.dataset.tw!, i.checked ? "on" : "off")));
  root.querySelectorAll<HTMLSelectElement>("select[data-tw]").forEach((i) => i.addEventListener("change", () => { if (i.value) void setTweak(i.dataset.tw!, i.value); }));
  root.querySelectorAll<HTMLElement>("button[data-k]").forEach((b) => b.addEventListener("click", () => void run(b.dataset.k!, b.dataset.a ?? "")));
  root.querySelectorAll<HTMLElement>("button[data-su]").forEach((b) => b.addEventListener("click", () => {
    const it = overview?.startup[Number(b.dataset.su)];
    if (it) void toggle(it);
  }));
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

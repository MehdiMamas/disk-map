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

export interface OptimizeHooks {
  isMac: boolean;
  fmt: (n: number) => string;
  /** macOS only: opens the existing installed-apps panel. */
  openMacApps: () => void;
}

const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]!));

let hooks: OptimizeHooks;
let root: HTMLElement;
let overview: Overview | null = null;
let installed: InstalledApp[] | null = null;
let procs: ProcReport | null = null;
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
  const [o, i, p] = await Promise.allSettled([
    invoke<Overview>("optimize_overview"),
    hooks.isMac ? Promise.resolve([] as InstalledApp[]) : invoke<InstalledApp[]>("optimize_installed"),
    invoke<ProcReport>("optimize_processes"),
  ]);
  const errs: string[] = [];
  if (o.status === "fulfilled") overview = o.value; else errs.push(String(o.reason));
  if (i.status === "fulfilled") installed = i.value; else errs.push(String(i.reason));
  if (p.status === "fulfilled") procs = p.value; else errs.push(String(p.reason));
  msg = errs.join(" ");
  loaded = true;
  loading = false;
  render();
}

function refreshing() {
  overview = null; installed = null; procs = null; loaded = false;
  void loadAll();
}

function actionBtn(a: Action | null, extra = ""): string {
  return a ? `<button data-k="${esc(a.kind)}" data-a="${esc(a.arg)}"${extra}>${esc(a.label)}</button>` : "";
}

function render() {
  if (loading && !overview) { root.innerHTML = `<p class="opt-load">Checking your computer…</p>`; return; }
  const o = overview;
  const cards = o ? o.cards.map((c) => `<div class="opt-card ${esc(c.level)}"><small>${esc(c.title)}</small><b>${esc(c.value)}</b><span>${esc(c.detail)}</span>${actionBtn(c.action)}</div>`).join("") : "";
  const sugg = o && o.suggestions.length
    ? o.suggestions.map((s) => `<li class="opt-sug ${esc(s.severity)}"><div><b>${esc(s.title)}</b><br><small>${esc(s.detail)}</small></div>${actionBtn(s.action)}</li>`).join("")
    : `<li class="opt-none">Nothing to suggest right now.</li>`;
  const su = o ? o.startup.map((s, i) => `<li><div class="opt-main"><b>${esc(s.name)}</b>${s.reason ? ` <span class="tag">${esc(s.reason)}</span>` : ""}<br><small>${esc(s.source)}${s.note ? " · " + esc(s.note) : ""}</small></div>
      <button data-su="${i}"${s.can_toggle ? "" : " disabled"}>${s.enabled ? "Turn off" : "Turn on"}</button></li>`).join("") : "";
  const q = filter.toLowerCase();
  const apps = (installed ?? []).filter((a) => !q || a.name.toLowerCase().includes(q) || a.publisher.toLowerCase().includes(q));
  const inst = hooks.isMac
    ? `<p>Apps and their leftover data are listed in the Apps panel.</p><button data-mac="1">Open installed apps…</button>`
    : `<input id="opt-filter" placeholder="Search apps" value="${esc(filter)}" spellcheck="false">
       <ul class="opt-list">${apps.map((a) => `<li><div class="opt-main"><b>${esc(a.name)}</b>${a.tag ? ` <span class="tag">${esc(a.tag)}</span>` : ""}<br><small>${esc([a.publisher, a.version, a.updated].filter(Boolean).join(" · "))}</small></div>
         <span class="sz">${a.size ? hooks.fmt(a.size) : ""}</span><button class="danger" data-un="${esc(a.id)}"${a.can_uninstall ? "" : " disabled"}>Uninstall…</button></li>`).join("") || `<li class="opt-none">No apps found.</li>`}</ul>`;
  const plist = (v: ProcGroup[], mem: boolean) => v.map((p) => `<li><span>${esc(p.name)}${p.count > 1 ? ` ×${p.count}` : ""}</span><span class="sz">${mem ? hooks.fmt(p.mem) : p.cpu.toFixed(1) + "%"}</span></li>`).join("");
  root.innerHTML = `<div class="opt-bar"><button id="opt-refresh"${loading ? " disabled" : ""}>${loading ? "Refreshing…" : "Refresh"}</button>${msg ? `<span class="err">${esc(msg)}</span>` : ""}</div>
    <h3>Health</h3><div class="opt-cards">${cards}</div>
    <h3>Suggestions</h3><ul class="opt-list">${sugg}</ul>
    <h3>Startup apps</h3><ul class="opt-list">${su || `<li class="opt-none">None found.</li>`}</ul>
    <h3>Installed apps</h3>${inst}
    <h3>Heavy processes</h3><div class="opt-procs"><div><small>By CPU</small><ul class="opt-list">${procs ? plist(procs.by_cpu, false) : ""}</ul></div>
    <div><small>By memory</small><ul class="opt-list">${procs ? plist(procs.by_mem, true) : ""}</ul></div></div>
    <button data-k="task_manager" data-a="">${hooks.isMac ? "Open Activity Monitor" : "Open Task Manager"}</button>`;
  wire();
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

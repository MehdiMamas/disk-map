import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open, confirm } from "@tauri-apps/plugin-dialog";
import { squarify } from "./treemap";
import "./style.css";

interface Hog { id: string; label: string; safe: string; note: string }
interface VNode {
  name: string; path: string; size: number; is_dir: boolean; files: number;
  other: boolean; hog: Hog | null; children: VNode[];
}
interface ScanResult {
  root: VNode; errors: number; permission_denied: number; denied_examples: string[];
  other_fs_skipped: number; elapsed_ms: number;
}

const app = document.getElementById("app")!;
app.innerHTML = `
<header>
  <input id="path" spellcheck="false" />
  <button id="pick">Choose…</button>
  <button id="scan" class="primary">Scan</button>
  <button id="upd" class="link">Check for updates</button>
</header>
<div id="status"></div>
<div id="hint" hidden></div>
<div id="update" hidden></div>
<nav id="crumbs"><button id="back" disabled>← Back</button><span id="trail"></span></nav>
<main><div id="map"></div><ul id="list"></ul></main>`;
const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const isWin = navigator.userAgent.includes("Windows");
const isMac = navigator.userAgent.includes("Mac");
const sep = isWin ? "\\" : "/";
let scanRoot = "";
let stack: VNode[] = [];

export function fmt(n: number): string {
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1000 && i < u.length - 1) { n /= 1000; i++; }
  return `${n.toFixed(i && n < 10 ? 1 : 0)} ${u[i]}`;
}
const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]!));
const cur = () => stack[stack.length - 1];

async function startScan() {
  const path = $<HTMLInputElement>("path").value.trim();
  if (!path) return;
  $("scan").setAttribute("disabled", "");
  $("status").textContent = "Scanning…";
  $("hint").hidden = true;
  try {
    const r = await invoke<ScanResult>("scan", { path });
    scanRoot = path;
    stack = [r.root];
    const extra = [
      r.errors ? `${r.errors} folders could not be read` : "",
      r.other_fs_skipped ? `${r.other_fs_skipped} other drives skipped` : "",
    ].filter(Boolean).join(", ");
    $("status").textContent = `${fmt(r.root.size)} in ${r.root.files.toLocaleString()} files, ${(r.elapsed_ms / 1000).toFixed(1)} s${extra ? " · " + extra : ""}`;
    const fda = isMac && r.permission_denied > 0 ? await invoke<string>("full_disk_access").catch(() => "unknown") : "";
    if (fda === "granted") {
      const h = $("hint");
      h.hidden = false;
      h.textContent = `${r.permission_denied} system-protected folders were skipped. macOS keeps these locked even with Full Disk Access; this is normal.`;
    } else if (r.permission_denied > 0 && isMac) {
      const h = $("hint");
      h.hidden = false;
      h.textContent = `Some folders were blocked by macOS (${r.denied_examples[0] ?? ""}). Grant access: System Settings → Privacy & Security → Full Disk Access → enable Disk Map, then scan again. `;
      const b = document.createElement("button");
      b.textContent = "Open System Settings";
      b.onclick = () => invoke("open_privacy_settings");
      h.appendChild(b);
    } else if (r.permission_denied > 0) {
      const h = $("hint");
      h.hidden = false;
      h.textContent = `${r.permission_denied} folders were not readable (permission denied). Run as administrator to include them.`;
    }
    render();
  } catch (e) {
    $("status").textContent = `Error: ${e}`;
  } finally {
    $("scan").removeAttribute("disabled");
  }
}

function render() {
  const node = cur();
  $<HTMLButtonElement>("back").disabled = stack.length < 2;
  $("trail").innerHTML = stack
    .map((n, i) => `<a data-i="${i}">${esc(i === 0 ? n.path : n.name)}</a>`)
    .join(` <span class="sep">${sep}</span> `) + ` <span class="size">${fmt(node.size)}</span>`;
  $("trail").querySelectorAll("a").forEach((a) =>
    a.addEventListener("click", () => { stack = stack.slice(0, Number((a as HTMLElement).dataset.i) + 1); render(); }));

  const kids = node.children;
  const map = $("map");
  const W = map.clientWidth, H = map.clientHeight;
  map.innerHTML = squarify(kids, { x: 0, y: 0, w: W, h: H })
    .map(({ item, rect }) => {
      const k = item as VNode;
      const idx = kids.indexOf(k);
      const hue = k.other ? 0 : (hash(k.name) % 300) + 20;
      const label = rect.w > 60 && rect.h > 28 ? `<b>${esc(k.name)}</b><br>${fmt(k.size)}` : "";
      return `<div class="cell${k.other ? " other" : ""}" data-i="${idx}" title="${esc(k.name)} · ${fmt(k.size)}"
        style="left:${rect.x}px;top:${rect.y}px;width:${Math.max(rect.w - 1, 0)}px;height:${Math.max(rect.h - 1, 0)}px;--h:${hue}">${label}</div>`;
    }).join("");
  map.querySelectorAll<HTMLElement>(".cell").forEach((c) =>
    c.addEventListener("click", () => drill(kids[Number(c.dataset.i)])));

  const max = kids[0]?.size || 1;
  $("list").innerHTML = kids.map((k, i) => {
    const tag = k.hog ? `<span class="tag ${k.hog.safe}" title="${esc(k.hog.note)}">${esc(k.hog.label)}: ${safeText(k.hog.safe)}</span><span class="note">${esc(k.hog.note)}</span>` : "";
    const acts = k.other ? "" : `<button data-act="reveal" data-i="${i}">${isMac ? "Reveal in Finder" : "Show in Explorer"}</button><button data-act="trash" data-i="${i}" class="danger">Move to Trash</button>`;
    return `<li class="${k.is_dir && !k.other ? "dir" : ""}" data-i="${i}">
      <div class="bar" style="width:${(k.size / max) * 100}%"></div>
      <div class="row"><span class="name">${k.is_dir ? "▸ " : ""}${esc(k.name)}</span><span class="sz">${fmt(k.size)}</span></div>
      <div class="meta">${tag}<span class="acts">${acts}</span></div></li>`;
  }).join("");
  $("list").querySelectorAll<HTMLElement>("li").forEach((li) =>
    li.addEventListener("click", (ev) => {
      const t = ev.target as HTMLElement;
      const k = kids[Number(li.dataset.i)];
      if (t.dataset.act === "reveal") { invoke("reveal", { path: k.path }); return; }
      if (t.dataset.act === "trash") { trash(k); return; }
      drill(k);
    }));
}

const safeText = (s: string) => (s === "yes" ? "safe to clear" : s === "mostly" ? "mostly safe" : "check first");
function hash(s: string) { let h = 0; for (const c of s) h = (h * 31 + c.charCodeAt(0)) | 0; return Math.abs(h); }

async function drill(k: VNode) {
  if (!k.is_dir || k.other) return;
  try {
    stack.push(await invoke<VNode>("view", { path: k.path }));
    render();
  } catch (e) { $("status").textContent = `Error: ${e}`; }
}

async function trash(k: VNode) {
  const ok = await confirm(`Move "${k.name}" (${fmt(k.size)}) to the ${isWin ? "Recycle Bin" : "Trash"}?\n\n${k.path}${k.hog ? "\n\n" + k.hog.note : ""}`,
    { title: "Move to Trash", kind: "warning" });
  if (!ok) return;
  try {
    await invoke("move_to_trash", { path: k.path });
    // Refresh the whole stack from the in-memory tree.
    const paths = stack.map((n) => n.path);
    stack = [];
    for (const p of paths) stack.push(await invoke<VNode>("view", { path: p }));
    render();
  } catch (e) { $("status").textContent = `Error: ${e}`; }
}

$("back").addEventListener("click", () => { if (stack.length > 1) { stack.pop(); render(); } });
$("scan").addEventListener("click", startScan);
$("pick").addEventListener("click", async () => {
  const p = await open({ directory: true, defaultPath: $<HTMLInputElement>("path").value });
  if (typeof p === "string") { $<HTMLInputElement>("path").value = p; startScan(); }
});
$("path").addEventListener("keydown", (e) => { if ((e as KeyboardEvent).key === "Enter") startScan(); });
window.addEventListener("resize", () => { if (stack.length) render(); });
listen<{ files: number; bytes: number }>("scan-progress", (e) => {
  $("status").textContent = `Scanning… ${e.payload.files.toLocaleString()} files, ${fmt(e.payload.bytes)}`;
});
invoke<string>("home_dir").then((h) => { $<HTMLInputElement>("path").value = h; });
void scanRoot;

// ---- macOS Full Disk Access onboarding ----
function fdaSkipped(): boolean {
  try { return localStorage.getItem("fdaSkipped") === "1"; } catch { return false; }
}

async function fdaOnboarding() {
  if (!isMac || fdaSkipped()) return;
  if ((await invoke<string>("full_disk_access")) === "granted") return;
  const ov = document.createElement("div");
  ov.id = "fda";
  ov.innerHTML = `<div class="card">
    <h2>Give Disk Map access to your whole disk</h2>
    <p>Without it, macOS hides Desktop, Documents, Downloads, iCloud, external drives and app data, and they show as unreadable.</p>
    <ol>
      <li>Click Open System Settings.</li>
      <li>Turn on Disk Map in the Full Disk Access list (click + and pick Disk Map if it isn't there).</li>
      <li>Come back here and it continues by itself.</li>
    </ol>
    <div class="row"><button id="fda-open" class="primary">Open System Settings</button>
    <button id="fda-relaunch" hidden>Relaunch Disk Map</button></div>
    <div id="fda-status">Waiting for access…</div>
    <a href="#" id="fda-skip">Skip for now</a>
    <small>Blocked folders will show as unreadable. You can grant access later in System Settings → Privacy &amp; Security → Full Disk Access.</small>
  </div>`;
  document.body.appendChild(ov);
  let done = false;
  let unlisten: (() => void) | undefined;
  let timer = 0;
  const close = () => { done = true; clearInterval(timer); unlisten?.(); ov.remove(); };
  const check = async (fromFocus = false) => {
    if (done) return;
    const g = (await invoke<string>("full_disk_access")) === "granted";
    if (done) return;
    if (g) {
      done = true;
      $("fda-status").textContent = "Access granted ✓";
      setTimeout(close, 900);
    } else if (fromFocus) {
      $("fda-relaunch").hidden = false;
    }
  };
  $("fda-open").onclick = () => invoke("open_privacy_settings");
  $("fda-relaunch").onclick = () => invoke("relaunch");
  $("fda-skip").onclick = (e) => {
    e.preventDefault();
    try { localStorage.setItem("fdaSkipped", "1"); } catch { /* ignore */ }
    close();
  };
  timer = window.setInterval(() => check(), 2000);
  unlisten = await getCurrentWindow().onFocusChanged(({ payload }) => { if (payload) check(true); });
}
fdaOnboarding().catch(() => {});

interface UpdateInfo { current: string; latest: string; newer: boolean; url: string }
const DISMISS_KEY = "dismissedUpdate";

function showUpdateBanner(text: string, info?: UpdateInfo) {
  const el = $("update");
  el.hidden = false;
  el.textContent = text + " ";
  if (info?.newer) {
    const d = document.createElement("button");
    d.textContent = "Download";
    d.onclick = () => invoke("open_url", { url: info.url }).catch((e) => showUpdateBanner(`Couldn't open the download: ${e}`));
    el.appendChild(d);
  }
  const x = document.createElement("button");
  x.textContent = "×";
  x.title = "Dismiss";
  x.onclick = () => {
    el.hidden = true;
    if (info?.newer) { try { localStorage.setItem(DISMISS_KEY, info.latest); } catch { /* ignore */ } }
  };
  el.appendChild(x);
}

async function checkForUpdate(manual: boolean) {
  try {
    const info = await invoke<UpdateInfo>("check_update");
    if (info.newer) {
      let dismissed: string | null = null;
      try { dismissed = localStorage.getItem(DISMISS_KEY); } catch { /* ignore */ }
      if (!manual && dismissed === info.latest) return;
      showUpdateBanner(`Disk Map ${info.latest} is available (you have ${info.current}).`, info);
    } else if (manual) {
      showUpdateBanner(`You're up to date (${info.current}).`);
    }
  } catch (e) {
    if (manual) showUpdateBanner(`Couldn't check for updates: ${e}`);
  }
}
$("upd").onclick = () => checkForUpdate(true);
setTimeout(() => checkForUpdate(false), 3000);

import Foundation

/// The page `barista report` writes: one self-contained HTML file (nothing loaded from the network).
///
/// This is the one template for every platform: the Rust build (rust/barista/build.rs) extracts the
/// text between the `#"""` markers below. The report JSON replaces the data placeholder; see the
/// "Visual report" section of docs/USAGE.md. Text from the data is only ever set with textContent.
public enum ReportTemplate {
    public static let html = #"""
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src data:">
<title>Barista · Usage report</title>
<style>
  :root {
    color-scheme: light;
    --bg: #F9F9F9; --card: #FFFFFF; --line: #E4E4E4; --track: #EDEDED;
    --text: #1F1F1F; --muted: #6B6B6B; --faint: #8F8F8F;
    --accent: #A84A1F; --accent-text: #9E4620; --accent-soft: rgba(199, 90, 43, 0.10);
    --s1: #2a78d6; --s2: #eb6834; --s3: #1baf7a; --s4: #eda100; --s5: #e87ba4; --s6: #008300; --other: #A3A3A3;
    --radius: 12px;
  }
  @media (prefers-color-scheme: dark) {
    :root {
      color-scheme: dark;
      --bg: #202020; --card: #2C2C2C; --line: #3D3D3D; --track: #383838;
      --text: #F1F1F1; --muted: #A3A3A3; --faint: #8A8A8A;
      --accent: #C75A2B; --accent-text: #F0956A; --accent-soft: rgba(199, 90, 43, 0.20);
      --s1: #3987e5; --s2: #d95926; --s3: #199e70; --s4: #c98500; --s5: #d55181; --s6: #008300; --other: #6B6B6B;
    }
  }
  * { box-sizing: border-box; }
  /* The hidden attribute always hides, whatever display a rule below sets. */
  [hidden] { display: none !important; }
  body { margin: 0; background: var(--bg); color: var(--text); font: 14px/1.45 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif; }
  main { max-width: 1080px; margin: 0 auto; padding: 28px 24px 48px; }
  header { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; margin-bottom: 20px; }
  header .cup { width: 30px; height: 30px; color: var(--accent-text); flex: none; }
  h1 { font-size: 22px; margin: 0; font-weight: 650; }
  header .sub { color: var(--muted); font-size: 12.5px; }
  header .grow { flex: 1; }
  .seg { display: inline-flex; background: var(--track); border-radius: 9px; padding: 3px; gap: 2px; }
  .seg button { border: 0; background: transparent; color: var(--muted); font: inherit; font-size: 13px; padding: 5px 12px; border-radius: 7px; cursor: pointer; }
  .seg button[aria-pressed="true"] { background: var(--card); color: var(--text); font-weight: 600; box-shadow: 0 1px 2px rgba(0,0,0,.12); }
  .seg button:focus-visible, .rowtoggle:focus-visible { outline: 2px solid var(--accent-text); outline-offset: 2px; }
  .tiles { display: grid; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); gap: 12px; margin-bottom: 16px; }
  .tile { background: var(--card); border: 1px solid var(--line); border-radius: var(--radius); padding: 14px 16px; }
  .tile .label { color: var(--muted); font-size: 12px; }
  .tile .value { font-size: 26px; font-weight: 650; font-variant-numeric: tabular-nums; margin-top: 2px; }
  .tile .note { color: var(--faint); font-size: 11.5px; }
  .card { background: var(--card); border: 1px solid var(--line); border-radius: var(--radius); padding: 16px 18px; margin-bottom: 16px; }
  .card h2 { font-size: 15px; margin: 0; font-weight: 650; }
  .card .head { display: flex; align-items: center; gap: 12px; margin-bottom: 12px; flex-wrap: wrap; }
  .card .head .grow { flex: 1; }
  .card .hint { color: var(--muted); font-size: 12px; }
  .bars { display: grid; grid-template-columns: minmax(90px, 200px) 1fr; gap: 6px 12px; align-items: center; }
  .bars .name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; }
  .bars .track { position: relative; height: 22px; display: flex; align-items: center; gap: 8px; }
  .bars .fill { height: 14px; border-radius: 0 4px 4px 0; background: var(--accent); min-width: 2px; }
  .bars .val { color: var(--muted); font-size: 12px; font-variant-numeric: tabular-nums; white-space: nowrap; }
  .legend { display: flex; flex-wrap: wrap; gap: 6px 14px; font-size: 12px; color: var(--muted); margin-bottom: 8px; }
  .legend span { display: inline-flex; align-items: center; gap: 6px; }
  .legend i { width: 10px; height: 10px; border-radius: 3px; display: inline-block; }
  svg { display: block; width: 100%; height: auto; overflow: visible; }
  svg text { fill: var(--muted); font-size: 11px; font-family: inherit; }
  svg .grid { stroke: var(--line); stroke-width: 1; }
  svg .hit { fill: transparent; }
  .tip { position: fixed; pointer-events: none; z-index: 10; background: var(--card); color: var(--text); border: 1px solid var(--line); border-radius: 8px; padding: 8px 10px; font-size: 12px; box-shadow: 0 4px 16px rgba(0,0,0,.18); max-width: 280px; display: none; }
  .tip b { display: block; margin-bottom: 2px; }
  .tip .m { color: var(--muted); }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  th { text-align: left; font-weight: 600; color: var(--muted); font-size: 12px; padding: 6px 8px; border-bottom: 1px solid var(--line); white-space: nowrap; }
  td { padding: 8px; border-bottom: 1px solid var(--line); vertical-align: top; }
  td.n, th.n { text-align: right; font-variant-numeric: tabular-nums; white-space: nowrap; }
  td .path { color: var(--faint); font-size: 11.5px; word-break: break-all; }
  tr.detail td { background: var(--accent-soft); font-size: 12.5px; }
  tr.detail td.indent { padding-left: 28px; }
  .rowtoggle { border: 0; background: none; color: inherit; font: inherit; cursor: pointer; padding: 0; text-align: left; display: inline-flex; gap: 6px; align-items: baseline; }
  .rowtoggle .chev { color: var(--muted); display: inline-block; width: 10px; transition: transform .15s; }
  .rowtoggle[aria-expanded="true"] .chev { transform: rotate(90deg); }
  .swatch { width: 9px; height: 9px; border-radius: 3px; display: inline-block; margin-right: 6px; flex: none; }
  .agents { color: var(--muted); font-size: 12px; }
  .empty { color: var(--muted); padding: 24px 0; text-align: center; }
  footer { color: var(--muted); font-size: 12px; line-height: 1.5; }
  footer p { margin: 4px 0; }
  .filter { display: flex; align-items: center; gap: 10px; margin: -6px 0 14px; color: var(--muted); font-size: 12.5px; }
  .filter b { color: var(--text); font-weight: 600; }
  .linkbtn { border: 0; background: none; color: var(--accent-text); font: inherit; padding: 0; cursor: pointer; }
  .linkbtn:hover { text-decoration: underline; }
  .linkbtn:focus-visible, .iconbtn:focus-visible, .btn:focus-visible { outline: 2px solid var(--accent-text); outline-offset: 2px; border-radius: 4px; }
  .iconbtn { border: 0; background: none; color: var(--muted); padding: 2px 4px; margin-left: 4px; border-radius: 5px; cursor: pointer; vertical-align: middle; display: none; }
  .iconbtn svg { width: 14px; height: 14px; display: inline-block; vertical-align: -2px; }
  .iconbtn:hover { color: var(--accent-text); background: var(--accent-soft); }
  html.embedded .iconbtn { display: inline-block; }
  .btn { border: 1px solid var(--line); background: var(--card); color: var(--text); font: inherit; font-size: 13px; padding: 5px 12px; border-radius: 8px; cursor: pointer; }
  .btn:hover { border-color: var(--faint); }
  .actions { position: relative; display: inline-flex; gap: 8px; }
  .menu { position: absolute; right: 0; top: calc(100% + 6px); z-index: 20; background: var(--card); border: 1px solid var(--line); border-radius: 10px; padding: 4px; min-width: 190px; box-shadow: 0 8px 24px rgba(0,0,0,.18); }
  .menu button { display: block; width: 100%; text-align: left; border: 0; background: none; color: var(--text); font: inherit; font-size: 13px; padding: 7px 10px; border-radius: 6px; cursor: pointer; }
  .menu button:hover, .menu button:focus-visible { background: var(--accent-soft); outline: none; }
  svg .day.dim { opacity: .3; }
  svg .hit { cursor: pointer; }
  .toast { position: fixed; left: 50%; bottom: 18px; transform: translateX(-50%); background: var(--text); color: var(--bg); font-size: 12.5px; padding: 6px 12px; border-radius: 8px; opacity: 0; transition: opacity .15s; pointer-events: none; z-index: 30; }
  .toast.show { opacity: .92; }
  .loading { color: var(--muted); padding: 60px 0; text-align: center; }
  /* Views: in Barista's window the report is split into tabs. In a browser everything shows. */
  html[data-view="projects"] .only-history, html[data-view="history"] .only-projects { display: none; }
  html.embedded #views { display: inline-flex; }
  #views { display: none; }
  html.embedded main { display: flex; flex-direction: column; }
  html.embedded main > * { order: 5; }
  html.embedded #filter { order: 1; } html.embedded #tiles { order: 2; } html.embedded #daily-card { order: 3; } html.embedded footer { order: 9; }
  html.embedded header { order: 0; }
  /* Inside the Mac window: no page header (the window's toolbar has the controls), the window's own
     background, and native density. */
  html.mac header { display: none; }
  html.mac body { background: transparent; font-size: 13px; }
  html.mac main { max-width: none; padding: 16px 20px 28px; }
  html.mac { --card: rgba(0,0,0,0.035); --line: rgba(0,0,0,0.08); --radius: 8px; }
  @media (prefers-color-scheme: dark) { html.mac { --card: rgba(255,255,255,0.05); --line: rgba(255,255,255,0.09); } }
  html.mac .tile .value { font-size: 22px; }
  html.mac td { padding: 5px 8px; }
  html.mac .card { padding: 12px 14px; margin-bottom: 12px; }
  html.mac .tiles, html.windows .tiles { gap: 10px; margin-bottom: 12px; grid-template-columns: repeat(auto-fit, minmax(128px, 1fr)); }
  /* Inside the Windows window: the header is the toolbar (tabs, period, export); no logo or title. */
  html.windows header .cup, html.windows header h1 { display: none; }
  html.windows main { max-width: none; padding: 16px 20px 28px; }
  /* While Barista saves an image or PDF: the whole report, on a solid background. */
  html.exporting .only-history:not([hidden]), html.exporting .only-projects:not([hidden]) { display: block !important; }
  html.exporting header, html.exporting .filter, html.exporting .iconbtn { display: none !important; }
  html.exporting body { background: var(--bg); }
  html.exporting.mac { --card: #FFFFFF; --line: #E4E4E4; }
  @media (prefers-color-scheme: dark) { html.exporting.mac { --card: #2C2C2C; --line: #3D3D3D; } }
  @media print {
    /* Paper is white: light colors whatever the screen uses. */
    html:root, html.mac, html.windows {
      color-scheme: light; --bg: #FFFFFF; --card: #FFFFFF; --line: #E4E4E4; --track: #EDEDED;
      --text: #1F1F1F; --muted: #6B6B6B; --faint: #8F8F8F; --accent-text: #9E4620; --other: #A3A3A3;
      --s1: #2a78d6; --s2: #eb6834; --s3: #1baf7a; --s4: #eda100; --s5: #e87ba4; --s6: #008300;
    }
    body { background: #fff; }
    .seg button:not([aria-pressed="true"]) { display: none; }
    .card, .tile { break-inside: avoid; }
  }
</style>
</head>
<body>
<div class="loading" id="loading">Reading agent logs…</div>
<main>
  <header>
    <svg class="cup" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 9h13v5a5 5 0 0 1-5 5H9a5 5 0 0 1-5-5V9z"/><path d="M17 11h1.5a2.5 2.5 0 0 1 0 5H17"/><path d="M8 3c0 1.5 1 1.5 1 3M12 3c0 1.5 1 1.5 1 3"/></svg>
    <div>
      <h1>Usage report</h1>
      <div class="sub" id="generated"></div>
    </div>
    <div class="seg" id="views" role="group" aria-label="View"></div>
    <div class="grow"></div>
    <div class="seg" id="periods" role="group" aria-label="Period"></div>
    <div class="actions" id="actions"></div>
  </header>
  <div class="filter" id="filter" hidden></div>
  <section class="tiles" id="tiles" aria-label="Totals"></section>
  <section class="card" aria-labelledby="by-project-title">
    <div class="head">
      <h2 id="by-project-title">By project</h2>
      <div class="grow"></div>
      <div class="seg" id="metrics" role="group" aria-label="Measure"></div>
    </div>
    <div id="bars"></div>
  </section>
  <section class="card only-history" id="daily-card" aria-labelledby="daily-title">
    <div class="head">
      <h2 id="daily-title">Active time per day</h2>
      <div class="grow"></div>
      <span class="hint" id="daily-hint">Select a day to see just that day</span>
    </div>
    <div class="legend" id="legend"></div>
    <div id="daily"></div>
  </section>
  <section class="card only-projects" aria-labelledby="table-title">
    <div class="head">
      <h2 id="table-title">Projects</h2>
      <div class="grow"></div>
      <span class="hint">Select a project for its agents and models</span>
    </div>
    <div id="table"></div>
  </section>
  <section class="card only-history" aria-labelledby="agents-title">
    <div class="head"><h2 id="agents-title">By agent</h2><div class="grow"></div><span class="hint" id="agents-hint"></span></div>
    <div id="agents"></div>
  </section>
  <footer id="footer"></footer>
</main>
<div class="tip" id="tip" role="tooltip"></div>
<div class="toast" id="toast" role="status" aria-live="polite"></div>
<script>
"use strict";
let DATA = /*BARISTA_REPORT_DATA*/null;
// Inside Barista's window (WKWebView on a Mac, WebView2 on Windows) the page is a view of live data:
// Barista sends the data and settings, and the page asks Barista to copy, export, or show a folder.
const MAC_HOST = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.barista;
const WIN_HOST = window.chrome && window.chrome.webview;
const EMBEDDED = !!(MAC_HOST || WIN_HOST);
const VIEWS = [["projects", "Projects"], ["history", "History"]];
function post(message) {
  if (MAC_HOST) MAC_HOST.postMessage(message);
  else if (WIN_HOST) WIN_HOST.postMessage(message);
}
if (MAC_HOST) document.documentElement.classList.add("embedded", "mac");
else if (WIN_HOST) document.documentElement.classList.add("embedded", "windows");
const SERIES = ["--s1", "--s2", "--s3", "--s4", "--s5", "--s6"];
const SVGNS = "http" + "://www.w3.org/2000/svg";
const METRICS = [["time", "Time"], ["tokens", "Tokens"], ["cost", "Cost"]];
const state = { period: null, metric: "time", open: new Set(), day: null, view: EMBEDDED ? "projects" : null, menu: false };

function el(tag, attrs, children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs || {})) {
    if (key === "text") node.textContent = value;
    else if (key === "class") node.className = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value);
  }
  for (const child of children || []) if (child) node.append(child);
  return node;
}
function svg(tag, attrs) {
  const node = document.createElementNS(SVGNS, tag);
  for (const [key, value] of Object.entries(attrs || {})) {
    if (key === "text") node.textContent = value; else node.setAttribute(key, value);
  }
  return node;
}
function duration(seconds) {
  seconds = Math.max(0, Math.round(seconds || 0));
  if (seconds >= 3600) return Math.floor(seconds / 3600) + "h " + Math.floor((seconds % 3600) / 60) + "m";
  if (seconds >= 60) return Math.floor(seconds / 60) + "m";
  return seconds > 0 ? "<1m" : "0m";
}
function tokens(value) {
  value = value || 0;
  if (value < 1000) return String(value);
  if (value < 1e6) return (value / 1e3).toFixed(1) + "K";
  if (value < 1e9) return (value / 1e6).toFixed(1) + "M";
  return (value / 1e9).toFixed(1) + "B";
}
function cost(value) {
  if (value === null || value === undefined) return "—";
  if (value > 0 && value < 0.01) return "<$0.01";
  return "~$" + value.toFixed(2);
}
function axisDuration(seconds) {
  if (seconds >= 3600 && seconds % 3600 === 0) return seconds / 3600 + "h";
  return duration(seconds);
}
// A project's color follows it everywhere: the period's top five get the categorical slots, the rest gray.
function projectColor(period, project) {
  const index = (period.projects || []).indexOf(project);
  return index >= 0 && index < 5 ? cssVar(SERIES[index]) : cssVar("--other");
}
// The same color for a project when a single day is showing: its place in the whole period.
function projectColorFor(p) {
  const period = basePeriod();
  const index = (period.projects || []).findIndex((q) => q.project === p.project && q.name === p.name);
  return index >= 0 && index < 5 ? cssVar(SERIES[index]) : cssVar("--other");
}
function plural(n, word) { return n + " " + word + (n === 1 ? "" : "s"); }
function agentName(id) { return { claude: "Claude Code", codex: "Codex", cursor: "Cursor" }[id] || id; }
function cssVar(name) { return getComputedStyle(document.documentElement).getPropertyValue(name).trim(); }
function shortDate(iso) {
  const [y, m, d] = iso.split("-").map(Number);
  return new Date(y, m - 1, d).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

const tip = document.getElementById("tip");
function showTip(event, title, lines) {
  tip.replaceChildren(el("b", { text: title }), ...lines.map((line) => el("div", { class: "m", text: line })));
  tip.style.display = "block";
  const x = Math.min(event.clientX + 14, window.innerWidth - tip.offsetWidth - 8);
  const y = Math.min(event.clientY + 14, window.innerHeight - tip.offsetHeight - 8);
  tip.style.left = x + "px"; tip.style.top = y + "px";
}
function hideTip() { tip.style.display = "none"; }
function hover(node, title, lines) {
  node.addEventListener("mousemove", (e) => showTip(e, title, lines));
  node.addEventListener("mouseleave", hideTip);
}

// Right-click a number, name, or folder in Barista's window to copy it.
function copyable(node, text) { if (text !== undefined && text !== null && text !== "") node.dataset.copy = String(text); return node; }
const toast = document.getElementById("toast");
let toastTimer = null;
function showToast(text) {
  toast.textContent = text;
  toast.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => toast.classList.remove("show"), 1400);
}
const FOLDER_ICON = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" aria-hidden="true"><path d="M3.5 7.5a2 2 0 0 1 2-2h4l2 2.5h7a2 2 0 0 1 2 2v7.5a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z"/></svg>';
function folderLabel() { return MAC_HOST ? "Show in Finder" : "Show in File Explorer"; }

function basePeriod() { return DATA.periods.find((p) => p.id === state.period) || DATA.periods[0]; }
// The selected period, or just the selected day of it: each project's day totals (sessions and
// requests aren't kept per day).
function currentPeriod() {
  const period = basePeriod();
  if (!state.day) return period;
  const projects = (period.projects || []).map((p) => {
    const day = (p.days || []).find((d) => d.date === state.day);
    if (!day || !(day.activeSeconds > 0 || day.totalTokens > 0)) return null;
    return { project: p.project, name: p.name, activeSeconds: day.activeSeconds, totalTokens: day.totalTokens, estimatedCost: day.estimatedCost,
      sessions: null, requests: null, turns: 0, agents: p.agents, models: [], days: [day], dayOnly: true };
  }).filter(Boolean).sort((a, b) => b.activeSeconds - a.activeSeconds || b.totalTokens - a.totalTokens);
  const sum = (key) => projects.reduce((n, p) => n + (p[key] || 0), 0);
  return { id: period.id, label: period.label, projects, agents: period.agents, dayOnly: true,
    activeSeconds: sum("activeSeconds"), totalTokens: sum("totalTokens"), estimatedCost: sum("estimatedCost") };
}
function colorPeriod() { return basePeriod(); }
function projectDetail(p) {
  const parts = [duration(p.activeSeconds) + " active"];
  if (p.sessions !== null && p.sessions !== undefined) parts.push(plural(p.sessions, "session"));
  if (p.turns) parts.push(plural(p.turns, "Cursor turn"));
  if (p.totalTokens) parts.push(tokens(p.totalTokens) + " tokens", cost(p.estimatedCost));
  return parts;
}

function setPeriod(id, fromHost) {
  if (!DATA.periods.some((p) => p.id === id) || id === state.period) return;
  state.period = id; state.open.clear(); state.day = null;
  if (!fromHost) post({ cmd: "period", id });
  render();
}
function setView(id) {
  if (!VIEWS.some(([v]) => v === id)) return;
  state.view = id;
  document.documentElement.dataset.view = id;
  render();
}
function renderPeriods() {
  const root = document.getElementById("periods");
  root.replaceChildren(...DATA.periods.map((p) => el("button", {
    type: "button", text: p.label, "aria-pressed": String(p.id === state.period),
    onclick: () => setPeriod(p.id),
  })));
  const metrics = document.getElementById("metrics");
  metrics.replaceChildren(...METRICS.map(([id, label]) => el("button", {
    type: "button", text: label, "aria-pressed": String(id === state.metric),
    onclick: () => { state.metric = id; render(); },
  })));
  if (EMBEDDED) {
    document.getElementById("views").replaceChildren(...VIEWS.map(([id, label]) => el("button", {
      type: "button", text: label, "aria-pressed": String(id === state.view),
      onclick: () => { setView(id); post({ cmd: "view", id }); },
    })));
  }
  renderActions();
  renderFilter();
}
function renderActions() {
  const root = document.getElementById("actions");
  if (MAC_HOST) { root.replaceChildren(); return; } // The Mac window's toolbar has Export and Share.
  if (!WIN_HOST) {
    root.replaceChildren(el("button", { class: "btn", type: "button", text: "Download CSV", onclick: () => downloadCsv() }));
    return;
  }
  const button = el("button", { class: "btn", type: "button", text: "Export \u25BE", "aria-haspopup": "menu", "aria-expanded": String(state.menu),
    onclick: (event) => { event.stopPropagation(); state.menu = !state.menu; renderActions(); } });
  const children = [button];
  if (state.menu) {
    const choose = (format) => () => { state.menu = false; renderActions(); exportAs(format); };
    const menu = el("div", { class: "menu", role: "menu" }, [
      el("button", { type: "button", role: "menuitem", text: "Save as CSV\u2026", onclick: choose("csv") }),
      el("button", { type: "button", role: "menuitem", text: "Save as web page\u2026", onclick: choose("html") }),
      el("button", { type: "button", role: "menuitem", text: "Save as PDF\u2026", onclick: choose("pdf") }),
      el("button", { type: "button", role: "menuitem", text: "Open in browser", onclick: choose("browser") }),
    ]);
    children.push(menu);
    requestAnimationFrame(() => { const first = menu.querySelector("button"); if (first) first.focus(); });
  }
  root.replaceChildren(...children);
}
document.addEventListener("click", () => { if (state.menu) { state.menu = false; renderActions(); } });
function renderFilter() {
  const root = document.getElementById("filter");
  if (!state.day) { root.hidden = true; root.replaceChildren(); return; }
  root.hidden = false;
  root.replaceChildren(
    el("span", {}, [document.createTextNode("Showing "), el("b", { text: longDate(state.day) }), document.createTextNode(" only. Sessions and requests aren\u2019t counted per day.")]),
    el("button", { class: "linkbtn", type: "button", text: "Show the whole period", onclick: () => selectDay(null) }),
  );
}
function selectDay(date) {
  state.day = state.day === date ? null : date;
  state.open.clear();
  render();
}
function longDate(iso) {
  const [y, m, d] = iso.split("-").map(Number);
  return new Date(y, m - 1, d).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
}

function renderTiles(period) {
  const projects = period.projects || [];
  const sessions = (period.agents || []).reduce((n, a) => n + (a.sessions || 0), 0);
  const known = projects.filter((p) => p.project !== null).length;
  const tiles = [
    ["Active time", duration(period.activeSeconds), "With an agent working or you replying"],
    ["Projects", String(known), projects.length > known ? "plus activity with no folder" : "Grouped by Git repository"],
    ["Sessions", period.dayOnly ? "\u2014" : String(sessions), period.dayOnly ? "Counted per period, not per day" : "Across Claude Code, Codex, and Cursor"],
    ["Tokens", tokens(period.totalTokens), "Input, output, and cache"],
    ["Estimated cost", cost(period.estimatedCost), "At API list prices, not your plan"],
  ];
  document.getElementById("tiles").replaceChildren(...tiles.map(([label, value, note]) =>
    el("div", { class: "tile" }, [el("div", { class: "label", text: label }), copyable(el("div", { class: "value", text: value }), value), el("div", { class: "note", text: note })])));
}

function metricValue(p) {
  if (state.metric === "tokens") return p.totalTokens || 0;
  if (state.metric === "cost") return p.estimatedCost || 0;
  return p.activeSeconds || 0;
}
function metricText(p) {
  if (state.metric === "tokens") return tokens(p.totalTokens);
  if (state.metric === "cost") return cost(p.estimatedCost);
  return duration(p.activeSeconds);
}

function renderBars(period) {
  const root = document.getElementById("bars");
  const rows = (period.projects || []).filter((p) => metricValue(p) > 0).sort((a, b) => metricValue(b) - metricValue(a)).slice(0, 15);
  if (!rows.length) {
    root.replaceChildren(el("div", { class: "empty", text: state.day ? "No project activity that day." : "No project activity in this period." }));
    return;
  }
  const max = Math.max(...rows.map(metricValue));
  const grid = el("div", { class: "bars" });
  for (const p of rows) {
    const fill = el("div", { class: "fill" });
    fill.style.width = Math.max(0.5, (metricValue(p) / max) * 82) + "%";
    fill.style.background = projectColorFor(p);
    const track = el("div", { class: "track" }, [fill, copyable(el("span", { class: "val", text: metricText(p) }), metricText(p))]);
    const name = copyable(el("div", { class: "name", text: p.name, title: p.project || "Activity with no folder" }), p.name);
    hover(track, p.name, projectDetail(p));
    grid.append(name, track);
  }
  root.replaceChildren(grid);
}

function renderDaily() {
  const period = basePeriod();
  const card = document.getElementById("daily-card");
  const projects = (period.projects || []).filter((p) => (p.days || []).some((d) => d.activeSeconds > 0));
  const dates = [...new Set(projects.flatMap((p) => p.days.filter((d) => d.activeSeconds > 0).map((d) => d.date)))].sort();
  if (dates.length < 2) { card.hidden = true; return; }
  document.getElementById("daily-hint").textContent = state.day ? "Select the day again to show the whole period" : "Select a day to see just that day";
  card.hidden = false;
  // Colors follow the project, in the period's order: the top five, then Other.
  const leaders = projects.filter((p) => period.projects.indexOf(p) < 5);
  const rest = projects.filter((p) => period.projects.indexOf(p) >= 5);
  const series = leaders.map((p) => ({ name: p.name, color: projectColor(period, p), projects: [p] }));
  if (rest.length) series.push({ name: "Other", color: cssVar("--other"), projects: rest });
  const values = dates.map((date) => series.map((s) => s.projects.reduce((n, p) => n + ((p.days.find((d) => d.date === date) || {}).activeSeconds || 0), 0)));
  const max = Math.max(...values.map((v) => v.reduce((a, b) => a + b, 0)));
  const hourStep = max > 4 * 3600 ? 3600 * Math.ceil(max / 3600 / 4) : max > 3600 ? 3600 : 900 * Math.ceil(max / 900 / 4) || 900;
  const ceiling = Math.ceil(max / hourStep) * hourStep || hourStep;

  document.getElementById("legend").replaceChildren(...series.map((s) => {
    const swatch = el("i"); swatch.style.background = s.color;
    return el("span", {}, [swatch, document.createTextNode(s.name)]);
  }));

  const width = 1000, height = 260, left = 48, bottom = 26, top = 8;
  const plot = height - bottom - top;
  const slot = (width - left) / dates.length;
  const barWidth = Math.max(4, Math.min(36, slot * 0.6));
  const chart = svg("svg", { viewBox: `0 0 ${width} ${height}`, role: "img", "aria-label": "Active time per day, stacked by project" });
  for (let v = 0; v <= ceiling; v += hourStep) {
    const y = top + plot - (v / ceiling) * plot;
    chart.append(svg("line", { class: "grid", x1: left, x2: width, y1: y, y2: y }));
    chart.append(svg("text", { x: left - 8, y: y + 4, "text-anchor": "end", text: axisDuration(v) }));
  }
  const labelEvery = Math.ceil(dates.length / 14);
  dates.forEach((date, i) => {
    const x = left + slot * i + (slot - barWidth) / 2;
    let y = top + plot;
    const stack = values[i];
    const group = svg("g", { class: "day" + (state.day && state.day !== date ? " dim" : "") });
    chart.append(group);
    const lastIndex = stack.reduce((last, v, k) => (v > 0 ? k : last), -1);
    stack.forEach((v, k) => {
      if (v <= 0) return;
      const h = Math.max(1, (v / ceiling) * plot);
      y -= h;
      // A 2px surface gap between segments; the top segment gets rounded ends.
      const gap = k === lastIndex ? 0 : 2;
      const segment = Math.max(1, h - gap);
      if (k === lastIndex) {
        const r = Math.min(4, segment, barWidth / 2), peak = y + gap;
        group.append(svg("path", { fill: series[k].color, d: `M${x},${peak + segment}V${peak + r}Q${x},${peak} ${x + r},${peak}H${x + barWidth - r}Q${x + barWidth},${peak} ${x + barWidth},${peak + r}V${peak + segment}Z` }));
      } else {
        group.append(svg("rect", { x, y: y + gap, width: barWidth, height: segment, fill: series[k].color }));
      }
    });
    if (i % labelEvery === 0) chart.append(svg("text", { x: x + barWidth / 2, y: height - 8, "text-anchor": "middle", text: shortDate(date) }));
    const total = stack.reduce((a, b) => a + b, 0);
    const hit = svg("rect", { class: "hit", x: left + slot * i, y: top, width: slot, height: plot, tabindex: "0", role: "button",
      "aria-pressed": String(state.day === date), "aria-label": longDate(date) + ", " + duration(total) });
    hit.addEventListener("click", () => { hideTip(); selectDay(date); });
    hit.addEventListener("keydown", (event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); selectDay(date); } });
    hover(hit, shortDate(date) + " · " + duration(total), series.map((s, k) => [s.name, stack[k]]).filter(([, v]) => v > 0).map(([name, v]) => name + ": " + duration(v)));
    chart.append(hit);
  });
  document.getElementById("daily").replaceChildren(chart);
}

function renderTable(period) {
  const root = document.getElementById("table");
  const projects = period.projects || [];
  if (!projects.length) {
    root.replaceChildren(el("div", { class: "empty", text: state.day ? "No project activity that day." : "No project activity in this period." }));
    return;
  }
  const head = el("tr", {}, ["Project", "Active", "Sessions", "Requests", "Tokens", "Est. cost"].map((h, i) => el("th", { class: i ? "n" : "", text: h })));
  const body = el("tbody");
  const number = (text) => copyable(el("td", { class: "n", text }), text);
  projects.forEach((p) => {
    const key = p.project === null ? "\u0000unknown" : p.project;
    const open = !p.dayOnly && state.open.has(key);
    const label = el("span", { text: p.name });
    const toggle = p.dayOnly ? el("span", {}, [label]) : el("button", { class: "rowtoggle", type: "button", "aria-expanded": String(open), onclick: () => {
      if (state.open.has(key)) state.open.delete(key); else state.open.add(key);
      renderTable(currentPeriod());
    } }, [el("span", { class: "chev", text: "\u25B8", "aria-hidden": "true" }), label]);
    copyable(toggle, p.name);
    const swatch = el("span", { class: "swatch" });
    swatch.style.background = projectColorFor(p);
    const agents = (p.agents || []).map((a) => agentName(a.agent) + (!p.dayOnly && a.activeSeconds ? " " + duration(a.activeSeconds) : "")).join(" \u00B7 ");
    const path = copyable(el("div", { class: "path", text: p.project || "Activity with no folder" }), p.project);
    if (p.project) {
      const button = el("button", { class: "iconbtn", type: "button", title: folderLabel(), "aria-label": folderLabel() + ": " + p.name,
        onclick: () => post({ cmd: "open-folder", path: p.project }) });
      button.innerHTML = FOLDER_ICON; // Our own icon, never data.
      path.append(button);
    }
    const cell = el("td", {}, [el("div", {}, [swatch, toggle]), path, el("div", { class: "agents", text: agents })]);
    const requests = p.dayOnly ? "\u2014" : (p.requests || 0) + (p.turns ? " + " + plural(p.turns, "turn") : "");
    body.append(el("tr", {}, [cell,
      number(duration(p.activeSeconds)),
      number(p.dayOnly ? "\u2014" : String(p.sessions)),
      number(String(requests)),
      number(tokens(p.totalTokens)),
      number(cost(p.estimatedCost)),
    ]));
    if (open) {
      for (const a of p.agents || []) {
        body.append(el("tr", { class: "detail" }, [
          el("td", { class: "indent", text: agentName(a.agent) }),
          el("td", { class: "n", text: duration(a.activeSeconds) }),
          el("td", { class: "n", text: String(a.sessions) }),
          el("td", { class: "n", text: a.agent === "cursor" ? plural(a.turns, "turn") : String(a.requests) }),
          el("td", { class: "n", text: a.agent === "cursor" ? "—" : tokens(a.totalTokens) }),
          el("td", { class: "n", text: cost(a.estimatedCost) }),
        ]));
      }
      for (const m of p.models || []) {
        body.append(el("tr", { class: "detail" }, [
          el("td", { class: "indent", text: "    " + m.model + " (" + agentName(m.agent) + ")" }),
          el("td", { class: "n", text: m.agent === "cursor" ? duration(m.activeSeconds) : "" }),
          el("td", { class: "n", text: String(m.sessions) }),
          el("td", { class: "n", text: m.agent === "cursor" ? plural(m.turns, "turn") : String(m.requests) }),
          el("td", { class: "n", text: m.agent === "cursor" ? "—" : tokens(m.totalTokens) }),
          el("td", { class: "n", text: cost(m.estimatedCost) }),
        ]));
      }
    }
  });
  root.replaceChildren(el("table", {}, [el("thead", {}, [head]), body]));
}

function renderAgents() {
  const period = basePeriod();
  document.getElementById("agents-hint").textContent = state.day ? "For all of " + period.label.toLowerCase() : "";
  const head = el("tr", {}, ["Agent", "Sessions", "Requests", "Tokens", "From cache", "Est. cost"].map((h, i) => el("th", { class: i ? "n" : "", text: h })));
  const body = el("tbody");
  for (const a of period.agents || []) {
    body.append(el("tr", {}, [
      el("td", { text: a.name }),
      el("td", { class: "n", text: String(a.sessions) }),
      el("td", { class: "n", text: a.agent === "cursor" ? plural(a.turns, "turn") : String(a.requests) }),
      el("td", { class: "n", text: a.agent === "cursor" ? "—" : tokens(a.totalTokens) }),
      el("td", { class: "n", text: a.agent === "cursor" ? "—" : tokens(a.cacheReadTokens) }),
      el("td", { class: "n", text: a.agent === "cursor" ? "—" : cost(a.estimatedCost) }),
    ]));
  }
  document.getElementById("agents").replaceChildren(el("table", {}, [el("thead", {}, [head]), body]));
}

// ---- CSV: one row per project, for the period or day showing -----------------------------------
function csvCell(value) {
  let text = value === null || value === undefined ? "" : String(value);
  // Spreadsheets run cells that start with these as formulas; a project name could.
  if (/^[=+\-@\t\r]/.test(text)) text = "'" + text;
  return /[",\n\r]/.test(text) ? '"' + text.replace(/"/g, '""') + '"' : text;
}
function csvText() {
  const period = currentPeriod();
  const rows = [["Project", "Folder", "Period", "Active minutes", "Sessions", "Requests", "Cursor turns", "Tokens", "Estimated cost (USD)"]];
  const label = state.day || period.label;
  for (const p of period.projects || []) {
    rows.push([p.name, p.project || "", label, Math.round((p.activeSeconds || 0) / 60),
      p.dayOnly ? "" : p.sessions, p.dayOnly ? "" : p.requests, p.dayOnly ? "" : p.turns || 0, p.totalTokens || 0,
      p.estimatedCost === null || p.estimatedCost === undefined ? "" : p.estimatedCost.toFixed(2)]);
  }
  return rows.map((row) => row.map(csvCell).join(",")).join("\r\n") + "\r\n";
}
function csvName() {
  const now = new Date();
  const stamp = now.getFullYear() + "-" + String(now.getMonth() + 1).padStart(2, "0") + "-" + String(now.getDate()).padStart(2, "0");
  return "barista-usage-" + (state.day || state.period) + "-" + stamp + ".csv";
}
function downloadCsv() {
  const link = el("a", { href: URL.createObjectURL(new Blob([csvText()], { type: "text/csv" })), download: csvName() });
  document.body.append(link); link.click(); link.remove();
}
function exportAs(format) {
  if (format === "csv") post({ cmd: "export", format, filename: csvName(), text: csvText() });
  else post({ cmd: "export", format, period: state.period });
}

function render() {
  if (!DATA) return;
  const period = currentPeriod();
  renderPeriods();
  renderTiles(period);
  renderBars(period);
  renderDaily();
  renderTable(period);
  renderAgents();
  renderFooter();
}
function renderFooter() {
  const generated = new Date(DATA.generatedAt);
  document.getElementById("generated").textContent = "From agent logs on this computer \u00B7 " + generated.toLocaleString();
  const lines = [
    DATA.estimateNote || "",
    "Active time joins agent activity in a project with gaps of up to 15 minutes; it includes time you spend replying, and a single tool running over 15 minutes without output leaves a gap. Projects are grouped by Git repository, so worktrees count with their main checkout.",
    EMBEDDED ? "Updated " + generated.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }) + ". Right-click a number or name to copy it." : "Made by Barista. Nothing in this report was uploaded anywhere.",
  ];
  document.getElementById("footer").replaceChildren(...lines.map((text) => el("p", { text })));
}

// New data from Barista: redraw, keeping the period, day, measure, and open rows.
function receive(message) {
  if (!message || typeof message !== "object") return;
  if (message.type === "data" && message.report && Array.isArray(message.report.periods) && message.report.periods.length) {
    const first = !DATA;
    DATA = message.report;
    if (first) {
      document.querySelector("main").hidden = false;
      document.getElementById("loading").hidden = true;
    }
    if (!DATA.periods.some((p) => p.id === state.period)) {
      state.period = DATA.periods.some((p) => p.id === message.period) ? message.period : DATA.periods[0].id;
    }
    const scroll = window.scrollY;
    render();
    window.scrollTo(0, scroll);
  } else if (message.type === "period") {
    if (DATA) setPeriod(message.id, true); else state.period = message.id;
  } else if (message.type === "view") {
    setView(message.id);
  } else if (message.type === "exporting") {
    document.documentElement.classList.toggle("exporting", !!message.on);
    if (DATA) render();
    // Barista captures (PDF, image) only after this layout has been drawn.
    // Two frames, or half a second if frames are paused (a minimized window), whichever is first.
    if (message.on) {
      let sent = false;
      const ready = () => { if (!sent) { sent = true; post({ cmd: "exporting-ready" }); } };
      requestAnimationFrame(() => requestAnimationFrame(ready));
      setTimeout(ready, 500);
    }
  } else if (message.type === "export-csv") {
    if (DATA) exportAs("csv");
  } else if (message.type === "copied") {
    showToast("Copied");
  } else if (message.type === "saved") {
    showToast(String(message.text || "Saved"));
  }
}
window.barista = { receive };

if (EMBEDDED) {
  document.documentElement.dataset.view = state.view;
  // Barista's menu, not the browser's: copy what's under the pointer, if anything.
  document.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    const target = event.target.closest && event.target.closest("[data-copy]");
    if (target) post({ cmd: "copy", text: target.dataset.copy });
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && state.day) { event.preventDefault(); selectDay(null); }
  });
  if (WIN_HOST) WIN_HOST.addEventListener("message", (event) => receive(event.data));
  if (DATA) { state.period = DATA.periods.some((p) => p.id === DATA.initialPeriod) ? DATA.initialPeriod : DATA.periods[0].id; render(); }
  else document.querySelector("main").hidden = true;
  post({ cmd: "ready" });
} else if (!DATA || !Array.isArray(DATA.periods) || !DATA.periods.length) {
  document.getElementById("loading").hidden = true;
  document.querySelector("main").replaceChildren(el("div", { class: "empty", text: "This page is filled in by Barista. Run: barista report" }));
} else {
  document.getElementById("loading").hidden = true;
  state.period = DATA.periods.some((p) => p.id === DATA.initialPeriod) ? DATA.initialPeriod : DATA.periods[0].id;
  render();
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && state.day) { event.preventDefault(); selectDay(null); }
  });
}
window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", render);
</script>
</body>
</html>
"""#

    /// The template filled in with `report` (docs/USAGE.md, "Visual report").
    public static func page(report: [String: Any], initialPeriod: String) -> String {
        var object = report
        object["initialPeriod"] = initialPeriod
        let data = (try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes])) ?? Data("null".utf8)
        let json = String(decoding: data, as: UTF8.self)
            .replacingOccurrences(of: "&", with: "\\u0026")
            .replacingOccurrences(of: "<", with: "\\u003c")
            .replacingOccurrences(of: ">", with: "\\u003e")
        return html.replacingOccurrences(of: "/*BARISTA_REPORT_DATA*/null", with: json)
    }

    /// `usage-report.html` in Barista's data folder.
    public static var defaultURL: URL { UsageLocations.dataDirectory.appendingPathComponent("usage-report.html") }

    /// Reads the logs, then writes the report for every period (atomically) to `url`.
    @discardableResult
    public static func write(initialPeriod: UsagePeriod = .week, to url: URL = defaultURL) throws -> URL {
        let report = UsageStore.report(periods: UsagePeriod.allCases)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(page(report: report.jsonObject, initialPeriod: initialPeriod.rawValue).utf8).write(to: url, options: .atomic)
        return url
    }
}

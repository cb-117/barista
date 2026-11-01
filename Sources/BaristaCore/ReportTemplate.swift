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
  @media print {
    body { background: #fff; }
    .seg button:not([aria-pressed="true"]) { display: none; }
    .card, .tile { break-inside: avoid; }
  }
</style>
</head>
<body>
<main>
  <header>
    <svg class="cup" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 9h13v5a5 5 0 0 1-5 5H9a5 5 0 0 1-5-5V9z"/><path d="M17 11h1.5a2.5 2.5 0 0 1 0 5H17"/><path d="M8 3c0 1.5 1 1.5 1 3M12 3c0 1.5 1 1.5 1 3"/></svg>
    <div>
      <h1>Usage report</h1>
      <div class="sub" id="generated"></div>
    </div>
    <div class="grow"></div>
    <div class="seg" id="periods" role="group" aria-label="Period"></div>
  </header>
  <section class="tiles" id="tiles" aria-label="Totals"></section>
  <section class="card" aria-labelledby="by-project-title">
    <div class="head">
      <h2 id="by-project-title">By project</h2>
      <div class="grow"></div>
      <div class="seg" id="metrics" role="group" aria-label="Measure"></div>
    </div>
    <div id="bars"></div>
  </section>
  <section class="card" id="daily-card" aria-labelledby="daily-title">
    <div class="head">
      <h2 id="daily-title">Active time per day</h2>
      <div class="grow"></div>
      <span class="hint">Top projects; the rest are grouped as Other</span>
    </div>
    <div class="legend" id="legend"></div>
    <div id="daily"></div>
  </section>
  <section class="card" aria-labelledby="table-title">
    <div class="head">
      <h2 id="table-title">Projects</h2>
      <div class="grow"></div>
      <span class="hint">Select a project for its agents and models</span>
    </div>
    <div id="table"></div>
  </section>
  <section class="card" aria-labelledby="agents-title">
    <div class="head"><h2 id="agents-title">By agent</h2></div>
    <div id="agents"></div>
  </section>
  <footer id="footer"></footer>
</main>
<div class="tip" id="tip" role="tooltip"></div>
<script>
"use strict";
const DATA = /*BARISTA_REPORT_DATA*/null;
const SERIES = ["--s1", "--s2", "--s3", "--s4", "--s5", "--s6"];
const SVGNS = "http" + "://www.w3.org/2000/svg";
const METRICS = [["time", "Time"], ["tokens", "Tokens"], ["cost", "Cost"]];
const state = { period: null, metric: "time", open: new Set() };

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

function currentPeriod() { return DATA.periods.find((p) => p.id === state.period) || DATA.periods[0]; }
function projectDetail(p) {
  const parts = [duration(p.activeSeconds) + " active", plural(p.sessions, "session")];
  if (p.turns) parts.push(plural(p.turns, "Cursor turn"));
  if (p.totalTokens) parts.push(tokens(p.totalTokens) + " tokens", cost(p.estimatedCost));
  return parts;
}

function renderPeriods() {
  const root = document.getElementById("periods");
  root.replaceChildren(...DATA.periods.map((p) => el("button", {
    type: "button", text: p.label, "aria-pressed": String(p.id === state.period),
    onclick: () => { state.period = p.id; state.open.clear(); render(); },
  })));
  const metrics = document.getElementById("metrics");
  metrics.replaceChildren(...METRICS.map(([id, label]) => el("button", {
    type: "button", text: label, "aria-pressed": String(id === state.metric),
    onclick: () => { state.metric = id; render(); },
  })));
}

function renderTiles(period) {
  const projects = period.projects || [];
  const sessions = (period.agents || []).reduce((n, a) => n + (a.sessions || 0), 0);
  const known = projects.filter((p) => p.project !== null).length;
  const tiles = [
    ["Active time", duration(period.activeSeconds), "With an agent working or you replying"],
    ["Projects", String(known), projects.length > known ? "plus activity with no folder" : "Grouped by Git repository"],
    ["Sessions", String(sessions), "Across Claude Code, Codex, and Cursor"],
    ["Tokens", tokens(period.totalTokens), "Input, output, and cache"],
    ["Estimated cost", cost(period.estimatedCost), "At API list prices, not your plan"],
  ];
  document.getElementById("tiles").replaceChildren(...tiles.map(([label, value, note]) =>
    el("div", { class: "tile" }, [el("div", { class: "label", text: label }), el("div", { class: "value", text: value }), el("div", { class: "note", text: note })])));
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
    root.replaceChildren(el("div", { class: "empty", text: "No project activity in this period." }));
    return;
  }
  const max = Math.max(...rows.map(metricValue));
  const grid = el("div", { class: "bars" });
  for (const p of rows) {
    const fill = el("div", { class: "fill" });
    fill.style.width = Math.max(0.5, (metricValue(p) / max) * 82) + "%";
    fill.style.background = projectColor(period, p);
    const track = el("div", { class: "track" }, [fill, el("span", { class: "val", text: metricText(p) })]);
    const name = el("div", { class: "name", text: p.name, title: p.project || "Activity with no folder" });
    hover(track, p.name, projectDetail(p));
    grid.append(name, track);
  }
  root.replaceChildren(grid);
}

function renderDaily(period) {
  const card = document.getElementById("daily-card");
  const projects = (period.projects || []).filter((p) => (p.days || []).some((d) => d.activeSeconds > 0));
  const dates = [...new Set(projects.flatMap((p) => p.days.filter((d) => d.activeSeconds > 0).map((d) => d.date)))].sort();
  if (dates.length < 2) { card.hidden = true; return; }
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
        chart.append(svg("path", { fill: series[k].color, d: `M${x},${peak + segment}V${peak + r}Q${x},${peak} ${x + r},${peak}H${x + barWidth - r}Q${x + barWidth},${peak} ${x + barWidth},${peak + r}V${peak + segment}Z` }));
      } else {
        chart.append(svg("rect", { x, y: y + gap, width: barWidth, height: segment, fill: series[k].color }));
      }
    });
    if (i % labelEvery === 0) chart.append(svg("text", { x: x + barWidth / 2, y: height - 8, "text-anchor": "middle", text: shortDate(date) }));
    const hit = svg("rect", { class: "hit", x: left + slot * i, y: top, width: slot, height: plot });
    const total = stack.reduce((a, b) => a + b, 0);
    hover(hit, shortDate(date) + " · " + duration(total), series.map((s, k) => [s.name, stack[k]]).filter(([, v]) => v > 0).map(([name, v]) => name + ": " + duration(v)));
    chart.append(hit);
  });
  document.getElementById("daily").replaceChildren(chart);
}

function renderTable(period) {
  const root = document.getElementById("table");
  const projects = period.projects || [];
  if (!projects.length) {
    root.replaceChildren(el("div", { class: "empty", text: "No project activity in this period." }));
    return;
  }
  const head = el("tr", {}, ["Project", "Active", "Sessions", "Requests", "Tokens", "Est. cost"].map((h, i) => el("th", { class: i ? "n" : "", text: h })));
  const body = el("tbody");
  projects.forEach((p, index) => {
    const key = p.project === null ? "\u0000unknown" : p.project;
    const open = state.open.has(key);
    const toggle = el("button", { class: "rowtoggle", type: "button", "aria-expanded": String(open), onclick: () => {
      if (state.open.has(key)) state.open.delete(key); else state.open.add(key);
      renderTable(currentPeriod());
    } }, [el("span", { class: "chev", text: "▸", "aria-hidden": "true" }), el("span", { text: p.name })]);
    const swatch = el("span", { class: "swatch" });
    swatch.style.background = projectColor(period, p);
    const agents = (p.agents || []).map((a) => agentName(a.agent) + (a.activeSeconds ? " " + duration(a.activeSeconds) : "")).join(" · ");
    const cell = el("td", {}, [
      el("div", {}, [swatch, toggle]),
      el("div", { class: "path", text: p.project || "Activity with no folder" }),
      el("div", { class: "agents", text: agents }),
    ]);
    const requests = (p.requests || 0) + (p.turns ? " + " + plural(p.turns, "turn") : "");
    body.append(el("tr", {}, [cell,
      el("td", { class: "n", text: duration(p.activeSeconds) }),
      el("td", { class: "n", text: String(p.sessions) }),
      el("td", { class: "n", text: requests }),
      el("td", { class: "n", text: tokens(p.totalTokens) }),
      el("td", { class: "n", text: cost(p.estimatedCost) }),
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

function renderAgents(period) {
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

function render() {
  const period = currentPeriod();
  renderPeriods();
  renderTiles(period);
  renderBars(period);
  renderDaily(period);
  renderTable(period);
  renderAgents(period);
}

if (!DATA || !Array.isArray(DATA.periods) || !DATA.periods.length) {
  document.querySelector("main").replaceChildren(el("div", { class: "empty", text: "This page is filled in by Barista. Run: barista report" }));
} else {
  state.period = DATA.periods.some((p) => p.id === DATA.initialPeriod) ? DATA.initialPeriod : DATA.periods[0].id;
  const generated = new Date(DATA.generatedAt);
  document.getElementById("generated").textContent = "From agent logs on this computer · " + generated.toLocaleString();
  document.getElementById("footer").replaceChildren(
    el("p", { text: DATA.estimateNote || "" }),
    el("p", { text: "Active time joins agent activity in a project with gaps of up to 15 minutes; it includes time you spend replying, and a single tool running over 15 minutes without output leaves a gap. Projects are grouped by Git repository, so worktrees count with their main checkout." }),
    el("p", { text: "Made by Barista. Nothing in this report was uploaded anywhere." }),
  );
  render();
  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", render);
}
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

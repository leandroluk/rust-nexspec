//! The interactive graph page (REQ-1202 in `.specs/features/graph-export/spec.md`, decision D4):
//! one self-contained HTML file, no network, no library. The graph is embedded as JSON and drawn on a
//! canvas with a small force layout; everything that comes from the repository is inserted as text.

use serde::Serialize;

use crate::export::{ExportGraph, compact_edges, embed_json};

/// Nodes drawn before the page keeps only the most connected ones (REQ-1202).
pub const DEFAULT_MAX_NODES: usize = 5000;

#[derive(Serialize)]
struct SlimNode<'a> {
    kind: &'a str,
    label: &'a str,
    path: Option<&'a str>,
    community: Option<usize>,
}

#[derive(Serialize)]
struct SlimCommunity<'a> {
    id: usize,
    label: &'a str,
    size: usize,
}

#[derive(Serialize)]
struct Page<'a> {
    nodes: Vec<SlimNode<'a>>,
    /// `[from, to, relation, inferred]` by node index.
    edges: Vec<[usize; 4]>,
    relations: Vec<String>,
    communities: Vec<SlimCommunity<'a>>,
    total_nodes: usize,
    omitted: usize,
}

pub fn render(graph: &ExportGraph, max_nodes: usize) -> String {
    let (shown, omitted) = graph.truncated(max_nodes);
    let (edges, relations) = compact_edges(&shown.nodes, &shown.edges);
    let page = Page {
        nodes: shown.nodes.iter().map(|n| SlimNode { kind: &n.kind, label: &n.label, path: n.path.as_deref(), community: n.community }).collect(),
        edges,
        relations,
        communities: shown.communities.iter().map(|c| SlimCommunity { id: c.id, label: &c.label, size: c.size }).collect(),
        total_nodes: graph.nodes.len(),
        omitted,
    };
    let json = serde_json::to_string(&page).expect("the page model serialises");
    TEMPLATE.replace("__DATA__", &embed_json(&json))
}

const TEMPLATE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>nexspec graph</title>
<style>
:root { --bg:#fafaf9; --fg:#1c1917; --muted:#78716c; --panel:#ffffff; --line:#d6d3d1; --accent:#2563eb; --edge:rgba(120,113,108,.28); }
@media (prefers-color-scheme: dark) { :root { --bg:#0c0a09; --fg:#e7e5e4; --muted:#a8a29e; --panel:#1c1917; --line:#44403c; --accent:#60a5fa; --edge:rgba(168,162,158,.28); } }
* { box-sizing: border-box; }
html, body { height: 100%; margin: 0; background: var(--bg); color: var(--fg); font: 14px/1.4 system-ui, sans-serif; }
#app { display: grid; grid-template-rows: auto 1fr; grid-template-columns: 1fr 320px; height: 100%; }
header { grid-column: 1 / 3; display: flex; flex-wrap: wrap; gap: 8px 14px; align-items: center; padding: 8px 12px; border-bottom: 1px solid var(--line); background: var(--panel); }
header input[type=search], header select { padding: 5px 8px; border: 1px solid var(--line); border-radius: 6px; background: var(--bg); color: var(--fg); font: inherit; }
header input[type=search] { width: 220px; }
.kinds { display: flex; flex-wrap: wrap; gap: 4px 10px; }
.kinds label { display: inline-flex; align-items: center; gap: 4px; cursor: pointer; }
.swatch { width: 10px; height: 10px; border-radius: 50%; display: inline-block; }
#status { color: var(--muted); margin-left: auto; }
main { position: relative; overflow: hidden; }
canvas { width: 100%; height: 100%; display: block; cursor: grab; }
canvas.drag { cursor: grabbing; }
aside { border-left: 1px solid var(--line); background: var(--panel); padding: 12px; overflow: auto; }
aside h2 { font-size: 15px; margin: 0 0 4px; word-break: break-word; }
aside .meta { color: var(--muted); font-size: 12px; margin-bottom: 10px; word-break: break-word; }
aside h3 { font-size: 12px; text-transform: uppercase; letter-spacing: .05em; color: var(--muted); margin: 14px 0 4px; }
aside ul { list-style: none; margin: 0; padding: 0; }
aside li button { all: unset; cursor: pointer; display: block; width: 100%; padding: 2px 0; word-break: break-word; }
aside li button:hover, aside li button:focus-visible { color: var(--accent); }
.rel { color: var(--muted); font-size: 12px; }
#note { position: absolute; left: 12px; bottom: 12px; padding: 6px 10px; border: 1px solid var(--line); border-radius: 6px; background: var(--panel); color: var(--muted); font-size: 12px; max-width: 70%; }
@media (max-width: 760px) { #app { grid-template-columns: 1fr; grid-template-rows: auto 1fr 220px; } header { grid-column: 1; } aside { border-left: 0; border-top: 1px solid var(--line); } }
</style>
</head>
<body>
<noscript>This page needs JavaScript to draw the graph.</noscript>
<div id="app">
  <header>
    <input id="search" type="search" placeholder="Search name or path" aria-label="Search">
    <select id="community" aria-label="Community"><option value="">All communities</option></select>
    <div class="kinds" id="kinds"></div>
    <span id="status"></span>
  </header>
  <main><canvas id="canvas"></canvas><div id="note" hidden></div></main>
  <aside id="details"><p class="meta">Click a node to see what it is connected to.</p></aside>
</div>
<script id="graph-data" type="application/json">__DATA__</script>
<script>
(function () {
  "use strict";
  var data = JSON.parse(document.getElementById("graph-data").textContent);
  var COLORS = { file: "#2563eb", symbol: "#0d9488", requirement: "#d97706", task: "#ca8a04", adr: "#9333ea", doc_section: "#78716c", table: "#dc2626", view: "#e11d48", column: "#f87171", constraint: "#fb923c", package: "#16a34a" };
  var nodes = data.nodes, edges = data.edges, REL = data.relations, i;
  for (i = 0; i < nodes.length; i++) { nodes[i].x = 0; nodes[i].y = 0; nodes[i].vx = 0; nodes[i].vy = 0; nodes[i].deg = 0; nodes[i].hidden = false; }
  var links = [];
  for (i = 0; i < edges.length; i++) {
    var a = edges[i][0], b = edges[i][1];
    if (a === b) continue;
    links.push({ a: a, b: b, e: { type: REL[edges[i][2]], confidence: edges[i][3] ? "inferred" : "extracted" } });
    nodes[a].deg++; nodes[b].deg++;
  }
  var adjacency = nodes.map(function () { return []; });
  links.forEach(function (l, k) { adjacency[l.a].push(k); adjacency[l.b].push(k); });

  // Deterministic start: a spiral, grouped by community so the layout settles quickly.
  var seed = 7;
  function rnd() { seed = (seed * 1664525 + 1013904223) >>> 0; return seed / 4294967296; }
  var centers = {};
  data.communities.forEach(function (c, k) { var ang = k * 2.399963; var r = 80 * Math.sqrt(k + 1); centers[c.id] = { x: Math.cos(ang) * r, y: Math.sin(ang) * r }; });
  nodes.forEach(function (n) {
    var c = centers[n.community] || { x: 0, y: 0 };
    n.x = c.x + (rnd() - .5) * 60; n.y = c.y + (rnd() - .5) * 60;
  });

  var canvas = document.getElementById("canvas"), ctx = canvas.getContext("2d");
  var view = { x: 0, y: 0, k: 1 }, dpr = window.devicePixelRatio || 1, W = 0, H = 0;
  function resize() {
    var r = canvas.getBoundingClientRect(); W = r.width; H = r.height;
    canvas.width = Math.max(1, Math.round(W * dpr)); canvas.height = Math.max(1, Math.round(H * dpr));
    draw();
  }
  window.addEventListener("resize", resize);

  var alpha = 1, running = true;
  function step() {
    var n = nodes.length, R = 70, cell = {}, j, k, dx, dy, d, f;
    for (j = 0; j < n; j++) { var key = Math.floor(nodes[j].x / R) + "," + Math.floor(nodes[j].y / R); (cell[key] = cell[key] || []).push(j); }
    for (j = 0; j < n; j++) {
      var p = nodes[j], cx = Math.floor(p.x / R), cy = Math.floor(p.y / R);
      for (var gx = -1; gx <= 1; gx++) for (var gy = -1; gy <= 1; gy++) {
        var bucket = cell[(cx + gx) + "," + (cy + gy)]; if (!bucket) continue;
        for (k = 0; k < bucket.length; k++) {
          if (bucket[k] <= j) continue;
          var q = nodes[bucket[k]]; dx = p.x - q.x; dy = p.y - q.y; d = Math.sqrt(dx * dx + dy * dy) || .01;
          if (d > R) continue;
          f = (1 - d / R) * 1.2 * alpha; dx /= d; dy /= d;
          p.vx += dx * f; p.vy += dy * f; q.vx -= dx * f; q.vy -= dy * f;
        }
      }
    }
    for (j = 0; j < links.length; j++) {
      var l = links[j], s = nodes[l.a], t = nodes[l.b];
      dx = t.x - s.x; dy = t.y - s.y; d = Math.sqrt(dx * dx + dy * dy) || .01;
      var kk = 0.6 / Math.max(1, Math.min(s.deg, t.deg)); if (kk > 0.12) kk = 0.12;
      f = (d - 36) * kk * alpha; dx /= d; dy /= d;
      s.vx += dx * f; s.vy += dy * f; t.vx -= dx * f; t.vy -= dy * f;
    }
    for (j = 0; j < n; j++) {
      var m = nodes[j], c = centers[m.community];
      if (c) { m.vx += (c.x - m.x) * 0.004 * alpha; m.vy += (c.y - m.y) * 0.004 * alpha; }
      m.vx -= m.x * 0.0008 * alpha; m.vy -= m.y * 0.0008 * alpha;
      m.vx *= 0.82; m.vy *= 0.82;
      if (m.vx > 25) m.vx = 25; else if (m.vx < -25) m.vx = -25;
      if (m.vy > 25) m.vy = 25; else if (m.vy < -25) m.vy = -25;
      m.x += m.vx; m.y += m.vy;
    }
    alpha *= 0.985;
  }
  var frame = 0, touched = false;
  function tick() {
    if (!running) return;
    // Work for about 14 ms per frame, so the page stays responsive however large the graph is.
    var t0 = performance.now();
    do { step(); } while (alpha > 0.02 && performance.now() - t0 < 14);
    frame++;
    if (!touched && frame % 8 === 1) fit();
    draw();
    if (alpha > 0.02) requestAnimationFrame(tick); else { running = false; if (!touched) fit(); }
  }

  function fit() {
    if (!nodes.length) return;
    // Fit the dense core around the median: a sparse halo of stray nodes must not push the bulk off screen.
    var xs = nodes.map(function (n) { return n.x; }).sort(function (a, b) { return a - b; }), ys = nodes.map(function (n) { return n.y; }).sort(function (a, b) { return a - b; });
    var n = nodes.length, cx = xs[n >> 1], cy = ys[n >> 1];
    var hx = Math.max(cx - xs[Math.floor(n * 0.1)], xs[Math.min(n - 1, Math.floor(n * 0.9))] - cx, 40) * 1.6;
    var hy = Math.max(cy - ys[Math.floor(n * 0.1)], ys[Math.min(n - 1, Math.floor(n * 0.9))] - cy, 40) * 1.6;
    view.k = Math.min(W / (2 * hx), H / (2 * hy), 3);
    view.x = W / 2 - cx * view.k; view.y = H / 2 - cy * view.k;
    draw();
  }

  var selected = -1, matches = null, neighbours = null;
  function visible(n) { return !n.hidden; }
  function radius(n) { return 2.5 + Math.min(6, Math.sqrt(n.deg)); }
  function draw() {
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, W, H);
    var css = getComputedStyle(document.documentElement), edgeColor = css.getPropertyValue("--edge").trim() || "rgba(120,113,108,.28)";
    ctx.save(); ctx.translate(view.x, view.y); ctx.scale(view.k, view.k);
    ctx.lineWidth = 1 / view.k;
    var focus = selected >= 0, k, n;
    ctx.strokeStyle = edgeColor; ctx.beginPath();
    for (k = 0; k < links.length; k++) {
      var l = links[k], s = nodes[l.a], t = nodes[l.b];
      if (!visible(s) || !visible(t)) continue;
      if (focus && !(l.a === selected || l.b === selected)) continue;
      ctx.moveTo(s.x, s.y); ctx.lineTo(t.x, t.y);
    }
    ctx.stroke();
    if (focus) {
      ctx.strokeStyle = css.getPropertyValue("--accent").trim() || "#2563eb"; ctx.lineWidth = 1.6 / view.k; ctx.beginPath();
      adjacency[selected].forEach(function (li) { var l2 = links[li]; ctx.moveTo(nodes[l2.a].x, nodes[l2.a].y); ctx.lineTo(nodes[l2.b].x, nodes[l2.b].y); });
      ctx.stroke();
    }
    for (k = 0; k < nodes.length; k++) {
      n = nodes[k]; if (!visible(n)) continue;
      var dim = (focus && k !== selected && !(neighbours && neighbours[k])) || (matches && !matches[k]);
      ctx.globalAlpha = dim ? 0.12 : 1;
      ctx.fillStyle = COLORS[n.kind] || "#78716c";
      ctx.beginPath(); ctx.arc(n.x, n.y, radius(n) * (k === selected ? 1.6 : 1), 0, 6.2832); ctx.fill();
    }
    ctx.globalAlpha = 1;
    if (view.k > 1.6 || selected >= 0) {
      ctx.fillStyle = css.getPropertyValue("--fg").trim() || "#1c1917"; ctx.font = (11 / view.k) + "px system-ui, sans-serif";
      for (k = 0; k < nodes.length; k++) {
        n = nodes[k]; if (!visible(n)) continue;
        if (view.k > 1.6 ? (n.deg > 0 && (k === selected || !focus || (neighbours && neighbours[k]))) : (k === selected || (neighbours && neighbours[k])))
          ctx.fillText(n.label, n.x + radius(n) + 2 / view.k, n.y + 3 / view.k);
      }
    }
    ctx.restore();
  }

  function toWorld(px, py) { return { x: (px - view.x) / view.k, y: (py - view.y) / view.k }; }
  function pick(px, py) {
    var w = toWorld(px, py), best = -1, bestD = Infinity;
    for (var k = 0; k < nodes.length; k++) {
      var n = nodes[k]; if (!visible(n)) continue;
      var dx = n.x - w.x, dy = n.y - w.y, d = dx * dx + dy * dy, r = radius(n) + 4 / view.k;
      if (d < r * r && d < bestD) { best = k; bestD = d; }
    }
    return best;
  }
  var dragging = null, moved = false;
  canvas.addEventListener("pointerdown", function (e) { touched = true; dragging = { x: e.clientX, y: e.clientY, vx: view.x, vy: view.y }; moved = false; canvas.classList.add("drag"); canvas.setPointerCapture(e.pointerId); });
  canvas.addEventListener("pointermove", function (e) {
    if (!dragging) return;
    var dx = e.clientX - dragging.x, dy = e.clientY - dragging.y;
    if (Math.abs(dx) + Math.abs(dy) > 3) moved = true;
    view.x = dragging.vx + dx; view.y = dragging.vy + dy; draw();
  });
  canvas.addEventListener("pointerup", function (e) {
    canvas.classList.remove("drag"); dragging = null;
    if (!moved) { var r = canvas.getBoundingClientRect(); select(pick(e.clientX - r.left, e.clientY - r.top)); }
  });
  canvas.addEventListener("wheel", function (e) {
    e.preventDefault(); touched = true;
    var r = canvas.getBoundingClientRect(), px = e.clientX - r.left, py = e.clientY - r.top, w = toWorld(px, py);
    view.k = Math.min(12, Math.max(0.05, view.k * Math.exp(-e.deltaY * 0.0015)));
    view.x = px - w.x * view.k; view.y = py - w.y * view.k; draw();
  }, { passive: false });

  var details = document.getElementById("details");
  function el(tag, text, cls) { var e = document.createElement(tag); if (text !== undefined) e.textContent = text; if (cls) e.className = cls; return e; }
  function describe(k) {
    details.textContent = "";
    if (k < 0) { details.appendChild(el("p", "Click a node to see what it is connected to.", "meta")); return; }
    var n = nodes[k], comm = null;
    data.communities.forEach(function (c) { if (c.id === n.community) comm = c; });
    details.appendChild(el("h2", n.label));
    details.appendChild(el("div", n.kind + (n.path ? " · " + n.path : "") + (comm ? " · community " + comm.id + " (" + comm.label + ")" : ""), "meta"));
    var groups = { out: [], "in": [] };
    adjacency[k].forEach(function (li) { var l = links[li]; if (l.a === k) groups.out.push([l.e.type, l.b, l.e]); else groups["in"].push([l.e.type, l.a, l.e]); });
    [["out", "Depends on / points to"], ["in", "Used by / pointed at by"]].forEach(function (pair) {
      var list = groups[pair[0]]; if (!list.length) return;
      details.appendChild(el("h3", pair[1] + " (" + list.length + ")"));
      var ul = document.createElement("ul");
      list.sort(function (x, y) { return x[0] < y[0] ? -1 : x[0] > y[0] ? 1 : nodes[x[1]].label < nodes[y[1]].label ? -1 : 1; });
      list.slice(0, 200).forEach(function (item) {
        var li = document.createElement("li"), b = document.createElement("button");
        b.appendChild(el("span", item[0] + (item[2].confidence === "inferred" ? "?" : "") + " ", "rel"));
        b.appendChild(document.createTextNode(nodes[item[1]].label));
        b.addEventListener("click", function () { select(item[1], true); });
        li.appendChild(b); ul.appendChild(li);
      });
      details.appendChild(ul);
      if (list.length > 200) details.appendChild(el("p", "… and " + (list.length - 200) + " more", "meta"));
    });
  }
  function select(k, center) {
    selected = k; neighbours = null;
    if (k >= 0) { neighbours = {}; adjacency[k].forEach(function (li) { neighbours[links[li].a] = true; neighbours[links[li].b] = true; }); }
    if (k >= 0 && center) { view.x = W / 2 - nodes[k].x * view.k; view.y = H / 2 - nodes[k].y * view.k; }
    describe(k); draw();
  }

  // Filters.
  var kindsBox = document.getElementById("kinds"), kindOn = {}, communitySel = document.getElementById("community"), search = document.getElementById("search"), status = document.getElementById("status");
  var present = {}; nodes.forEach(function (n) { present[n.kind] = (present[n.kind] || 0) + 1; });
  Object.keys(present).sort().forEach(function (kind) {
    kindOn[kind] = true;
    var label = document.createElement("label"), box = document.createElement("input"), sw = el("span", "", "swatch");
    box.type = "checkbox"; box.checked = true; sw.style.background = COLORS[kind] || "#78716c";
    box.addEventListener("change", function () { kindOn[kind] = box.checked; applyFilters(); });
    label.appendChild(box); label.appendChild(sw); label.appendChild(document.createTextNode(kind + " " + present[kind]));
    kindsBox.appendChild(label);
  });
  data.communities.forEach(function (c) { var o = document.createElement("option"); o.value = String(c.id); o.textContent = c.id + " · " + c.label + " (" + c.size + ")"; communitySel.appendChild(o); });
  function applyFilters() {
    var wanted = communitySel.value, shown = 0, q = search.value.trim().toLowerCase();
    nodes.forEach(function (n) { n.hidden = !kindOn[n.kind] || (wanted !== "" && String(n.community) !== wanted); if (!n.hidden) shown++; });
    matches = null;
    if (q) { matches = {}; nodes.forEach(function (n, k) { if (!n.hidden && (n.label.toLowerCase().indexOf(q) >= 0 || (n.path && n.path.toLowerCase().indexOf(q) >= 0))) matches[k] = true; }); }
    status.textContent = shown + " of " + nodes.length + " nodes shown" + (matches ? " · " + Object.keys(matches).length + " match" : "");
    draw();
  }
  communitySel.addEventListener("change", applyFilters);
  search.addEventListener("input", applyFilters);
  search.addEventListener("keydown", function (e) {
    if (e.key !== "Enter" || !matches) return;
    // The exact name first, else the shortest label: "tb_x" before "tb_x.some_column".
    var q = search.value.trim().toLowerCase(), best = -1;
    Object.keys(matches).forEach(function (key) {
      var k = Number(key), label = nodes[k].label.toLowerCase();
      if (best < 0 || (label === q && nodes[best].label.toLowerCase() !== q) || (nodes[best].label.toLowerCase() !== q && label.length < nodes[best].label.length)) best = k;
    });
    if (best >= 0) select(best, true);
  });

  var note = document.getElementById("note");
  if (data.omitted > 0) { note.hidden = false; note.textContent = "Showing the " + nodes.length + " most connected of " + data.total_nodes + " nodes (" + data.omitted + " left out). Export with --path or --kind to see the rest."; }

  resize(); applyFilters(); requestAnimationFrame(tick);
})();
</script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::ExportFilter;
    use crate::graph::edge::EdgeType;
    use crate::report::communities::communities;
    use crate::report::snapshot::test_support::*;

    fn graph() -> ExportGraph {
        let snap = snapshot(
            vec![(1, file("src/a.ts")), (2, file("src/b.ts")), (3, symbol("</script><img src=x onerror=alert(1)>"))],
            vec![edge(1, 1, 2, EdgeType::Imports), edge(2, 3, 1, EdgeType::DefinedIn)],
        );
        ExportGraph::from_snapshot(&snap, &communities(&snap, 5, 0), &ExportFilter::default()).unwrap()
    }

    #[test]
    fn the_page_is_self_contained_deterministic_and_cannot_be_broken_out_of() {
        let page = render(&graph(), DEFAULT_MAX_NODES);
        assert_eq!(page, render(&graph(), DEFAULT_MAX_NODES), "same graph, same bytes");
        assert!(!page.contains("cochanges"), "co-change edges are left out of the page");
        for forbidden in ["http://", "https://", "src=\"", "href=\""] {
            assert!(!page.contains(forbidden), "no network and no external resource: found {forbidden}");
        }
        let data_start = page.find("id=\"graph-data\"").unwrap();
        let data_end = page[data_start..].find("</script>").unwrap() + data_start;
        assert!(!page[data_start..data_end].contains("</script"), "the embedded JSON cannot close its own element");
        assert!(page.contains("textContent") && !page.contains("innerHTML"), "repository text goes in as text");
    }

    #[test]
    fn a_big_graph_keeps_the_most_connected_nodes_and_says_so() {
        let page = render(&graph(), 2);
        assert!(page.contains("\"omitted\":1") && page.contains("\"total_nodes\":3"), "{}", &page[page.find("graph-data").unwrap()..][..300]);
        let full = render(&graph(), DEFAULT_MAX_NODES);
        assert!(full.contains("\"omitted\":0"));
    }
}

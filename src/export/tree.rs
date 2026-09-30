//! The collapsible tree (REQ-1203 in `.specs/features/graph-export/spec.md`, decision D5):
//! directory → file → symbol on the left, the selected node's outgoing and incoming edges on the
//! right. One self-contained file; folders render when opened, so large repositories stay light.

use serde::Serialize;

use crate::export::{ExportGraph, compact_edges, embed_json};

#[derive(Serialize)]
struct Item<'a> {
    id: usize,
    kind: &'a str,
    label: &'a str,
    path: Option<&'a str>,
}

#[derive(Serialize)]
struct Page<'a> {
    nodes: Vec<Item<'a>>,
    /// `[from, to, relation, inferred]` by node id (its index).
    edges: Vec<[usize; 4]>,
    relations: Vec<String>,
}

pub fn render(graph: &ExportGraph) -> String {
    let (edges, relations) = compact_edges(&graph.nodes, &graph.edges);
    let page = Page { nodes: graph.nodes.iter().enumerate().map(|(i, n)| Item { id: i, kind: &n.kind, label: &n.label, path: n.path.as_deref() }).collect(), edges, relations };
    let json = serde_json::to_string(&page).expect("the page model serialises");
    TEMPLATE.replace("__DATA__", &embed_json(&json))
}

const TEMPLATE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>nexspec tree</title>
<style>
:root { --bg:#fafaf9; --fg:#1c1917; --muted:#78716c; --panel:#ffffff; --line:#d6d3d1; --accent:#2563eb; }
@media (prefers-color-scheme: dark) { :root { --bg:#0c0a09; --fg:#e7e5e4; --muted:#a8a29e; --panel:#1c1917; --line:#44403c; --accent:#60a5fa; } }
* { box-sizing: border-box; }
html, body { height: 100%; margin: 0; background: var(--bg); color: var(--fg); font: 14px/1.45 system-ui, sans-serif; }
#app { display: grid; grid-template-columns: minmax(280px, 1fr) minmax(260px, 380px); grid-template-rows: auto 1fr; height: 100%; }
header { grid-column: 1 / 3; padding: 8px 12px; border-bottom: 1px solid var(--line); background: var(--panel); display: flex; gap: 12px; align-items: center; }
header input { padding: 5px 8px; border: 1px solid var(--line); border-radius: 6px; background: var(--bg); color: var(--fg); font: inherit; width: 260px; max-width: 100%; }
#count { color: var(--muted); }
#tree { overflow: auto; padding: 8px 12px; }
#inspector { overflow: auto; padding: 12px; border-left: 1px solid var(--line); background: var(--panel); }
details { margin-left: 14px; }
details > summary { cursor: pointer; padding: 1px 0; }
details > summary .n { color: var(--muted); font-size: 12px; }
.leaf { all: unset; display: block; cursor: pointer; margin-left: 28px; padding: 1px 0; word-break: break-word; }
.leaf:hover, .leaf:focus-visible, .sel { color: var(--accent); }
.kind { color: var(--muted); font-size: 12px; }
#inspector h2 { font-size: 15px; margin: 0 0 4px; word-break: break-word; }
#inspector h3 { font-size: 12px; text-transform: uppercase; letter-spacing: .05em; color: var(--muted); margin: 14px 0 4px; }
#inspector ul { list-style: none; margin: 0; padding: 0; }
#inspector li button { all: unset; cursor: pointer; display: block; padding: 2px 0; word-break: break-word; }
#inspector li button:hover, #inspector li button:focus-visible { color: var(--accent); }
.meta { color: var(--muted); font-size: 12px; }
@media (max-width: 760px) { #app { grid-template-columns: 1fr; grid-template-rows: auto 1fr 240px; } header { grid-column: 1; } #inspector { border-left: 0; border-top: 1px solid var(--line); } }
</style>
</head>
<body>
<noscript>This page needs JavaScript to build the tree.</noscript>
<div id="app">
  <header><input id="filter" type="search" placeholder="Find a file or symbol" aria-label="Find"><span id="count"></span></header>
  <div id="tree" role="tree"></div>
  <aside id="inspector"><p class="meta">Select a file or symbol to see its edges.</p></aside>
</div>
<script id="graph-data" type="application/json">__DATA__</script>
<script>
(function () {
  "use strict";
  var data = JSON.parse(document.getElementById("graph-data").textContent);
  var nodes = {}, out = {}, inc = {}, root = { name: "", dirs: {}, files: {}, parent: null, el: null, rendered: false, key: "" }, loose = {};
  data.nodes.forEach(function (n) { nodes[n.id] = n; });
  data.edges.forEach(function (a) {
    var e = { from: a[0], to: a[1], type: data.relations[a[2]], confidence: a[3] ? "inferred" : "extracted" };
    (out[e.from] = out[e.from] || []).push(e); (inc[e.to] = inc[e.to] || []).push(e);
  });

  function dirOf(parts) {
    var d = root;
    parts.forEach(function (p) { d = d.dirs[p] || (d.dirs[p] = { name: p, dirs: {}, files: {}, parent: d, el: null, rendered: false, key: (d.key ? d.key + "/" : "") + p }); });
    return d;
  }
  function fileEntry(path) {
    var parts = path.split("/"), name = parts.pop(), d = dirOf(parts);
    return d.files[name] || (d.files[name] = { name: name, dir: d, node: null, symbols: [], el: null, rendered: false });
  }
  var place = {}; // node id -> { file } | { dir }
  data.nodes.forEach(function (n) {
    if (n.kind === "file" && n.path) { var f = fileEntry(n.path); f.node = n; place[n.id] = f; }
  });
  data.nodes.forEach(function (n) {
    if (n.kind === "file") return;
    if (n.path) { var f = fileEntry(n.path); f.symbols.push(n); place[n.id] = f; }
    else (loose[n.kind] = loose[n.kind] || []).push(n);
  });

  var treeEl = document.getElementById("tree"), inspector = document.getElementById("inspector"), selectedBtn = null;
  function el(tag, text, cls) { var e = document.createElement(tag); if (text !== undefined) e.textContent = text; if (cls) e.className = cls; return e; }
  function sorted(obj) { return Object.keys(obj).sort().map(function (k) { return obj[k]; }); }
  function countFiles(d) { var c = Object.keys(d.files).length; sorted(d.dirs).forEach(function (s) { c += countFiles(s); }); return c; }

  function leaf(node, label) {
    var b = el("button", label, "leaf"); b.type = "button"; b.dataset.id = node.id;
    b.appendChild(el("span", "  " + node.kind, "kind"));
    b.addEventListener("click", function () { inspect(node.id, b); });
    return b;
  }
  function renderFile(f, host) {
    var d = el("details"), s = el("summary", f.name); d.appendChild(s); f.el = d;
    if (f.node) { s.appendChild(el("span", "  " + f.symbols.length + " symbols", "n")); s.addEventListener("click", function () { inspect(f.node.id, s); }); }
    d.addEventListener("toggle", function () { if (d.open && !f.rendered) fillFile(f); });
    host.appendChild(d);
  }
  function fillFile(f) {
    f.rendered = true;
    f.symbols.slice().sort(function (a, b) { return a.label < b.label ? -1 : a.label > b.label ? 1 : 0; }).forEach(function (s) { f.el.appendChild(leaf(s, s.label)); });
  }
  function renderDir(d, host) {
    var el2 = el("details"), s = el("summary", d.name + "/"); s.appendChild(el("span", "  " + countFiles(d) + " files", "n")); el2.appendChild(s); d.el = el2;
    el2.addEventListener("toggle", function () { if (el2.open && !d.rendered) fillDir(d); });
    host.appendChild(el2);
  }
  function fillDir(d) {
    d.rendered = true;
    sorted(d.dirs).forEach(function (s) { renderDir(s, d.el); });
    sorted(d.files).forEach(function (f) { renderFile(f, d.el); });
  }
  function fillRoot() {
    sorted(root.dirs).forEach(function (d) { renderDir(d, treeEl); });
    sorted(root.files).forEach(function (f) { renderFile(f, treeEl); });
    Object.keys(loose).sort().forEach(function (kind) {
      var d = el("details"); d.appendChild(el("summary", "(" + kind + ")")); var list = loose[kind].slice().sort(function (a, b) { return a.label < b.label ? -1 : 1; });
      var done = false; d.addEventListener("toggle", function () { if (d.open && !done) { done = true; list.forEach(function (n) { d.appendChild(leaf(n, n.label)); }); } });
      d.style.marginLeft = "0"; treeEl.appendChild(d);
    });
  }

  function expandTo(id) {
    var p = place[id]; if (!p) return null;
    var chain = []; for (var d = p.dir; d && d !== root; d = d.parent) chain.unshift(d);
    // Top-level folders exist from the start; each opened folder creates the next one of the chain.
    chain.forEach(function (dir) { if (!dir.rendered) fillDir(dir); dir.el.open = true; });
    if (!p.rendered && p.node && p.node.id !== id) { p.el.open = true; if (!p.rendered) fillFile(p); }
    return p;
  }
  function reveal(id) {
    var p = expandTo(id); if (!p) return;
    var target = null;
    if (p.node && p.node.id === id) target = p.el.firstChild; else { p.el.open = true; target = p.el.querySelector('[data-id="' + id + '"]'); }
    if (target && target.scrollIntoView) target.scrollIntoView({ block: "center" });
  }
  function inspect(id, btn) {
    if (selectedBtn) selectedBtn.classList.remove("sel");
    if (btn) { selectedBtn = btn; btn.classList.add("sel"); }
    var n = nodes[id]; inspector.textContent = "";
    inspector.appendChild(el("h2", n.label)); inspector.appendChild(el("div", n.kind + (n.path ? " · " + n.path : ""), "meta"));
    [["Depends on / points to", out[id], "to"], ["Used by / pointed at by", inc[id], "from"]].forEach(function (g) {
      var list = g[1]; if (!list || !list.length) return;
      inspector.appendChild(el("h3", g[0] + " (" + list.length + ")"));
      var ul = el("ul");
      list.slice().sort(function (a, b) { return a.type < b.type ? -1 : a.type > b.type ? 1 : nodes[a[g[2]]].label < nodes[b[g[2]]].label ? -1 : 1; }).slice(0, 300).forEach(function (e) {
        var other = nodes[e[g[2]]], li = el("li"), b = el("button"); b.type = "button";
        b.appendChild(el("span", e.type + (e.confidence === "inferred" ? "?" : "") + "  ", "kind")); b.appendChild(document.createTextNode(other.label));
        b.addEventListener("click", function () { reveal(other.id); inspect(other.id); });
        li.appendChild(b); ul.appendChild(li);
      });
      inspector.appendChild(ul);
      if (list.length > 300) inspector.appendChild(el("p", "… and " + (list.length - 300) + " more", "meta"));
    });
  }

  var filter = document.getElementById("filter"), count = document.getElementById("count");
  filter.addEventListener("keydown", function (e) {
    if (e.key !== "Enter") return;
    var q = filter.value.trim().toLowerCase(); if (!q) return;
    var hit = data.nodes.filter(function (n) { return n.label.toLowerCase().indexOf(q) >= 0 || (n.path && n.path.toLowerCase().indexOf(q) >= 0); });
    count.textContent = hit.length + " match" + (hit.length === 1 ? "" : "es") + (hit.length ? " — showing the first" : "");
    if (hit.length) { reveal(hit[0].id); inspect(hit[0].id); }
  });
  count.textContent = data.nodes.length + " nodes";
  fillRoot();
})();
</script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{ExportEdge, ExportNode, ExportPayload};

    fn graph() -> ExportGraph {
        let node = |id: &str, kind: &str, label: &str, path: Option<&str>| ExportNode {
            id: id.into(),
            kind: kind.into(),
            label: label.into(),
            path: path.map(str::to_string),
            community: None,
            payload: ExportPayload::File { path: label.into(), source_hash: "00".repeat(32) },
        };
        ExportGraph {
            schema_version: 1,
            nodes: vec![node("a", "file", "src/a.ts", Some("src/a.ts")), node("s", "symbol", "</script>evil", Some("src/a.ts")), node("r", "requirement", "REQ-1", None)],
            edges: vec![ExportEdge { from: "s".into(), to: "a".into(), relation: "defined_in".into(), confidence: "extracted".into(), context: "runtime".into() }],
            communities: vec![],
        }
    }

    #[test]
    fn the_tree_page_is_self_contained_deterministic_and_safe() {
        let page = render(&graph());
        assert_eq!(page, render(&graph()));
        for forbidden in ["http://", "https://", "innerHTML", "href=\"", "src=\""] {
            assert!(!page.contains(forbidden), "found {forbidden}");
        }
        let start = page.find("id=\"graph-data\"").unwrap();
        let end = page[start..].find("</script>").unwrap() + start;
        assert!(!page[start..end].contains("</script"), "embedded data cannot close its element");
        assert!(page[start..end].contains("src/a.ts") && page[start..end].contains("defined_in"));
        assert!(!page.contains("cochanges"));
    }
}

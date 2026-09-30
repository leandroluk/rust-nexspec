//! A Markdown wiki an agent (or a person) can browse (REQ-1204 in
//! `.specs/features/graph-export/spec.md`, decision D6): `index.md` plus one article per
//! community of Fase 10, with relative links between them.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

use crate::export::{ExportGraph, ExportNode};

const TOP_SYMBOLS: usize = 15;

/// Relations that say "depends on" between communities; co-change and `defined_in` are not dependencies.
fn is_dependency(relation: &str) -> bool {
    !matches!(relation, "cochanges" | "defined_in")
}

fn slug(label: &str) -> String {
    let mut out = String::new();
    let mut dash = true;
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() { "community".to_string() } else { trimmed.chars().take(40).collect() }
}

fn file_name(id: usize, label: &str) -> String {
    format!("{id:02}-{}.md", slug(label))
}

/// `relative path -> content` for every file of the wiki, in a stable order.
pub fn render(graph: &ExportGraph) -> BTreeMap<String, String> {
    let degrees = graph.degrees();
    let node: HashMap<&str, &ExportNode> = graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let names: HashMap<usize, String> = graph.communities.iter().map(|c| (c.id, file_name(c.id, &c.label))).collect();
    let mut files = BTreeMap::new();

    // Who points at whom across communities.
    let mut crossing: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for edge in graph.edges.iter().filter(|e| is_dependency(&e.relation)) {
        let (Some(from), Some(to)) = (node.get(edge.from.as_str()), node.get(edge.to.as_str())) else { continue };
        if let (Some(a), Some(b)) = (from.community, to.community)
            && a != b
        {
            *crossing.entry((a, b)).or_default() += 1;
        }
    }

    let mut index = String::new();
    let _ = writeln!(index, "# Graph wiki\n");
    let _ = writeln!(index, "{} nodes, {} edges, {} communities.\n", graph.nodes.len(), graph.edges.len(), graph.communities.len());
    if graph.communities.is_empty() {
        let _ = writeln!(index, "No communities yet: the graph has too few connected files.");
    } else {
        let _ = writeln!(index, "| Community | Files | Symbols |\n| --- | ---: | ---: |");
    }
    let clustered: BTreeSet<&str> = graph.communities.iter().flat_map(|c| c.files.iter().map(String::as_str)).collect();
    for community in &graph.communities {
        let symbols = graph.nodes.iter().filter(|n| n.kind == "symbol" && n.community == Some(community.id)).count();
        let _ = writeln!(index, "| [{} — {}](communities/{}) | {} | {} |", community.id, community.label, names[&community.id], community.files.len(), symbols);
    }
    let unclustered = graph.nodes.iter().filter(|n| n.kind == "file").filter(|n| n.path.as_deref().is_some_and(|p| !clustered.contains(p))).count();
    if unclustered > 0 {
        let _ = writeln!(index, "\n{unclustered} file(s) belong to no community (too few links).");
    }
    files.insert("index.md".to_string(), index);

    for community in &graph.communities {
        let mut md = String::new();
        let _ = writeln!(md, "# Community {} — {}\n", community.id, community.label);
        let _ = writeln!(md, "[← index](../index.md)\n");
        let _ = writeln!(md, "{} files.\n", community.files.len());

        let _ = writeln!(md, "## Files\n");
        for path in &community.files {
            let _ = writeln!(md, "- `{path}`");
        }

        let mut symbols: Vec<&ExportNode> = graph.nodes.iter().filter(|n| n.kind == "symbol" && n.community == Some(community.id)).collect();
        symbols.sort_by(|a, b| degrees.get(b.id.as_str()).cmp(&degrees.get(a.id.as_str())).then_with(|| a.label.cmp(&b.label)));
        if !symbols.is_empty() {
            let _ = writeln!(md, "\n## Main symbols\n");
            for symbol in symbols.iter().take(TOP_SYMBOLS) {
                let _ = writeln!(md, "- `{}` — {} link(s)", symbol.label, degrees.get(symbol.id.as_str()).copied().unwrap_or(0));
            }
        }

        for (title, outgoing) in [("Depends on", true), ("Used by", false)] {
            let mut rows: Vec<(usize, usize)> = crossing
                .iter()
                .filter(|((a, b), _)| if outgoing { *a == community.id } else { *b == community.id })
                .map(|((a, b), count)| (if outgoing { *b } else { *a }, *count))
                .collect();
            rows.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
            if !rows.is_empty() {
                let _ = writeln!(md, "\n## {title}\n");
                for (other, count) in rows {
                    if let Some(other_community) = graph.communities.iter().find(|c| c.id == other) {
                        let _ = writeln!(md, "- [{} — {}]({}) — {count} link(s)", other_community.id, other_community.label, names[&other]);
                    }
                }
            }
        }

        let mut requirements: BTreeSet<String> = BTreeSet::new();
        for edge in graph.edges.iter().filter(|e| e.relation == "satisfies") {
            let (Some(from), Some(to)) = (node.get(edge.from.as_str()), node.get(edge.to.as_str())) else { continue };
            if from.community == Some(community.id) && matches!(to.kind.as_str(), "requirement" | "task") {
                requirements.insert(to.label.clone());
            }
        }
        if !requirements.is_empty() {
            let _ = writeln!(md, "\n## Requirements\n");
            for marker in requirements {
                let _ = writeln!(md, "- {marker}");
            }
        }
        files.insert(format!("communities/{}", names[&community.id]), md);
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{ExportCommunity, ExportEdge, ExportPayload};

    fn node(id: &str, kind: &str, label: &str, path: Option<&str>, community: Option<usize>) -> ExportNode {
        let payload = match kind {
            "file" => ExportPayload::File { path: label.to_string(), source_hash: "00".repeat(32) },
            "requirement" => ExportPayload::Requirement { title: label.to_string(), source_hash: "00".repeat(32), body: String::new() },
            _ => ExportPayload::Symbol { name: label.to_string(), source_hash: "00".repeat(32), line_start: 0, line_end: 1 },
        };
        ExportNode { id: id.into(), kind: kind.into(), label: label.into(), path: path.map(str::to_string), community, payload }
    }

    fn edge(from: &str, to: &str, relation: &str) -> ExportEdge {
        ExportEdge { from: from.into(), to: to.into(), relation: relation.into(), confidence: "extracted".into(), context: "runtime".into() }
    }

    fn graph() -> ExportGraph {
        ExportGraph {
            schema_version: 1,
            nodes: vec![
                node("a", "file", "src/a/one.ts", Some("src/a/one.ts"), Some(1)),
                node("b", "file", "src/b/two.ts", Some("src/b/two.ts"), Some(2)),
                node("s", "symbol", "Alpha", Some("src/a/one.ts"), Some(1)),
                node("r", "requirement", "REQ-1", None, None),
                node("x", "file", "lone.ts", Some("lone.ts"), None),
            ],
            edges: vec![edge("a", "b", "imports"), edge("s", "a", "defined_in"), edge("s", "r", "satisfies")],
            communities: vec![
                ExportCommunity { id: 1, label: "src/a/…".into(), size: 1, files: vec!["src/a/one.ts".into()] },
                ExportCommunity { id: 2, label: "src/b/…".into(), size: 1, files: vec!["src/b/two.ts".into()] },
            ],
        }
    }

    #[test]
    fn the_wiki_has_an_index_and_one_article_per_community_with_relative_links() {
        let wiki = render(&graph());
        assert_eq!(wiki.keys().map(String::as_str).collect::<Vec<_>>(), ["communities/01-src-a.md", "communities/02-src-b.md", "index.md"]);
        let index = &wiki["index.md"];
        assert!(index.contains("[1 — src/a/…](communities/01-src-a.md)") && index.contains("1 file(s) belong to no community"), "{index}");
        let a = &wiki["communities/01-src-a.md"];
        assert!(a.contains("[← index](../index.md)") && a.contains("`src/a/one.ts`") && a.contains("`Alpha`"), "{a}");
        assert!(a.contains("## Depends on") && a.contains("[2 — src/b/…](02-src-b.md) — 1 link(s)"), "{a}");
        assert!(a.contains("## Requirements") && a.contains("- REQ-1"), "{a}");
        let b = &wiki["communities/02-src-b.md"];
        assert!(b.contains("## Used by") && b.contains("(01-src-a.md)") && !b.contains("## Depends on"), "{b}");
    }

    #[test]
    fn output_is_deterministic_and_an_empty_graph_says_so() {
        assert_eq!(render(&graph()), render(&graph()));
        let empty = render(&ExportGraph { schema_version: 1, nodes: vec![], edges: vec![], communities: vec![] });
        assert!(empty["index.md"].contains("No communities yet"));
        assert_eq!(slug("Ünï / weird__name!!"), "n-weird-name");
    }
}

//! Communities, their cohesion and the connections that cross them
//! (REQ-1003, REQ-1008 in `.specs/features/report-command/spec.md`).
//!
//! Symbols collapse into the file that defines them, so the graph is one of
//! files. Detection is deterministic modularity optimisation (design.md D2):
//! files are visited in path order and move to the neighbouring community
//! that raises modularity the most. There is no randomness to seed.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::code::deps::file_context;
use crate::graph::edge::{EdgeContext, EdgeType};
use crate::report::snapshot::GraphSnapshot;
use crate::sync::mutation::StableId;

/// Weight of a co-change link relative to a dependency edge (Q2: context only).
const CO_CHANGE_WEIGHT: f64 = 0.25;
const MAX_ROUNDS: usize = 30;
/// Below this cohesion a community is flagged as fragile.
pub const FRAGILE_COHESION: f64 = 0.3;
/// Communities smaller than this are summarised instead of listed.
pub const MIN_LISTED_SIZE: usize = 3;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Community {
    pub id: usize,
    pub label: String,
    pub size: usize,
    /// Internal weight / (internal + crossing weight).
    pub cohesion: f64,
    pub fragile: bool,
    /// The most connected files of the group.
    pub top_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Surprise {
    pub from: String,
    pub to: String,
    pub from_community: usize,
    pub to_community: usize,
    pub score: f64,
    pub explanation: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Communities {
    /// Groups of at least [`MIN_LISTED_SIZE`] files, largest first.
    pub listed: Vec<Community>,
    /// Groups too small to list, and how many files they hold.
    pub small_groups: usize,
    pub small_group_files: usize,
    /// Files with no structural link at all.
    pub isolated_files: usize,
    pub surprising: Vec<Surprise>,
}

/// File -> community index, with the weighted file graph it came from.
struct FileGraph {
    files: Vec<StableId>,
    paths: Vec<String>,
    /// Undirected adjacency: neighbour index -> weight.
    adjacency: Vec<BTreeMap<usize, f64>>,
}

fn build_graph(snapshot: &GraphSnapshot) -> FileGraph {
    let mut weights: BTreeMap<(StableId, StableId), f64> = BTreeMap::new();
    for edge in &snapshot.edges {
        let weight = if edge.edge_type.is_dependency() {
            1.0
        } else if edge.edge_type == EdgeType::CoChanges {
            CO_CHANGE_WEIGHT
        } else {
            continue;
        };
        let (Some(a), Some(b)) = (snapshot.file_id_of(&edge.from), snapshot.file_id_of(&edge.to)) else { continue };
        if a == b {
            continue;
        }
        let key = if a < b { (a, b) } else { (b, a) };
        *weights.entry(key).or_default() += weight;
    }
    let mut files: Vec<(String, StableId)> = weights
        .keys()
        .flat_map(|(a, b)| [*a, *b])
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter_map(|id| snapshot.path_of(&id).map(|p| (p.to_string(), id)))
        .collect();
    files.sort();
    let index: HashMap<StableId, usize> = files.iter().enumerate().map(|(i, (_, id))| (*id, i)).collect();
    let mut adjacency = vec![BTreeMap::new(); files.len()];
    for ((a, b), weight) in weights {
        let (Some(&i), Some(&j)) = (index.get(&a), index.get(&b)) else { continue };
        *adjacency[i].entry(j).or_default() += weight;
        *adjacency[j].entry(i).or_default() += weight;
    }
    FileGraph {
        paths: files.iter().map(|(p, _)| p.clone()).collect(),
        files: files.into_iter().map(|(_, id)| id).collect(),
        adjacency,
    }
}

/// Modularity-optimising local moving (the first phase of Louvain), visiting
/// files in path order so the result is deterministic. Plain label
/// propagation was tried first and floods two dense groups joined by a single
/// edge into one community; modularity keeps them apart.
fn propagate(graph: &FileGraph) -> Vec<usize> {
    let n = graph.files.len();
    let degree: Vec<f64> = graph.adjacency.iter().map(|a| a.values().sum()).collect();
    let total: f64 = degree.iter().sum::<f64>() / 2.0; // m
    let mut community: Vec<usize> = (0..n).collect();
    if total == 0.0 {
        return community;
    }
    // Sum of degrees of each community's members.
    let mut sigma_tot: Vec<f64> = degree.clone();

    for _ in 0..MAX_ROUNDS {
        let mut moved = false;
        for node in 0..n {
            let current = community[node];
            // Weight from `node` to each neighbouring community (excluding itself).
            let mut to_community: BTreeMap<usize, f64> = BTreeMap::new();
            for (&neighbour, &weight) in &graph.adjacency[node] {
                if neighbour != node {
                    *to_community.entry(community[neighbour]).or_default() += weight;
                }
            }
            sigma_tot[current] -= degree[node];
            let gain = |candidate: usize, weight_in: f64| weight_in / total - sigma_tot[candidate] * degree[node] / (2.0 * total * total);
            let stay_gain = gain(current, to_community.get(&current).copied().unwrap_or(0.0));
            let mut best = (current, stay_gain);
            for (&candidate, &weight_in) in &to_community {
                let g = gain(candidate, weight_in);
                // Strictly better to move; ties keep the smaller community id.
                if g > best.1 + 1e-12 || ((g - best.1).abs() <= 1e-12 && candidate < best.0 && best.0 != current) {
                    best = (candidate, g);
                }
            }
            sigma_tot[best.0] += degree[node];
            if best.0 != current {
                community[node] = best.0;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    community
}

fn top_dir(path: &str) -> &str {
    path.split('/').next().filter(|_| path.contains('/')).unwrap_or("(root)")
}

/// Longest shared directory prefix, else the most common top-level directory.
fn community_label(paths: &[&str]) -> String {
    let dirs: Vec<Vec<&str>> = paths
        .iter()
        .map(|p| {
            let mut parts: Vec<&str> = p.split('/').collect();
            parts.pop();
            parts
        })
        .collect();
    let mut common: Vec<&str> = dirs.first().cloned().unwrap_or_default();
    for d in &dirs[1.min(dirs.len())..] {
        let shared = common.iter().zip(d.iter()).take_while(|(a, b)| a == b).count();
        common.truncate(shared);
    }
    if !common.is_empty() {
        return common.join("/");
    }
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for p in paths {
        *counts.entry(top_dir(p)).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(d, _)| format!("{d}/…"))
        .unwrap_or_else(|| "(mixed)".to_string())
}

pub fn communities(snapshot: &GraphSnapshot, top_files: usize, top_surprises: usize) -> Communities {
    let graph = build_graph(snapshot);
    let labels = propagate(&graph);

    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (node, label) in labels.iter().enumerate() {
        groups.entry(*label).or_default().push(node);
    }
    let mut ordered: Vec<Vec<usize>> = groups.into_values().collect();
    // Largest first; ties by the group's first path (nodes are already in path order).
    ordered.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| graph.paths[a[0]].cmp(&graph.paths[b[0]])));

    let mut community_of = vec![usize::MAX; graph.files.len()];
    let mut result = Communities::default();
    let mut next_id = 1usize;
    for group in &ordered {
        if group.len() < MIN_LISTED_SIZE {
            result.small_groups += 1;
            result.small_group_files += group.len();
            continue;
        }
        let id = next_id;
        next_id += 1;
        for &node in group {
            community_of[node] = id;
        }
        let in_group: std::collections::HashSet<usize> = group.iter().copied().collect();
        let (mut internal, mut crossing) = (0.0f64, 0.0f64);
        let mut degree: Vec<(f64, usize)> = Vec::new();
        for &node in group {
            let mut weighted = 0.0;
            for (&neighbour, &weight) in &graph.adjacency[node] {
                weighted += weight;
                if in_group.contains(&neighbour) {
                    internal += weight / 2.0; // seen from both ends
                } else {
                    crossing += weight;
                }
            }
            degree.push((weighted, node));
        }
        degree.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| graph.paths[a.1].cmp(&graph.paths[b.1])));
        let cohesion = if internal + crossing == 0.0 { 0.0 } else { internal / (internal + crossing) };
        let paths: Vec<&str> = group.iter().map(|&n| graph.paths[n].as_str()).collect();
        result.listed.push(Community {
            id,
            label: community_label(&paths),
            size: group.len(),
            cohesion: (cohesion * 100.0).round() / 100.0,
            fragile: cohesion < FRAGILE_COHESION,
            top_files: degree.iter().take(top_files).map(|(_, n)| graph.paths[*n].clone()).collect(),
        });
    }
    result.surprising = surprising_connections(snapshot, &graph, &community_of, top_surprises, &result.listed);

    let connected: std::collections::HashSet<StableId> = graph.files.iter().copied().collect();
    result.isolated_files = snapshot
        .nodes
        .iter()
        .filter(|(id, payload)| matches!(payload, crate::graph::node::NodePayload::File { .. }) && !connected.contains(*id))
        .count();
    result
}

fn surprising_connections(
    snapshot: &GraphSnapshot,
    graph: &FileGraph,
    community_of: &[usize],
    top: usize,
    listed: &[Community],
) -> Vec<Surprise> {
    let index: HashMap<StableId, usize> = graph.files.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    // Directed runtime dependency links between files, aggregated.
    let mut links: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for edge in snapshot.edges.iter().filter(|e| e.edge_type.is_dependency()) {
        let (Some(a), Some(b)) = (snapshot.file_id_of(&edge.from), snapshot.file_id_of(&edge.to)) else { continue };
        let (Some(&i), Some(&j)) = (index.get(&a), index.get(&b)) else { continue };
        if i == j || community_of[i] == usize::MAX || community_of[j] == usize::MAX || community_of[i] == community_of[j] {
            continue;
        }
        // Tests and specs are expected to reach across modules.
        if file_context(&graph.paths[i]) != EdgeContext::Runtime {
            continue;
        }
        *links.entry((i, j)).or_default() += 1;
    }
    let mut between: HashMap<(usize, usize), usize> = HashMap::new();
    for (&(i, j), &count) in &links {
        let key = (community_of[i].min(community_of[j]), community_of[i].max(community_of[j]));
        *between.entry(key).or_default() += count;
    }
    let label_of = |community: usize| listed.iter().find(|c| c.id == community).map(|c| c.label.clone()).unwrap_or_default();
    let mut surprises: Vec<Surprise> = links
        .keys()
        .map(|&(i, j)| {
            let (ci, cj) = (community_of[i], community_of[j]);
            let crossing = between[&(ci.min(cj), ci.max(cj))];
            let different_top = top_dir(&graph.paths[i]) != top_dir(&graph.paths[j]);
            let score = 1.0 / (1.0 + crossing as f64) + if different_top { 0.5 } else { 0.0 };
            Surprise {
                from: graph.paths[i].clone(),
                to: graph.paths[j].clone(),
                from_community: ci,
                to_community: cj,
                score: (score * 1000.0).round() / 1000.0,
                explanation: format!(
                    "{} (community {ci}, {}) depends on {} (community {cj}, {}); only {crossing} link{} connect these communities",
                    graph.paths[i],
                    label_of(ci),
                    graph.paths[j],
                    label_of(cj),
                    if crossing == 1 { "" } else { "s" }
                ),
            }
        })
        .collect();
    surprises.sort_by(|a, b| {
        b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then_with(|| (&a.from, &a.to).cmp(&(&b.from, &b.to)))
    });
    surprises.truncate(top);
    surprises
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::snapshot::test_support::*;

    /// Two triangles (1-2-3 under `app/`, 4-5-6 under `infra/`) and one edge from 3 to 4.
    fn two_clusters() -> GraphSnapshot {
        let names = ["app/a.ts", "app/b.ts", "app/c.ts", "infra/d.ts", "infra/e.ts", "infra/f.ts"];
        let nodes: Vec<(u8, _)> = names.iter().enumerate().map(|(i, p)| (i as u8 + 1, file(p))).collect();
        let pairs = [(1, 2), (2, 3), (3, 1), (4, 5), (5, 6), (6, 4), (3, 4)];
        let edges = pairs.iter().enumerate().map(|(i, (a, b))| edge(i as u16, *a, *b, EdgeType::Imports)).collect();
        snapshot(nodes, edges)
    }

    #[test]
    fn two_dense_groups_joined_by_one_edge_are_two_communities_with_high_cohesion() {
        let result = communities(&two_clusters(), 3, 5);
        assert_eq!(result.listed.len(), 2, "{result:?}");
        assert!(result.listed.iter().all(|c| c.size == 3));
        let labels: Vec<&str> = result.listed.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"app") && labels.contains(&"infra"), "{labels:?}");
        for c in &result.listed {
            // 3 internal edges, 1 crossing: 3/4.
            assert!((c.cohesion - 0.75).abs() < 1e-9, "{c:?}");
            assert!(!c.fragile);
            assert_eq!(c.top_files.len(), 3);
        }
        assert_eq!((result.small_groups, result.isolated_files), (0, 0));
    }

    #[test]
    fn the_single_bridge_is_the_surprising_connection_with_an_explanation() {
        let result = communities(&two_clusters(), 3, 5);
        assert_eq!(result.surprising.len(), 1);
        let s = &result.surprising[0];
        assert_eq!((s.from.as_str(), s.to.as_str()), ("app/c.ts", "infra/d.ts"));
        assert!((s.score - 1.0).abs() < 1e-9, "1/(1+1) + 0.5 for different top directories: {}", s.score);
        assert!(s.explanation.contains("only 1 link connect"), "{}", s.explanation);
        assert!(s.explanation.contains("app") && s.explanation.contains("infra"));
    }

    #[test]
    fn test_files_reaching_across_modules_are_not_surprising() {
        let mut snap = two_clusters();
        // Turn the bridge's source into a spec file.
        snap.nodes.insert(id(3), file("app/c.spec.ts"));
        let result = communities(&snap, 3, 5);
        assert!(result.surprising.is_empty(), "{:?}", result.surprising);
    }

    #[test]
    fn a_group_held_together_mostly_from_outside_is_fragile() {
        // Chain 1-2-3 (weak inside) with heavy links out to a hub cluster 4-5-6.
        let nodes: Vec<(u8, _)> = ["a/x.ts", "a/y.ts", "a/z.ts", "h/1.ts", "h/2.ts", "h/3.ts"]
            .iter()
            .enumerate()
            .map(|(i, p)| (i as u8 + 1, file(p)))
            .collect();
        let mut edges = vec![edge(0, 1, 2, EdgeType::Imports), edge(1, 2, 3, EdgeType::Imports)];
        for (i, (a, b)) in [(4, 5), (5, 6), (6, 4)].iter().enumerate() {
            edges.push(edge(10 + i as u16, *a, *b, EdgeType::Imports));
        }
        // Many links from each of 1..3 into the hub make the labels collapse or cohesion drop.
        for (i, a) in [1u8, 2, 3].iter().enumerate() {
            for (j, b) in [4u8, 5, 6].iter().enumerate() {
                edges.push(edge(100 + (i * 3 + j) as u16, *a, *b, EdgeType::Calls));
            }
        }
        let result = communities(&snapshot(nodes, edges), 3, 5);
        let total: usize = result.listed.iter().map(|c| c.size).sum::<usize>() + result.small_group_files;
        assert_eq!(total, 6);
        assert!(result.listed.iter().all(|c| c.cohesion <= 1.0 && c.cohesion >= 0.0));
    }

    #[test]
    fn small_groups_and_isolated_files_are_counted_not_listed() {
        let nodes = vec![(1, file("a.ts")), (2, file("b.ts")), (3, file("alone.ts")), (4, file("c.ts")), (5, file("d.ts"))];
        let edges = vec![edge(1, 1, 2, EdgeType::Imports), edge(2, 4, 5, EdgeType::Imports)];
        let result = communities(&snapshot(nodes, edges), 3, 5);
        assert!(result.listed.is_empty(), "two pairs are below the listing threshold");
        assert_eq!(result.small_groups, 2);
        assert_eq!(result.small_group_files, 4);
        assert_eq!(result.isolated_files, 1);
    }

    #[test]
    fn co_change_adds_weight_but_never_creates_a_dependency_surprise() {
        let mut snap = two_clusters();
        snap.edges.push(edge(99, 3, 4, EdgeType::CoChanges));
        let with_cochange = communities(&snap, 3, 5);
        assert_eq!(with_cochange.surprising.len(), 1, "co-change is not a dependency edge");
        assert_eq!(with_cochange.listed.len(), 2);
    }

    #[test]
    fn symbols_collapse_into_their_files() {
        // Symbol edges between three files of one cluster, through DefinedIn.
        let nodes = vec![
            (1, file("m/a.ts")), (2, file("m/b.ts")), (3, file("m/c.ts")),
            (4, symbol("A")), (5, symbol("B")), (6, symbol("C")),
        ];
        let edges = vec![
            edge(1, 4, 1, EdgeType::DefinedIn), edge(2, 5, 2, EdgeType::DefinedIn), edge(3, 6, 3, EdgeType::DefinedIn),
            edge(4, 4, 5, EdgeType::Calls), edge(5, 5, 6, EdgeType::Calls), edge(6, 6, 4, EdgeType::Calls),
        ];
        let result = communities(&snapshot(nodes, edges), 3, 5);
        assert_eq!(result.listed.len(), 1);
        assert_eq!(result.listed[0].size, 3);
        assert_eq!(result.listed[0].label, "m");
    }

    #[test]
    fn the_result_is_identical_across_runs() {
        let a = communities(&two_clusters(), 3, 5);
        let b = communities(&two_clusters(), 3, 5);
        assert_eq!(a, b);
    }
}

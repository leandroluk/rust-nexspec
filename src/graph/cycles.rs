//! Import cycle detection (REQ-712 in
//! `.specs/features/dependency-edges/spec.md`): strongly connected components
//! of the file-level `Imports` graph with more than one file. Consumed by the
//! `report` command (Fase 10).

use std::collections::HashMap;

use crate::graph::edge::{Edge, EdgeType};
use crate::sync::mutation::StableId;

/// Groups of files that import each other (directly or through a chain).
/// Each group is sorted by id and the groups are ordered by size, largest
/// first, then by their first id, so the output is stable.
pub fn import_cycles(edges: &[Edge]) -> Vec<Vec<StableId>> {
    let mut index_of: HashMap<StableId, usize> = HashMap::new();
    let mut nodes: Vec<StableId> = Vec::new();
    let intern = |id: StableId, index_of: &mut HashMap<StableId, usize>, nodes: &mut Vec<StableId>| -> usize {
        *index_of.entry(id).or_insert_with(|| {
            nodes.push(id);
            nodes.len() - 1
        })
    };
    let mut adjacency: Vec<Vec<usize>> = Vec::new();
    for edge in edges.iter().filter(|e| e.edge_type == EdgeType::Imports && e.from != e.to) {
        let from = intern(edge.from, &mut index_of, &mut nodes);
        let to = intern(edge.to, &mut index_of, &mut nodes);
        adjacency.resize(nodes.len(), Vec::new());
        adjacency[from].push(to);
    }
    adjacency.resize(nodes.len(), Vec::new());

    let components = strongly_connected(&adjacency);
    let mut cycles: Vec<Vec<StableId>> = components
        .into_iter()
        .filter(|c| c.len() > 1)
        .map(|c| {
            let mut ids: Vec<StableId> = c.into_iter().map(|i| nodes[i]).collect();
            ids.sort();
            ids
        })
        .collect();
    cycles.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a[0].cmp(&b[0])));
    cycles
}

/// Tarjan's algorithm, iterative (import graphs can be deep).
fn strongly_connected(adjacency: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = adjacency.len();
    let mut index = vec![usize::MAX; n];
    let mut lowlink = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut components = Vec::new();
    let mut counter = 0usize;

    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        // (node, next neighbour to visit)
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        index[root] = counter;
        lowlink[root] = counter;
        counter += 1;
        stack.push(root);
        on_stack[root] = true;

        while let Some(&mut (node, ref mut next)) = work.last_mut() {
            if *next < adjacency[node].len() {
                let neighbour = adjacency[node][*next];
                *next += 1;
                if index[neighbour] == usize::MAX {
                    index[neighbour] = counter;
                    lowlink[neighbour] = counter;
                    counter += 1;
                    stack.push(neighbour);
                    on_stack[neighbour] = true;
                    work.push((neighbour, 0));
                } else if on_stack[neighbour] {
                    lowlink[node] = lowlink[node].min(index[neighbour]);
                }
            } else {
                work.pop();
                if let Some(&(parent, _)) = work.last() {
                    lowlink[parent] = lowlink[parent].min(lowlink[node]);
                }
                if lowlink[node] == index[node] {
                    let mut component = Vec::new();
                    while let Some(member) = stack.pop() {
                        on_stack[member] = false;
                        component.push(member);
                        if member == node {
                            break;
                        }
                    }
                    components.push(component);
                }
            }
        }
    }
    components
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> StableId {
        [n; 32]
    }

    fn imports(pairs: &[(u8, u8)]) -> Vec<Edge> {
        pairs
            .iter()
            .enumerate()
            .map(|(i, (a, b))| {
                let mut edge_id = [0u8; 32];
                edge_id[..8].copy_from_slice(&(i as u64).to_le_bytes());
                Edge { id: edge_id, from: id(*a), to: id(*b), edge_type: EdgeType::Imports, meta: 0 }
            })
            .collect()
    }

    #[test]
    fn a_two_file_cycle_is_found() {
        assert_eq!(import_cycles(&imports(&[(1, 2), (2, 1)])), vec![vec![id(1), id(2)]]);
    }

    #[test]
    fn acyclic_graphs_and_self_imports_report_nothing() {
        assert!(import_cycles(&imports(&[(1, 2), (2, 3), (1, 3)])).is_empty());
        assert!(import_cycles(&imports(&[(1, 1)])).is_empty());
        assert!(import_cycles(&[]).is_empty());
    }

    #[test]
    fn longer_cycles_and_separate_components_are_reported_largest_first() {
        // 1 -> 2 -> 3 -> 1 (size 3); 7 <-> 8 (size 2); 3 -> 4 leaves the cycle.
        let cycles = import_cycles(&imports(&[(1, 2), (2, 3), (3, 1), (3, 4), (7, 8), (8, 7)]));
        assert_eq!(cycles, vec![vec![id(1), id(2), id(3)], vec![id(7), id(8)]]);
    }

    #[test]
    fn only_import_edges_count() {
        let mut edges = imports(&[(1, 2)]);
        edges.push(Edge { id: [9; 32], from: id(2), to: id(1), edge_type: EdgeType::Calls, meta: 0 });
        assert!(import_cycles(&edges).is_empty(), "a call back is not an import cycle");
    }

    #[test]
    fn deep_chains_do_not_overflow_the_stack() {
        let pairs: Vec<(u8, u8)> = (0u8..200).map(|i| (i, i + 1)).chain(std::iter::once((200, 0))).collect();
        let cycles = import_cycles(&imports(&pairs));
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].len(), 201);
    }
}

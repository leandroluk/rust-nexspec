//! Hybrid retrieval: RRF seed discovery (REQ-405) + bounded k-hop CSR
//! expansion (REQ-406). Lives at the crate root, not under `search::`/
//! `graph::`/`vector::` — it composes all three, same boundary reasoning
//! already applied to `sync_orchestrator.rs` since Fase 2.

use std::collections::HashSet;

use crate::graph::csr::Csr;
use crate::graph::edge::EdgeType;
use crate::sync::mutation::StableId;

/// Breadth-first expansion from `seeds` along `edge_types`, up to
/// `max_depth` hops, over the CSR's merged base+delta view
/// (`Csr::edges_from`, unchanged since Fase 1). Returns the seeds plus
/// every node reached, each exactly once, order not significant beyond
/// "seeds first, then hop 1, then hop 2, ...".
pub fn expand(seeds: &[StableId], csr: &Csr, edge_types: &[EdgeType], max_depth: u8) -> Vec<StableId> {
    let mut visited: HashSet<StableId> = seeds.iter().copied().collect();
    let mut result: Vec<StableId> = seeds.to_vec();
    let mut frontier: Vec<StableId> = seeds.to_vec();

    for _ in 0..max_depth {
        if frontier.is_empty() {
            break;
        }
        let mut next_frontier = Vec::new();
        for node in &frontier {
            for edge_type in edge_types {
                for edge in csr.edges_from(node, *edge_type) {
                    if visited.insert(edge.to) {
                        next_frontier.push(edge.to);
                        result.push(edge.to);
                    }
                }
            }
        }
        frontier = next_frontier;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::csr::CsrBase;
    use crate::graph::edge::Edge;
    use tempfile::NamedTempFile;

    fn edge(from: StableId, to: StableId, edge_type: EdgeType) -> Edge {
        Edge {
            id: [0u8; 32],
            from,
            to,
            edge_type,
        }
    }

    // seed -> a -> b -> c, all via DependsOn.
    fn three_hop_csr() -> (NamedTempFile, Csr) {
        let seed = [1u8; 32];
        let a = [2u8; 32];
        let b = [3u8; 32];
        let c = [4u8; 32];
        let edges = vec![
            edge(seed, a, EdgeType::DependsOn),
            edge(a, b, EdgeType::DependsOn),
            edge(b, c, EdgeType::DependsOn),
        ];
        let file = NamedTempFile::new().unwrap();
        CsrBase::build(&edges, file.path()).unwrap();
        let base = CsrBase::open(file.path()).unwrap();
        (file, Csr::new(base))
    }

    #[test]
    fn depth_1_returns_only_direct_neighbors() {
        let (_file, csr) = three_hop_csr();
        let seed = [1u8; 32];
        let result = expand(&[seed], &csr, &[EdgeType::DependsOn], 1);

        assert_eq!(result, vec![seed, [2u8; 32]]);
    }

    #[test]
    fn depth_2_includes_the_second_hop() {
        let (_file, csr) = three_hop_csr();
        let seed = [1u8; 32];
        let result = expand(&[seed], &csr, &[EdgeType::DependsOn], 2);

        assert_eq!(result, vec![seed, [2u8; 32], [3u8; 32]]);
    }

    #[test]
    fn node_beyond_max_depth_is_excluded() {
        let (_file, csr) = three_hop_csr();
        let seed = [1u8; 32];
        let result = expand(&[seed], &csr, &[EdgeType::DependsOn], 2);

        assert!(!result.contains(&[4u8; 32]), "c is 3 hops away, outside depth 2");
    }
}

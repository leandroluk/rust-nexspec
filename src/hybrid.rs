//! Hybrid retrieval: RRF seed discovery (REQ-405) + bounded k-hop CSR
//! expansion (REQ-406). Lives at the crate root, not under `search::`/
//! `graph::`/`vector::` — it composes all three, same boundary reasoning
//! already applied to `sync_orchestrator.rs` since Fase 2.

use std::collections::{HashMap, HashSet};

use crate::graph::csr::Csr;
use crate::graph::edge::EdgeType;
use crate::sync::mutation::StableId;

/// The `k` constant from the standard RRF formula (`1 / (k + rank)`) —
/// 60 is the value from the original paper and the one most hybrid-search
/// implementations default to; it damps the influence of very high ranks
/// without needing per-corpus tuning.
const RRF_K: f32 = 60.0;

/// Reciprocal Rank Fusion (REQ-405) of two already-ranked id lists (rank 0
/// = best). A document present in only one list still contributes — it
/// isn't dropped just because the other signal never saw it — but a
/// document ranked well in *both* naturally outranks one ranked well in
/// only one, since its score is a sum over both contributions. Results are
/// sorted by descending fused score. Equal weights; see
/// [`seed_discovery_weighted`] for the production weighting.
pub fn seed_discovery(bm25_ranked: &[StableId], hnsw_ranked: &[StableId]) -> Vec<(StableId, f32)> {
    seed_discovery_weighted(bm25_ranked, hnsw_ranked, FusionWeights { bm25: 1.0, vector: 1.0 })
}

/// How much each retrieval signal counts in the fusion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FusionWeights {
    pub bm25: f32,
    pub vector: f32,
}

/// Production weights. Measured on the retrieval benchmark
/// (`.specs/features/retrieval-benchmark/design.md`, T-808d): an equal-weight
/// fusion lowered MRR on locate questions (condominium 0.87 -> 0.64) because
/// the embedding model ranks identifier-heavy text poorly, while a weight of
/// 0.1 kept MRR at the BM25-only level. The vector signal therefore acts as
/// a low-weight assistant: it never outranks a strong lexical hit, but still
/// surfaces results BM25 cannot reach.
impl Default for FusionWeights {
    fn default() -> Self {
        Self { bm25: 1.0, vector: 0.1 }
    }
}

impl FusionWeights {
    /// Defaults, with the vector weight overridable through
    /// `NEXSPEC_VECTOR_WEIGHT` (a non-negative number) so the benchmark can
    /// compare settings without a rebuild.
    pub fn from_env() -> Self {
        let vector = std::env::var("NEXSPEC_VECTOR_WEIGHT")
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .filter(|w| w.is_finite() && *w >= 0.0);
        match vector {
            Some(vector) => Self { vector, ..Self::default() },
            None => Self::default(),
        }
    }
}

/// [`seed_discovery`] with explicit per-signal weights (`weight / (k + rank)`).
pub fn seed_discovery_weighted(bm25_ranked: &[StableId], hnsw_ranked: &[StableId], weights: FusionWeights) -> Vec<(StableId, f32)> {
    let mut scores: HashMap<StableId, f32> = HashMap::new();
    for (rank, id) in bm25_ranked.iter().enumerate() {
        *scores.entry(*id).or_insert(0.0) += weights.bm25 / (RRF_K + (rank + 1) as f32);
    }
    for (rank, id) in hnsw_ranked.iter().enumerate() {
        *scores.entry(*id).or_insert(0.0) += weights.vector / (RRF_K + (rank + 1) as f32);
    }

    let mut result: Vec<(StableId, f32)> = scores.into_iter().collect();
    result.sort_by(|a, b| b.1.partial_cmp(&a.1).expect("RRF scores are always finite"));
    result
}

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
            meta: 0,
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

    #[test]
    fn rrf_gives_both_documents_a_chance_and_rewards_agreement() {
        let bm25_only = [1u8; 32]; // #1 in BM25, absent from HNSW
        let hnsw_only = [2u8; 32]; // #1 in HNSW, absent from BM25
        let both = [3u8; 32]; // #1 in both

        let bm25_ranked = vec![bm25_only, both];
        let hnsw_ranked = vec![hnsw_only, both];

        let fused = seed_discovery(&bm25_ranked, &hnsw_ranked);
        let ids: Vec<StableId> = fused.iter().map(|(id, _)| *id).collect();

        assert!(ids.contains(&bm25_only), "BM25-only hit must not be dropped");
        assert!(ids.contains(&hnsw_only), "HNSW-only hit must not be dropped");
        assert_eq!(fused[0].0, both, "a document ranked #1 in both signals must win the fusion");

        let bm25_only_score = fused.iter().find(|(id, _)| *id == bm25_only).unwrap().1;
        let hnsw_only_score = fused.iter().find(|(id, _)| *id == hnsw_only).unwrap().1;
        assert!(
            fused[0].1 > bm25_only_score && fused[0].1 > hnsw_only_score,
            "agreement between both signals must outscore either alone"
        );
    }

    #[test]
    fn top_ranked_in_both_lists_wins_the_fusion() {
        let winner = [9u8; 32];
        let bm25_ranked = vec![winner, [8u8; 32], [7u8; 32]];
        let hnsw_ranked = vec![winner, [6u8; 32]];

        let fused = seed_discovery(&bm25_ranked, &hnsw_ranked);
        assert_eq!(fused[0].0, winner);
    }

    #[test]
    fn low_vector_weight_keeps_lexical_hits_on_top_but_still_surfaces_vector_only_ones() {
        let lexical_first = [1u8; 32];
        let lexical_second = [2u8; 32];
        let vector_only = [3u8; 32];
        let bm25 = [lexical_first, lexical_second];
        let vector = [vector_only, lexical_second];

        // Equal weights: the vector's top pick ties into the lead.
        let equal = seed_discovery(&bm25, &vector);
        assert_eq!(equal[0].0, lexical_second, "present in both lists");

        let weighted = seed_discovery_weighted(&bm25, &vector, FusionWeights { bm25: 1.0, vector: 0.1 });
        let order: Vec<StableId> = weighted.iter().map(|(id, _)| *id).collect();
        assert_eq!(order, vec![lexical_second, lexical_first, vector_only]);
        // Agreement between both signals still counts a little, but the vector's
        // own top pick stays below every lexical hit.
        assert!(order.contains(&vector_only), "a vector-only hit is not dropped");
    }

    #[test]
    fn zero_vector_weight_equals_bm25_only_ranking() {
        let ids: Vec<StableId> = (1u8..=4).map(|b| [b; 32]).collect();
        let weighted = seed_discovery_weighted(&ids, &[ids[3], ids[2]], FusionWeights { bm25: 1.0, vector: 0.0 });
        let order: Vec<StableId> = weighted.iter().map(|(id, _)| *id).collect();
        assert_eq!(order, ids);
    }

    #[test]
    fn vector_weight_env_override_is_parsed_and_invalid_values_ignored() {
        // Single test touching the environment: no other test reads this var.
        unsafe { std::env::set_var("NEXSPEC_VECTOR_WEIGHT", "0.5") };
        assert_eq!(FusionWeights::from_env().vector, 0.5);
        unsafe { std::env::set_var("NEXSPEC_VECTOR_WEIGHT", "-1") };
        assert_eq!(FusionWeights::from_env(), FusionWeights::default());
        unsafe { std::env::set_var("NEXSPEC_VECTOR_WEIGHT", "abc") };
        assert_eq!(FusionWeights::from_env(), FusionWeights::default());
        unsafe { std::env::remove_var("NEXSPEC_VECTOR_WEIGHT") };
        assert_eq!(FusionWeights::from_env().vector, 0.1);
    }
}

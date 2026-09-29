//! Parallel multi-file extraction (REQ-305 in
//! `.specs/features/ast-lexical-search/spec.md`) — cold-start (first index
//! of a repo, or a large batch) parses files concurrently via `rayon`,
//! since Tree-sitter parsing is embarrassingly parallel per file. Merging
//! results back into one [`MutationSet`] stays sequential (cheap:
//! `Vec::extend`), matching REQ-007/REQ-205's "one atomic cycle" — this
//! function never touches `Coordinator::stage()` itself.

use std::collections::HashMap;
use std::path::PathBuf;

use rayon::prelude::*;

use crate::code::parser::{CodeError, Language, extract};
use crate::sync::mutation::{MutationSet, StableId};

/// Parse every `(path, source, language)` tuple in parallel and merge the
/// results into one [`MutationSet`] — identical output to calling
/// [`extract`] on each file individually and concatenating, just faster.
/// Fails fast on the first file that errors (consistent with the rest of
/// the crate's error handling — no partial/best-effort mode).
pub fn extract_all(
    files: &[(PathBuf, String, Language)],
    known_markers: &HashMap<String, StableId>,
) -> Result<MutationSet, CodeError> {
    let results: Vec<MutationSet> = files
        .par_iter()
        .map(|(path, source, language)| extract(source, *language, path, known_markers))
        .collect::<Result<Vec<_>, _>>()?;

    let mut combined = MutationSet::default();
    for set in results {
        combined.nodes.extend(set.nodes);
        combined.edges.extend(set.edges);
        combined.docs.extend(set.docs);
    }
    Ok(combined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_sequential_extraction_over_the_same_files() {
        let files = vec![
            (
                PathBuf::from("a.rs"),
                "fn a() {}\n".to_string(),
                Language::Rust,
            ),
            (
                PathBuf::from("b.py"),
                "def b():\n    pass\n".to_string(),
                Language::Python,
            ),
            (
                PathBuf::from("c.go"),
                "package main\nfunc c() {}\n".to_string(),
                Language::Go,
            ),
        ];

        let known = HashMap::new();
        let parallel = extract_all(&files, &known).unwrap();

        let mut sequential = MutationSet::default();
        for (path, source, language) in &files {
            let set = extract(source, *language, path, &known).unwrap();
            sequential.nodes.extend(set.nodes);
            sequential.edges.extend(set.edges);
            sequential.docs.extend(set.docs);
        }

        let mut parallel_node_ids: Vec<_> = parallel
            .nodes
            .iter()
            .map(|n| match n {
                crate::sync::mutation::NodeMutation::Upsert { id, .. } => *id,
                crate::sync::mutation::NodeMutation::Remove { id } => *id,
            })
            .collect();
        let mut sequential_node_ids: Vec<_> = sequential
            .nodes
            .iter()
            .map(|n| match n {
                crate::sync::mutation::NodeMutation::Upsert { id, .. } => *id,
                crate::sync::mutation::NodeMutation::Remove { id } => *id,
            })
            .collect();
        parallel_node_ids.sort();
        sequential_node_ids.sort();

        assert_eq!(parallel_node_ids, sequential_node_ids);
        assert_eq!(parallel.nodes.len(), 3, "one symbol per file in this fixture");
    }
}

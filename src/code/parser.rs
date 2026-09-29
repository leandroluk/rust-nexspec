//! Symbol extraction from source code via Tree-sitter (REQ-301, REQ-302 in
//! `.specs/features/ast-lexical-search/spec.md`). One `Query` per
//! [`Language`], selecting function/method/type definitions.

use std::path::Path;

use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

use crate::graph::node::NodePayload;
use crate::sync::mutation::{MutationSet, NodeMutation, StableId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    TypeScript,
    JavaScript,
    Python,
    Go,
    Rust,
}

impl Language {
    pub fn from_extension(path: &Path) -> Option<Self> {
        match path.extension().and_then(|e| e.to_str())? {
            "ts" | "tsx" => Some(Language::TypeScript),
            "js" | "jsx" | "mjs" | "cjs" => Some(Language::JavaScript),
            "py" => Some(Language::Python),
            "go" => Some(Language::Go),
            "rs" => Some(Language::Rust),
            _ => None,
        }
    }

    fn ts_language(self) -> tree_sitter::Language {
        match self {
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Language::Python => tree_sitter_python::LANGUAGE.into(),
            Language::Go => tree_sitter_go::LANGUAGE.into(),
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
        }
    }

    /// Captures `@name` (the identifier) and `@def` (the whole definition,
    /// for its line range) for every function/method/type this language
    /// grammar exposes at a granularity worth indexing (REQ-302's decision:
    /// no variable/field-level symbols — too much noise).
    fn symbol_query(self) -> &'static str {
        match self {
            Language::Rust => {
                "(function_item name: (_) @name) @def
                 (struct_item name: (_) @name) @def
                 (enum_item name: (_) @name) @def
                 (trait_item name: (_) @name) @def"
            }
            Language::Python => {
                "(function_definition name: (_) @name) @def
                 (class_definition name: (_) @name) @def"
            }
            Language::Go => {
                "(function_declaration name: (_) @name) @def
                 (method_declaration name: (_) @name) @def
                 (type_spec name: (_) @name) @def"
            }
            Language::JavaScript => {
                "(function_declaration name: (_) @name) @def
                 (class_declaration name: (_) @name) @def
                 (method_definition name: (_) @name) @def"
            }
            Language::TypeScript => {
                "(function_declaration name: (_) @name) @def
                 (class_declaration name: (_) @name) @def
                 (method_definition name: (_) @name) @def
                 (interface_declaration name: (_) @name) @def"
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CodeError {
    #[error("failed to set tree-sitter language: {0}")]
    Language(String),
    #[error("failed to parse source (tree-sitter returned no tree)")]
    Parse,
    #[error("invalid tree-sitter query for this language: {0}")]
    Query(String),
}

fn stable_id(bytes: &[u8]) -> StableId {
    *blake3::hash(bytes).as_bytes()
}

fn rkyv_bytes(payload: &NodePayload) -> Vec<u8> {
    rkyv::to_bytes::<rkyv::rancor::Error>(payload)
        .expect("NodePayload must always serialize")
        .to_vec()
}

/// Parse `source` and return one [`crate::graph::node::NodeType::Symbol`]
/// node per function/method/type definition found. Does not yet attach
/// `DefinedIn`/`DependsOn`/`Satisfies` edges — see T-303/T-304.
pub fn extract(source: &str, language: Language) -> Result<MutationSet, CodeError> {
    let ts_language = language.ts_language();
    let mut parser = Parser::new();
    parser
        .set_language(&ts_language)
        .map_err(|e| CodeError::Language(e.to_string()))?;
    let tree = parser.parse(source, None).ok_or(CodeError::Parse)?;

    let query = Query::new(&ts_language, language.symbol_query())
        .map_err(|e| CodeError::Query(e.to_string()))?;
    let name_ix = query
        .capture_index_for_name("name")
        .expect("every symbol_query defines a @name capture");
    let def_ix = query
        .capture_index_for_name("def")
        .expect("every symbol_query defines a @def capture");

    let source_bytes = source.as_bytes();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source_bytes);

    let mut nodes = Vec::new();
    while let Some(m) = matches.next() {
        let name_text = m
            .captures()
            .iter()
            .find(|c| c.index == name_ix)
            .and_then(|c| c.node.utf8_text(source_bytes).ok());
        let def_node = m.captures().iter().find(|c| c.index == def_ix).map(|c| c.node);

        if let (Some(name), Some(def_node)) = (name_text, def_node) {
            let line_start = def_node.start_position().row as u32;
            let line_end = def_node.end_position().row as u32;
            let id = stable_id(format!("{name}@{line_start}").as_bytes());
            nodes.push(NodeMutation::Upsert {
                id,
                payload: rkyv_bytes(&NodePayload::Symbol {
                    name: name.to_string(),
                    source_hash: id,
                    line_start,
                    line_end,
                }),
            });
        }
    }

    Ok(MutationSet {
        nodes,
        edges: Vec::new(),
        docs: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol_names(set: &MutationSet) -> Vec<String> {
        set.nodes
            .iter()
            .map(|m| match m {
                NodeMutation::Upsert { payload, .. } => {
                    let mut aligned = rkyv::util::AlignedVec::<16>::new();
                    aligned.extend_from_slice(payload);
                    let decoded =
                        rkyv::from_bytes::<NodePayload, rkyv::rancor::Error>(&aligned).unwrap();
                    match decoded {
                        NodePayload::Symbol { name, .. } => name,
                        other => panic!("expected Symbol payload, got {other:?}"),
                    }
                }
                NodeMutation::Remove { .. } => panic!("extract() only upserts"),
            })
            .collect()
    }

    #[test]
    fn extracts_function_and_type_per_language() {
        let cases: &[(Language, &str)] = &[
            (Language::Rust, "fn hello() {}\nstruct Point { x: i32 }\n"),
            (Language::Python, "def hello():\n    pass\n\nclass Point:\n    pass\n"),
            (Language::Go, "package main\nfunc hello() {}\ntype Point struct { X int }\n"),
            (Language::JavaScript, "function hello() {}\nclass Point {}\n"),
            (Language::TypeScript, "function hello(): void {}\nclass Point {}\n"),
        ];

        for (language, source) in cases {
            let set = extract(source, *language).unwrap();
            let mut names = symbol_names(&set);
            names.sort();
            assert_eq!(
                names,
                vec!["Point".to_string(), "hello".to_string()],
                "language {language:?} produced unexpected symbols: {names:?}"
            );
        }
    }
}

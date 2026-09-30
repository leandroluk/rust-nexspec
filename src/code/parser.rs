//! Symbol extraction from source code via Tree-sitter (REQ-301, REQ-302 in
//! `.specs/features/ast-lexical-search/spec.md`). One `Query` per
//! [`Language`], selecting function/method/type definitions.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

use crate::graph::edge::EdgeType;
use crate::graph::markdown::{find_markers, marker_node_id};
use crate::graph::node::{NodePayload, file_node_id};
use crate::sync::mutation::{EdgeMutation, MutationSet, NodeMutation, StableId};

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

    pub(crate) fn ts_language(self) -> tree_sitter::Language {
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
    pub(crate) fn symbol_query(self) -> &'static str {
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
                 (abstract_class_declaration name: (_) @name) @def
                 (method_definition name: (_) @name) @def
                 (interface_declaration name: (_) @name) @def
                 (enum_declaration name: (_) @name) @def
                 (type_alias_declaration name: (_) @name) @def"
            }
        }
    }

    /// Captures `@callee` for direct-name-identifier call sites — matches
    /// REQ-303's same-file-only scope (no method-call/selector resolution,
    /// no import following).
    fn call_query(self) -> &'static str {
        match self {
            Language::Rust => "(call_expression function: (identifier) @callee)",
            Language::Python => "(call function: (identifier) @callee)",
            Language::Go => "(call_expression function: (identifier) @callee)",
            Language::JavaScript | Language::TypeScript => {
                "(call_expression function: (identifier) @callee)"
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

/// Queries compiled once per language and shared by every `extract` call
/// (and every rayon worker): compiling them per file dominated cold-start
/// time (`.specs/features/performance-guard/design.md`, measurement T-903).
struct CompiledQueries {
    symbol: Query,
    call: Query,
    name_ix: u32,
    def_ix: u32,
    callee_ix: u32,
}

fn compiled_queries(language: Language) -> Result<&'static CompiledQueries, CodeError> {
    static CELLS: [OnceLock<Result<CompiledQueries, String>>; 5] =
        [const { OnceLock::new() }; 5];
    let slot = match language {
        Language::TypeScript => 0,
        Language::JavaScript => 1,
        Language::Python => 2,
        Language::Go => 3,
        Language::Rust => 4,
    };
    CELLS[slot]
        .get_or_init(|| {
            let ts_language = language.ts_language();
            let symbol = Query::new(&ts_language, language.symbol_query()).map_err(|e| e.to_string())?;
            let call = Query::new(&ts_language, language.call_query()).map_err(|e| e.to_string())?;
            let name_ix = symbol
                .capture_index_for_name("name")
                .expect("every symbol_query defines a @name capture");
            let def_ix = symbol
                .capture_index_for_name("def")
                .expect("every symbol_query defines a @def capture");
            let callee_ix = call
                .capture_index_for_name("callee")
                .expect("every call_query defines a @callee capture");
            Ok(CompiledQueries { symbol, call, name_ix, def_ix, callee_ix })
        })
        .as_ref()
        .map_err(|e| CodeError::Query(e.clone()))
}

fn stable_id(bytes: &[u8]) -> StableId {
    *blake3::hash(bytes).as_bytes()
}

fn rkyv_bytes(payload: &NodePayload) -> Vec<u8> {
    rkyv::to_bytes::<rkyv::rancor::Error>(payload)
        .expect("NodePayload must always serialize")
        .to_vec()
}

struct SymbolInfo {
    id: StableId,
    name: String,
    start_byte: usize,
    end_byte: usize,
}

fn edge_id(kind: &str, from: &StableId, to: &StableId) -> StableId {
    let mut bytes = Vec::with_capacity(kind.len() + 65);
    bytes.extend_from_slice(kind.as_bytes());
    bytes.push(b':');
    bytes.extend_from_slice(from);
    bytes.extend_from_slice(to);
    stable_id(&bytes)
}

/// Parse `source` (the file at `path`) and return: one
/// [`crate::graph::node::NodeType::Symbol`] node per function/method/type
/// definition (REQ-302), a `DefinedIn` edge from each symbol to the file
/// node (REQ-303), a `DependsOn` edge for each call site whose callee
/// resolves to another symbol *in the same file* (REQ-303's documented
/// same-file-only scope), and a `Satisfies` edge for each symbol whose
/// immediately preceding comment mentions `@spec REQ-XXX`/`@adr ADR-XXX`
/// **and** that marker is a key in `known_markers` (REQ-304) — a symbol's
/// own id can be computed locally, but a requirement/ADR's id depends on
/// its body text (see `graph::markdown::extract`), which this function has
/// no way to see; the caller (which already ran `markdown::extract` over
/// `.specs/`) supplies the resolved marker → id map.
pub fn extract(
    source: &str,
    language: Language,
    path: &Path,
    known_markers: &HashMap<String, StableId>,
) -> Result<MutationSet, CodeError> {
    let ts_language = language.ts_language();
    let mut parser = Parser::new();
    parser
        .set_language(&ts_language)
        .map_err(|e| CodeError::Language(e.to_string()))?;
    let tree = parser.parse(source, None).ok_or(CodeError::Parse)?;
    let source_bytes = source.as_bytes();
    let file_id = file_node_id(&path.to_string_lossy());

    let queries = compiled_queries(language)?;
    let (name_ix, def_ix) = (queries.name_ix, queries.def_ix);

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut symbols: Vec<SymbolInfo> = Vec::new();

    {
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&queries.symbol, tree.root_node(), source_bytes);
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
                edges.push(EdgeMutation::Upsert {
                    id: edge_id("defined-in", &id, &file_id),
                    from: id,
                    to: file_id,
                    edge_type: EdgeType::DefinedIn.to_code(),
                    payload: Vec::new(),
                });

                if let Some(comment) = def_node
                    .prev_sibling()
                    .filter(|s| s.kind().contains("comment"))
                    && let Ok(comment_text) = comment.utf8_text(source_bytes)
                {
                    let mut markers = find_markers(comment_text, "REQ-");
                    markers.extend(find_markers(comment_text, "ADR-"));
                    for marker in markers {
                        // Known in this sync cycle, or resolved by its
                        // deterministic marker id (spec indexed earlier).
                        let target = known_markers
                            .get(&marker)
                            .copied()
                            .unwrap_or_else(|| marker_node_id(&marker));
                        edges.push(EdgeMutation::Upsert {
                            id: edge_id("satisfies", &id, &target),
                            from: id,
                            to: target,
                            edge_type: EdgeType::Satisfies.to_code(),
                            payload: Vec::new(),
                        });
                    }
                }

                symbols.push(SymbolInfo {
                    id,
                    name: name.to_string(),
                    start_byte: def_node.start_byte(),
                    end_byte: def_node.end_byte(),
                });
            }
        }
    }

    let callee_ix = queries.callee_ix;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&queries.call, tree.root_node(), source_bytes);
    while let Some(m) = matches.next() {
        let Some(callee_cap) = m.captures().iter().find(|c| c.index == callee_ix) else {
            continue;
        };
        let Ok(callee_name) = callee_cap.node.utf8_text(source_bytes) else {
            continue;
        };
        let call_byte = callee_cap.node.start_byte();
        let caller = symbols
            .iter()
            .find(|s| s.start_byte <= call_byte && call_byte < s.end_byte);
        let callee = symbols.iter().find(|s| s.name == callee_name);
        if let (Some(caller), Some(callee)) = (caller, callee)
            && caller.id != callee.id
        {
            edges.push(EdgeMutation::Upsert {
                id: edge_id("depends-on", &caller.id, &callee.id),
                from: caller.id,
                to: callee.id,
                edge_type: EdgeType::DependsOn.to_code(),
                payload: Vec::new(),
            });
        }
    }

    Ok(MutationSet {
        nodes,
        edges,
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
    fn compiled_queries_are_built_once_per_language() {
        for language in [
            Language::TypeScript,
            Language::JavaScript,
            Language::Python,
            Language::Go,
            Language::Rust,
        ] {
            let first = compiled_queries(language).unwrap() as *const CompiledQueries;
            let second = compiled_queries(language).unwrap() as *const CompiledQueries;
            assert_eq!(first, second, "{language:?} queries must be cached, not recompiled per file");
        }
    }

    fn extract_at(source: &str, language: Language) -> MutationSet {
        extract(source, language, Path::new("test_file"), &HashMap::new()).unwrap()
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
            let set = extract_at(source, *language);
            let mut names = symbol_names(&set);
            names.sort();
            assert_eq!(
                names,
                vec!["Point".to_string(), "hello".to_string()],
                "language {language:?} produced unexpected symbols: {names:?}"
            );
        }
    }

    #[test]
    fn typescript_extracts_abstract_class_enum_and_type_alias() {
        let set = extract_at(
            "export abstract class Reader {}
export enum Kind { A }
export type Id = string;
",
            Language::TypeScript,
        );
        let mut names = symbol_names(&set);
        names.sort();
        assert_eq!(names, vec!["Id", "Kind", "Reader"]);
    }

    fn symbol_id(set: &MutationSet, name: &str) -> StableId {
        set.nodes
            .iter()
            .find_map(|m| match m {
                NodeMutation::Upsert { id, payload } => {
                    let mut aligned = rkyv::util::AlignedVec::<16>::new();
                    aligned.extend_from_slice(payload);
                    let decoded =
                        rkyv::from_bytes::<NodePayload, rkyv::rancor::Error>(&aligned).unwrap();
                    match decoded {
                        NodePayload::Symbol { name: n, .. } if n == name => Some(*id),
                        _ => None,
                    }
                }
                NodeMutation::Remove { .. } => None,
            })
            .unwrap_or_else(|| panic!("no symbol named {name:?}"))
    }

    fn has_edge(set: &MutationSet, from: StableId, to: StableId, edge_type: EdgeType) -> bool {
        set.edges.iter().any(|e| match e {
            EdgeMutation::Upsert {
                from: f,
                to: t,
                edge_type: et,
                ..
            } => *f == from && *t == to && *et == edge_type.to_code(),
            EdgeMutation::Remove { .. } => false,
        })
    }

    #[test]
    fn defined_in_edge_points_from_every_symbol_to_the_file() {
        let set = extract(
            "fn a() {}\nfn b() {}\n",
            Language::Rust,
            Path::new("src/lib.rs"),
            &HashMap::new(),
        )
        .unwrap();
        let file_id = file_node_id("src/lib.rs");
        let a = symbol_id(&set, "a");
        let b = symbol_id(&set, "b");
        assert!(has_edge(&set, a, file_id, EdgeType::DefinedIn));
        assert!(has_edge(&set, b, file_id, EdgeType::DefinedIn));
    }

    #[test]
    fn depends_on_edge_links_caller_to_callee_in_same_file() {
        let set = extract_at("fn a() { b(); }\nfn b() {}\n", Language::Rust);
        let a = symbol_id(&set, "a");
        let b = symbol_id(&set, "b");
        assert!(has_edge(&set, a, b, EdgeType::DependsOn));
    }

    #[test]
    fn call_to_unresolved_function_produces_no_edge_and_no_error() {
        let set = extract_at("fn a() { unknown_function(); }\n", Language::Rust);
        assert!(set.edges.iter().all(|e| !matches!(e,
            EdgeMutation::Upsert { edge_type, .. } if *edge_type == EdgeType::DependsOn.to_code()
        )));
    }

    #[test]
    fn satisfies_edge_links_symbol_to_known_requirement() {
        let req_id: StableId = [42u8; 32];
        let mut known = HashMap::new();
        known.insert("REQ-701".to_string(), req_id);

        let source = "// @spec REQ-701\nfn f() {}\n";
        let set = extract(source, Language::Rust, Path::new("f.rs"), &known).unwrap();

        let f = symbol_id(&set, "f");
        assert!(has_edge(&set, f, req_id, EdgeType::Satisfies));
    }

    #[test]
    fn satisfies_edge_resolves_unknown_marker_by_deterministic_id() {
        // The spec defining REQ-999 was indexed in an earlier sync cycle, so
        // it is not in `known_markers`: the edge still lands on its stable id.
        let source = "// @spec REQ-999\nfn f() {}\n";
        let set = extract_at(source, Language::Rust);
        let f = symbol_id(&set, "f");
        assert!(has_edge(&set, f, marker_node_id("REQ-999"), EdgeType::Satisfies));
    }
}

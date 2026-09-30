//! Which edges a query may follow (REQ-1101, REQ-1104, REQ-1105): relations
//! by stable lowercase names, a minimum confidence and a set of contexts.

use crate::graph::edge::{Confidence, Edge, EdgeContext, EdgeType};

/// Relation names accepted on the command line and in MCP parameters.
pub const RELATION_NAMES: &[&str] = &[
    "imports",
    "reexports",
    "calls",
    "instantiates",
    "extends",
    "references",
    "satisfies",
    "implements",
    "defined_in",
    "depends_on",
    "cochanges",
    "dependencies",
];

/// Edge types a bare name stands for, or `None` when the name is unknown.
/// `dependencies` is every dependency-flavoured type.
pub fn relation_types(name: &str) -> Option<Vec<EdgeType>> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "imports" => vec![EdgeType::Imports],
        "reexports" | "re_exports" | "re-exports" => vec![EdgeType::ReExports],
        "calls" => vec![EdgeType::Calls],
        "instantiates" => vec![EdgeType::Instantiates],
        "extends" => vec![EdgeType::Extends],
        "references" => vec![EdgeType::References],
        "satisfies" => vec![EdgeType::Satisfies],
        "implements" => vec![EdgeType::Implements],
        "defined_in" | "defined-in" => vec![EdgeType::DefinedIn],
        "depends_on" | "depends-on" => vec![EdgeType::DependsOn],
        "cochanges" | "co_changes" | "co-changes" => vec![EdgeType::CoChanges],
        "dependencies" | "dependency" => EdgeType::DEPENDENCY_TYPES.to_vec(),
        _ => return None,
    })
}

/// Lowercase stable name of an edge type, as printed in query output.
pub fn relation_name(edge_type: EdgeType) -> &'static str {
    match edge_type {
        EdgeType::Imports => "imports",
        EdgeType::ReExports => "reexports",
        EdgeType::Calls => "calls",
        EdgeType::Instantiates => "instantiates",
        EdgeType::Extends => "extends",
        EdgeType::References => "references",
        EdgeType::Satisfies => "satisfies",
        EdgeType::Implements => "implements",
        EdgeType::DefinedIn => "defined_in",
        EdgeType::DependsOn => "depends_on",
        EdgeType::CoChanges => "cochanges",
    }
}

pub fn context_name(context: EdgeContext) -> &'static str {
    match context {
        EdgeContext::Runtime => "runtime",
        EdgeContext::TypeOnly => "type-only",
        EdgeContext::Test => "test",
        EdgeContext::Spec => "spec",
    }
}

pub fn parse_context(name: &str) -> Option<EdgeContext> {
    match name.trim().to_ascii_lowercase().as_str() {
        "runtime" => Some(EdgeContext::Runtime),
        "type-only" | "type_only" | "typeonly" | "type" => Some(EdgeContext::TypeOnly),
        "test" => Some(EdgeContext::Test),
        "spec" => Some(EdgeContext::Spec),
        _ => None,
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FilterError {
    #[error("unknown relation {0:?} (expected one of: {expected})", expected = RELATION_NAMES.join(", "))]
    UnknownRelation(String),
    #[error("unknown context {0:?} (expected runtime, type-only, test or spec)")]
    UnknownContext(String),
    #[error("unknown confidence {0:?} (expected extracted or inferred)")]
    UnknownConfidence(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeFilter {
    /// Allowed edge types (never empty).
    pub types: Vec<EdgeType>,
    /// `Extracted` drops every inferred edge; `Inferred` keeps both.
    pub min_confidence: Confidence,
    /// Allowed contexts; empty means all.
    pub contexts: Vec<EdgeContext>,
}

impl Default for EdgeFilter {
    /// Dependencies plus `Satisfies`/`Implements`; never co-change or `DefinedIn`
    /// unless asked for.
    fn default() -> Self {
        let mut types = EdgeType::DEPENDENCY_TYPES.to_vec();
        types.extend([EdgeType::Satisfies, EdgeType::Implements]);
        Self { types, min_confidence: Confidence::Inferred, contexts: Vec::new() }
    }
}

impl EdgeFilter {
    /// Builds a filter from user-facing strings; an empty `relations` keeps the default types.
    pub fn from_strings(relations: &[String], min_confidence: Option<&str>, contexts: &[String]) -> Result<Self, FilterError> {
        let mut filter = EdgeFilter::default();
        if !relations.is_empty() {
            let mut types = Vec::new();
            for name in relations {
                for ty in relation_types(name).ok_or_else(|| FilterError::UnknownRelation(name.clone()))? {
                    if !types.contains(&ty) {
                        types.push(ty);
                    }
                }
            }
            filter.types = types;
        }
        if let Some(level) = min_confidence {
            filter.min_confidence = match level.trim().to_ascii_lowercase().as_str() {
                "extracted" => Confidence::Extracted,
                "inferred" => Confidence::Inferred,
                other => return Err(FilterError::UnknownConfidence(other.to_string())),
            };
        }
        for name in contexts {
            let context = parse_context(name).ok_or_else(|| FilterError::UnknownContext(name.clone()))?;
            if !filter.contexts.contains(&context) {
                filter.contexts.push(context);
            }
        }
        Ok(filter)
    }

    pub fn allows(&self, edge: &Edge) -> bool {
        if !self.types.contains(&edge.edge_type) {
            return false;
        }
        if self.min_confidence == Confidence::Extracted && edge.confidence() == Confidence::Inferred {
            return false;
        }
        self.contexts.is_empty() || self.contexts.contains(&edge.context())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::edge::encode_meta;

    fn edge(edge_type: EdgeType, confidence: Confidence, context: EdgeContext) -> Edge {
        Edge { id: [0; 32], from: [1; 32], to: [2; 32], edge_type, meta: encode_meta(confidence, context) }
    }

    #[test]
    fn default_filter_follows_dependencies_satisfies_and_implements_only() {
        let f = EdgeFilter::default();
        for ty in EdgeType::DEPENDENCY_TYPES {
            assert!(f.allows(&edge(ty, Confidence::Extracted, EdgeContext::Runtime)), "{ty:?}");
        }
        assert!(f.allows(&edge(EdgeType::Satisfies, Confidence::Extracted, EdgeContext::Runtime)));
        assert!(!f.allows(&edge(EdgeType::CoChanges, Confidence::Extracted, EdgeContext::Runtime)));
        assert!(!f.allows(&edge(EdgeType::DefinedIn, Confidence::Extracted, EdgeContext::Runtime)));
    }

    #[test]
    fn relations_confidence_and_contexts_narrow_the_filter() {
        let f = EdgeFilter::from_strings(
            &["imports".to_string(), "calls".to_string()],
            Some("extracted"),
            &["runtime".to_string(), "type".to_string()],
        )
        .unwrap();
        assert!(f.allows(&edge(EdgeType::Imports, Confidence::Extracted, EdgeContext::Runtime)));
        assert!(f.allows(&edge(EdgeType::Calls, Confidence::Extracted, EdgeContext::TypeOnly)));
        assert!(!f.allows(&edge(EdgeType::Extends, Confidence::Extracted, EdgeContext::Runtime)), "relation not requested");
        assert!(!f.allows(&edge(EdgeType::Imports, Confidence::Inferred, EdgeContext::Runtime)), "inferred is dropped");
        assert!(!f.allows(&edge(EdgeType::Imports, Confidence::Extracted, EdgeContext::Test)), "context not requested");
    }

    #[test]
    fn the_dependencies_alias_expands_and_cochanges_can_be_asked_for() {
        let f = EdgeFilter::from_strings(&["dependencies".to_string(), "cochanges".to_string()], None, &[]).unwrap();
        assert!(f.allows(&edge(EdgeType::Extends, Confidence::Inferred, EdgeContext::Spec)));
        assert!(f.allows(&edge(EdgeType::CoChanges, Confidence::Extracted, EdgeContext::Runtime)));
        assert!(!f.allows(&edge(EdgeType::Satisfies, Confidence::Extracted, EdgeContext::Runtime)));
    }

    #[test]
    fn unknown_names_fail_with_a_message_listing_the_valid_ones() {
        let err = EdgeFilter::from_strings(&["imoprts".to_string()], None, &[]).unwrap_err();
        assert_eq!(err, FilterError::UnknownRelation("imoprts".into()));
        assert!(err.to_string().contains("imports") && err.to_string().contains("calls"), "{err}");
        assert!(matches!(EdgeFilter::from_strings(&[], Some("sure"), &[]), Err(FilterError::UnknownConfidence(_))));
        assert!(matches!(EdgeFilter::from_strings(&[], None, &["prod".to_string()]), Err(FilterError::UnknownContext(_))));
    }

    #[test]
    fn names_roundtrip_for_every_edge_type() {
        for ty in [
            EdgeType::Imports, EdgeType::ReExports, EdgeType::Calls, EdgeType::Instantiates, EdgeType::Extends,
            EdgeType::References, EdgeType::Satisfies, EdgeType::Implements, EdgeType::DefinedIn, EdgeType::DependsOn,
            EdgeType::CoChanges,
        ] {
            assert_eq!(relation_types(relation_name(ty)), Some(vec![ty]), "{ty:?}");
        }
        assert_eq!(parse_context(context_name(EdgeContext::TypeOnly)), Some(EdgeContext::TypeOnly));
    }
}

//! AST signature pruner (REQ-501 in
//! `.specs/features/token-budgeting/spec.md`). Strips a code symbol's
//! implementation body, keeping only its signature/header.

use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

use crate::code::parser::Language;

/// Re-parses `source` (the file `line_start..=line_end` came from — both
/// 0-indexed, inclusive, matching [`crate::graph::node::NodePayload::Symbol`])
/// and, if a matching definition is found, replaces its body with a short
/// placeholder (`{ ... }` for brace-delimited bodies, `...` for
/// colon-introduced ones like Python). Falls back to the original,
/// unmodified line range whenever the definition can't be re-located (source
/// changed since indexing) or has no identifiable body (e.g. a type alias).
pub fn prune_symbol(source: &str, language: Language, line_start: u32, line_end: u32) -> String {
    let original = || -> String {
        source
            .lines()
            .skip(line_start as usize)
            .take((line_end - line_start + 1) as usize)
            .collect::<Vec<_>>()
            .join("\n")
    };

    let ts_language = language.ts_language();
    let mut parser = Parser::new();
    if parser.set_language(&ts_language).is_err() {
        return original();
    }
    let Some(tree) = parser.parse(source, None) else {
        return original();
    };
    let source_bytes = source.as_bytes();

    let Ok(query) = Query::new(&ts_language, language.symbol_query()) else {
        return original();
    };
    let Some(def_ix) = query.capture_index_for_name("def") else {
        return original();
    };

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source_bytes);
    let mut def_node = None;
    while let Some(m) = matches.next() {
        if let Some(cap) = m.captures().iter().find(|c| c.index == def_ix) {
            let n = cap.node;
            if n.start_position().row as u32 == line_start && n.end_position().row as u32 == line_end {
                def_node = Some(n);
                break;
            }
        }
    }
    let Some(def_node) = def_node else {
        return original();
    };

    let mut body = None;
    let mut child_cursor = def_node.walk();
    for child in def_node.children(&mut child_cursor) {
        let text = child.utf8_text(source_bytes).unwrap_or("");
        if text.trim_start().starts_with('{') || child.kind() == "block" {
            body = Some(child);
        }
    }
    let Some(body) = body else {
        return original();
    };

    let before = std::str::from_utf8(&source_bytes[def_node.start_byte()..body.start_byte()])
        .unwrap_or("")
        .trim_end();
    let body_text = body.utf8_text(source_bytes).unwrap_or("");
    let placeholder = if body_text.trim_start().starts_with('{') {
        "{ ... }"
    } else {
        "..."
    };
    format!("{before} {placeholder}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_body_but_keeps_signature_per_language() {
        let cases: &[(Language, &str, &str)] = &[
            (Language::Rust, "fn hello(x: i32) -> i32 {\n    x + 1\n}\n", "fn hello(x: i32) -> i32 { ... }"),
            (
                Language::Python,
                "def hello(x):\n    return x + 1\n",
                "def hello(x): ...",
            ),
            (Language::Go, "func hello(x int) int {\n\treturn x + 1\n}\n", "func hello(x int) int { ... }"),
            (
                Language::JavaScript,
                "function hello(x) {\n    return x + 1;\n}\n",
                "function hello(x) { ... }",
            ),
            (
                Language::TypeScript,
                "function hello(x: number): number {\n    return x + 1;\n}\n",
                "function hello(x: number): number { ... }",
            ),
        ];

        for (language, source, expected) in cases {
            let line_end = source.lines().count() as u32 - 1;
            let pruned = prune_symbol(source, *language, 0, line_end);
            assert_eq!(&pruned, expected, "language {language:?} produced unexpected pruning");
        }
    }

    #[test]
    fn symbol_without_a_body_falls_through_unchanged() {
        let source = "struct Unit;\n";
        let pruned = prune_symbol(source, Language::Rust, 0, 0);
        assert_eq!(pruned, "struct Unit;");
    }

    #[test]
    fn line_range_not_matching_any_definition_falls_through_to_raw_lines() {
        let source = "fn hello() {\n    1\n}\n";
        // line 5 doesn't exist as a definition -- falls back to whatever raw
        // lines are in range (empty here, since the file has only 3 lines).
        let pruned = prune_symbol(source, Language::Rust, 10, 10);
        assert_eq!(pruned, "");
    }
}

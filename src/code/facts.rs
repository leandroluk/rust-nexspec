//! File facts for TS/JS (REQ-701, REQ-711 in
//! `.specs/features/dependency-edges/spec.md`): what a file imports and
//! re-exports, and where it uses the names it imported. Facts are collected
//! from the Tree-sitter tree without resolving anything; turning specifiers
//! into paths and usages into edges happens later (`resolve`, `deps`).

use std::collections::HashSet;

use tree_sitter::{Node, Tree};

use crate::code::parser::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    /// `import ... from "x"` / `import "x"` / `import x = require("x")`.
    Static,
    /// `export ... from "x"`, `export * from "x"`.
    ReExport,
    /// `import("x")`.
    Dynamic,
    /// `require("x")`.
    Require,
}

/// One name brought in (or re-exported) by an import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// Name in the other module: an export name, `"default"`, or `"*"` for a namespace.
    pub imported: String,
    /// Name visible in this file (the alias when there is one).
    pub local: String,
    pub type_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFact {
    pub specifier: String,
    pub kind: ImportKind,
    /// `import type ...` / `export type ... from`.
    pub type_only: bool,
    pub bindings: Vec<Binding>,
}

/// How an imported name is used at one site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageRole {
    Extends,
    Implements,
    /// `new X(...)`.
    New,
    /// `X(...)` or `ns.f(...)`.
    Call,
    /// `@X` / `@X(...)`.
    Decorator,
    /// Only in a type position.
    Type,
    Other,
}

/// A use of a name imported into this file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    /// The local name (an import alias when one was given).
    pub name: String,
    pub byte: usize,
    pub role: UsageRole,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileFacts {
    pub imports: Vec<ImportFact>,
    pub usages: Vec<Usage>,
}

/// Parses `source` and collects its facts. Non-TS/JS languages have none.
pub fn extract_facts(source: &str, language: Language) -> FileFacts {
    if !language.has_module_facts() {
        return FileFacts::default();
    }
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language.ts_language()).is_err() {
        return FileFacts::default();
    }
    match parser.parse(source, None) {
        Some(tree) => collect(&tree, source),
        None => FileFacts::default(),
    }
}

/// Collects facts from an already parsed tree.
pub fn collect(tree: &Tree, source: &str) -> FileFacts {
    let bytes = source.as_bytes();
    let mut imports = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "import_statement" => {
                if let Some(fact) = import_statement(node, bytes) {
                    imports.push(fact);
                }
                continue;
            }
            "export_statement" if node.child_by_field_name("source").is_some() => {
                if let Some(fact) = reexport_statement(node, bytes) {
                    imports.push(fact);
                }
                continue;
            }
            "call_expression" => {
                if let Some(fact) = dynamic_or_require(node, bytes) {
                    imports.push(fact);
                }
            }
            _ => {}
        }
        push_children_reversed(node, &mut stack);
    }
    // The stack is LIFO and children are pushed reversed, so `imports` is in
    // document order already.

    let locals: HashSet<&str> = imports
        .iter()
        .filter(|i| i.kind != ImportKind::ReExport)
        .flat_map(|i| i.bindings.iter().map(|b| b.local.as_str()))
        .collect();
    let mut usages = Vec::new();
    if !locals.is_empty() {
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "import_statement" => continue,
                "export_statement" if node.child_by_field_name("source").is_some() => continue,
                "identifier" | "type_identifier" | "shorthand_property_identifier" => {
                    if let Ok(text) = node.utf8_text(bytes)
                        && locals.contains(text)
                    {
                        usages.push(Usage { name: text.to_string(), byte: node.start_byte(), role: role_of(node) });
                    }
                    continue;
                }
                _ => {}
            }
            push_children_reversed(node, &mut stack);
        }
    }
    FileFacts { imports, usages }
}

fn push_children_reversed<'t>(node: Node<'t>, stack: &mut Vec<Node<'t>>) {
    let mut cursor = node.walk();
    let children: Vec<Node<'t>> = node.children(&mut cursor).collect();
    stack.extend(children.into_iter().rev());
}

fn text<'a>(node: Node<'_>, bytes: &'a [u8]) -> Option<&'a str> {
    node.utf8_text(bytes).ok()
}

/// The contents of a plain string literal (`"x"`, `'x'`); template strings are not literals.
fn string_value(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }
    let raw = text(node, bytes)?;
    let inner = raw.get(1..raw.len().checked_sub(1)?)?;
    Some(inner.to_string())
}

fn has_type_keyword(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|c| c.kind() == "type" && !c.is_named())
}

fn import_statement(node: Node<'_>, bytes: &[u8]) -> Option<ImportFact> {
    let mut cursor = node.walk();
    let children: Vec<Node<'_>> = node.children(&mut cursor).collect();
    let type_only = children.iter().any(|c| c.kind() == "type" && !c.is_named());

    let mut specifier = node.child_by_field_name("source").and_then(|s| string_value(s, bytes));
    let mut bindings = Vec::new();
    for child in &children {
        match child.kind() {
            "import_clause" => {
                let mut c = child.walk();
                for part in child.children(&mut c) {
                    match part.kind() {
                        "identifier" => {
                            if let Some(local) = text(part, bytes) {
                                bindings.push(Binding { imported: "default".into(), local: local.into(), type_only });
                            }
                        }
                        "namespace_import" => {
                            let mut nc = part.walk();
                            let name = part.children(&mut nc).find(|n| n.kind() == "identifier");
                            if let Some(local) = name.and_then(|n| text(n, bytes)) {
                                bindings.push(Binding { imported: "*".into(), local: local.into(), type_only });
                            }
                        }
                        "named_imports" => {
                            let mut sc = part.walk();
                            for spec in part.children(&mut sc).filter(|n| n.kind() == "import_specifier") {
                                if let Some(b) = specifier_binding(spec, bytes, type_only) {
                                    bindings.push(b);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            // `import x = require("y")`
            "import_require_clause" => {
                let mut c = child.walk();
                let parts: Vec<Node<'_>> = child.children(&mut c).collect();
                let local = parts.iter().find(|p| p.kind() == "identifier").and_then(|p| text(*p, bytes));
                let source = parts.iter().find_map(|p| string_value(*p, bytes));
                if let (Some(local), Some(source)) = (local, source) {
                    bindings.push(Binding { imported: "*".into(), local: local.into(), type_only });
                    specifier = Some(source);
                }
            }
            _ => {}
        }
    }
    Some(ImportFact { specifier: specifier?, kind: ImportKind::Static, type_only, bindings })
}

/// `{ a }`, `{ a as b }`, `{ type a }` (import) or the same in an export clause.
fn specifier_binding(spec: Node<'_>, bytes: &[u8], statement_type_only: bool) -> Option<Binding> {
    let name = spec.child_by_field_name("name").and_then(|n| text(n, bytes))?;
    let alias = spec.child_by_field_name("alias").and_then(|n| text(n, bytes));
    Some(Binding {
        imported: name.trim_matches(['"', '\'']).to_string(),
        local: alias.unwrap_or(name).trim_matches(['"', '\'']).to_string(),
        type_only: statement_type_only || has_type_keyword(spec),
    })
}

fn reexport_statement(node: Node<'_>, bytes: &[u8]) -> Option<ImportFact> {
    let specifier = string_value(node.child_by_field_name("source")?, bytes)?;
    let type_only = has_type_keyword(node);
    let mut bindings = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "*" => bindings.push(Binding { imported: "*".into(), local: "*".into(), type_only }),
            "namespace_export" => {
                let mut c = child.walk();
                let name = child.children(&mut c).find(|n| n.kind() == "identifier");
                if let Some(local) = name.and_then(|n| text(n, bytes)) {
                    bindings.push(Binding { imported: "*".into(), local: local.into(), type_only });
                }
            }
            "export_clause" => {
                let mut c = child.walk();
                for spec in child.children(&mut c).filter(|n| n.kind() == "export_specifier") {
                    if let Some(b) = specifier_binding(spec, bytes, type_only) {
                        bindings.push(b);
                    }
                }
            }
            _ => {}
        }
    }
    Some(ImportFact { specifier, kind: ImportKind::ReExport, type_only, bindings })
}

/// `import("x")` and `require("x")` with a literal specifier.
fn dynamic_or_require(node: Node<'_>, bytes: &[u8]) -> Option<ImportFact> {
    let function = node.child_by_field_name("function")?;
    let kind = match function.kind() {
        "import" => ImportKind::Dynamic,
        "identifier" if text(function, bytes) == Some("require") => ImportKind::Require,
        _ => return None,
    };
    let arguments = node.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let specifier = arguments.named_children(&mut cursor).next().and_then(|a| string_value(a, bytes))?;
    Some(ImportFact { specifier, kind, type_only: false, bindings: Vec::new() })
}

/// Syntactic role of an identifier, judged from its immediate surroundings.
fn role_of(node: Node<'_>) -> UsageRole {
    // `Base<T>` and `ns.Base` are still "the name Base" for the parent's purposes.
    let mut current = node;
    if let Some(parent) = node.parent() {
        let as_member_name = matches!(parent.kind(), "generic_type" | "nested_type_identifier" | "member_expression")
            && parent.child_by_field_name("name") == Some(node);
        let as_member_object = matches!(parent.kind(), "member_expression" | "nested_identifier")
            && parent.child_by_field_name("object") == Some(node);
        if as_member_name || as_member_object {
            current = parent;
        }
    }
    let Some(parent) = current.parent() else { return UsageRole::Other };
    match parent.kind() {
        "extends_clause" | "extends_type_clause" => UsageRole::Extends,
        "implements_clause" => UsageRole::Implements,
        "decorator" => UsageRole::Decorator,
        "new_expression" if parent.child_by_field_name("constructor") == Some(current) => UsageRole::New,
        "call_expression" if parent.child_by_field_name("function") == Some(current) => {
            if parent.parent().is_some_and(|g| g.kind() == "decorator") {
                UsageRole::Decorator
            } else {
                UsageRole::Call
            }
        }
        _ if node.kind() == "type_identifier" => UsageRole::Type,
        _ => UsageRole::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(source: &str) -> FileFacts {
        extract_facts(source, Language::TypeScript)
    }

    fn only(source: &str) -> ImportFact {
        let f = facts(source);
        assert_eq!(f.imports.len(), 1, "{:?}", f.imports);
        f.imports.into_iter().next().unwrap()
    }

    fn binding(imported: &str, local: &str, type_only: bool) -> Binding {
        Binding { imported: imported.into(), local: local.into(), type_only }
    }

    #[test]
    fn named_default_alias_and_namespace_imports() {
        let i = only("import Def, { a, b as bee } from './mod';");
        assert_eq!(i.specifier, "./mod");
        assert_eq!(i.kind, ImportKind::Static);
        assert!(!i.type_only);
        assert_eq!(i.bindings, vec![binding("default", "Def", false), binding("a", "a", false), binding("b", "bee", false)]);

        let ns = only("import * as utils from \"../utils\";");
        assert_eq!(ns.specifier, "../utils");
        assert_eq!(ns.bindings, vec![binding("*", "utils", false)]);
    }

    #[test]
    fn side_effect_import_has_no_bindings() {
        let i = only("import './polyfill';");
        assert_eq!(i.specifier, "./polyfill");
        assert!(i.bindings.is_empty());
    }

    #[test]
    fn type_only_imports_are_flagged_per_statement_and_per_specifier() {
        let whole = only("import type { T, U } from './types';");
        assert!(whole.type_only);
        assert!(whole.bindings.iter().all(|b| b.type_only));

        let mixed = only("import { a, type B } from './m';");
        assert!(!mixed.type_only);
        assert_eq!(mixed.bindings, vec![binding("a", "a", false), binding("B", "B", true)]);
    }

    #[test]
    fn reexports_named_star_and_namespace() {
        let f = facts("export { a as b } from './a';\nexport * from './b';\nexport * as ns from './c';\nexport type { T } from './t';\n");
        assert_eq!(f.imports.len(), 4);
        assert!(f.imports.iter().all(|i| i.kind == ImportKind::ReExport));
        assert_eq!(f.imports[0].bindings, vec![binding("a", "b", false)]);
        assert_eq!(f.imports[1].bindings, vec![binding("*", "*", false)]);
        assert_eq!(f.imports[2].bindings, vec![binding("*", "ns", false)]);
        assert!(f.imports[3].type_only);
    }

    #[test]
    fn dynamic_import_and_require_with_literals_only() {
        let f = facts("const m = await import('./lazy');\nconst x = require('./cjs');\nimport(someVar);\nrequire(name);\n");
        assert_eq!(f.imports.len(), 2);
        assert_eq!((f.imports[0].specifier.as_str(), f.imports[0].kind), ("./lazy", ImportKind::Dynamic));
        assert_eq!((f.imports[1].specifier.as_str(), f.imports[1].kind), ("./cjs", ImportKind::Require));
    }

    #[test]
    fn import_equals_require() {
        let i = only("import fs = require('fs');");
        assert_eq!(i.specifier, "fs");
        assert_eq!(i.bindings, vec![binding("*", "fs", false)]);
    }

    #[test]
    fn js_files_use_the_same_facts() {
        let f = extract_facts("import { a } from './a.js';\nmodule.exports = a();\n", Language::JavaScript);
        assert_eq!(f.imports.len(), 1);
        assert_eq!(f.usages.len(), 1);
        assert_eq!(f.usages[0].role, UsageRole::Call);
    }

    #[test]
    fn usage_roles_cover_heritage_new_call_decorator_and_types() {
        let f = facts(
            "import { Base, Iface, Thing, helper, Dep, Inj, Shape } from './m';\n\
             @Inj()\n\
             class A extends Base implements Iface {\n\
               constructor(private dep: Dep) {}\n\
               run() { const t = new Thing(); helper(); return t; }\n\
               shape: Shape;\n\
             }\n",
        );
        let role = |name: &str| f.usages.iter().find(|u| u.name == name).map(|u| u.role);
        assert_eq!(role("Base"), Some(UsageRole::Extends));
        assert_eq!(role("Iface"), Some(UsageRole::Implements));
        assert_eq!(role("Thing"), Some(UsageRole::New));
        assert_eq!(role("helper"), Some(UsageRole::Call));
        assert_eq!(role("Inj"), Some(UsageRole::Decorator));
        assert_eq!(role("Dep"), Some(UsageRole::Type), "constructor parameter type");
        assert_eq!(role("Shape"), Some(UsageRole::Type));
    }

    #[test]
    fn namespace_member_calls_count_as_calls_of_the_namespace() {
        let f = facts("import * as u from './u';\nexport function f() { return u.compute(); }\n");
        let usage = f.usages.iter().find(|u| u.name == "u").expect("namespace use");
        assert_eq!(usage.role, UsageRole::Call);
    }

    #[test]
    fn property_names_and_unrelated_identifiers_are_not_usages() {
        let f = facts("import { name } from './m';\nconst o = { name: 1 };\nconsole.log(o.name);\nconst other = 2;\n");
        assert!(f.usages.is_empty(), "{:?}", f.usages);
    }

    #[test]
    fn imports_themselves_are_not_counted_as_usages() {
        let f = facts("import { a } from './a';\nexport { a } from './a';\n");
        assert!(f.usages.is_empty(), "{:?}", f.usages);
    }

    #[test]
    fn tsx_components_are_usages_of_their_imports() {
        let f = extract_facts(
            "import { Button } from './button';
export const App = () => <Button label=\"x\" />;
",
            Language::Tsx,
        );
        assert_eq!(f.imports.len(), 1);
        assert!(f.usages.iter().any(|u| u.name == "Button"), "{:?}", f.usages);
    }

    #[test]
    fn languages_without_modules_have_no_facts() {
        assert_eq!(extract_facts("use std::fmt;", Language::Rust), FileFacts::default());
    }
}

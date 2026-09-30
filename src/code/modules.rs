//! Module imports for Rust, Python and Go (REQ-705 in
//! `.specs/features/dependency-edges/spec.md`): file-level only. Facts carry
//! the language's own notion of a specifier (`crate::a::B`, `.utils`,
//! `example.com/mod/pkg`) and [`resolve`] turns it into tracked files.
//! Symbol-level links stay TS/JS-only until they show a clear gain.

use std::collections::HashMap;

use tree_sitter::{Node, Tree};

use crate::code::facts::{FileFacts, ImportFact, ImportKind};
use crate::code::parser::Language;

fn fact(specifier: String) -> ImportFact {
    ImportFact { specifier, kind: ImportKind::Static, type_only: false, bindings: Vec::new() }
}

/// Imports of a non-TS/JS file, from its parsed tree.
pub fn collect_other(language: Language, tree: &Tree, source: &str) -> FileFacts {
    let bytes = source.as_bytes();
    let mut specifiers: Vec<String> = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        match (language, node.kind()) {
            (Language::Rust, "use_declaration") => {
                if let Some(argument) = node.child_by_field_name("argument") {
                    expand_rust_use(argument, "", bytes, &mut specifiers);
                }
                continue;
            }
            (Language::Rust, "mod_item") if node.child_by_field_name("body").is_none() => {
                if let Some(name) = node.child_by_field_name("name").and_then(|n| n.utf8_text(bytes).ok()) {
                    specifiers.push(format!("mod:{name}"));
                }
                continue;
            }
            (Language::Python, "import_statement") => {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    let dotted = if child.kind() == "aliased_import" { child.child_by_field_name("name") } else { Some(child) };
                    if let Some(text) = dotted.and_then(|n| n.utf8_text(bytes).ok()) {
                        specifiers.push(text.to_string());
                    }
                }
                continue;
            }
            (Language::Python, "import_from_statement") => {
                if let Some(module) = node.child_by_field_name("module_name").and_then(|n| n.utf8_text(bytes).ok()) {
                    specifiers.push(module.to_string());
                    // `from . import x` / `from .. import y`: the names may be sibling modules.
                    if module.chars().all(|c| c == '.') {
                        let mut cursor = node.walk();
                        for name in node.children_by_field_name("name", &mut cursor) {
                            let target = if name.kind() == "aliased_import" { name.child_by_field_name("name") } else { Some(name) };
                            if let Some(text) = target.and_then(|n| n.utf8_text(bytes).ok()) {
                                specifiers.push(format!("{module}{text}"));
                            }
                        }
                    }
                }
                continue;
            }
            (Language::Go, "import_spec") => {
                if let Some(path) = node.child_by_field_name("path").and_then(|n| n.utf8_text(bytes).ok()) {
                    specifiers.push(path.trim_matches(['"', '`']).to_string());
                }
                continue;
            }
            _ => {}
        }
        let mut cursor = node.walk();
        let children: Vec<Node<'_>> = node.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    let mut seen = std::collections::HashSet::new();
    FileFacts {
        imports: specifiers.into_iter().filter(|s| seen.insert(s.clone())).map(fact).collect(),
        usages: Vec::new(),
    }
}

/// Every path a `use` declaration names (`use a::{b, c::d}` -> `a::b`, `a::c::d`).
fn expand_rust_use(node: Node<'_>, prefix: &str, bytes: &[u8], out: &mut Vec<String>) {
    let join = |tail: &str| if prefix.is_empty() { tail.to_string() } else { format!("{prefix}::{tail}") };
    match node.kind() {
        "scoped_identifier" | "identifier" | "crate" | "self" | "super" => {
            if let Ok(text) = node.utf8_text(bytes) {
                out.push(if text == "self" && !prefix.is_empty() { prefix.to_string() } else { join(text) });
            }
        }
        "use_wildcard" => {
            // `path::*`
            let mut cursor = node.walk();
            if let Some(path) = node.named_children(&mut cursor).next().and_then(|n| n.utf8_text(bytes).ok()) {
                out.push(join(path));
            }
        }
        "use_as_clause" => {
            if let Some(path) = node.child_by_field_name("path") {
                expand_rust_use(path, prefix, bytes, out);
            }
        }
        "scoped_use_list" => {
            let path = node.child_by_field_name("path").and_then(|n| n.utf8_text(bytes).ok()).unwrap_or_default();
            let new_prefix = join(path);
            if let Some(list) = node.child_by_field_name("list") {
                expand_rust_use(list, &new_prefix, bytes, out);
            }
        }
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                expand_rust_use(child, prefix, bytes, out);
            }
        }
        _ => {}
    }
}

/// Facts about the repository that language resolvers need.
#[derive(Default)]
pub struct ModuleContext {
    /// Rust crate name (hyphens as underscores) -> directory of its `Cargo.toml`.
    pub cargo_crates: HashMap<String, String>,
    /// Directory of a `go.mod` -> its module path.
    pub go_modules: HashMap<String, String>,
}

impl ModuleContext {
    pub fn add_cargo_toml(&mut self, dir: &str, text: &str) {
        if let Ok(value) = text.parse::<toml::Table>()
            && let Some(name) = value.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str())
        {
            self.cargo_crates.insert(name.replace('-', "_"), dir.to_string());
        }
    }

    pub fn add_go_mod(&mut self, dir: &str, text: &str) {
        let module = text
            .lines()
            .find_map(|l| l.trim().strip_prefix("module "))
            .map(|m| m.trim().trim_matches('"').to_string());
        if let Some(module) = module {
            self.go_modules.insert(dir.to_string(), module);
        }
    }
}

fn split_dir(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

fn join_path(dir: &str, rest: &str) -> String {
    if dir.is_empty() { rest.to_string() } else if rest.is_empty() { dir.to_string() } else { format!("{dir}/{rest}") }
}

fn nearest_ancestor<'a>(dir: &str, candidates: impl Iterator<Item = &'a String>) -> Option<&'a String> {
    candidates
        .filter(|c| c.is_empty() || dir == c.as_str() || dir.starts_with(&format!("{c}/")))
        .max_by_key(|c| c.len())
}

/// Tracked files a specifier written in `from` refers to (Rust, Python, Go).
pub fn resolve(
    context: &ModuleContext,
    files: &std::collections::HashSet<String>,
    from: &str,
    specifier: &str,
) -> Vec<String> {
    match from.rsplit_once('.').map(|(_, e)| e) {
        Some("rs") => resolve_rust(context, files, from, specifier).into_iter().collect(),
        Some("py") => resolve_python(files, from, specifier).into_iter().collect(),
        Some("go") => resolve_go(context, files, from, specifier),
        _ => Vec::new(),
    }
}

/// Module path of a Rust file inside its crate: `src/graph/csr/base.rs` -> `["graph", "csr", "base"]`.
fn rust_module_path(file: &str, crate_dir: &str) -> Option<Vec<String>> {
    let relative = if crate_dir.is_empty() { file } else { file.strip_prefix(&format!("{crate_dir}/"))? };
    let relative = relative.strip_prefix("src/")?;
    let mut segments: Vec<String> = relative.strip_suffix(".rs")?.split('/').map(str::to_string).collect();
    if matches!(segments.last().map(String::as_str), Some("mod" | "lib" | "main")) && segments.len() > 1 || segments == ["lib"] || segments == ["main"] {
        segments.pop();
    }
    Some(segments)
}

/// First tracked file for module path `segments` under `crate_dir/src`, trying
/// the longest prefix first (the tail is usually an item, not a module).
fn rust_module_file(files: &std::collections::HashSet<String>, crate_dir: &str, segments: &[String]) -> Option<String> {
    let src = join_path(crate_dir, "src");
    for take in (0..=segments.len()).rev() {
        let joined = segments[..take].join("/");
        let base = join_path(&src, &joined);
        let candidates: Vec<String> = if take == 0 {
            vec![join_path(&src, "lib.rs"), join_path(&src, "main.rs")]
        } else {
            vec![format!("{base}.rs"), format!("{base}/mod.rs")]
        };
        if let Some(found) = candidates.into_iter().find(|c| files.contains(c)) {
            return Some(found);
        }
    }
    None
}

fn resolve_rust(context: &ModuleContext, files: &std::collections::HashSet<String>, from: &str, specifier: &str) -> Option<String> {
    let (from_dir, _) = split_dir(from);

    if let Some(name) = specifier.strip_prefix("mod:") {
        // `mod name;` -> sibling file, or a file under this module's directory.
        let stem = from.rsplit('/').next()?.strip_suffix(".rs")?;
        let mut dirs = vec![from_dir.to_string()];
        if !matches!(stem, "mod" | "lib" | "main") {
            dirs.push(join_path(from_dir, stem));
        }
        for dir in dirs {
            for candidate in [join_path(&dir, &format!("{name}.rs")), join_path(&dir, &format!("{name}/mod.rs"))] {
                if files.contains(&candidate) {
                    return Some(candidate);
                }
            }
        }
        return None;
    }

    let mut parts = specifier.split("::");
    let first = parts.next()?;
    let rest: Vec<String> = parts.map(str::to_string).collect();

    let crate_dirs: Vec<String> = context.cargo_crates.values().cloned().collect();
    let own_crate_dir = nearest_ancestor(from_dir, crate_dirs.iter())?.clone();

    let (crate_dir, module_path): (String, Vec<String>) = match first {
        "crate" => (own_crate_dir, rest),
        "self" | "super" => {
            let mut here = rust_module_path(from, &own_crate_dir)?;
            let mut tail = rest;
            if first == "super" {
                here.pop();
            }
            while tail.first().map(String::as_str) == Some("super") {
                here.pop();
                tail.remove(0);
            }
            here.extend(tail);
            (own_crate_dir, here)
        }
        // `use nexspec::engine::Engine` from tests/benches/examples/other crates.
        other => (context.cargo_crates.get(other)?.clone(), rest),
    };
    let found = rust_module_file(files, &crate_dir, &module_path)?;
    (found != from).then_some(found)
}

fn resolve_python(files: &std::collections::HashSet<String>, from: &str, specifier: &str) -> Option<String> {
    let (from_dir, _) = split_dir(from);
    let dots = specifier.chars().take_while(|c| *c == '.').count();
    let module = &specifier[dots..];
    let as_path = module.replace('.', "/");

    let probe = |base: &str| -> Option<String> {
        let candidates = if as_path.is_empty() {
            vec![join_path(base, "__init__.py")]
        } else {
            vec![join_path(base, &format!("{as_path}.py")), join_path(base, &format!("{as_path}/__init__.py"))]
        };
        candidates.into_iter().find(|c| files.contains(c))
    };

    if dots > 0 {
        let mut base = from_dir.to_string();
        for _ in 1..dots {
            base = split_dir(&base).0.to_string();
        }
        return probe(&base);
    }
    if as_path.is_empty() {
        return None;
    }
    // Absolute import: accept only an unambiguous match anywhere in the repo.
    let wanted = [format!("{as_path}.py"), format!("{as_path}/__init__.py")];
    let mut matches = files.iter().filter(|f| {
        wanted.iter().any(|w| f.as_str() == w.as_str() || f.ends_with(&format!("/{w}")))
    });
    let first = matches.next()?;
    matches.next().is_none().then(|| first.clone())
}

fn resolve_go(context: &ModuleContext, files: &std::collections::HashSet<String>, from: &str, specifier: &str) -> Vec<String> {
    let (from_dir, _) = split_dir(from);
    let mod_dirs: Vec<String> = context.go_modules.keys().cloned().collect();
    let Some(mod_dir) = nearest_ancestor(from_dir, mod_dirs.iter()) else { return Vec::new() };
    let Some(module) = context.go_modules.get(mod_dir) else { return Vec::new() };
    let Some(rest) = specifier.strip_prefix(module.as_str()) else { return Vec::new() };
    let package_dir = join_path(mod_dir, rest.trim_start_matches('/'));
    let mut found: Vec<String> = files
        .iter()
        .filter(|f| f.ends_with(".go") && !f.ends_with("_test.go") && split_dir(f).0 == package_dir && f.as_str() != from)
        .cloned()
        .collect();
    found.sort();
    found.truncate(30);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn facts(language: Language, source: &str) -> Vec<String> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language.ts_language()).unwrap();
        let tree = parser.parse(source, None).unwrap();
        collect_other(language, &tree, source).imports.into_iter().map(|i| i.specifier).collect()
    }

    fn set(paths: &[&str]) -> HashSet<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn rust_use_declarations_expand_groups_aliases_wildcards_and_mods() {
        let got = facts(
            Language::Rust,
            "use crate::graph::csr::Csr;\nuse crate::sync::{mutation::MutationSet, participant::{self, SyncError}};\nuse super::thing as t;\nuse crate::hybrid::*;\nuse std::fmt;\nmod helper;\nmod inline { fn x() {} }\n",
        );
        for expected in [
            "crate::graph::csr::Csr",
            "crate::sync::mutation::MutationSet",
            "crate::sync::participant",
            "crate::sync::participant::SyncError",
            "super::thing",
            "crate::hybrid",
            "std::fmt",
            "mod:helper",
        ] {
            assert!(got.iter().any(|g| g == expected), "{expected} missing from {got:?}");
        }
        assert!(!got.iter().any(|g| g == "mod:inline"), "inline modules are not files");
    }

    #[test]
    fn rust_paths_resolve_through_crate_self_super_mod_and_crate_names() {
        let mut context = ModuleContext::default();
        context.add_cargo_toml("", "[package]\nname = \"nexspec\"\n");
        let files = set(&[
            "Cargo.toml",
            "src/lib.rs",
            "src/engine.rs",
            "src/hybrid.rs",
            "src/graph/mod.rs",
            "src/graph/csr/mod.rs",
            "src/graph/csr/base.rs",
            "src/graph/csr/delta.rs",
            "tests/it.rs",
        ]);
        let r = |from: &str, spec: &str| resolve(&context, &files, from, spec);
        assert_eq!(r("src/engine.rs", "crate::hybrid::seed_discovery"), vec!["src/hybrid.rs"], "item name is dropped");
        assert_eq!(r("src/engine.rs", "crate::graph::csr::delta::CsrDelta"), vec!["src/graph/csr/delta.rs"]);
        assert_eq!(r("src/engine.rs", "crate::graph::csr::Csr"), vec!["src/graph/csr/mod.rs"]);
        assert_eq!(r("src/graph/csr/base.rs", "super::delta::CsrDelta"), vec!["src/graph/csr/delta.rs"]);
        assert_eq!(r("src/graph/csr/base.rs", "super::super::csr"), vec!["src/graph/csr/mod.rs"]);
        assert_eq!(r("src/graph/csr/mod.rs", "self::base::CsrBase"), vec!["src/graph/csr/base.rs"]);
        assert_eq!(r("src/lib.rs", "mod:engine"), vec!["src/engine.rs"]);
        assert_eq!(r("src/graph/mod.rs", "mod:csr"), vec!["src/graph/csr/mod.rs"]);
        assert_eq!(r("tests/it.rs", "nexspec::engine::Engine"), vec!["src/engine.rs"], "integration tests import by crate name");
        assert!(r("src/engine.rs", "std::fmt").is_empty(), "external crates resolve to nothing");
        assert!(r("src/engine.rs", "serde::Serialize").is_empty());
    }

    #[test]
    fn python_imports_relative_and_absolute() {
        let got = facts(Language::Python, "import os\nimport pkg.util as u\nfrom pkg.core import thing\nfrom . import sibling\nfrom ..shared import helper\n");
        for expected in ["os", "pkg.util", "pkg.core", ".", ".sibling", "..shared"] {
            assert!(got.iter().any(|g| g == expected), "{expected} missing from {got:?}");
        }
        let files = set(&["app/main.py", "app/pkg/util.py", "app/pkg/core/__init__.py", "app/sibling.py", "shared.py", "other/util.py"]);
        let context = ModuleContext::default();
        let r = |from: &str, spec: &str| resolve(&context, &files, from, spec);
        assert_eq!(r("app/main.py", ".sibling"), vec!["app/sibling.py"]);
        assert_eq!(r("app/pkg/util.py", "..sibling"), vec!["app/sibling.py"]);
        assert_eq!(r("app/pkg/util.py", "...shared"), vec!["shared.py"]);
        assert_eq!(r("app/main.py", "pkg.util"), vec!["app/pkg/util.py"]);
        assert_eq!(r("app/main.py", "pkg.core"), vec!["app/pkg/core/__init__.py"]);
        assert!(r("app/main.py", "os").is_empty(), "stdlib");
        assert!(r("app/main.py", "util").is_empty() || r("app/main.py", "util").len() == 1);
    }

    #[test]
    fn python_absolute_import_is_skipped_when_ambiguous() {
        let files = set(&["a/util.py", "b/util.py", "main.py"]);
        assert!(resolve(&ModuleContext::default(), &files, "main.py", "util").is_empty(), "two candidates: no edge");
    }

    #[test]
    fn go_imports_resolve_through_go_mod_to_package_files() {
        let got = facts(Language::Go, "package main\n\nimport (\n  \"fmt\"\n  \"example.com/app/internal/store\"\n)\nimport \"example.com/app/util\"\n");
        for expected in ["fmt", "example.com/app/internal/store", "example.com/app/util"] {
            assert!(got.iter().any(|g| g == expected), "{expected} missing from {got:?}");
        }
        let mut context = ModuleContext::default();
        context.add_go_mod("", "module example.com/app\n\ngo 1.22\n");
        let files = set(&["go.mod", "main.go", "internal/store/store.go", "internal/store/cache.go", "internal/store/store_test.go", "util/util.go"]);
        let r = |spec: &str| resolve(&context, &files, "main.go", spec);
        assert_eq!(r("example.com/app/internal/store"), vec!["internal/store/cache.go", "internal/store/store.go"], "every non-test file of the package");
        assert_eq!(r("example.com/app/util"), vec!["util/util.go"]);
        assert!(r("fmt").is_empty());
        assert!(r("github.com/other/lib").is_empty());
    }

    #[test]
    fn cargo_toml_with_a_hyphenated_name_maps_to_an_underscored_crate() {
        let mut context = ModuleContext::default();
        context.add_cargo_toml("tools/my-tool", "[package]\nname = \"my-tool\"\nversion = \"0.1.0\"\n");
        assert_eq!(context.cargo_crates.get("my_tool").map(String::as_str), Some("tools/my-tool"));
    }
}

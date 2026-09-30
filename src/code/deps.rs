//! Dependency edges between files and symbols (REQ-703, REQ-704, REQ-710,
//! REQ-711 in `.specs/features/dependency-edges/spec.md`).
//!
//! Inputs are the facts of one changed file plus a [`SpecifierResolver`];
//! output is `Imports`/`ReExports` edges (file -> file) and
//! `Calls`/`Instantiates`/`Extends`/`References` edges (symbol -> symbol,
//! falling back to file level). Resolution is conservative: an import that
//! does not resolve to a tracked file produces nothing, and a name is linked
//! to a symbol only when the destination (following `export ... from` chains
//! through barrels) actually declares it.

use std::collections::HashMap;
use std::rc::Rc;

use crate::code::facts::{Binding, FileFacts, ImportFact, ImportKind, UsageRole};
use crate::code::parser::{CallResolver, Language, SymbolSpan, edge_id};
use crate::code::resolve::SpecifierResolver;
use crate::graph::edge::{Confidence, EdgeContext, EdgeType, encode_meta};
use crate::graph::node::{file_node_id, symbol_node_id};
use crate::sync::mutation::{EdgeMutation, StableId};

/// Everything extracted from one code file in a single parse.
#[derive(Debug, Clone, Default)]
pub struct ExtractedFile {
    pub set: crate::sync::mutation::MutationSet,
    pub facts: FileFacts,
    pub symbols: Vec<SymbolSpan>,
}

/// What other files need to know about a file to link to it.
#[derive(Debug, Clone, Default)]
pub struct FileSummary {
    /// Names of the symbols the file declares.
    pub declared: std::collections::HashSet<String>,
    /// `export ... from` statements, to follow barrels.
    pub reexports: Vec<ImportFact>,
}

impl FileSummary {
    pub fn of(extracted: &ExtractedFile) -> Self {
        Self {
            declared: extracted.symbols.iter().map(|s| s.name.clone()).collect(),
            reexports: extracted.facts.imports.iter().filter(|i| i.kind == ImportKind::ReExport).cloned().collect(),
        }
    }
}

/// Context of an edge that starts in `path`: tests and specs are told apart
/// from runtime code so queries can filter them out (REQ-710).
pub fn file_context(path: &str) -> EdgeContext {
    let path = path.replace('\\', "/");
    let name = path.rsplit('/').next().unwrap_or(&path);
    if name.contains(".spec.") || name.contains(".e2e-spec.") || path.contains("/spec/") {
        EdgeContext::Spec
    } else if name.contains(".test.") || ["/test/", "/tests/", "/__tests__/", "/e2e/"].iter().any(|d| path.contains(d)) || path.starts_with("test/") || path.starts_with("tests/") {
        EdgeContext::Test
    } else {
        EdgeContext::Runtime
    }
}

/// Loads `(source, language)` for a path that was not part of this sync's
/// batch (an unchanged file someone imports), to learn what it declares.
pub type SourceLoader<'a> = Box<dyn FnMut(&str) -> Option<(String, Language)> + 'a>;

pub struct DependencyBuilder<'a> {
    resolver: &'a SpecifierResolver,
    summaries: HashMap<String, Option<Rc<FileSummary>>>,
    loader: SourceLoader<'a>,
}

/// One edge before it is turned into an `EdgeMutation`; merged by identity.
#[derive(Clone, Copy)]
struct Pending {
    edge_type: EdgeType,
    confidence: Confidence,
    context: EdgeContext,
}

impl<'a> DependencyBuilder<'a> {
    pub fn new(resolver: &'a SpecifierResolver, loader: SourceLoader<'a>) -> Self {
        Self { resolver, summaries: HashMap::new(), loader }
    }

    /// Registers a file of this batch so importers find its declarations
    /// without re-reading it.
    pub fn seed(&mut self, path: &str, extracted: &ExtractedFile) {
        self.summaries.insert(normalize(path), Some(Rc::new(FileSummary::of(extracted))));
    }

    fn summary(&mut self, path: &str) -> Option<Rc<FileSummary>> {
        if let Some(known) = self.summaries.get(path) {
            return known.clone();
        }
        let loaded = (self.loader)(path).map(|(source, language)| {
            let parsed = crate::code::parser::extract_with_facts(&source, language, std::path::Path::new(path), &HashMap::new());
            parsed.ok().map(|e| Rc::new(FileSummary::of(&e)))
        });
        let summary = loaded.flatten();
        self.summaries.insert(path.to_string(), summary.clone());
        summary
    }

    /// Where `name`, exported by `file`, is actually declared: `file` itself,
    /// or the file behind an `export { name } from` / `export * from` chain.
    fn declaring_file(&mut self, file: &str, name: &str, depth: usize) -> Option<String> {
        if depth > 8 {
            return None;
        }
        let summary = self.summary(file)?;
        if summary.declared.contains(name) {
            return Some(file.to_string());
        }
        for fact in &summary.reexports {
            let Some(next) = self.resolver.resolve(file, &fact.specifier) else { continue };
            for binding in &fact.bindings {
                // `export * from` passes every name through; `export { x as y }`
                // passes `y` and renames it to `x` in the other module.
                let wanted = if binding.imported == "*" && binding.local == "*" {
                    Some(name)
                } else if binding.local == name && binding.imported != "*" {
                    Some(binding.imported.as_str())
                } else {
                    None
                };
                if let Some(wanted) = wanted
                    && let Some(found) = self.declaring_file(&next, wanted, depth + 1)
                {
                    return Some(found);
                }
            }
        }
        None
    }

    /// All dependency edges contributed by the file at `path`.
    pub fn edges_for(&mut self, path: &str, extracted: &ExtractedFile) -> Vec<EdgeMutation> {
        let path = normalize(path);
        let file_id = file_node_id(&path);
        let base_context = file_context(&path);
        let mut pending: HashMap<(StableId, StableId), Pending> = HashMap::new();
        let mut add = |from: StableId, to: StableId, p: Pending| {
            if from == to {
                return;
            }
            let entry = pending.entry((from, to)).or_insert(p);
            // A runtime use beats a type-only one for the same pair; an
            // extracted edge beats an inferred one.
            if entry.edge_type != p.edge_type {
                return;
            }
            if entry.context == EdgeContext::TypeOnly && p.context != EdgeContext::TypeOnly {
                entry.context = p.context;
            }
            if entry.confidence == Confidence::Inferred && p.confidence == Confidence::Extracted {
                entry.confidence = Confidence::Extracted;
            }
        };
        let context_for = |type_only: bool| {
            if base_context != EdgeContext::Runtime {
                base_context
            } else if type_only {
                EdgeContext::TypeOnly
            } else {
                EdgeContext::Runtime
            }
        };

        // File-level edges: one per resolved import/re-export.
        let mut resolved: HashMap<&str, Option<String>> = HashMap::new();
        for fact in &extracted.facts.imports {
            let target = resolved
                .entry(fact.specifier.as_str())
                .or_insert_with(|| self.resolver.resolve(&path, &fact.specifier))
                .clone();
            let Some(target) = target else { continue };
            let edge_type = if fact.kind == ImportKind::ReExport { EdgeType::ReExports } else { EdgeType::Imports };
            add(
                file_id,
                file_node_id(&target),
                Pending { edge_type, confidence: Confidence::Extracted, context: context_for(fact.type_only) },
            );
        }

        // Symbol-level edges: each use of an imported name.
        let call_resolver = CallResolver::new(&extracted.symbols);
        for usage in &extracted.facts.usages {
            let Some((fact, binding)) = find_binding(&extracted.facts, &usage.name) else { continue };
            let Some(Some(target_file)) = resolved.get(fact.specifier.as_str()).cloned() else { continue };

            let from = call_resolver
                .caller_at(&extracted.symbols, usage.byte)
                .map(|s| s.id)
                .unwrap_or(file_id);
            let (to, confidence) = match binding.imported.as_str() {
                "*" => (file_node_id(&target_file), Confidence::Extracted),
                "default" => (file_node_id(&target_file), Confidence::Inferred),
                name => match self.declaring_file(&target_file, name, 0) {
                    Some(declaring) => (symbol_node_id(&declaring, name, 0), Confidence::Extracted),
                    None => (file_node_id(&target_file), Confidence::Inferred),
                },
            };
            let type_position = usage.role == UsageRole::Type;
            let edge_type = match usage.role {
                UsageRole::Extends | UsageRole::Implements => EdgeType::Extends,
                UsageRole::New => EdgeType::Instantiates,
                UsageRole::Call => EdgeType::Calls,
                UsageRole::Decorator | UsageRole::Type | UsageRole::Other => EdgeType::References,
            };
            add(
                from,
                to,
                Pending { edge_type, confidence, context: context_for(fact.type_only || binding.type_only || type_position) },
            );
        }

        let mut edges: Vec<EdgeMutation> = pending
            .into_iter()
            .map(|((from, to), p)| EdgeMutation::Upsert {
                id: edge_id(edge_kind(p.edge_type), &from, &to),
                from,
                to,
                edge_type: p.edge_type.to_code(),
                payload: vec![encode_meta(p.confidence, p.context)],
            })
            .collect();
        // Deterministic order (HashMap iteration is not).
        edges.sort_by_key(|e| match e {
            EdgeMutation::Upsert { id, .. } | EdgeMutation::Remove { id } => *id,
        });
        edges
    }
}

/// The import (not re-export) that brought `local` into the file.
fn find_binding<'f>(facts: &'f FileFacts, local: &str) -> Option<(&'f ImportFact, &'f Binding)> {
    facts
        .imports
        .iter()
        .filter(|i| i.kind != ImportKind::ReExport)
        .find_map(|fact| fact.bindings.iter().find(|b| b.local == local).map(|b| (fact, b)))
}

fn edge_kind(edge_type: EdgeType) -> &'static str {
    match edge_type {
        EdgeType::Imports => "imports",
        EdgeType::ReExports => "re-exports",
        EdgeType::Calls => "calls",
        EdgeType::Instantiates => "instantiates",
        EdgeType::Extends => "extends",
        EdgeType::References => "references",
        _ => "depends-on",
    }
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_context_tells_runtime_test_and_spec_apart() {
        assert_eq!(file_context("src/a.ts"), EdgeContext::Runtime);
        assert_eq!(file_context("pkgs/x/test/module/a.spec.ts"), EdgeContext::Spec);
        assert_eq!(file_context("src/a.spec.ts"), EdgeContext::Spec);
        assert_eq!(file_context("src/a.test.ts"), EdgeContext::Test);
        assert_eq!(file_context("src/__tests__/a.ts"), EdgeContext::Test);
        assert_eq!(file_context("tests/helper.ts"), EdgeContext::Test);
        assert_eq!(file_context("src/latest/a.ts"), EdgeContext::Runtime, "`latest` is not `test`");
    }
}

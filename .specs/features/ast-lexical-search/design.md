# Design: Multi-Language AST Parsing & Lexical Search (Fase 3)

## Architecture Overview

```
                 source file (.ts/.py/.go/.rs)
                          │
                 [ tree-sitter grammar, by extension ]
                          │
                 code::extract(source, language)
                          │
              MutationSet (nodes: Symbol, edges: DefinedIn/
                            DependsOn/Satisfies)
                          │
                          ▼
         sync_orchestrator::SyncOrchestrator::run_once()
    (REQ-306: routes .md -> markdown::extract, code exts -> code::extract,
     same combined MutationSet, same single Coordinator::stage() call)
                          │
                          ▼
              sync::coordinator::Coordinator::stage()
         (Fase 0, unchanged — fan-out to 3 participants now)
        ┌─────────────────┼──────────────────────┐
        ▼                 ▼                      ▼
 RedbParticipant   CsrParticipant        TantivyParticipant (new)
 (Fase 0)          (Fase 1)              stage(): buffer docs in
                                          IndexWriter (not searchable
                                          yet); commit(): writer.commit()
                                          (durable + searchable);
                                          abort(): writer.rollback()
```

Cold-start (REQ-305): when `SyncOrchestrator` processes a large batch (e.g.
the very first sync of a repo, `since: None`), file reads + Tree-sitter
parsing run in parallel via `rayon::par_iter` over the file list, producing
one `MutationSet` per file; these are merged sequentially (cheap, just
`Vec::extend`) before the single `Coordinator::stage()` call — parsing is
parallel, staging is not (matches REQ-205/REQ-007's "one atomic cycle").

## Dependency Paths

- REQ-301/302 → `code::parser::{Language, extract}` (new module
  `src/code/parser.rs`), one `tree_sitter::Parser` configured per call with
  the grammar matching `Language`.
- REQ-303 → `code::extract` walks the parsed AST for call expressions
  within the same file, resolving callee name → symbol already extracted
  from that file's own symbol table (built in the same pass, single-file
  scope per Open Questions).
- REQ-304 → reuses `graph::markdown::find_markers` (already `pub(crate)`
  since Fase 2's `git::spec_link`) — three callers now, all module-private
  to the crate, no public API churn.
- REQ-305 → `code::batch::extract_all(files: &[(PathBuf, String, Language)]) -> MutationSet`
  (new, `src/code/batch.rs`), uses `rayon::par_iter` internally; this is
  what `SyncOrchestrator`'s cold-start path calls instead of looping
  `code::extract` one file at a time.
- REQ-306 → `sync_orchestrator::SyncOrchestrator::run_once()` extended:
  the existing `is_markdown(path)` branch gets a sibling `code_language(path)
  -> Option<Language>` branch; both funnel into the same `combined:
  MutationSet` before the one `Coordinator::stage()` call — no change to
  the coordinator boundary itself.
- REQ-307/308 → `search::tantivy_participant::TantivyParticipant` (new
  top-level module `src/search/`, sibling to `graph`/`sync`/`git` — it's a
  storage/index participant like `CsrParticipant`, not a graph or git
  concern), implements `sync::SyncParticipant`.

## New Components

| Component | Responsibility | Location |
|---|---|---|
| `Language` | Enum (TypeScript, JavaScript, Python, Go, Rust) + `from_extension(&Path) -> Option<Language>` | `src/code/parser.rs` |
| `code::extract` | Parse one file's source, return `MutationSet` (Symbol nodes + DefinedIn/DependsOn/Satisfies edges) | `src/code/parser.rs` |
| `code::batch::extract_all` | Parallel (`rayon`) multi-file wrapper around `extract`, merges results | `src/code/batch.rs` |
| `TantivySchema` | Field definitions: `id` (stored, fast-path exact match), `kind`, `text` (BM25-tokenized), `path` | `src/search/schema.rs` |
| `TantivyParticipant` | `SyncParticipant` impl: owns one long-lived `tantivy::IndexWriter`, stage/commit/abort map directly onto Tantivy's own buffer/commit/rollback | `src/search/tantivy_participant.rs` |
| `search::query` | Thin wrapper over `tantivy::collector::TopDocs` for exact-id fast path and BM25 free-text | `src/search/query.rs` |

## Modified Components

| Component | Change | Risk |
|---|---|---|
| `sync_orchestrator::SyncOrchestrator` | New routing branch for code file extensions, alongside the existing Markdown branch | Low — additive, same pattern already proven for `.md` |
| `sync::coordinator::Coordinator` (construction sites) | Callers now pass 3 participants instead of 2 | None — `Coordinator::new` already takes a `Vec<Box<dyn SyncParticipant>>`, no signature change |

## Risks

- **`SyncParticipant` contract under a genuinely different storage model**:
  `RedbParticipant` and `CsrParticipant` both map naturally onto
  stage-then-atomic-rename. Tantivy's own commit model (writer buffers,
  `commit()` creates a new segment + is durable, `rollback()` discards back
  to the last commit) is *already* stage/commit/abort-shaped, which is a
  good sign the Fase 0 trait generalizes — but `rollback()` in Tantivy
  discards *everything* added since the last commit, not just "this
  cycle's" documents specifically. Mitigation: `TantivyParticipant`
  enforces the same discipline the coordinator already enforces (one
  `stage()` per cycle before `commit()`/`abort()`), so "everything since
  last commit" and "this cycle's documents" are always the same set in
  practice — documented as an invariant, not re-derived from Tantivy's own
  guarantees.
- **Tree-sitter grammar binary size**: four grammars statically linked
  grows the binary meaningfully (each grammar is a few hundred KB to low
  MB of generated C). Acceptable for now (`.defs/NexSpec.md` already
  accepted this tradeoff); revisit only if it becomes a real distribution
  concern (Fase 6).
- **Single-file symbol resolution (REQ-303)**: a real limitation, not just
  a risk — cross-file `DependsOn` edges are simply absent until a future
  phase adds module resolution. Documented in spec.md's Out of Scope, not
  hidden.
- **Rayon + `Coordinator::stage()` ordering**: parallel parsing must fully
  complete (and be merged into one `MutationSet`) before `stage()` is
  called — never call `stage()` from within a `rayon` worker thread, since
  the coordinator's WAL-then-fan-out sequence must run once, atomically,
  from the orchestrator's thread. `code::batch::extract_all` returns a
  plain `MutationSet`, never touches `Coordinator` itself — keeps this
  invariant structural rather than a rule someone has to remember.

## Decision Log

- All 4 grammars linked statically, selected by file extension at the
  `SyncOrchestrator` call site — simplest build, matches `.defs/NexSpec.md`.
- Symbol resolution for `DependsOn` (REQ-303) scoped to same-file only —
  cross-file/module resolution deferred; documented as a real limitation,
  not silently approximated.
- `TantivyParticipant` owns a long-lived `IndexWriter` (not one per cycle)
  — matches Tantivy's own recommended usage (writer creation is expensive,
  involves merge-thread setup) and lets `stage()`/`commit()`/`abort()` map
  directly onto `add_document`/`commit`/`rollback` without an extra
  buffering layer in front of Tantivy's own.
- `search::` is a new top-level module (sibling to `graph`/`sync`/`git`),
  not nested under `graph::` — it's an index/storage concern parallel to
  the CSR, not a graph-topology concept itself.

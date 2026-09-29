# Design: Interface, MCP Server & Tooling (Fase 6)

## Architecture Overview

```
                     nexspec (bin, src/bin/nexspec.rs)
                              │
                 ┌────────────┴────────────┐
                 ▼                         ▼
        clap subcommands            `nexspec mcp` subcommand
     init/sync/compact/search/        spins up rmcp's stdio
     trace/blame/diff                 server (tokio runtime)
                 │                         │
                 └────────────┬────────────┘
                              ▼
                    engine::Engine (new, crate root)
      owns: redb::Database, Arc<Csr>, index_dir paths, repo root
      methods: sync/resume/compact/search/trace/blame/diff_staged
                              │
              ┌───────────────┼───────────────────┐
              ▼               ▼                   ▼
   sync::{Coordinator,   graph::csr::Csr    git::GitSource +
   RedbParticipant,      (long-lived Arc,    sync_orchestrator::
   VersionPointer, Wal}  read-only queries   SyncOrchestrator
   + search::Tantivy     never reopen it)    (sync only)
   Participant + vector::
   HnswParticipant
   (reconstructed fresh
   per write call)
```

Every `Engine` method that *writes* (`sync`, `compact`) builds its
`SyncParticipant`s fresh inside the call, borrowing `&self.db` and
`Arc::clone(&self.csr)` — none of Fases 0-4's participant types were
designed to be kept alive across a whole CLI process's lifetime (Tantivy's
`IndexWriter` holds an exclusive lock file; `HnswParticipant::new` reloads
from disk each time). Every method that *reads* (`search`, `trace`,
`blame`, `diff_staged`) either uses `self.csr` directly (no participant
needed — `Csr::edges_from` has always been read-only and lock-free) or
opens a short-lived read-only handle (a fresh `TantivyParticipant`/
`HnswParticipant` just for its `.handle()`/`.index()` accessor, dropped
at the end of the call). This mirrors exactly how
`tests/four_participants_integration.rs` and
`tests/three_participants_integration.rs` already construct participants
in every existing integration test — `Engine` is the first place this
pattern becomes a *product* composition root instead of a test fixture.

## Dependency Paths

- REQ-601 → `src/engine.rs` (new, crate root — composes `sync::`/`graph::`/
  `git::`/`search::`/`vector::`, same boundary reasoning as
  `sync_orchestrator.rs`/`hybrid.rs`). `Engine::open(index_dir: &Path,
  repo_root: &Path) -> Result<Engine, EngineError>`.
- REQ-602 → `Engine::open` itself is idempotent-safe to call against an
  existing directory (creates only what's missing: `CsrBase::build(&[],
  ..)` only if `edges.bin` doesn't exist yet, etc.); the CLI's `init`
  subcommand is a thin call to it plus a friendly message.
- REQ-603 → `Engine::sync(&self) -> Result<SyncReport, EngineError>` builds
  `GitSource::open(&self.repo_root)`, a fresh `Coordinator` (Redb + Csr +
  Tantivy + Hnsw-under-`full` participants), wraps it in
  `SyncOrchestrator::new(...)`, calls `run_once()`. `Engine::resume(&self)`
  is the same participant construction, calling `Coordinator::resume()`
  instead — `--resume` on the CLI calls this first, then `sync`.
- REQ-604 → `CsrParticipant::compact_now(&self) -> Result<(), SyncError>`
  (new `pub` method, one-line wrapper over the existing private `compact`)
  + `Engine::compact(&self)` builds a `CsrParticipant` over `self.csr`
  and calls it directly (no `Coordinator` involved — compaction isn't a
  mutation cycle, it doesn't touch the WAL or bump `sync_version`).
- REQ-605 → `Engine::search(&self, query: &str, max_tokens: Option<u32>) ->
  Result<SearchResult, EngineError>`. BM25 branch: opens a short-lived
  Tantivy read handle, `search_text`. HNSW branch (`#[cfg(feature =
  "full")]`): if `self.embedder` (an `Option<Embedder>`, built once at
  `Engine::open` time — construction is free per REQ-401/402, the actual
  model load stays lazy inside `Embedder` itself) succeeds in embedding the
  query text, opens a short-lived `HnswParticipant` and searches it;
  `VectorError::ModelNotAvailable` is caught and turned into an empty
  ranked list (same "capability gap, not hard failure" reasoning already
  recorded in `vector-engine/design.md`). Fuses via `hybrid::seed_discovery`,
  expands via `hybrid::expand` (depth 1, all edge types). If `max_tokens`
  is `Some`, resolves each result id's `NodePayload` (via `Engine`'s Redb
  read access), prunes `Symbol` payloads (`token::prune_symbol`, needs the
  file's source text — read via `GitSource::read_blob_at_head`), tiers them
  (first result = `Target`, rest = `Seed`, expansion-only ids =
  `Dependency`), and runs `Budget::with_default_margin(max_tokens).fit(...)`
  + `token::serialize`.
- REQ-606 → `Engine::trace(&self, target: &str) -> Result<TraceResult,
  EngineError>` — resolves `target` to a `StableId` (tries hex-decoding it
  directly first; falls back to a Tantivy exact-match on `REQ-`/`ADR-`/
  symbol-name text, reusing `search::find_by_id`-adjacent lookup logic),
  then BFS over `Csr::edges_from` across `{Satisfies, DependsOn, DefinedIn,
  Implements}`, recording `(depth, edge_type, node_id)` per visited node —
  same shape as `hybrid::expand`, but this fase's version also tags the
  edge type per hop (which `hybrid::expand` intentionally doesn't need for
  its own purpose).
- REQ-607 → `src/git/blame.rs` (new): `blame_symbol(git: &GitSource, path:
  &Path, line_start: u32, line_end: u32) -> Result<Vec<BlameHunk>,
  GitError>` — calls `gix::Repository::blame_file` (confirmed present on
  `gix` 0.88's `Repository` directly, backed by the `gix-blame` crate
  already a transitive dependency) with `gix_blame::BlameRanges::
  from_one_based_inclusive_range((line_start+1)..=(line_end+1))` (this
  crate's `line_start`/`line_end` are 0-indexed inclusive, per
  `NodePayload::Symbol`'s doc comment — `gix_blame` wants 1-indexed
  inclusive, hence the `+1`). Maps each `gix_blame::BlameEntry` to a
  `BlameHunk { commit_oid, author_name, author_email, time, lines: Range
  <u32> }` by resolving `entry.commit_id` via `repo.find_commit(..)`.
  `Engine::blame(&self, symbol_name: &str, full_history: bool) ->
  Result<BlameResult, EngineError>` resolves the symbol (same lookup as
  `trace`), calls `blame_symbol`, and — using `full_history` to pick
  `CoChangeWindow::default()` vs. an unbounded window (REQ-206's own
  `--full-history` knob, exposed here for the first time via CLI) — adds
  `git::cochange`'s co-change neighbors for the symbol's file.
- REQ-608 → `Engine::diff_staged(&self) -> Result<DiffResult, EngineError>`
  — `GitSource::is_dirty()` + `tracked_paths_at_head()` +
  `DirtyCache::scan` (all Fase 2, reused verbatim, a fresh `DirtyCache` per
  call since this command doesn't need cross-call memory) to find changed
  files, `code::extract` on each changed file's *working-tree* content
  (not committed — that's the point of `--staged`/dirty analysis) to get
  its current symbol set, then for each symbol looks up 1-hop
  `DependsOn`-incoming neighbors via `self.csr` (who depends on this
  symbol — REQ-608's "impacted code"). No Tantivy/HNSW touched (REQ-608 is
  explicitly lean/structural).
- REQ-609 → `src/mcp.rs` (new): `NexSpecMcp` struct wrapping `Arc<Engine>`
  (needs `Arc` here, unlike the CLI's single-owner `Engine`, because
  `rmcp`'s `ServerHandler` methods take `&self` behind a `Service` that may
  be cloned/shared across the connection's async tasks); six `#[tool]`
  methods, each: deserialize `Parameters<...Args>`, call the matching
  `Engine` method, `serde_json::to_string` the result (or a plain string
  for text-shaped results like `blame`), return `Result<String, String>`
  (confirmed as a directly-supported tool return type by `rmcp`'s own
  doctest in `handler/server/wrapper/parameters.rs`). `nexspec mcp`
  subcommand builds a `tokio` current-thread runtime, constructs
  `NexSpecMcp`, and calls `.serve(rmcp::transport::stdio()).await?.
  waiting().await?` (both confirmed directly in `rmcp`'s own source: `
  transport::io::stdio() -> (Stdin, Stdout)`, `ServiceExt::serve`,
  `RunningService::waiting`).

## New Components

| Component | Responsibility | Location |
|---|---|---|
| `Engine` | Composition root: opens/creates `.specs/.index/`, owns `Database`+`Arc<Csr>`, exposes sync/compact/search/trace/blame/diff_staged | `src/engine.rs` |
| `git::blame::blame_symbol` | AST-aware blame via `gix_blame`, scoped to a line range | `src/git/blame.rs` |
| `NexSpecMcp` | `rmcp` tool router — 6 tools wrapping `Engine` | `src/mcp.rs` |
| `nexspec` (binary) | `clap` CLI: `init/sync/compact/search/trace/blame/diff/mcp` | `src/bin/nexspec.rs` |

## Modified Components

| Component | Change | Risk |
|---|---|---|
| `CsrParticipant` | New `pub fn compact_now()` wrapping the existing private `compact()` | None — additive, no behavior change to existing callers |
| `Cargo.toml` | New deps: `clap` (derive), `tokio` (rt-multi-thread/macros/io-std), `rmcp` (server/macros/transport-io), `serde`/`serde_json`/`schemars` (direct, for MCP tool arg/result types) — all non-optional (CLI is the crate's primary deliverable at this point, not an add-on) | Low — no native/binary weight comparable to `ort`; doesn't interact with the `lean`/`full` split |

## Risks

- **`rmcp` is a fast-moving, recently-redesigned crate (v3.5.0, SEP-2663
  task extensions already in its `ServerHandler` default methods)**:
  mitigated by reading its actual downloaded source
  (`~/.cargo/registry/src/.../rmcp-3.5.0` and `rmcp-macros-3.5.0`) before
  writing any code against it, rather than relying on possibly-stale
  training-data knowledge of an earlier API shape — same discipline
  already used for Tantivy's `TopDocs::order_by_score()` surprise in Fase
  3. `#[tool_router]`/`#[tool_handler]` macros generate `get_info`/
  `list_tools`/`call_tool` automatically for the common case (confirmed by
  `rmcp-macros`' own doc example), keeping `src/mcp.rs` small.
- **`Engine` reconstructing participants per call has a real cost**: a
  `TantivyParticipant::new()` call re-opens the mmap directory and spins up
  an `IndexWriter` even for a read-only `search`. Accepted for this fase
  (CLI invocations are already process-startup-dominated — Tantivy/CSR
  open cost is noise next to `cargo`-adjacent process spawn overhead) but
  flagged in case a future `nexspec mcp` long-running session profile
  shows otherwise, at which point a read-only Tantivy handle (skip the
  writer entirely, `Index::open_in_dir` + `.reader()`) would be a
  worthwhile follow-up — not done now to avoid a second, subtly different
  Tantivy-opening code path this fase doesn't need.
- **`blame`'s `gix_blame::file` walks full commit history by design** (it's
  a real blame algorithm, not a heuristic) — this can be slow on large
  repos with long histories. No mitigation implemented this fase beyond
  what `gix_blame`'s own `since: Option<gix_date::Time>` option already
  offers (left unset — full accuracy over speed, consistent with REQ-607
  not specifying a time-bounded blame mode); revisit only if it becomes a
  real reported pain point.
- **CLI and MCP must never duplicate business logic**: every `#[tool]`
  method in `src/mcp.rs` is required to be a thin call into `Engine`, never
  a parallel reimplementation — enforced by code review during Execute, not
  by a type-system guarantee, so flagged explicitly here as a design
  invariant to watch.

## Decision Log

- `Engine` lives at the crate root (`src/engine.rs`), not under any
  existing module — it is the first component that composes *every* prior
  fase's storage layer at once (Redb + Csr + Tantivy + Hnsw + Git + Token),
  a strictly wider composition than `sync_orchestrator.rs` (Git + Markdown/
  Code + Coordinator) or `hybrid.rs` (Csr + ranked-list fusion). Same
  "composes multiple subsystems, isn't itself one" boundary rule applied
  consistently since Fase 2.
- Participants are reconstructed per `Engine` call rather than kept as
  long-lived `Engine` fields — the alternative (storing
  `Coordinator<'engine>`/`TantivyParticipant` as fields) runs into a
  self-referential-struct problem (`Coordinator<'a>` borrows `&'a
  Database`, which would have to be a field of the same struct) that Rust
  doesn't support without unsafe or a crate like `ouroboros`; rebuilding
  per call sidesteps it entirely and matches the pattern already validated
  by every Fase 0-4 integration test.
- `nexspec blame`'s AST-aware line-range scoping (REQ-607) is implemented
  in this fase rather than retroactively added to `git-integration`
  (Fase 2) — it was explicitly deferred there *because* it needs
  `Symbol.line_start/line_end`, which didn't exist until Fase 3. Doing it
  now, once, closing out that fase's documented deferral, was judged
  cleaner than reopening a "completed" fase's spec.
- MCP tool functions return `Result<String, String>` (JSON-encoded success,
  plain-text error) rather than `CallToolResult`/`ErrorData` directly —
  confirmed supported by `rmcp`'s own doctest for `Parameters<T>`, and it
  keeps `src/mcp.rs` free of any direct dependency on `rmcp::model` types
  beyond what the macros need, matching the "wrapper stays thin" invariant
  above.

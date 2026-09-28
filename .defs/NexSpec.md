# NexSpec — Product & Architecture Roadmap

> High-performance, in-process context engine for AI coding agents. Unifies AST analysis, `.specs/` requirements, topological graph traversal (CSR), vector/BM25 hybrid retrieval, and native Git integration.

---

## Architecture Overview

```text
.specs/*.md + Source Code + Git Tree + ADRs/Docs
                        │
          [ Tree-sitter / Comrak / gix ]
                        │
   ┌────────────────────┼────────────────────────┐
   ▼                    ▼                        ▼
Metadata & Nodes      Topological Edges       Git Context (Commits,
(redb + zstd)         (Binary CSR / mmap      Diffs, AST Blame)
                        + in-memory delta)
   │                    │                        │
   ├─► HNSW             │                        │
   └─► Tantivy          │                        │
          │             │                        │
          └─────────────┼────────────────────────┘
                        ▼
          Staged Sync Coordinator (WAL + atomic version pointer)
                        ▼
            RRF (Reciprocal Rank Fusion)
                        ▼
              AST-Aware Token Pruning
                        ▼
             Context Serializers & Adapters
     (CLI Markdown / MCP Protocol / JSON-LD / CI-Diff)
```

**Design principle added in this revision:** every write path funnels through a single **Sync Coordinator** that owns cross-store consistency. No individual store (redb, CSR, Tantivy, HNSW) is ever considered "source of truth" on its own — the coordinator's version pointer is.

---

## Universal Scope & Extended Use Cases

Although conceived as the storage and traversal engine for `graph-spec-design`, the core of **NexSpec** is completely decoupled from any single methodology. It functions as a general-purpose, embedded **Codebase GraphRAG Engine** that can be consumed by any external agent, tool, or CI/CD pipeline:

1. **Universal MCP Context Server (Cursor, Claude Code, Windsurf)**
   * Exposes standardized tools over stdio: `query_context`, `trace_requirement`, `find_impacted_code`, `semantic_search`.
   * Acts as an in-process, drop-in replacement for naive workspace greps and memory-heavy external daemons.
   * *(Tool names are now unified with the Phase 6 MCP server list below — see Note on Tooling Consistency.)*

2. **CI/CD Impact Analysis & Automated PR Review**
   * Runs `specdb diff --staged` inside pipelines to isolate only changed AST symbols and their 1-hop topological dependants.
   * Feeds LLM review bots with surgical 400-token payloads rather than entire diff files, cutting CI token bills.
   * Boots in a **lean mode** (see Phase 4) that skips ONNX/HNSW initialization entirely, since structural diffing needs no embeddings.

3. **Structural Linting & Architecture Governance**
   * Validates clean-architecture or domain boundaries directly on the CSR edge graph without calling an LLM (e.g., verifying whether Domain nodes have inbound `DependsOn` edges from Infrastructure).

4. **ADRs & Technical Documentation Linker**
   * Ingests RFCs, OpenAPI schemas, and `docs/adr/*.md` using the same Markdown AST pipeline.
   * Connects business decisions (`ADR-005`) directly to the code modules implementing them, enabling automated historical reasoning.

5. **Deterministic Refactoring & Migration Tracking**
   * Replaces fragile regex search with Tree-sitter AST symbol resolution.
   * Emits unambiguous, comprehensive lists of all call-sites, implementations, and consumers for symbols undergoing breaking changes.

---

## Milestone Breakdown

### Phase 0: Sync Coordinator & Transactional Integrity *(new)*

Cross-store consistency is treated as a first-class problem rather than an emergent property, since redb, the CSR file, Tantivy, and the HNSW vector store each commit independently.

* **Write-Ahead Staging Log:** Before any store is touched, the intended set of node/edge/doc mutations for a sync cycle is appended to a small WAL (`.specs/.index/sync.wal`).
* **Atomic Version Pointer:** A single monotonic `sync_version` lives in `metadata.redb`. Readers only trust data at or below the last *fully committed* version. Each store writes to a staging path (e.g. `edges.bin.staging`) and is only renamed into place after all four stores confirm success.
* **Crash Recovery:** On startup, if `sync.wal` has uncommitted entries, `specdb sync --resume` replays or discards them deterministically — the index never silently serves a half-synced state.
* **Idempotent Apply:** All mutation operations are designed to be safely re-appliable, so a resumed sync cannot double-count edges or duplicate Tantivy documents.

---

### Phase 1: Storage Primitives & Graph Topology

* **Typed Entity Models:** Define core structs (`Node`, `Edge`, `NodeType`, `EdgeType`). Decouple domain entities (`Requirement`, `Task`, `ADR`, `DocSection`, `Symbol`, `File`).
* **Stable ID vs. Dense Index *(revised)***:
  * Every entity gets a **stable logical ID** — the Blake3 content hash already computed for the node. This is what specs, edges-in-flight, and the WAL reference.
  * A separate **dense `u32` physical index** is assigned only at CSR build/compaction time and is purely an implementation detail of the mmap layout. This decouples "identity" from "physical offset," so deletions and renumbering never invalidate external references.
* **Embedded Key-Value Engine:** `redb` for transactional metadata storage (`stable_id -> NodePayload`) with `zstd`-compressed payloads, plus the `sync_version` pointer from Phase 0.
* **Compressed Sparse Row (CSR) Engine — Two-Layer Model *(revised)***:
  * **Base layer:** memory-mapped flat binary file (`.specs/.index/edges.bin`) via `rkyv`, immutable between compactions, giving O(1) directional lookups for `Satisfies`, `DependsOn`, `Implements`, `DefinedIn`.
  * **Delta layer:** a small append-only edge log (backed by redb or an in-memory structure snapshotted to disk) holding additions/removals since the last compaction. Queries merge base + delta transparently.
  * **Compaction:** triggered automatically once the delta exceeds a configurable threshold (default: 5% of base edge count) or on an explicit `specdb compact`, folding the delta into a freshly built base file. This keeps `specdb sync` a true incremental append instead of a near-full CSR rebuild.
  * **Concurrent read/write on the delta *(new)***: the MCP server may be answering queries while a background `specdb sync` mutates the delta layer. The delta structure is therefore **Copy-on-Write**, published behind an `ArcSwap` (or `crossbeam-epoch` guarded pointer) so readers always see an atomically-swapped, consistent snapshot without taking a global lock. Writers build the next delta version off-thread and swap the pointer only once it's ready — no reader is ever blocked by a sync in progress, and no reader ever observes a torn/partial delta.
* **Markdown & Doc AST Extraction:** `comrak` streaming parser extracts specifications (`REQ-XXX`), tasks (`TASK-XXX`), architecture decisions (`ADR-XXX`), and document relationships, each hashed with Blake3 to produce its stable ID.

---

### Phase 2: In-Process Git Integration & Incremental Sync

* **Native Git Engine (`gix`):** Embedded pure Rust Gitoxide runtime, no spawned subshells.
  * **Non-standard repo coverage *(new)***: `gix` (Gitoxide) evolves quickly, so the test matrix must explicitly cover exotic-but-common working trees rather than assume a "clean" repo: submodules, `git-lfs`-tracked paths, sparse checkouts, worktrees with no clean tree at `HEAD`, and detached-HEAD states. CI includes fixture repos for each case so a `gix` version bump that silently changes behavior on these is caught before release, not by users.
* **Tree-Diff Incremental Sync:**
  * Track `last_indexed_commit` in `metadata.redb`, versioned alongside `sync_version`.
  * Traverse changes between `HEAD` and the last indexed commit via native in-memory tree diffing (`Added`, `Modified`, `Deleted`).
  * Maintain an ephemeral Blake3 cache for dirty/uncommitted working-tree changes.
* **AST-Aware Blame & Co-change Graph:**
  * Query Git blame scoped to symbol line-ranges (`line_start..line_end`) rather than whole-file scans.
  * **Bounded co-change window *(revised)***: commit co-occurrence weighting runs over a configurable window (default: last 500 commits or 6 months, whichever is smaller) instead of a full-history walk, avoiding O(commits × files) cost on large repos. `specdb blame --full-history` opts into the unbounded walk explicitly.
* **Temporal Spec Linking:** Link commits to specifications when commit messages mention requirements (`feat(auth): satisfy REQ-001`).

---

### Phase 3: Multi-Language AST Parsing & Lexical Search

* **Tree-sitter Parsing Pipeline:**
  * Statically linked grammars (TypeScript/JavaScript, Python, Go, Rust).
  * Extract symbols, type definitions, call expressions, imports, and docstring annotations (`@spec REQ-001`, `@adr ADR-005`).
  * **Parallel cold-start *(new)***: initial `specdb init` parses files concurrently via `rayon` (`par_iter` over the file list), since AST extraction is embarrassingly parallel per file. Git blame lookups are similarly parallelized where `gix`'s object store allows concurrent reads.
* **Lexical Indexing with Tantivy (BM25):**
  * Index exact symbols, requirement IDs, file paths, and normalized code tokens under `.specs/.index/tantivy/`.
  * Fast-path queries for exact identifier matching.

---

### Phase 4: Local Vector Engine & Hybrid Traversal

* **In-Process ONNX Runtime — Lazy Initialization *(revised)***:
  * Integrate `ort` with native CPU multi-threading, bundling a quantized INT8 embedding model (`bge-small-en-v1.5` or `all-MiniLM-L6-v2`, ~30MB).
  * The runtime is **not loaded at process startup**. It initializes lazily on the first call that actually needs `semantic_search`/HNSW. Commands that never touch embeddings (`specdb diff --staged`, `specdb trace`, structural lint) skip this cost entirely — important for CI, where every second of cold start counts.
  * A compile-time "lean" feature flag can exclude `ort`/HNSW from the binary altogether for CI-only deployments.
* **HNSW Vector Storage:** Build and persist the local vector index (`.specs/.index/vectors.bin`) via cosine similarity, participating in the Phase 0 staged-commit protocol like every other store.
* **Hybrid Retrieval Strategy:**
  1. **Seed Discovery:** Query Tantivy (BM25) and HNSW simultaneously; combine ranks via Reciprocal Rank Fusion (RRF).
  2. **Bounded k-Hop Expansion:** Expand outward from seed nodes along merged base+delta CSR edges, filtered by relation type (`Satisfies`, `DependsOn`), up to depth k (default: 1–2).

---

### Phase 5: Token Budgeting & LLM Serialization

* **AST Signature Pruner:** Strip implementation bodies from code nodes, retaining only typed signatures, contracts, and imports.
* **Strict Token Limiter — Tokenizer-Aware *(revised)***:
  * Fast BPE estimation via `tiktoken-rs` remains the default, but since NexSpec targets multiple downstream agents (Claude Code, Cursor, Windsurf) whose tokenizers differ from OpenAI's, a `Tokenizer` trait allows plugging alternate estimators.
  * A configurable **safety margin** (default: enforce 90% of the declared `--max-tokens` budget) absorbs estimation drift between BPE (OpenAI/tiktoken) and the tokenizers used by Claude or Gemini, so a hard budget like `800` doesn't overshoot the consuming model's real limit.
  * **Offline fallback *(new)***: if the BPE vocabulary/dictionary fails to load (e.g. no network on first run, or a stripped-down CI image), the limiter degrades to a simple `char_count / 3.5` heuristic rather than failing the command outright. This keeps `specdb` fully functional offline, at the cost of a coarser (but still safety-margined) token estimate.
  * Deterministic priority cutoff preserved: Target Spec/ADR > Seed Symbol > Direct Dependencies.
* **High-Density Markdown Serializer:** Emit compact context payloads, avoiding verbose JSON formatting.

---

### Phase 6: Interface, MCP Server & Tooling Integration

* **CLI Interface (`clap`):**
  * `specdb init`: Initialize index directory structure (parallelized cold-start per Phase 3).
  * `specdb sync [--resume]`: Run incremental sync against Git diff through the Phase 0 staging coordinator.
  * `specdb compact`: Force CSR delta-layer compaction.
  * `specdb search "<query>"`: Hybrid search + bounded context expansion.
  * `specdb trace <ID>`: Deterministic topological dependency tracing (REQs, ADRs, Symbols).
  * `specdb blame <SYMBOL> [--full-history]`: Semantic AST-level commit and author history.
  * `specdb diff --staged`: Structural change analysis for CI and review workflows, lean-mode by default.

* **Embedded MCP Server (Model Context Protocol) — Unified Tool Surface *(revised)***:
  * Stdio transport support for Claude Code, Cursor, Windsurf, and custom agent environments.
  * Canonical tool set (single source of truth, matching the Universal Scope section above): `query_context`, `trace_requirement`, `find_impacted_code`, `semantic_search`, `get_symbol_history`, `sync_workspace`.

* **`graph-spec-design` Skill Integration:**
  * Replace `graphifyy` CLI calls in `SKILL.md` with native `specdb` commands.
  * Zero-runtime installation via standalone static binary.

---

## Dependency & Crate Matrix

| Module                  | Core Crate                     | Role                                                  | Target Footprint        |
| ----------------------- | ------------------------------ | ----------------------------------------------------- | ----------------------- |
| **CLI & Transport**     | `clap`, `tokio`, `rmcp`        | Argument parsing and MCP stdio server                 | Minimal                 |
| **Sync Coordination**   | `redb`                         | WAL + atomic `sync_version` pointer (Phase 0)         | In-process              |
| **Metadata Storage**    | `redb`, `zstd`                 | ACID key-value tables and payload compression         | In-process              |
| **Graph Serialization** | `rkyv`, `memmap2`              | Zero-copy CSR base layer + in-memory delta merge      | Sub-millisecond         |
| **Parallelism**         | `rayon`                        | Parallel AST parsing and blame lookups on cold start  | CPU-bound, no daemon    |
| **Concurrency (delta)** | `arc-swap`, `crossbeam-epoch`  | Lock-free COW publish/read of the CSR delta layer     | Non-blocking reads      |
| **Git Integration**     | `gix` (Gitoxide)               | Pure Rust Git inspection, diffing, and blame          | Zero daemon             |
| **Code AST**            | `tree-sitter`, `tree-sitter-*` | Grammars for TypeScript, Python, Go, Rust             | Static bindings         |
| **Spec & Docs AST**     | `comrak`                       | Markdown parsing and requirement/ADR extraction       | Static                  |
| **Lexical Search**      | `tantivy`                      | Embedded BM25 index                                   | Disk-backed             |
| **Vector Engine**       | `ort`, `instant-distance`      | INT8 ONNX embeddings and HNSW search (lazy-loaded)    | ~30MB memory, on-demand |
| **Token Budgeting**     | `tiktoken-rs`                  | Real-time BPE budget estimation (pluggable via trait) | Microsecond             |

---

## Summary of Changes vs. Prior Revision

1. **Phase 0 added** — WAL-based sync coordinator with atomic version pointer, so a crash mid-sync can never leave redb/CSR/Tantivy/HNSW disagreeing.
2. **CSR is now two-layer** (immutable base + append-only delta, compacted on threshold), making `specdb sync` a genuine incremental append rather than a near-rebuild.
3. **Stable ID vs. dense physical index** separated, so deletions/renumbering no longer invalidate external references to nodes.
4. **Bounded co-change window** for blame-based weighting, avoiding full-history O(commits × files) cost by default.
5. **`rayon` added** for parallel cold-start parsing and blame lookups.
6. **ONNX/HNSW lazy-loaded**, with a compile-time lean feature flag, so CI-only commands (`diff --staged`, `trace`) never pay embedding-model startup cost.
7. **Tokenizer made pluggable with a safety margin**, since `tiktoken-rs`'s BPE estimate doesn't match every downstream agent's real tokenizer.
8. **MCP tool names unified** between the Universal Scope section and the Phase 6 server spec (previously listed two different sets for the same surface).
9. **Delta layer made lock-free** via Copy-on-Write + `ArcSwap`/`crossbeam-epoch`, so the MCP server can keep answering queries without blocking while a background `sync` mutates the delta.
10. **`gix` edge-case test coverage specified**: submodules, git-lfs, sparse checkout, dirty/detached-HEAD worktrees are now explicit fixtures, not assumed-away.
11. **Offline tokenizer fallback added**: `char_count / 3.5` heuristic when the BPE dictionary can't be loaded, so token budgeting never hard-fails offline.
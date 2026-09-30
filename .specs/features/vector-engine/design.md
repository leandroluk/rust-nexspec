# Design: Local Vector Engine & Hybrid Traversal (Fase 4)

## Architecture Overview

```
                         query text
                              │
           ┌──────────────────┴──────────────────┐
           ▼                                      ▼
  search::query::search_text          vector::embed(query) -- lazy ort init
  (Tantivy BM25, Fase 3, unchanged)    (REQ-401/402 -- may return
           │                            ModelNotAvailable, handled below)
           ▼                                      ▼
     Vec<(StableId, bm25_rank)>          HnswIndex::search(vec)
                                          Vec<(StableId, cosine_rank)>
           └──────────────────┬──────────────────┘
                              ▼
                  hybrid::seed_discovery (RRF)
                    Vec<(StableId, fused_score)>
                              │
                              ▼
                  hybrid::expand(seeds, csr, edge_types, max_depth)
                    (bounded k-hop over Csr::edges_from, Fase 1)
                              │
                              ▼
                    Vec<StableId>  (seeds + their neighborhood)
```

**Degradation path (REQ-402):** if the embedding model isn't on disk,
`vector::embed()` returns `Err(VectorError::ModelNotAvailable)`.
`hybrid::seed_discovery` catches that specific error and falls back to
BM25-only results (still useful, just not hybrid) — a missing model is a
capability gap, not a hard failure of the whole search path.

## Dependency Paths

- REQ-401 → `vector::runtime::embedder() -> Result<&'static Embedder, VectorError>`,
  backed by `std::sync::OnceLock<Result<Embedder, VectorError>>` — the
  `ort::Session` and tokenizer are constructed exactly once, on first call,
  behind this lazily-initialized static.
- REQ-402 → `Embedder::embed(&self, text: &str) -> Result<Vec<f32>, VectorError>`;
  model/tokenizer file paths are configuration (constructor parameters), not
  hardcoded — the caller decides where the ~30MB model lives on disk (out of
  scope for this crate to bundle/download itself).
- REQ-403 → `vector::hnsw::{HnswIndex, HnswParticipant}` (new module
  `src/vector/hnsw.rs`) — same staged-commit shape as `CsrParticipant`
  (Fase 1): base file + in-memory delta, `SyncParticipant` impl.
- REQ-404 → Cargo feature `lean` in `Cargo.toml`; `ort` and `instant-distance`
  become optional dependencies, only pulled in when `lean` is *not* set
  (default-on "full" build); `#[cfg(feature = "lean")]` stubs
  `vector::embed`/`HnswParticipant` with a `ModelNotAvailable`-equivalent
  compile-time-absent path — actual `#[cfg]` wiring decided at task time,
  not re-litigated here.
- REQ-405 → `hybrid::seed_discovery` (new top-level module `src/hybrid.rs`,
  sibling to `sync_orchestrator.rs` — same "depends on multiple subsystems,
  isn't itself one" reasoning).
- REQ-406 → `hybrid::expand`, calling `Csr::edges_from` (Fase 1, unchanged)
  breadth-first up to `max_depth`, deduplicating visited nodes.

## New Components

| Component                | Responsibility                                                                               | Location                 |
| ------------------------ | -------------------------------------------------------------------------------------------- | ------------------------ |
| `Embedder`               | Lazy-loaded `ort::Session` + tokenizer, `embed(text) -> Vec<f32>`                            | `src/vector/embedder.rs` |
| `VectorError`            | `ModelNotAvailable`, `Inference(String)`, `Io(...)`                                          | `src/vector/embedder.rs` |
| `HnswIndex`              | Wraps `instant_distance::Hnsw`, cosine similarity, `search(vec, k) -> Vec<(StableId, f32)>`  | `src/vector/hnsw.rs`     |
| `HnswParticipant`        | `SyncParticipant` impl — stage/commit/abort over `HnswIndex`, same shape as `CsrParticipant` | `src/vector/hnsw.rs`     |
| `hybrid::seed_discovery` | RRF fusion of BM25 + HNSW rankings                                                           | `src/hybrid.rs`          |
| `hybrid::expand`         | Bounded k-hop CSR traversal from seeds                                                       | `src/hybrid.rs`          |

## Modified Components

| Component                                             | Change                                                                                                    | Risk                                                                                                                                        |
| ----------------------------------------------------- | --------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| `Cargo.toml`                                          | New optional deps (`ort`, `instant-distance`) gated by default-on feature, `lean` feature to exclude them | Medium — first feature-flag split in the crate; must verify both `cargo build` and `cargo build --features lean` (or equivalent) stay green |
| `sync::coordinator::Coordinator` (construction sites) | 4th participant                                                                                           | None — same `Vec<Box<dyn SyncParticipant>>`                                                                                                 |

## Risks

- **Model file is an external asset, not code**: unlike every previous
  phase, REQ-402 depends on a ~30MB binary file this session does not
  fetch on its own initiative (download requires explicit user
  confirmation). Mitigation: design and test everything (HNSW storage, RRF,
  k-hop expansion) against synthetic vectors first; real-model inference is
  its own task, gated on the user's go-ahead, and the rest of the fase does
  not block on it.
- **`ort` binary/runtime dependencies**: ONNX Runtime typically needs a
  native shared library (`onnxruntime.dll`/`.so`/`.dylib`) available at
  runtime, which `ort`'s `download-binaries` feature can fetch
  automatically — another download-shaped action, same confirmation
  requirement as the model file itself. Both are bundled into the single
  "ask before Fase 4 does real inference" checkpoint, not two separate asks.
- **Feature-flag correctness (REQ-404)**: easy to accidentally let a `lean`
  build silently include `ort`/`instant-distance` via a transitive
  default-features leak. Mitigation: CI-shaped task explicitly builds with
  the `lean` feature and asserts (via `cargo tree` or binary inspection)
  that the heavy crates are absent — deferred to task-level detail, flagged
  here so it isn't forgotten.
- **RRF fusion correctness**: needs at least one test with adversarial
  input (a document that ranks well in BM25 but not at all in HNSW, and
  vice versa) to confirm the fusion formula doesn't let one signal
  dominate unfairly.

## Decision Log

- Missing model degrades `seed_discovery` to BM25-only rather than failing
  the whole hybrid search — a capability gap, not a hard error, consistent
  with how the rest of the crate treats optional/unavailable capabilities
  (e.g. `git-integration`'s dirty-cache short-circuit).
- `HnswParticipant` mirrors `CsrParticipant`'s two-layer shape (base file +
  in-memory delta) rather than inventing a new persistence pattern — same
  rationale as Fase 1: consistency of mental model across participants.
- `hybrid.rs` lives at the crate root next to `sync_orchestrator.rs`, not
  under `search::`/`vector::` — it's a composition of both, same boundary
  reasoning applied since Fase 2.

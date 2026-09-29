# Design: Token Budgeting & LLM Serialization (Fase 5)

## Architecture Overview

```
       ranked node ids (from hybrid::seed_discovery + hybrid::expand, Fase 4)
                              │
                              ▼
              caller resolves each id to (NodePayload, priority tier)
                     tier 1: target Spec/ADR
                     tier 2: seed symbol
                     tier 3: direct dependency (1-hop)
                              │
                              ▼
        for NodeType::Symbol payloads only ──► token::pruner::prune(...)
        (Requirement/Task/Adr/DocSection/File pass through unpruned)
                              │
                              ▼
                token::budget::Budget::new(max_tokens, margin)
                token::budget::Budget::fit(tiered_items, &impl Tokenizer)
                 -- walks tier 1 → 2 → 3, stops before any item that would
                    exceed the margined budget, never truncates one item
                              │
                              ▼
                  token::serializer::serialize(fitted_items)
                    -- dense Markdown, one block per node
```

`Tokenizer` is a trait (REQ-502) with two implementations: `TiktokenTokenizer`
(default, wraps `tiktoken-rs`'s `cl100k_base`) and `CharHeuristicTokenizer`
(fallback, `char_count / 3.5`). `Budget::fit` takes `&impl Tokenizer` — it is
generic over the estimator, never hardcoded to one implementation, so a
caller can swap in a different `Tokenizer` (e.g. once a CLI in Fase 6 wants a
model-specific one) without touching `Budget`.

## Dependency Paths

- REQ-501 → `token::pruner::prune_symbol(source: &str, language: Language, line_start: u32, line_end: u32) -> String`
  (new module `src/token/pruner.rs`) — reuses `code::parser::Language` (Fase
  3, unchanged) to pick the Tree-sitter grammar, parses just enough to find
  the body node overlapping the given line range, and replaces its byte span
  with a placeholder. No new `NodePayload` variant: the pruner is a pure
  function over source text, called by whoever is building the serialization
  payload (not wired into `code::parser::extract` or the sync pipeline).
- REQ-502 → `token::budget::Tokenizer` trait + `TiktokenTokenizer` +
  `CharHeuristicTokenizer` (new module `src/token/budget.rs`).
  `TiktokenTokenizer::new()` returns `Result<Self, TokenError>`; on
  construction failure (network/cache unavailable for the BPE ranks file),
  callers are expected to fall back to `CharHeuristicTokenizer::default()` —
  `Budget` itself doesn't auto-fallback (it just takes whichever `&impl
  Tokenizer` it's given), keeping the fallback decision at the composition
  boundary, consistent with how `vector::embed`'s `ModelNotAvailable` is
  handled one layer up in `hybrid::seed_discovery` rather than inside
  `Embedder`.
- REQ-503 → `token::budget::Budget::new(max_tokens: u32, margin: f32) ->
  Budget` (`margin` default `0.9` via `Budget::with_default_margin
  (max_tokens)`); `Budget::effective_limit() -> u32` exposes `(max_tokens as
  f32 * margin).floor() as u32` for tests/inspection.
- REQ-504 → `token::budget::Budget::fit(items: Vec<TieredItem>, tokenizer:
  &impl Tokenizer) -> Vec<TieredItem>` — `TieredItem { tier: Tier, text:
  String }` where `Tier` is `Target`/`Seed`/`Dependency` (in that priority
  order, matching REQ-504's three tiers exactly). Input is assumed
  pre-sorted by tier by the caller (composition responsibility, same as
  REQ-501's pruning); `fit` itself does not resolve graph structure — it only
  walks the given order and accumulates until the next item would exceed
  `effective_limit()`, then stops (remaining items dropped whole).
- REQ-505 → `token::serializer::serialize(items: &[TieredItem]) -> String`
  (new module `src/token/serializer.rs`) — one `### <label>` heading per item
  followed by a fenced code block; no other per-item metadata.

## New Components

| Component | Responsibility | Location |
|---|---|---|
| `token::pruner::prune_symbol` | Strip a symbol's body, keep signature/header | `src/token/pruner.rs` |
| `token::budget::Tokenizer` (trait) | `estimate(&self, text: &str) -> u32` | `src/token/budget.rs` |
| `token::budget::TiktokenTokenizer` | BPE estimate via `tiktoken-rs` | `src/token/budget.rs` |
| `token::budget::CharHeuristicTokenizer` | Offline fallback (`char_count / 3.5`) | `src/token/budget.rs` |
| `token::budget::Budget` | Safety margin + priority-ordered `fit()` | `src/token/budget.rs` |
| `token::budget::Tier`/`TieredItem` | Priority tagging for REQ-504 | `src/token/budget.rs` |
| `token::serializer::serialize` | Dense Markdown output | `src/token/serializer.rs` |

## Modified Components

| Component | Change | Risk |
|---|---|---|
| `Cargo.toml` | New dependency `tiktoken-rs` (not optional — REQ-502's trait needs a default impl available in every build, `lean` or `full`; `tiktoken-rs` itself is lightweight, no ONNX-scale footprint) | Low |
| `src/lib.rs` | Export `token::{Tokenizer, TiktokenTokenizer, CharHeuristicTokenizer, Budget, Tier, TieredItem, prune_symbol, serialize}` | None |

## Risks

- **`tiktoken-rs` network dependency on first use**: its BPE ranks file is
  fetched from a public URL and cached locally (not vendored in the crate).
  Mitigation: `TiktokenTokenizer::new()` returns `Result`, never panics; the
  fallback tokenizer is a fully independent, allocation-free implementation
  so a caller can always produce *some* estimate. Tests for
  `TiktokenTokenizer` that need network access follow the same `#[ignore]`
  pattern used for T-406's real-inference tests in `vector-engine` — never
  block CI/offline dev by default.
- **Tree-sitter body-node heuristics vary per grammar**: "the body" isn't
  named consistently across the 5 grammars (`block` in Rust/Go/JS/TS,
  `block` in Python too, but struct/interface/trait bodies use different
  node kinds than function bodies). Mitigation: `prune_symbol` targets the
  *last* child node of the definition that starts with `{`/`:`-introduced
  block syntax generically (byte-range based, not a per-language node-kind
  allowlist), matching the `(_) @name` wildcard precedent already
  established in `code::parser::symbol_query` (Fase 3) for avoiding
  per-grammar special-casing. Symbols with no such block (e.g. a one-line
  type alias) fall through unchanged — explicitly allowed by REQ-501.
- **Priority-tier resolution lives outside this feature**: REQ-504 assumes
  the caller already knows which tier each node belongs to (from
  `hybrid::seed_discovery`'s seed set vs. `hybrid::expand`'s frontier, plus
  whatever "target Spec/ADR" means for a given query — likely the query's
  own resolved node if it names a REQ/ADR directly). This feature does not
  invent a new resolver for that; it's Fase 6's CLI/MCP layer that will
  first have an actual end-to-end query to derive tiers from. Design choice
  recorded here so Fase 6 isn't surprised `Budget::fit` needs pre-tiered
  input rather than raw graph output.

## Decision Log

- `token::` is a new top-level module (sibling of `graph`/`sync`/`git`/
  `code`/`search`/`vector`/`hybrid`), not nested under any existing one —
  same reasoning as `hybrid.rs`/`sync_orchestrator.rs`: it composes output
  from multiple subsystems (graph payloads, code source text) rather than
  belonging to one.
- Pruning is a pure function over caller-supplied source text, not a new
  `NodePayload` variant or a `SyncParticipant`. Rationale: pruning is a
  presentation-time transform (depends on the *current* budget/query), not
  a durable fact about the node worth persisting/syncing — keeping it
  outside the sync pipeline avoids a 5th `SyncParticipant` for something
  that's cheap to recompute on demand from already-stored `line_start`/
  `line_end` + the file's source text.
- `Budget::fit` takes pre-tiered, pre-ordered input rather than reaching
  into `hybrid`/`graph` itself to compute tiers — keeps `token::` decoupled
  from `hybrid::`/`graph::` (no new dependency edge added to the module
  graph), consistent with `hybrid::seed_discovery`/`expand` themselves
  taking already-ranked lists rather than raw queries (Fase 4 precedent).
- `tiktoken-rs` is a required (non-optional) dependency rather than gated
  behind `full`/`lean` — REQ-502 needs *some* default `Tokenizer` impl to
  exist unconditionally, and `tiktoken-rs` has no native/binary/model-file
  weight comparable to `ort`/`instant-distance`, so it doesn't fit the
  `lean` feature's purpose (excluding *heavy* optional runtime cost).

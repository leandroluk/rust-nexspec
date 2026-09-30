<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/public/logo-mark-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="docs/public/logo-mark-light.svg">
    <img alt="NexSpec" src="docs/public/logo-mark-dark.svg" width="120">
  </picture>
</p>

<h1 align="center">NexSpec</h1>

<p align="center">
  An in-process context engine for AI coding agents, written in Rust.
</p>

NexSpec indexes your source code **and** your `.specs/*.md` files into a single graph, then lets an agent (or you) search it, trace requirements to code, and retrieve a token-budgeted slice of context — from one static binary, with no daemons and no interpreter. It is the Rust successor to the `graphify` index used by the [`graph-spec-design`](https://github.com/leandroluk/graph-spec-design) skill.

## Features

- **Code + spec graph** — Tree-sitter (TypeScript/JavaScript, Python, Go, Rust) and Comrak (`REQ-XXX`, `TASK-XXX`, `ADR-XXX`) feed one typed graph.
- **Two-layer CSR topology** — immutable `rkyv` base read via `mmap`, plus a lock-free append-only delta (`ArcSwap`). O(1) neighbour lookups.
- **Hybrid search** — BM25 (Tantivy) fused with HNSW vector search via Reciprocal Rank Fusion, with k-hop graph expansion. Vector search is lazy-loaded and optional.
- **Native Git** — `gix` tree diffs for incremental sync and AST-aware `blame`; no `git` subprocesses.
- **Token budgeting** — AST signature pruning, pluggable `Tokenizer`, 90% safety margin, deterministic priority cut, dense Markdown output.
- **Crash-safe** — a single Sync Coordinator (WAL + atomic `sync_version`) keeps `redb`, CSR, Tantivy and HNSW consistent.
- **CLI and MCP server** — the same `Engine` powers both.

## Install

```bash
git clone https://github.com/leandroluk/rust-nexspec.git
cd rust-nexspec
cargo build --release          # full build (ONNX embedder + HNSW)
# or, without the vector engine:
cargo build --release --no-default-features --features lean
```

The binary is `target/release/nexspec`. The crate uses Rust edition 2024.

## Quick start

```bash
nexspec init                              # create .specs/.index/
nexspec sync                              # incremental sync from Git history
nexspec search "sync coordinator"         # ranked hits
nexspec search "sync coordinator" --max-tokens 4000   # pruned, budgeted Markdown
nexspec trace REQ-105                     # requirement → code dependency tree
nexspec blame compact                     # AST-aware blame for a symbol
nexspec diff --staged                     # symbols touched by the dirty tree + dependants
nexspec mcp                               # MCP server over stdio
```

Use `--repo <path>` to operate on another repository.

## Commands

| Command | Description |
| --- | --- |
| `init` | Create `.specs/.index/` (idempotent). |
| `sync [--resume]` | One incremental sync cycle; `--resume` replays unfinished WAL frames first. |
| `compact` | Force CSR delta compaction. |
| `search <query> [--max-tokens N]` | Hybrid search, optionally budgeted into dense Markdown. |
| `trace <ID>` | Deterministic dependency trace from a `REQ`/`ADR` marker or symbol. |
| `blame <symbol> [--full-history]` | AST-aware blame scoped to the symbol's lines. |
| `diff --staged` | Structural impact of the dirty/staged tree. |
| `mcp` | Embedded MCP server (stdio). |

## MCP

```json
{
  "mcpServers": {
    "nexspec": { "command": "nexspec", "args": ["--repo", ".", "mcp"] }
  }
}
```

Tools: `query_context`, `semantic_search`, `trace_requirement`, `find_impacted_code`, `get_symbol_history`, `sync_workspace`.

## Semantic search model

Vector search needs the default `full` build and an INT8-quantized embedding model in `.models/` (`all-MiniLM-L6-v2`, ~23 MB, not committed). Without either, search degrades to BM25 — it never fails.

## Development

```bash
cargo test                                            # full build
cargo test --no-default-features --features lean      # lean build
cargo clippy -- -D warnings
bash scripts/check-lean-build.sh                      # lean must exclude ort / instant-distance
```

## Spec-driven development

This project is developed with the `graph-spec-design` workflow. [`.specs/`](.specs) is the source of truth:

- [`.specs/project/`](.specs/project) — `PROJECT.md`, `ROADMAP.md`, `STATE.md`
- [`.specs/codebase/`](.specs/codebase) — stack and architecture notes
- [`.specs/features/`](.specs/features) — `spec.md`, `design.md` and `tasks.md` per phase (0–6)

## Documentation

The documentation site lives in [`docs/`](docs) and is built with [Fumadocs](https://fumadocs.dev) (Next.js):

```bash
cd docs
npm install
npm run dev     # http://localhost:3000
```

## License

Apache-2.0 — see [LICENSE](LICENSE).

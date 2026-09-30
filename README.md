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

| Command                                                                                | Description                                                                                     |
| -------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| `init`                                                                                 | Create `.specs/.index/` (idempotent).                                                           |
| `sync [--resume]`                                                                      | One incremental sync cycle; `--resume` replays unfinished WAL frames first.                     |
| `compact`                                                                              | Force CSR delta compaction.                                                                     |
| `search <query> [--max-tokens N]`                                                      | Hybrid search, optionally budgeted into dense Markdown.                                         |
| `trace <ID> [--depth N] [--max-tokens N]`                                              | Deterministic dependency trace from a `REQ`/`ADR` marker or symbol.                             |
| `query "<question>"`                                                                   | Answer from the graph: search seeds expanded through their relations, within a token budget.    |
| `path <A> <B>`                                                                         | Shortest chain of relations between two nodes.                                                  |
| `explain <X>`                                                                          | Describe one node: location, signature, links, requirements, community, authors.                |
| `affected <X> [--relation R]… [--depth N]`                                             | Who depends on it, transitively, grouped by file.                                               |
| `blame <symbol> [--full-history]`                                                      | AST-aware blame scoped to the symbol's lines.                                                   |
| `diff --staged`                                                                        | Structural impact of the dirty/staged tree.                                                     |
| `report [--format md\|json] [--max-tokens N] [--top N] [--fail-on-cycle] [--diff REV]` | Structural report of the graph (see below).                                                     |
| `bench [--corpus FILE] [--check] [--update-baseline] [--compare-enrich] [--compare-memory] [--embed]` | Retrieval quality and token cost against a question corpus; `--compare-enrich` runs it without and with the `enrich` summaries. |
| `enrich [--lang en,pt] [--top 20%] [--dry-run] [--status] [--clear] [--yes]` | Opt-in: an LLM writes a short summary per file so prose questions find code (see below). |
| `extract --postgres DSN [--dry-run]` | Opt-in, read-only: compare the changesets with a live PostgreSQL database (`drift: …` first line) and add the objects that exist only there to the graph. |
| `export [--format json\|html\|tree\|wiki] [--out PATH] [--path GLOB] [--kind K] [--check]` | Portable JSON, an interactive HTML page, a collapsible tree or a Markdown wiki of the graph; `--check` exits 7 when the export on disk is stale. |
| `global add\|list\|remove\|path` | The graph of several repositories in one (`~/.nexspec/global`); `--global` on `query`/`path`/`explain`/`affected` asks it, `--repo TAG` picks the target's repository. |
| `merge-graphs A.json B.json --out FILE` | Unite exports of several repositories (ids tagged, package and HTTP links across them). |
| `merge-driver BASE OURS THEIRS` | Git merge driver for `*.graph.json` (registered by `hook install`). |
| `save-result --question Q --nodes N… --outcome useful\|dead_end\|corrected [--correction C]` | Remember how an answer went and which nodes it cited (`.specs/.memory/notes/`). |
| `reflect [--max-tokens N]` | Lessons from the saved results (`.specs/.memory/LESSONS.md`) and a light nudge for ranking; with `--max-tokens`, only the short session summary. |
| `annotate <target> [--label L] [--note N] [--relation R --to T] [--outcome O]` | Record a conclusion about a node with provenance (`.specs/.memory/annotations.jsonl`); `community:<n>` names a community; `annotate list\|show\|remove\|lint` manage them. |
| `sync --embed [--similar]` | Real embeddings for new or changed documents and symbols; `--similar` adds `SimilarTo` edges (needs the model). |
| `watch [--debounce MS]`                                                                | Sync after each burst of file changes (one watcher per repository; Ctrl+C stops it).            |
| `hook install\|uninstall\|status`                                                      | Git hooks (`post-commit`, `post-merge`, `post-checkout`) that run `sync` in the background.     |
| `check-update`                                                                         | `up-to-date`, `stale: <reason>` or `no-index` on the first line; never writes.                  |
| `install\|uninstall --platform P [--scope user] [--dry-run]`                           | Register the MCP server with claude, gemini, cursor, vscode or codex.                           |
| `doctor`                                                                               | Checks build, model, index, WAL, hooks, agents and `.gitignore`, with the fix for each problem. |
| `mcp [--watch]`                                                                        | Embedded MCP server (stdio); `--watch` also keeps the index fresh.                              |

## Exit codes

| Code | Meaning                                                                              |
| ---- | ------------------------------------------------------------------------------------ |
| `0`  | Success (`check-update`: index up to date; `doctor`: no failures, warnings allowed). |
| `1`  | Error; the message is on stderr.                                                     |
| `2`  | `report --fail-on-cycle` found an import cycle.                                      |
| `3`  | `check-update`: the index is stale (HEAD moved or the tree has uncommitted changes). |
| `4`  | `check-update`: there is no index.                                                   |
| `5`  | `doctor`: at least one check failed.                                                 |
| `6`  | `enrich`: some files could not be summarised (the rest were kept).                   |
| `7`  | `export --check`: the export on disk is out of date.                                  |
| `8`  | `annotate lint`: it found something (stale, dangling, duplicate, overlong or secret-looking annotations). |

## Retrieval enrichment

Questions in prose ("how are invoices charged to residents") do not match identifiers, so `search` and `query` return nothing useful. `nexspec enrich` asks an LLM for one short summary per file, in the languages you choose, and indexes it next to the code text. It is strictly opt-in: nothing is sent until you run it, `sync` and the hooks never call a provider, and it is not an MCP tool.

```bash
export GEMINI_API_KEY=…
nexspec enrich --dry-run --lang en,pt     # files, tokens and cost estimate, offline
nexspec enrich --lang en,pt --top 20%     # the most important 20 % first; Ctrl+C keeps what is done
nexspec enrich --status                   # up to date / stale / pending, tokens and cost so far
nexspec search --no-enrich "…"            # rank on code text only
nexspec bench --compare-enrich            # with vs without, judged against the acceptance criteria
```

Files that look like they hold a secret, `.env*`, keys and generated code are never sent; only the first 2500 characters of each file leave the machine. Summaries live in `.specs/.cache/enrichment.jsonl` (git-ignored, outside the index, so rebuilding the index does not cost again). A file that changed since its summary was written is *stale* and leaves the ranking until it is enriched again. See [the docs page](docs/content/docs/features/retrieval-enrichment.mdx).

## Database and package nodes

`sync` also reads the database schema and the workspace packages (see [the docs page](docs/content/docs/features/domain-extractors.mdx)): tables, views, columns and named constraints from `*.sql` and Liquibase XML/YAML changesets, `Package` nodes from `package.json` and `Cargo.toml`, and entity-to-table links for TypeORM, sequelize-typescript, SQLAlchemy and Prisma when the table name is literal. `nexspec affected tb_contract_reminder` then reaches the entity, the repository and the use cases. `nexspec extract --postgres DSN` (opt-in, read-only) reports the drift between the changesets and a live database.

## Several repositories

`nexspec global add <repo>` puts a synced repository into a global graph; `merge-graphs` does the same into a file. Package dependencies and HTTP calls that cross repositories become inferred edges, so `nexspec affected @acme/shared --global` or `nexspec affected "GET /contracts/{}" --global` answers across repository boundaries. See [the docs page](docs/content/docs/features/multi-repo-graph.mdx).

## Work memory

`nexspec save-result` records how an answer went (`useful`, `dead_end`, `corrected`) and the nodes it cited; `nexspec reflect` turns the notes into lessons (preferred sources, dead ends, corrections, with a 30-day half-life) in `.specs/.memory/LESSONS.md`, and `search`/`query` lean towards preferred nodes and away from dead ends unless `--no-memory`. `nexspec reflect --max-tokens 400` prints the short summary a skill loads at session start. Raw notes stay out of Git, lessons can be versioned. See [the docs page](docs/content/docs/features/work-memory.mdx).

## Annotations

`nexspec annotate` keeps what an agent or a person concluded about a node (what a community does, that a doc explains a symbol, that a path was a dead end) in `.specs/.memory/annotations.jsonl`, with author, date and the hash of the target; the index is derived from it. An annotation turns `stale` when its target changes and `dangling` when the target disappears; it shows in `explain`, `query` and `affected`, names communities in `report` and the wiki, and its outcome nudges ranking. `nexspec sync --similar` adds embedding-based `SimilarTo` edges. See [the docs page](docs/content/docs/features/semantic-annotations.mdx).

## MCP

```json
{
  "mcpServers": {
    "nexspec": { "command": "nexspec", "args": ["--repo", ".", "mcp"] }
  }
}
```

Tools: `query_context`, `semantic_search`, `trace_requirement`, `find_impacted_code`, `get_symbol_history`, `graph_report`, `query_graph`, `find_path`, `explain_node`, `find_affected`, `sync_workspace`.

## Graph report

`nexspec report` summarises the structure of the graph. The Markdown sections and the JSON keys are a stable contract for tools and skills:

| Markdown section            | JSON key                 | What it holds                                                                                                                                                                                   |
| --------------------------- | ------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `## Summary`                | `summary`                | Nodes and edges by type, files by language, last indexed commit, index size.                                                                                                                    |
| `## God Nodes`              | `god_nodes`              | The most connected files/symbols (degree over imports, calls, extends, `Satisfies`…), with dependents and whether a requirement is linked. Barrel files are left out (`barrel_files_excluded`). |
| `## Communities`            | `communities`            | Groups of files that belong together, with **cohesion** (internal ÷ total link weight; below 0.3 = fragile). Co-change only adds weight.                                                        |
| `## Requirement Coverage`   | `requirement_coverage`   | Requirements not linked to any code or task, tasks without a requirement, `@spec` references to missing requirements.                                                                           |
| `## Surprising Connections` | `communities.surprising` | Dependencies that cross communities unexpectedly, with a one-line reason.                                                                                                                       |
| `## Import Cycles`          | `import_cycles`          | Files that import each other and an approximate set of imports to cut.                                                                                                                          |
| `## Suggested Questions`    | `suggested_questions`    | Questions derived from the findings above.                                                                                                                                                      |

`--max-tokens` fits the Markdown into a budget by shrinking the least important sections first. `--fail-on-cycle` exits with status 2 when a cycle exists (with `--diff`, only when a cycle is *new* since that revision). `--diff REV` compares the current graph with a branch, tag or commit — nodes and edges added or removed, God nodes that moved, communities that merged or split, cycles and requirements whose state changed — by indexing `REV` read-only in a temporary directory.

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

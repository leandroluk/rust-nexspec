# Tasks: Semantic Annotations (Fase 18)

## T-1801: Modelo: nó `Annotation`, arestas e contextos (REQ-1804, REQ-1808) [ ]
- **What**: `NodePayload::Annotation`, `NodeType`, `EdgeType::{AnnotatedBy, SimilarTo}`, `EdgeContext::{Annotation, Embedding}`, pontuação nos bits altos de `meta`, filtros, export, busca; `INDEX_FORMAT` 9.
- **Where**: `src/graph/{node,edge}.rs`, `src/query/filter.rs`, `src/export/mod.rs`, `src/search/schema.rs`, `src/engine.rs`
- **Gate**: `cargo test graph:: query::filter export::`

## T-1802: Armazenamento das anotações (REQ-1801, REQ-1802, REQ-1807) [ ]
- **What**: `annotations.jsonl` (formato, ordem, id por conteúdo), chaves estáveis, `add/remove/list/show/lint`, segredo e limite de 500 caracteres.
- **Where**: `src/annotate/store.rs`
- **Gate**: `cargo test annotate::store`

## T-1803: Estados e materialização (REQ-1802, REQ-1803, REQ-1804) [ ]
- **What**: resolver chaves, `fresh/stale/dangling`, nós e arestas derivados, reconciliação, gancho no `sync`.
- **Where**: `src/annotate/materialize.rs`, `src/engine.rs`
- **Depends on**: T-1801, T-1802
- **Gate**: `cargo test --test annotate_cli`

## T-1804: `annotate` (CLI/MCP), consultas e rótulos de comunidade (REQ-1801, REQ-1805, REQ-1806, REQ-1809) [ ]
- **What**: comandos, `annotate_node`, anotações em `explain`/`query`/`affected`, rótulo de comunidade em `report` e wiki, desfechos no ranking (`--no-memory`).
- **Where**: `src/bin/nexspec.rs`, `src/mcp.rs`, `src/query/*.rs`, `src/report/*.rs`, `src/export/wiki.rs`, `src/memory/overlay.rs`
- **Depends on**: T-1803
- **Gate**: `cargo test --test annotate_cli`

## T-1805: `SimilarTo` por embeddings (REQ-1808) [ ]
- **What**: vizinhos por similaridade (K, limiar), arestas com pontuação, `sync --similar`, substituição a cada rodada.
- **Where**: `src/annotate/similar.rs`, `src/engine.rs`, `src/bin/nexspec.rs`
- **Depends on**: T-1801
- **Gate**: `cargo test annotate::similar`

## T-1806: Fechamento [ ]
- **What**: README, docs, ROADMAP/STATE, CI verde.
- **Gate**: `cargo test && cargo test --no-default-features --features lean && cargo clippy --all-targets -- -D warnings`

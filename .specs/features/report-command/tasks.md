# Tasks: Report Command (Fase 10)

## T-1001: `GraphSnapshot` e iteração de nós (base) [x]
- **REQ**: REQ-1001..1004 (base)
- **What**: `RedbParticipant::all_nodes()`; `GraphSnapshot { nodes, edges, file_of }` com `Engine::snapshot()`; helpers de rótulo/caminho.
- **Where**: `src/sync/redb_participant.rs`, `src/report/snapshot.rs`, `src/engine.rs`
- **Gate**: `cargo test report::snapshot`

## T-1002: Resumo (REQ-1001) [x]
- **What**: contagem por tipo de nó/aresta, arquivos por linguagem, último commit indexado + data, tamanho em disco.
- **Where**: `src/report/analysis.rs`, `src/engine.rs` (`index_info`)
- **Depends on**: T-1001
- **Gate**: `cargo test report::analysis::summary`

## T-1003: God nodes (REQ-1002) [x]
- **What**: top-N por grau (sem `CoChanges`/`DefinedIn`), dependentes diretos, caminho, presença de REQ.
- **Depends on**: T-1001
- **Gate**: `cargo test report::analysis::god`

## T-1004: Comunidades e coesão (REQ-1003) [x]
- **What**: grafo de arquivos ponderado; propagação de rótulos determinística; coesão; `fragile` < 0,3; comunidades pequenas resumidas.
- **Where**: `src/report/communities.rs`
- **Depends on**: T-1001
- **Gate**: `cargo test report::communities`

## T-1005: Cobertura de rastreabilidade (REQ-1004) [x]
- **What**: REQs sem implementação, tasks sem REQ, `@spec` órfãos.
- **Depends on**: T-1001
- **Gate**: `cargo test report::analysis::coverage`

## T-1006: Conexões inesperadas (REQ-1008) [x]
- **Depends on**: T-1004
- **Gate**: `cargo test report::communities::surprising`

## T-1007: Ciclos e perguntas (REQ-1009, REQ-1010) [x]
- **What**: SCCs de `Imports` + feedback arc set aproximado; perguntas por gabarito.
- **Depends on**: T-1003, T-1004, T-1005
- **Gate**: `cargo test report::analysis::cycles report::analysis::questions`

## T-1008: Renderização e orçamento (REQ-1005, REQ-1007) [x]
- **What**: Markdown com seções estáveis, JSON, `--max-tokens` por prioridade com "+N omitidos".
- **Where**: `src/report/render.rs`
- **Depends on**: T-1002..T-1007
- **Gate**: `cargo test report::render`

## T-1009: CLI e MCP (REQ-1006, REQ-1010) [x]
- **What**: `nexspec report [--format md|json] [--max-tokens N] [--top N] [--fail-on-cycle]`; tool MCP `graph_report`; README (contrato).
- **Depends on**: T-1008
- **Gate**: `cargo test --test report_cli`

## T-1010: `report --diff <rev>` (REQ-1011) [ ]
- **What**: `GitSource::at_revision`; indexar a revisão em diretório temporário; comparar (nós/arestas, God nodes, comunidades).
- **Where**: `src/git/source.rs`, `src/report/diff.rs`
- **Depends on**: T-1009
- **Gate**: `cargo test --test report_diff`

## T-1011: Aceite e fechamento [ ]
- **What**: rodar nos dois repos de referência; tempo; docs; ROADMAP/STATE; CI verde.
- **Depends on**: T-1001..T-1010
- **Gate**: `cargo test && cargo test --no-default-features --features lean && cargo clippy --all-targets -- -D warnings`

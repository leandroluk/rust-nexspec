# Tasks: Graph Query Surface (Fase 11)

## T-1101: `GraphView` e resolvedor de alvo (REQ-1108) [x]
- **What**: `GraphView { snapshot, fwd, rev }`; `TargetResolver::resolve(&str) -> Resolved::{One(id), Ambiguous(Vec<Candidate>), None(suggestions)}` com a ordem da design.md; `Engine::view()` e `Engine::resolve_target_id()`.
- **Where**: `src/query/view.rs`, `src/query/target.rs`, `src/engine.rs`
- **Gate**: `cargo test query::view query::target`

## T-1102: Filtros e relações (REQ-1105) [x]
- **What**: `Relation` (nomes estáveis, alias `dependencies`), `EdgeFilter` (relações, confiança mínima, contextos), parser de strings com erro legível.
- **Where**: `src/query/filter.rs`
- **Gate**: `cargo test query::filter`

## T-1103: `affected` (REQ-1104) [x]
- **What**: BFS reverso com profundidade e teto por hop; agrupa por arquivo (e comunidade quando disponível); "+N omitidos".
- **Depends on**: T-1101, T-1102
- **Gate**: `cargo test query::affected`

## T-1104: `path` (REQ-1102) [x]
- **What**: menor caminho não dirigido com direção/tipo/confiança por salto; "sem caminho" como resultado.
- **Depends on**: T-1101, T-1102
- **Gate**: `cargo test query::path`

## T-1105: `explain` (REQ-1103) [x]
- **What**: seções Node, Signature, Depends on, Depended on by, Requirements, Community, Recent authors; `Engine::signature_of`.
- **Depends on**: T-1101, T-1102
- **Gate**: `cargo test query::explain`

## T-1106: `query` (REQ-1101) [x]
- **What**: sementes da busca híbrida + BFS/DFS sob orçamento, filtros, ordem determinística.
- **Depends on**: T-1101, T-1102
- **Gate**: `cargo test query::query_graph`

## T-1107: Orçamento uniforme (REQ-1106) [x]
- **What**: `fit_lines`; `trace` ganha `--max-tokens`, `--depth` e filtros; todas as consultas o usam.
- **Where**: `src/query/budget.rs`, `src/bin/nexspec.rs`
- **Depends on**: T-1103..T-1106
- **Gate**: `cargo test query::budget && cargo test --test query_cli`

## T-1108: CLI (REQ-1101..1106) [x]
- **What**: `query`, `path`, `explain`, `affected` com `--max-tokens/--budget`, `--context`, `--relation`, `--depth`, `--min-confidence`, `--pick`, `--format md|json`.
- **Depends on**: T-1107
- **Gate**: `cargo test --test query_cli`

## T-1109: MCP (REQ-1107) [x]
- **What**: `query_graph`, `find_path`, `explain_node`, `find_affected`; README/docs com os nomes estáveis.
- **Depends on**: T-1108
- **Gate**: `cargo test mcp::`

## T-1110: Aceite, docs e fechamento [x]
- **What**: nos repos de referência: `affected CachePort` = dependentes conhecidos; `path` entre dois módulos; `explain` de um símbolo; nenhuma saída padrão > 2 000 tokens; docs; ROADMAP/STATE; CI verde.
- **Depends on**: T-1101..T-1109
- **Gate**: `cargo test && cargo test --no-default-features --features lean && cargo clippy --all-targets -- -D warnings`

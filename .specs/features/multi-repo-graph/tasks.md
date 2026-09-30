# Tasks: Multi-Repo Graph (Fase 13)

## T-1301: Tag, prefixo e união; `merge-graphs` (REQ-1301, REQ-1303) [x]
- **What**: campo `repo` no export; `tag_export` (ids globais e prefixos de origem), `merge` com regra documentada, comando `merge-graphs`.
- **Where**: `src/export/mod.rs`, `src/global/merge.rs`, `src/bin/nexspec.rs`
- **Gate**: `cargo test global::merge`

## T-1302: Ligação por pacote (REQ-1304) [x]
- **What**: `Package.dependencies` (payload, manifestos, export), `DependsOn` inferida entre repositórios por nome.
- **Where**: `src/graph/node.rs`, `src/domain/{manifest,graph}.rs`, `src/export/mod.rs`, `src/global/merge.rs`
- **Depends on**: T-1301
- **Gate**: `cargo test global::merge domain::manifest`

## T-1303: Endpoints HTTP e ligação entre repositórios (REQ-1304) [x]
- **What**: nós `Endpoint` de OpenAPI e de chamadas de cliente com caminho literal; normalização; `Calls` inferida consumidor → provedor no global.
- **Where**: `src/graph/node.rs`, `src/domain/http.rs`, `src/domain/graph.rs`, `src/global/merge.rs`
- **Depends on**: T-1301
- **Gate**: `cargo test domain::http global::merge`

## T-1304: Grafo global: `add|list|remove|path` (REQ-1302) [x]
- **What**: pasta global, exports guardados, reconstrução do índice, idempotência por `--as`.
- **Where**: `src/global/store.rs`, `src/engine.rs` (`apply_mutations`), `src/bin/nexspec.rs`
- **Depends on**: T-1301..T-1303
- **Gate**: `cargo test --test global_cli`

## T-1305: Consultas globais (REQ-1305) [x]
- **What**: `--global` e `--repo` em `query`, `path`, `explain`, `affected`; origem visível nos resultados.
- **Where**: `src/bin/nexspec.rs`, `src/query/target.rs`, `src/report/snapshot.rs`
- **Depends on**: T-1304
- **Gate**: `cargo test --test global_cli`

## T-1306: Driver de merge do Git (REQ-1306) [x]
- **What**: `merge-driver` (união), registro em `hook install`/`uninstall`/`status`.
- **Where**: `src/global/driver.rs`, `src/workflow/hooks.rs`, `src/bin/nexspec.rs`
- **Gate**: `cargo test --test global_driver`

## T-1307: Fechamento [x]
- **What**: README, docs, ROADMAP/STATE, CI verde; condominium quebrado em dois exports como prova de ligação.
- **Gate**: `cargo test && cargo test --no-default-features --features lean && cargo clippy --all-targets -- -D warnings`

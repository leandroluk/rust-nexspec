# Tasks: Graph Export (Fase 12)

## T-1201: Modelo de exportação e JSON (REQ-1201) [x]
- **What**: `ExportGraph`/`ExportPayload`, ordenação estável, filtros `--path`/`--kind`, ida e volta do payload, `Engine::export_graph`.
- **Where**: `Cargo.toml`, `src/export/mod.rs`, `src/engine.rs`
- **Gate**: `cargo test export::`

## T-1202: HTML interativo (REQ-1202) [x]
- **What**: arquivo único com layout, busca, filtros, detalhes, vizinhança, teto de nós com aviso; verificado no navegador.
- **Where**: `src/export/html.rs`
- **Depends on**: T-1201
- **Gate**: `cargo test export::html`

## T-1203: Árvore colapsável (REQ-1203) [x]
- **What**: diretório → arquivo → símbolo com inspetor de arestas.
- **Where**: `src/export/tree.rs`
- **Depends on**: T-1201
- **Gate**: `cargo test export::tree`

## T-1204: Wiki por comunidade (REQ-1204) [x]
- **What**: `index.md` + artigos com arquivos, símbolos, dependências entre comunidades e REQs; links relativos.
- **Where**: `src/export/wiki.rs`
- **Depends on**: T-1201
- **Gate**: `cargo test export::wiki`

## T-1205: Comando `export` e `--check` (REQ-1201..1205) [x]
- **What**: CLI com `--format`, `--out`, `--path`, `--kind`, `--max-nodes`, `--check` (saída 7); determinismo byte a byte.
- **Where**: `src/bin/nexspec.rs`
- **Depends on**: T-1201..T-1204
- **Gate**: `cargo test --test export_cli`

## T-1206: Fechamento [x]
- **What**: README (comandos, código 7), docs, ROADMAP/STATE, CI verde.
- **Gate**: `cargo test && cargo test --no-default-features --features lean && cargo clippy --all-targets -- -D warnings`

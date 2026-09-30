# Tasks: Work Memory (Fase 15)

## T-1501: Notas e `save-result` (REQ-1501, REQ-1505) [x]
- **What**: formato da nota (frontmatter + corpo), id estável, leitura/escrita, `save-result` (resolve nós, exige correção, recusa segredo), idempotência.
- **Where**: `src/memory/note.rs`, `src/bin/nexspec.rs`
- **Gate**: `cargo test memory::note`, `cargo test --test memory_cli`

## T-1502: `reflect` (REQ-1502, REQ-1504) [x]
- **What**: decaimento, classificação (Preferred/Tentative/Contested/Dead ends/Corrections), descarte de nós que sumiram, `LESSONS.md` determinístico, cache do ranking, `--max-tokens`.
- **Where**: `src/memory/reflect.rs`
- **Depends on**: T-1501
- **Gate**: `cargo test memory::reflect`, `cargo test --test memory_cli`

## T-1503: Sobreposição no ranking (REQ-1503) [x]
- **What**: cache → ajuste da pontuação em `Engine::search`; `--no-memory`; variáveis de ajuste.
- **Where**: `src/memory/overlay.rs`, `src/engine.rs`, `src/bin/nexspec.rs`
- **Depends on**: T-1502
- **Gate**: `cargo test --test memory_cli`

## T-1504: `bench --compare-memory` (REQ-1503) [x]
- **What**: com × sem memória, reprova se algum tipo perder mais de 2 pontos de `recall@5`.
- **Where**: `src/bench/compare.rs`, `src/bin/nexspec.rs`
- **Depends on**: T-1503
- **Gate**: `cargo test --test memory_cli`

## T-1505: MCP e `init` (REQ-1501, REQ-1504) [x]
- **What**: ferramentas `save_result` e `reflect`; `init` ignora `.specs/.memory/notes/`.
- **Where**: `src/mcp.rs`, `src/bin/nexspec.rs`
- **Depends on**: T-1502
- **Gate**: `cargo test mcp::`

## T-1506: Fechamento [x]
- **What**: README, docs, ROADMAP/STATE, CI verde.
- **Gate**: `cargo test && cargo test --no-default-features --features lean && cargo clippy --all-targets -- -D warnings`

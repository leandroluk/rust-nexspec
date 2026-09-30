# Tasks: Performance & Scale Guard (Fase 9)

Primeiro corte = T-901…T-906 incluindo T-903b, T-904b, T-904c (aprovado em 2026-09-29). Segundo corte = T-907…T-912.

## T-901: `PhaseTimings` e `sync --verbose` [x]
- **REQ**: REQ-908 (parte)
- **What**: `PhaseTimings { diff, markdown, code, co_change, stage, nodes, edges }` (`Duration` + contagens) preenchido em `run_once` e exposto em `SyncReport.timings`. Flag `--verbose` em `nexspec sync` imprime uma linha por fase. Necessário antes de qualquer otimização (medir primeiro).
- **Where**: `src/sync_orchestrator.rs`, `src/bin/nexspec.rs`
- **Depends on**: none
- **Done when**: teste de integração roda `sync` num fixture e verifica `timings.stage > 0` e `edges == soma das arestas staged`; testes existentes de `SyncReport` seguem verdes.
- **Gate**: `cargo test sync_orchestrator && cargo test --test cli_integration`

## T-902: Gerador de repositório sintético (REQ-901) [x]
- **REQ**: REQ-901
- **What**: `SyntheticParams { files, markdown_files, commits, big_commit_files, seed, scale }` com default espelhando o repo de referência (1.300 arq., ~160 commits, 1 commit de 800). `SyntheticRepo::generate(&params) -> SyntheticRepo` (TempDir). Escreve TS com classes/imports e `.md` com `REQ-…`, monta o stream e roda `git fast-import`. Timestamps retroativos a partir de `now` (D2). PRNG xorshift local.
- **Where**: `tests/fixtures/synthetic.rs`, `tests/fixtures/mod.rs` (`pub mod synthetic;`)
- **Depends on**: none
- **[P]**: A (paralelo com T-901)
- **Done when**: (a) mesma seed → mesmos `git rev-tree`/blobs em duas gerações (compara lista de `(path, blake3)` e topologia, não OIDs); (b) `git rev-list --count HEAD` == commits pedidos; (c) existe exatamente 1 commit com ≥ `big_commit_files` arquivos; (d) gerar o default leva < 15 s.
- **Gate**: `cargo test --test synthetic_repo`

## T-903: Medição de cold start e investigação 27 s vs 15 s [x]
- **REQ**: REQ-902 (baseline), REQ-903
- **What**: Rodar o sintético default e o repo real com `sync --verbose` em `--release`; registrar tempo por fase, tamanho de `sync.wal`, `edges.bin`, RSS de pico. Testar hipóteses 1–5 do design.md (uma variável por vez). Sem alterar produção além de T-901. Registrar tabela antes/depois em `STATE.md` e neste design.md.
- **Where**: `.specs/features/performance-guard/design.md` (seção "Medições"), `STATE.md`
- **Depends on**: T-901, T-902
- **Done when**: há uma tabela com fase → segundos para as duas bases e a causa dominante dos 27 s está nomeada (ou explicitamente "não reproduzido"). Nenhum processo nexspec vivo ao terminar.
- **Gate**: medição registrada (sem gate de código)

## T-903b: Cache de queries Tree-sitter + extração paralela [x]
- **REQ**: REQ-902 (cold start)
- **What**: `compiled_queries(language)` com `OnceLock` por linguagem (símbolo + chamada); `SyncOrchestrator` junta os arquivos de código (commitados + sujos) e chama `code::extract_all` (rayon) em vez de laço sequencial. Achado da T-903.
- **Where**: `src/code/parser.rs`, `src/sync_orchestrator.rs`
- **Depends on**: T-903
- **Done when**: teste unitário garante que as queries são compiladas uma vez por linguagem; suíte inteira verde; `code` no repo real < 1 s (medido: 11,9 s → 0,6 s).
- **Gate**: `cargo test`

## T-904: Teto de co-change (REQ-903) [x]
- **REQ**: REQ-903
- **What**: `CoChangeWindow` ganha `max_files_per_commit` (default 200, `COCHANGE_MAX_FILES`) e `max_pairs_per_file` (default 50). `co_change_edges` ignora commits acima do teto e limita parceiros por arquivo, priorizando commits recentes. Documentar o trade-off no doc comment e em `docs/`. Reindexação tratada por T-904b.
- **Where**: `src/git/cochange.rs`, `src/sync_orchestrator.rs`, `docs/`
- **Depends on**: T-903 (a medição confirma que co-change é o gargalo antes de fixar defaults)
- **Done when**: testes: (a) commit com 201 arquivos não gera arestas, com 200 gera; (b) arquivo que co-muda com 60 outros mantém ≤ 50 parceiros, os mais recentes; (c) o sintético default produz < 300 mil arestas de co-change (vs ~1,1 mi) e o cold start medido cai; (d) `COCHANGE_MAX_FILES=5` é respeitado.
- **Gate**: `cargo test git::cochange && cargo test --test perf_smoke`

## T-904c: O índice não conta como sujo [x]
- **REQ**: REQ-902 (sync sem mudanças), REQ-905(b)
- **What**: `dirty_paths` ignora o `index_dir` do engine (`.specs/.index/`) mesmo sem `.gitignore`; `init` passa a sugerir/gravar a entrada no `.gitignore` do projeto.
- **Where**: `src/sync_orchestrator.rs`, `src/git/source.rs`, `src/engine.rs`
- **Depends on**: T-903b
- **Done when**: repo sem `.gitignore`: `init` + 2 `sync` seguidos → segundo com `files_dirty == 0` e sem frame novo no WAL.
- **Gate**: `cargo test --test sync_orchestrator`

## T-904b: `index_format` e reconstrução automática (D8) [x]
- **REQ**: REQ-903 (migração)
- **What**: `VersionPointer::index_format()/set_index_format()` (chave em `meta`), `const INDEX_FORMAT: u64 = 2` em `engine.rs`. `Engine::open`: índice existente com formato ausente/diferente → remove `index_dir` (só ele) e recria; aviso em stderr. Índice novo grava o formato atual.
- **Where**: `src/sync/version.rs`, `src/engine.rs`
- **Depends on**: T-904
- **Done when**: testes: (a) índice novo grava o formato; (b) índice com formato antigo é apagado e `sync` seguinte o reconstrói com o mesmo resultado de um índice novo; (c) arquivos fora de `index_dir` (ex.: `.specs/graph/x`) permanecem intactos.
- **Gate**: `cargo test engine && cargo test --test cli_integration`

## T-905: Regressões nomeadas (REQ-905) [ ]
- **REQ**: REQ-905
- **What**: `tests/regressions.rs` com: `regression_csr_upsert_bulk_is_linear` (a; 200k upserts < 2 s e razão 2N/N < 2,5), `regression_noop_sync_stages_nothing` (b; segundo `sync` → `target_version == None`, WAL não cresce), `regression_untracked_only_tree_is_indexed` (c), `regression_dirty_respects_git_status_with_autocrlf` (d; `core.autocrlf=true`, arquivos CRLF não tocados não aparecem), `regression_abstract_class_enum_type_alias_are_symbols` (e), `regression_prefixed_and_suffixed_requirement_ids` (f; `REQ-CTR-001`, `REQ-021b`), `regression_task_links_to_requirement_across_files` (g; `trace REQ-…` devolve o implementador). Cada um falha se a correção correspondente for revertida (verificar revertendo localmente uma vez por teste).
- **Where**: `tests/regressions.rs`
- **Depends on**: none (usa `FixtureRepo` existente)
- **[P]**: A (paralelo com T-902)
- **Done when**: 7 testes verdes; para cada um, `git stash`/revert manual da correção → teste vermelho (registrar no SUMMARY).
- **Gate**: `cargo test --test regressions`

## T-906: Fechamento do primeiro corte [ ]
- **REQ**: (todos do corte)
- **What**: `cargo test` full e lean, `cargo clippy --all-targets -- -D warnings`; atualizar STATE.md/ROADMAP.md; commit + push. `cargo install --path . --force` só com nenhum nexspec rodando; reindexar `condominium-management-system` e medir de novo (comparar com 27 s / 0,9–1,6 s).
- **Where**: `.specs/project/STATE.md`, `.specs/project/ROADMAP.md`
- **Depends on**: T-901…T-905, T-904b
- **Done when**: gates verdes, medição real registrada, commit feito.
- **Gate**: `cargo test && cargo test --no-default-features --features lean && cargo clippy --all-targets -- -D warnings`

## T-907: Teste de orçamento de tempo (REQ-902) [ ]
- **REQ**: REQ-902
- **What**: `tests/perf_budget.rs` `#[ignore]`: init+sync frio, sync sem mudança (min de 3; e sem novo frame no WAL), sync após 1 arquivo, `search`/`trace`. Limites via `NEXSPEC_BUDGET_*`, escala via `NEXSPEC_SYNTH_SCALE`.
- **Where**: `tests/perf_budget.rs`
- **Depends on**: T-902, T-904
- **Done when**: passa com folga local em `--release`; forçar um limite absurdo (`NEXSPEC_BUDGET_COLD_S=1`) o faz falhar com mensagem que nomeia a fase.
- **Gate**: `cargo test --release --test perf_budget -- --ignored`

## T-908: Micro-benchmarks de complexidade (REQ-904) [ ]
- **REQ**: REQ-904
- **What**: `criterion` (dev-dependency) para `CsrDelta::upsert/remove/edges_from`, `co_change_edges`, `markdown::extract`, extração de símbolos; verificação T(2N)/T(N) ≤ 2,2 em teste normal (não no bench) usando medida de operações/tempo com N pequeno.
- **Where**: `benches/complexity.rs`, `Cargo.toml`
- **Depends on**: T-904
- **Done when**: `cargo bench --no-run` compila; teste de razão verde; `edges_from` documentado como O(delta) com decisão (corrigir ou aceitar).
- **Gate**: `cargo bench --no-run && cargo test complexity`

## T-909: Lock de sync com espera (REQ-907) [ ]
- **REQ**: REQ-907
- **What**: `SyncLock` com lock de SO em `.specs/.index/sync.lock`, espera com timeout (`NEXSPEC_LOCK_TIMEOUT_S`, default 30) antes de `Database::create`; mensagem clara ao estourar. Ajustar hook `post-commit` da skill para não competir.
- **Where**: `src/sync/lock.rs`, `src/engine.rs`, skill `graph-spec-design`
- **Depends on**: T-906
- **Done when**: teste sobe dois `sync` em paralelo (processos) → ambos terminam sem "Database already open"; com timeout de 1 s e um processo segurando o lock → erro legível.
- **Gate**: `cargo test --test sync_lock`

## T-910: Higiene do WAL (REQ-908) [ ]
- **REQ**: REQ-908
- **What**: `Wal::truncate_if_idle()` chamado após `mark_done`; testes de crash recovery existentes seguem verdes; `sync.wal` do sintético < 10 MB após dois syncs.
- **Where**: `src/sync/wal.rs`, `src/sync/coordinator.rs`
- **Depends on**: T-906
- **Done when**: teste: após sync completo o arquivo tem 0 bytes; simulação de crash entre `stage` e `mark_done` ainda recupera via `resume()`.
- **Gate**: `cargo test sync::wal && cargo test --test sync_crash_recovery`

## T-911: CI matrix (REQ-906) [ ]
- **REQ**: REQ-906
- **What**: `.github/workflows/ci.yml`: `windows-latest` e `ubuntu-latest` × `cargo test` (full e lean) + `clippy -D warnings`; job de orçamento com escala 0,25 em PR e 1,0 noturno (`schedule`); cache de `target/` e do modelo ONNX.
- **Where**: `.github/workflows/ci.yml`
- **Depends on**: T-907
- **Done when**: workflow válido (`actionlint` ou execução no GitHub); primeira execução verde nas duas plataformas.
- **Gate**: execução do workflow

## T-912: Fechamento da fase [ ]
- **REQ**: todos
- **What**: Atualizar ROADMAP (Fase 9 completa), STATE, docs; atualizar skill (commit + push + copiar para `~/.claude/skills` e `~/.gemini/config/skills`).
- **Depends on**: T-907…T-911
- **Gate**: `cargo test && cargo test --no-default-features --features lean && cargo clippy --all-targets -- -D warnings`

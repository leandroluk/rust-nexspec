# Spec: Performance & Scale Guard — CI com orçamento de tempo e regressões reais (Fase 9)

> Origem: avaliação de 2026-09-29 (item 3). Bugs achados só ao rodar num repositório real e que os testes unitários não pegaram:
> `CsrDelta::upsert` O(n²) (sync não terminava com ~1,1 mi de arestas de co-change), arestas de co-change recalculadas em todo sync,
> `DirtyCache` marcando todos os arquivos como sujos, arquivos não rastreados ignorados, WAL crescendo (113 MB), lock do redb.

## Summary

Transformar cada falha de escala/plataforma já vista em teste automatizado e adicionar um teste de orçamento de tempo sobre um repositório sintético grande e determinístico, executado em CI em Windows e Linux. O objetivo é que o próximo O(n²) falhe no CI, não no projeto do usuário.

## Requirements

- REQ-901: **Gerador de repositório sintético determinístico** — cria N arquivos TS/MD, M commits e commits "grandes" (ex.: 800 arquivos no mesmo commit) com seed fixa, num diretório temporário. Parâmetros default espelham o repositório de referência (~1,3 mil arquivos, ~160 commits, 1 commit de ~800 arquivos).
- REQ-902: **Orçamentos de tempo** — `init + sync` inicial < 30 s no runner de CI; `sync` sem mudanças < 2 s e sem novo frame no WAL; `sync` após 1 arquivo alterado < 3 s; `search`/`trace` < 1 s. Estouro falha o job (limites configuráveis por variável para runners lentos).
- REQ-903: **Limite do grafo de co-change** — ignorar commits com mais de `COCHANGE_MAX_FILES` arquivos (default 200) e limitar pares por arquivo; documenta o trade-off. Evita crescimento quadrático de arestas independentemente do tamanho do commit.
- REQ-904: **Micro-benchmarks de complexidade** — `criterion` para `CsrDelta::upsert/remove`, `co_change_edges`, extração de Markdown e de símbolos, com verificação de que dobrar N não mais que ~2,2× o tempo (linear).
- REQ-905: **Regressões nomeadas** — um teste por bug real: (a) upsert em massa linear; (b) sync sem mudanças não reestagia arestas; (c) árvore com só arquivo **não rastreado** entra no índice; (d) `dirty` lista apenas arquivos que o git reporta (autocrlf ligado); (e) `export abstract class`/`enum`/`type` viram símbolos; (f) ids com prefixo (`REQ-CTR-001`) e sufixo (`REQ-021b`); (g) task em `tasks.md` liga a REQ de `spec.md` e `trace REQ-…` devolve implementadores.
- REQ-906: **Matriz de CI** — `windows-latest` e `ubuntu-latest`: `cargo test` (full e lean), `cargo clippy -D warnings`, mais o job de orçamento (REQ-902). Cache do `target/` e do modelo ONNX.
- REQ-907: **Concorrência e locks** — segundo `nexspec sync` durante outro em andamento espera (com timeout configurável, default 30 s) em vez de falhar com "Database already open"; mensagem clara ao estourar. Hook `post-commit` da skill deixa de causar corrida com sync manual.
- REQ-908: **Observabilidade e higiene do WAL** — `nexspec sync --verbose` imprime tempo por fase (diff, markdown, código, co-change, stage/commit) e contagem de nós/arestas; frames concluídos do WAL são truncados/compactados (tamanho do `sync.wal` limitado).

## Out of Scope

- Benchmark de latência de embeddings (ONNX) em hardware específico.
- Testes de carga concorrente multi-processo além do cenário de REQ-907.

## Open Questions

- Q1: **Resolvida (2026-09-30)**: PR com repositório sintético reduzido (escala 0,25); completo noturno (`schedule`). Ver `.github/workflows/ci.yml`.
- Q2: `COCHANGE_MAX_FILES` = 200 (default, configurável) segue sendo ajustado na Fase 8 — ver Q3 do spec `retrieval-benchmark`.

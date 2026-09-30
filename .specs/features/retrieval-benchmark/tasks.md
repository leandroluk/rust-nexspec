# Tasks: Retrieval Benchmark (Fase 8)

## T-801: Corpus TOML (REQ-801) [x]
- **REQ**: REQ-801
- **What**: `cargo add toml`. `bench::corpus::{Corpus, Query, Kind}` com `serde`; validação (ids únicos, `expect` não vazio, `kind` válido, `grep` default = 1ª palavra). Campo opcional `commit`.
- **Where**: `Cargo.toml`, `src/bench/mod.rs`, `src/bench/corpus.rs`, `src/lib.rs`
- **Depends on**: none
- **Done when**: testes: arquivo válido carrega; id duplicado, `kind` desconhecido e `expect` vazio falham com mensagem que cita o id.
- **Gate**: `cargo test bench::corpus`

## T-802: Hit → localizações (`Engine::hit_locations`) [x]
- **REQ**: REQ-802
- **What**: `Location { path: Option<String>, symbol: Option<String>, marker: Option<String> }` e `Engine::hit_locations(&SearchHit)`: `File` → path; `Symbol` → path via `DefinedIn` + nome; `Requirement/Task/Adr` → marcador + caminhos dos implementadores por `Satisfies` de entrada. `bench::locate::ranked_files(hits)` = arquivos únicos na ordem do primeiro aparecimento.
- **Where**: `src/engine.rs`, `src/bench/locate.rs`
- **Depends on**: T-801
- **Done when**: teste de integração com fixture (spec + `.ts` com `@spec`): busca por símbolo devolve o arquivo dele; busca por REQ devolve o arquivo implementador; duplicatas colapsam mantendo a 1ª posição.
- **Gate**: `cargo test bench::locate && cargo test --test bench_locate`

## T-803: Métricas (`recall@k`, `MRR`) [x]
- **REQ**: REQ-802
- **What**: funções puras `recall_at_k(ranked, expect, k)`, `reciprocal_rank`, agregação por `kind`; casamento por sufixo de caminho, nome de símbolo e marcador.
- **Where**: `src/bench/metrics.rs`
- **Depends on**: T-801
- **[P]**: A (paralelo com T-802)
- **Done when**: testes com casos à mão (acerto parcial, nenhum acerto → MRR 0, sufixo de caminho, `expect` com 2 itens e 1 no top-k = 0,5).
- **Gate**: `cargo test bench::metrics`

## T-804: Baselines de custo (REQ-803, REQ-808) [x]
- **REQ**: REQ-803, REQ-808
- **What**: `grep_tokens(repo, term)` (em processo, mesmos arquivos que o git rastreia, saída `arquivo:linha:texto`), `read_tokens(repo, expect)` (conteúdo integral dos arquivos esperados), `corpus_tokens(repo)` (todos os arquivos indexáveis). Usa `Tokenizer` da Fase 5 (`--tokenizer`).
- **Where**: `src/bench/baselines.rs`
- **Depends on**: T-801
- **[P]**: A
- **Done when**: fixture com 3 arquivos: contagem de grep confere com linhas esperadas; arquivo inexistente em `expect` é erro explícito (não zero silencioso); corpus inteiro = soma dos blobs do HEAD.
- **Gate**: `cargo test bench::baselines`

## T-805: Runner, relatório e `nexspec bench` [x]
- **REQ**: REQ-802, REQ-803, REQ-807, REQ-808
- **What**: `bench::runner::run(&Corpus, &Options) -> BenchReport` (índice temporário + `sync`, latência por consulta, tokens da resposta via `search(query, Some(budget))`). `report`: JSON e Markdown com `recall@k/MRR/tokens/latência` por consulta e por `kind`, economia `1 − nexspec/baseline`, `behavior` com custo somado, bloco "custo fixo do fluxo" separado, razão de redução no formato do `graphify benchmark`. CLI: `nexspec bench --corpus --repo --k --index-dir --format --tokenizer`.
- **Where**: `src/bench/runner.rs`, `src/bench/report.rs`, `src/bin/nexspec.rs`
- **Depends on**: T-802, T-803, T-804
- **Done when**: teste de CLI roda um corpus de 3 perguntas num fixture e valida o JSON (campos, `recall@5 == 1.0`) e o Markdown (seção por `kind`, aviso de que tokens são proxy); o repositório-alvo fica sem `.specs/.index`.
- **Gate**: `cargo test --test bench_cli`

## T-806: Corpora (REQ-804) [x]
- **REQ**: REQ-804
- **What**: `bench/self.toml` (~20 perguntas sobre este repo, 4 kinds, origem em `notes`) e um corpus para `condominium-management-system` (~20 perguntas, incluindo o caso `outbox`) guardado **no repo-alvo** (`.specs/bench/queries.toml`, convenção D9), só lido daqui pelo runner. Verificar cada `expect` contra os arquivos reais antes de gravar.
- **Where**: `bench/self.toml`, repo externo
- **Depends on**: T-805
- **Done when**: `nexspec bench --corpus bench/self.toml` roda limpo; todo `expect` existe no alvo (teste que valida os caminhos de `self.toml` contra `git ls-files`).
- **Gate**: `cargo test --test bench_corpora`

## T-807: Baseline gravado e portão de regressão (REQ-805) [x]
- **REQ**: REQ-805
- **What**: `--update-baseline` grava `bench/baseline.json` (resultado + commit); `--check` falha se `recall@5` em `locate` < 0,8 **ou** cair > 5 pontos vs. o baseline. Job `bench` no CI (Linux) rodando `--check` no corpus próprio.
- **Where**: `src/bench/report.rs`, `src/bin/nexspec.rs`, `bench/baseline.json`, `.github/workflows/ci.yml`
- **Depends on**: T-806
- **Done when**: teste: baseline artificialmente alto → `--check` sai com código ≠ 0 e mensagem com os deltas; baseline igual → 0. Nesta tarefa o baseline grava o **estado atual, mesmo abaixo de 0,8** (o portão de 0,8 passa a valer ao fim da T-808).
- **Gate**: `cargo test --test bench_check`

## T-808: Melhorias de ranking medidas (REQ-806) [ ]
- **REQ**: REQ-806
- **What**: em sequência, cada uma com delta no benchmark (corpus próprio + condominium) registrado no design.md: (a) tokenização ciente de identificadores (camelCase/snake/Pascal, tokenizador customizado no Tantivy); (b) campos `name` e `path` separados com boost; (c) penalização de arquivos genéricos do mesmo padrão; (d) pesos BM25×vetor no RRF; (e) variante "expansão por co-change" (resolve a Q3 herdada da Fase 9). Mudanças que alteram o índice sobem `INDEX_FORMAT`. Meta: `recall@5 ≥ 0,8` em `locate` no corpus próprio.
- **Where**: `src/search/*`, `src/hybrid.rs`, `src/engine.rs`
- **Depends on**: T-807
- **Done when**: cada mudança aceita tem delta positivo ou neutro no `locate` sem piorar `structure`/`traceability` > 2 pontos; o caso `outbox` passa a ter `outbox.decorator.ts` no top-5; `bench/baseline.json` atualizado.
- **Gate**: `cargo test && nexspec bench --corpus bench/self.toml --check`

## T-809: Relatório honesto (REQ-807) [ ]
- **REQ**: REQ-807, REQ-808
- **What**: bloco de custo fixo por sessão (tokens de STATE.md + skill, medidos por arquivo, configuráveis por `--fixed-cost-file`), economia de localização separada da economia total, comparação `nexspec` vs. `graphify benchmark` nas mesmas perguntas (formato de razão de redução).
- **Where**: `src/bench/report.rs`, `docs/content`
- **Depends on**: T-808
- **Done when**: relatório de exemplo mostra as duas economias lado a lado; docs descrevem como reproduzir.
- **Gate**: `cargo test --test bench_cli`

## T-810: Fechamento da fase [ ]
- **REQ**: todos
- **What**: ROADMAP/STATE/docs; se a skill mudar, commit + push + copiar para `~/.claude/skills` e `~/.gemini/config/skills`.
- **Depends on**: T-801…T-809
- **Gate**: `cargo test && cargo test --no-default-features --features lean && cargo clippy --all-targets -- -D warnings`

# Design: Performance & Scale Guard (Fase 9)

## Architecture Overview

Nenhum componente novo de produção além de dois pontos pequenos (teto de co-change e timer de fases). O grosso da fase é **infraestrutura de teste**: um gerador de repositório sintético compartilhado por três consumidores (regressões nomeadas, orçamento de tempo, benchmarks) e um job de CI.

```
tests/fixtures/synthetic.rs  (REQ-901)
   ├─► tests/regressions.rs        (REQ-905)  repos minúsculos, rodam em todo `cargo test`
   ├─► tests/perf_budget.rs        (REQ-902)  repo ~1,3k arq., #[ignore], `--release`, limites via env
   └─► benches/*.rs (criterion)    (REQ-904)  micro-benchmarks, N e 2N

src (produção)
   git/cochange.rs        + CoChangeWindow { max_files_per_commit, max_pairs_per_file }   (REQ-903)
   sync_orchestrator.rs   + PhaseTimings em SyncReport                                    (REQ-908)
   sync/wal.rs            + truncate quando não há frame pendente                         (REQ-908)
   engine.rs              + lock de arquivo com espera/timeout em sync()                  (REQ-907)
```

## Dependency Paths

Confirmados lendo o código (o repo ainda não é indexado pelo nexspec, e o graphify não foi rodado para esta fase — só o código foi lido):

- `nexspec sync` → `Engine::sync` (`src/engine.rs:219`) → `SyncOrchestrator::run_once` (`src/sync_orchestrator.rs:71`) → `GitSource::co_change_edges` (`src/git/cochange.rs:50`) → `Coordinator::stage` (`src/sync/coordinator.rs:48`) → `Wal::append_frame` + participantes (redb, CSR, tantivy, hnsw).
- REQ-903 → `co_change_edges`. Hoje: todo commit com k arquivos gera k·(k−1) arestas dirigidas; um commit de 800 arquivos = 639.200 arestas. É a causa dos ~1,1 mi de arestas.
- REQ-907 → `Engine::open` (`Database::create` em `src/engine.rs:157`) — o lock do redb é tomado aqui, então o lock de espera precisa ser adquirido **antes** de `Database::create`, no `open`.
- REQ-908 → `Wal` só cresce: `append_raw_frame` abre em modo append e nada trunca (`src/sync/wal.rs:65`).

## Hipóteses para "27 s vs 15 s do graphify" (T-903 confirma ou refuta, não assumir)

1. **Arestas de co-change (~1,1 mi):** serialização rkyv do `MutationSet` inteiro + `fsync` do frame no WAL + `stage` em 3–4 participantes (CSR delta em memória, redb, tantivy). Provável dominante.
2. **Um único frame gigante no WAL** (1 frame com todas as arestas) — memória e `fsync` grandes; explica os 113 MB de `sync.wal`.
3. **Hash/parse por arquivo é sequencial** em `run_once` (loop `for path in diff.added…`); `rayon` está nas dependências mas não é usado aqui. `code::batch` existe — verificar se o orquestrador o usa.
4. `read_blob_at_head` por arquivo abre o objeto via `gix` sem cache de objetos.
5. Tantivy: commit único grande com merge de segmentos.

Método: `--verbose` (T-901) imprime tempo por fase; comparar com e sem co-change (REQ-903 ligado/desligado). Só otimiza o que a medição apontar; o alvo do REQ-902 é < 30 s, não paridade com o graphify.

Risco correlato descoberto na leitura: `CsrDelta::edges_from` (`src/graph/csr/delta.rs:47`) é um filtro linear sobre `added`. Com o teto de co-change o delta encolhe (~150 mil arestas em vez de 1,1 mi), mas `trace`/`search` antes da compactação continuam O(delta). Coberto por T-908 (benchmark `edges_from`); correção fica fora desta fase se o orçamento de `trace` < 1 s for cumprido.

## Medições (T-903, 2026-09-29, `--release`, Windows, 1 execução, `sync` frio)

Bases: sintético default (1.300 arq., 160 commits, 1 commit de 800) e clone local do condominium-management-system (1.267 arq., 164 commits).

| Fase                              | Sintético (antes) | Real (antes) | Real (após T-903b) |
| --------------------------------- | ----------------- | ------------ | ------------------ |
| diff (+ scan de sujos)            | 0,18 s            | 1,12 s       | 0,08 s             |
| markdown                          | ~0                | ~0           | 0,04 s             |
| **code (Tree-sitter)**            | **12,1 s**        | **11,9 s**   | **0,60 s**         |
| co-change (cálculo)               | 0,32 s            | 1,02 s       | 0,66 s             |
| **stage (WAL + 4 participantes)** | 3,1 s             | 3,7 s        | 3,7 s              |
| **total (parede)**                | 15,9 s            | 18,0 s       | 6,6 s              |

Arestas staged: 646 mil (sintético) / 1,04 mi (real), das quais 99,4% são co-change. Tamanhos após o sync real: `metadata.redb` 539 MB, `sync.wal` 113 MB, `edges.bin` 102 MB.

**Conclusões**
- Hipótese 1 (co-change) só se confirma para o *stage* (3,7 s, WAL e redb enormes); o cálculo em si custa < 1 s.
- **Causa dominante do cold start era outra, não listada:** `code::extract` recompilava as duas `Query` do Tree-sitter a cada arquivo (~9 ms/arquivo) e o orquestrador não usava o `extract_all` paralelo. Correção (T-903b): cache de queries por linguagem (`OnceLock`) + `extract_all` (rayon). 11,9 s → 0,6 s.
- O "27 s" do relatório anterior não se reproduziu: o mesmo repo dá 18 s antes da correção e 6,6 s depois (o número anterior provavelmente incluía a build/aquecimento; não investigado).
- Achado lateral: um `sync` sem mudanças leva ~1,2 s em `stage` e reporta `dirty=24` num repo sem `.gitignore` para `.specs/.index/`: os próprios arquivos do índice contam como sujos e viram um frame novo no WAL a cada sync. Vai para T-904c.

## New Components

| Component                           | Responsibility                                                                                            | Location                               |
| ----------------------------------- | --------------------------------------------------------------------------------------------------------- | -------------------------------------- |
| `SyntheticRepo` / `SyntheticParams` | Gera repo git determinístico (arquivos TS/MD, commits, commit grande) via `git fast-import` com seed fixa | `tests/fixtures/synthetic.rs`          |
| `PhaseTimings`                      | Tempo por fase do sync (diff, markdown, código, co-change, stage) + contagem de nós/arestas               | `src/sync_orchestrator.rs`             |
| `perf_budget` test                  | Executa init+sync, sync sem mudança, sync com 1 arquivo, search/trace e compara com limites               | `tests/perf_budget.rs`                 |
| `regressions` tests                 | Um teste por bug real (a–g)                                                                               | `tests/regressions.rs`                 |
| Benchmarks criterion                | `CsrDelta`, `co_change_edges`, markdown, símbolos; razão T(2N)/T(N) ≤ 2,2                                 | `benches/complexity.rs`                |
| `IndexFormat`                       | Chave `index_format` em `meta`; `Engine::open` reconstrói o índice em caso de divergência                 | `src/sync/version.rs`, `src/engine.rs` |
| `SyncLock`                          | Lock de arquivo (`.specs/.index/sync.lock`) com espera e timeout                                          | `src/sync/lock.rs`                     |
| CI matrix                           | windows/ubuntu × (full, lean, clippy) + job de orçamento                                                  | `.github/workflows/ci.yml`             |

## Modified Components

| Component                            | Change                                                                                                                                                                                                                                                                                                       | Risk                                                                                                                                        |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------- |
| `CoChangeWindow` (`git/cochange.rs`) | Novos campos `max_files_per_commit` (default 200, env `COCHANGE_MAX_FILES`) e `max_pairs_per_file` (default 50). Commit acima do teto é ignorado por inteiro. Cada arquivo mantém no máximo `max_pairs_per_file` parceiros, priorizando os de commits mais recentes (percurso é do mais novo ao mais antigo) | Muda o grafo de co-change de projetos existentes → precisa reindexar; `Default` é usado em `sync_orchestrator.rs:162` e `engine.rs` (blame) |
| `SyncReport`                         | Campo `timings: PhaseTimings`; `bin/nexspec.rs` imprime com `--verbose`                                                                                                                                                                                                                                      | Struct é `PartialEq`; testes existentes comparam relatórios — usar `..Default`                                                              |
| `Wal`                                | `truncate_if_idle()`: após `mark_done` do último frame, se `pending_frames()` é vazio, `set_len(0)`                                                                                                                                                                                                          | Truncar quebra `resume()` se chamado antes do commit-marker durável; só chamar depois do `fsync` do marker                                  |
| `Engine::open`/`sync`                | Adquire `SyncLock` antes do `Database::create`                                                                                                                                                                                                                                                               | Lock órfão após crash: usar lock de SO (`fs2`/`std::fs::File::lock`), não arquivo-existe                                                    |
| `Coordinator::stage`                 | Nenhuma (frame único mantido) — só reavaliar se T-903 apontar o frame gigante                                                                                                                                                                                                                                | God node de escrita; qualquer mudança exige os testes de crash recovery                                                                     |

## Decision Log

- **D1 — Repositório sintético via `git fast-import`, não `git commit` em loop.** 160 commits × `add -A` na fixture atual levaria minutos no Windows. `fast-import` cria tudo em um processo, com timestamps explícitos. O commit "grande" (800 arquivos) é um único commit do stream.
- **D2 — Determinismo = conteúdo e estrutura, não datas.** A janela de co-change usa `now − 6 meses`, então os timestamps dos commits sintéticos são relativos a `now` (um por hora, retroativos). Conteúdo, nomes e topologia dependem só da seed (PRNG xorshift local, sem a crate `rand`).
- **D3 — Teto de co-change ignora o commit inteiro, não trunca.** Truncar um commit gigante escolheria arquivos arbitrários e criaria arestas espúrias; commits de renomeação/formatação em massa são exatamente o ruído a descartar. `max_pairs_per_file` protege o caso de muitos commits médios (100 arquivos × muitos commits).
- **D4 — Orçamento de tempo em teste `#[ignore]` chamado com `--release`**, lendo `NEXSPEC_BUDGET_COLD_S` etc. (defaults do REQ-902). `debug` é ordens de grandeza mais lento e geraria falso positivo. Resolve Q1 do spec: job de PR com repo reduzido (`NEXSPEC_SYNTH_SCALE=0.25`, limites proporcionais), job noturno com escala 1.0.
- **D5 — Q2 resolvida (2026-09-29): 200 é o default e é configurável.** O teto é proteção de DX/escala (evita crescimento quadrático), independente de qualidade de ranking; o benchmark da Fase 8 só afina o valor depois.
- **D8 — Versão de formato do índice (resolve a pergunta de reindexação).** O índice é derivado (git + specs), então é descartável. `meta` ganha `index_format` (u64). `Engine::open` compara com `INDEX_FORMAT` do binário: diferente (ou ausente em índice existente) → apaga `.specs/.index/` e o próximo `sync` reconstrói, com aviso na saída. O teto de co-change sobe o formato para 2; as Fases 7/11/etc. reaproveitam o mecanismo. Descartada a alternativa 'rode init de novo' (depende do usuário lembrar) e a de remover só arestas CoChanges (exige enumerar arestas antigas no CSR).
- **D6 — Regressões nomeadas ficam em `tests/regressions.rs` com o nome `regression_<bug>`**, para o `grep`/`cargo test regression_` listar o inventário. Onde já existe teste equivalente no módulo, o teste novo é de integração (via `Engine`), não duplicata.
- **D7 — Escopo do primeiro corte (aprovação pendente):** T-901…T-905 (medir, teto, regressões). REQ-902/904/906/907/908 completos viram um segundo corte, dentro da mesma fase.
- **D9 — `CsrDelta::edges_from` fica O(delta) (decisão da T-908).** Medido: 215 µs por chamada com 100 mil arestas no delta. O delta é limitado pela compactação (5% da base) e o cold start compacta de imediato, então o custo real é de dezenas de µs; indexar por `from` complicaria `remove` (swap_remove) sem ganho mensurável. Reavaliar se `trace` passar de 1 s no orçamento.
- **D10 — Razão de linearidade = 2,5 (spec dizia ~2,2).** Melhor de 5 execuções em N e 2N; quadrático dá ~4 (medido 2,88 em `code::extract` com 12 mil funções antes da correção). Medições atuais: 1,8 a 2,1.

## Risks

- Sem `GRAPH_REPORT`/God nodes (nexspec não indexa o próprio repo, decisão do usuário): a análise de raio de mudança veio da leitura direta. `Coordinator::stage` e `Engine` são os pontos de maior raio.
- Testes de tempo são intrinsecamente ruidosos: usar `min` de 3 execuções para sync sem mudança e limites com folga (2×) sobre a medição de referência do runner.
- Windows: `fast-import` e `core.autocrlf` (regressão d) dependem do `git` do PATH — os testes já assumem isso (`tests/fixtures/mod.rs`).
- Reindexação: mudar o teto altera arestas já gravadas; resolvido por D8 (`index_format` + reconstrução automática). Risco: apagar o diretório errado — só apagar `index_dir` recebido pelo `Engine`, nunca `.specs/graph` nem o restante de `.specs/`.

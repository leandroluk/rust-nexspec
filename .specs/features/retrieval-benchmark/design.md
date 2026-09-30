# Design: Retrieval Benchmark (Fase 8)

## Architecture Overview

O benchmark é um módulo novo (`src/bench/`) mais um subcomando (`nexspec bench`) que **só usa a API pública do `Engine`** — nada de atalhos no índice. Ele roda cada pergunta do corpus, reduz os hits a uma lista ordenada de *localizações* (arquivos), compara com o esperado e mede tokens contra três baselines sem índice.

```
bench/self.toml ─┐                      ┌──► recall@k, MRR por kind
(corpus TOML)    ├─► corpus::load ─► runner ─┼─► tokens: nexspec | grep | ler esperados | corpus inteiro
--repo <alvo> ───┘         │            │    └─► latência por consulta
                           │            ▼
                  Engine::open(index-dir temporário) + sync  ──► Engine::search / trace
                                        │
                                report.rs ─► JSON (máquina) + Markdown (humano)
                                        │
                        bench/baseline.json ◄─ gate de regressão (--check)
```

Decisões estruturais:
- O índice do benchmark vive num **diretório temporário** (`--index-dir`, default um `TempDir`): `bench` lê o repositório-alvo e nunca escreve nele. Isso também permite medir este próprio repositório sem criar `.specs/.index` nele.
- O mesmo `Engine::search` que o agente usa é o que se mede (inclusive expansão de 1 salto e orçamento de tokens); não existe um caminho "de benchmark".
- As melhorias de ranking (REQ-806) entram **depois** do harness e cada uma carrega o delta medido.

## Dependency Paths

Lidos no código (nexspec ainda não indexa este repo):

- `Engine::search` (`src/engine.rs`, `pub fn search`) → `search_text` (Tantivy, campo `text`) + HNSW (opcional, só com modelo) → `seed_discovery` (RRF k=60, `src/hybrid.rs`) → `expand(..., 1 salto)` → `SearchHit { id, payload, score }`.
- Um hit vira localização assim: `File{path}` → o próprio caminho; `Symbol` → arquivo via aresta `DefinedIn` (`Csr::edges_from`) + `File` no redb; `Requirement/Task/Adr` → o marcador (`REQ-…`) e, por `Satisfies` de entrada, os arquivos que os implementam. Hoje **não existe API** que faça isso: T-802.
- Texto indexado (`src/search/schema.rs::describe`): `File` = caminho; `Symbol` = só o nome; `Requirement/Task/Adr` = título+corpo. Tokenizador padrão do Tantivy: quebra em não-alfanuméricos (então `outbox.decorator.ts` → `outbox`, `decorator`, `ts`), **não** quebra camelCase (`OutboxDecorator` é um token) e o `QueryParser` combina termos com OR.
- Expansão de 1 salto usa `DependsOn, Satisfies, DefinedIn, Implements` — **`CoChanges` não entra na busca.** Logo a Q3 herdada da Fase 9 (tetos de co-change vs. ranking) é, hoje, irrelevante para `search`; o benchmark só pode avaliá-la se a expansão passar a usar co-change (T-808 decide com medição, sem mudar a semântica antes).
- Tokens: `Tokenizer` (`src/token/budget.rs`), `CharHeuristicTokenizer` (offline, determinístico, `chars/3.5`) e `TiktokenTokenizer`.
- Arquivos rastreados do alvo: `GitSource::tracked_paths_at_head` + `read_blob_at_head`.

## New Components

| Component                                    | Responsibility                                                                                                           | Location                                             |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------- |
| `Corpus`, `Query`, `Kind`                    | Parse/validação do TOML (REQ-801)                                                                                        | `src/bench/corpus.rs`                                |
| `locations_of_hit` / `Engine::hit_locations` | Hit → arquivos (+ marcadores) para comparar com `expect`                                                                 | `src/engine.rs` (método novo), `src/bench/locate.rs` |
| `metrics`                                    | `recall@k`, `MRR`, agregação por `kind` (funções puras)                                                                  | `src/bench/metrics.rs`                               |
| `baselines`                                  | Tokens de `grep -rn` do termo-chave, de ler os arquivos esperados e do corpus inteiro; grep feito em processo (portável) | `src/bench/baselines.rs`                             |
| `runner`                                     | Indexa (temp), executa, cronometra, monta `BenchReport`                                                                  | `src/bench/runner.rs`                                |
| `report`                                     | JSON + Markdown; economia por `kind`; custo fixo separado; formato comparável ao `graphify benchmark`                    | `src/bench/report.rs`                                |
| `nexspec bench`                              | `--corpus --repo --k 5,10 --index-dir --format json\|md --check --update-baseline`                                       | `src/bin/nexspec.rs`                                 |
| Corpora                                      | `bench/self.toml` (este repo, ~20 perguntas) e corpus do projeto real, mantido **fora** deste repo                       | `bench/`, repo-alvo                                  |
| `bench/baseline.json`                        | Último resultado aceito; base do portão de regressão                                                                     | `bench/baseline.json`                                |
| CI job                                       | `bench --check` sobre `bench/self.toml`                                                                                  | `.github/workflows/ci.yml`                           |

## Modified Components

| Component                          | Change                                                                                                                               | Risk                                                                               |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------- |
| `Engine`                           | `hit_locations(&SearchHit) -> Vec<Location>`; eventualmente `search_with(SearchOptions)` para o runner variar pesos sem API paralela | Engine é ponto de raio alto; só adições                                            |
| `Cargo.toml`                       | dependência `toml` (Q1: TOML)                                                                                                        | dependência leve, sem rede                                                         |
| `TantivySchema`/`describe` (T-808) | tokenização ciente de identificadores; campos `name`, `path` separados com boost                                                     | Muda o índice → sobe `INDEX_FORMAT` (mecanismo da Fase 9, reconstrução automática) |
| `seed_discovery` (T-808)           | pesos BM25×vetor                                                                                                                     | Só com o benchmark mostrando ganho                                                 |

## Formato do corpus (REQ-801)

```toml
[[query]]
id = "locate-outbox-decorator"
kind = "locate"                 # locate | structure | behavior | traceability
query = "outbox decorator repository dispatch"
grep = "outbox"                 # termo-chave do baseline grep (default: 1ª palavra da query)
expect = ["src/outbox/outbox.decorator.ts"]      # caminhos (sufixo vale) ou `Símbolo`, `REQ-123`
notes = "caso real que falhou em 2026-09-29"
```

- Um hit **acerta** se sua localização termina com algum `expect` de caminho, ou se nomeia um símbolo/marcador esperado.
- Ranking de localizações = arquivos únicos na ordem do primeiro aparecimento nos hits (o que o agente lê primeiro). `recall@k` = fração dos `expect` dentro dos k primeiros; `MRR` = 1/posição do primeiro acerto.
- `behavior`: o custo reportado soma a busca **mais** a leitura dos arquivos esperados (não se omite a leitura posterior).

## Decision Log

- **D1 — TOML** (Q1): já é o formato dos manifests Rust, legível e sem indentação significativa; YAML traria dependência maior e ambiguidades.
- **D2 — Índice temporário, repositório intocado.** Permite medir repos de terceiros e este próprio repositório sem `.specs/.index` neles. Custo: um sync por execução (segundos, medido na Fase 9).
- **D3 — Tokens com `CharHeuristicTokenizer` por padrão** (offline, determinístico, reprodutível em CI); `--tokenizer tiktoken` opcional. É um proxy: o relatório declara isso.
- **D4 — grep em processo, não o binário `grep`.** Mesmo resultado em Windows/Linux, sem depender do PATH; emite `arquivo:linha:texto` como `grep -rn` para contar os mesmos tokens.
- **D5 — Congelar o alvo (Q2): o corpus guarda `commit = "<sha>"`** e o runner avisa (não falha) quando o HEAD do alvo difere; o `baseline.json` também registra o commit. O corpus próprio é medido sobre o commit atual do repo (muda com o tempo, então o baseline é atualizado de propósito com `--update-baseline`).
- **D6 — Portão de CI só no corpus próprio** (REQ-805): reproduzível porque o alvo é o próprio checkout. Corpus externo roda localmente.
- **D7 — Ordem:** harness completo e baseline **antes** de qualquer mudança de ranking, para que T-808 prove cada ganho.
- **D9 — Convenção de corpus por projeto: `<repo>/.specs/bench/queries.toml`** (decidido em 2026-09-30). É o caminho default de `nexspec bench` quando `--corpus` é omitido. O corpus do condominium-management-system é gerado lá já na fase de desenvolvimento; em produção o mesmo caminho serve a qualquer projeto que queira medir a própria busca (a skill/agente pode propor e gerar o arquivo — Fases 16/18). O corpus deste repo fica em `bench/self.toml`.
- **D8 — Q3 (co-change):** hoje co-change não entra no ranking de `search`; em T-808 mede-se uma variante "expansão por co-change" e só então se decide usar e ajustar os tetos.

## Risks

- **Corpus enviesado:** perguntas escritas por quem conhece a resposta inflam o recall. Mitigação: incluir as falhas reais já vistas (outbox), perguntas de 4 `kind`s, e registrar a origem de cada uma em `notes`.
- **Auto-referência:** medir o repo do próprio nexspec com um índice construído por ele mesmo pode premiar ajustes específicos deste repo; por isso o corpus externo (condominium) é o juiz final.
- **`behavior` subestimado:** a busca localiza, não explica. O relatório separa localização de comportamento (REQ-807) e mostra o custo fixo por sessão (~10 k tokens do STATE.md medidos antes) à parte.
- **Ruído de latência** em CI: a latência é informativa, não entra no portão.

## Resultados da T-808 (2026-09-30)

Todas as medições: `nexspec bench --no-vector` (BM25) salvo onde dito; recall@5 / MRR por `kind`. Corpora: `bench/self.toml` (22 perguntas) e o corpus do condominium-management-system (24 perguntas, clone local @ `83fdcd3`). O repo próprio muda enquanto é editado, então a MRR de `locate` no corpus próprio oscila (0,71–0,91) entre execuções; os deltas abaixo são lidos sobre recall, que é o que o portão usa.

| Etapa                                       | self locate r5 | condo locate r5 | condo traceability r5 | Observação                                                                                                                    |
| ------------------------------------------- | -------------: | --------------: | --------------------: | ----------------------------------------------------------------------------------------------------------------------------- |
| Ponto de partida (BM25)                     |           0,87 |            0,94 |              **0,00** | `outbox` já estava em 1º lugar: a falha de 2026-09-29 foi corrigida pelos ajustes de ids da sessão anterior                   |
| (f) formatos reais de requisito em Markdown |           0,87 |            0,94 |              **1,00** | `### REQ-x:`, `- **REQ-x (Rótulo)**:`, bullets aninhados como corpo. Custo: respostas maiores (+65% de tokens no condominium) |
| (a) tokenizador ciente de identificadores   |       **0,93** |        **1,00** |                  1,00 | `CsrDelta` ⇄ "csr delta". Subiu `INDEX_FORMAT` para 3                                                                         |
| (g) sem nós `File` para binários/lockfiles  |           0,93 |            1,00 |                  1,00 | MRR de locate 0,81 → 0,91 (um PNG aparecia nos resultados). `INDEX_FORMAT` 4                                                  |
| (d) peso do vetor no RRF                    |           0,93 |            1,00 |                  1,00 | ver abaixo                                                                                                                    |

**(d) Peso BM25 × vetor** (com o modelo ONNX presente, recall@5 locate self/condo e MRR condo): peso 1,0 (o default antigo) → 0,93/0,94, MRR condo **0,64**; 0,5 → 0,93/1,00, 0,84; 0,25 → 0,93/1,00, 0,84; 0,1 → 0,93/1,00, **0,87**; sem vetor → 0,93/1,00, 0,87. O embedding (MiniLM) ordena mal texto cheio de identificadores; o vetor com peso igual *piorava* a ordem. Default novo: **0,1** (assistente de baixo peso: não passa um acerto lexical forte, mas ainda traz o que o BM25 não alcança). Ganho do vetor **não demonstrado** por este corpus; ele só aparece em paráfrases (`locate-wal`: "write ahead log" não contém "wal"), que o corpus quase não cobre. Reavaliar com mais perguntas semânticas.

**(e) Expansão por co-change** (resolve a Q3 herdada da Fase 9): incluir `CoChanges` na expansão de 1 salto não mudou recall nem MRR em nenhum `kind`, e elevou os tokens da resposta (condominium 23,7 mil → 43,5 mil). **Decisão: não usar co-change na busca.** Consequência: `COCHANGE_MAX_FILES`/`COCHANGE_MAX_PAIRS` não afetam o ranking de `search`; só alimentam `blame` (co-changed files). Os tetos ficam como estão (proteção de escala).

**(b) campos `name`/`path` separados com boost** e **(c) penalização de arquivos genéricos**: **não implementados.** Depois de (a) e (g) o único `locate` fora do top-5 é `locate-wal` (lacuna semântica, não lexical) e o `login-usecase` do condominium voltou ao 1º–2º lugar; nenhuma falha restante se explica por caminho ou por arquivos genéricos. Reabrir se um corpus futuro mostrar o contrário.

**O que ainda não funciona (e por quê):** `structure` = 0,00 nos dois corpora. "Quem usa X" precisa de arestas de import/chamada entre arquivos, que só existem na Fase 7. O benchmark já mede isso: essas perguntas estão no corpus como MISS esperados.

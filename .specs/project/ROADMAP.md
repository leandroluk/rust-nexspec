# NexSpec — Roadmap

Fonte primária de escopo técnico: `.defs/NexSpec.md` (documento gerado com Gemini).
Este roadmap traduz as fases dele em milestones rastreáveis pela skill
`graph-spec-design`. Cada fase vira uma feature em `.specs/features/` quando
começar a ser trabalhada.

## Fases

| # | Fase | Entrega central | Status |
|---|------|------------------|--------|
| 0 | Sync Coordinator & Transactional Integrity | WAL (`sync.wal`), `sync_version` atômico em `redb`, staging+rename por store, `nexspec sync --resume`, apply idempotente | **Completo** (`.specs/features/sync-coordinator/`) |
| 1 | Storage Primitives & Graph Topology | `Node`/`Edge` tipados, ID estável (Blake3) vs índice físico denso (`u32`), redb+zstd, CSR **duas camadas** (base rkyv/mmap + delta lock-free via `ArcSwap`/`crossbeam-epoch`, compactação por threshold), extração Markdown (comrak) | **Completo** (`.specs/features/storage-primitives/`) |
| 2 | Git Integration & Incremental Sync | gix embutido, tree-diff incremental, grafo de co-mudança com janela limitada (default 500 commits/6 meses), linking commit→spec | **Completo** (`.specs/features/git-integration/`) |
| 3 | Multi-Language AST & Lexical Search | Tree-sitter (TS/JS, Python, Go, Rust) com parsing paralelo via `rayon` no cold-start, índice Tantivy (BM25) | **Completo** (`.specs/features/ast-lexical-search/`) |
| 4 | Vector Engine & Hybrid Traversal | ONNX (ort) **lazy-loaded** + modelo INT8 quantizado, flag de compilação "lean" (sem ort/HNSW), HNSW, RRF, expansão k-hop sobre base+delta | **Completo** (`.specs/features/vector-engine/`) |
| 5 | Token Budgeting & LLM Serialization | Pruning AST, `Tokenizer` trait plugável (default tiktoken-rs) com margem de segurança (90% do budget), fallback offline (`char_count / 3.5`), serializer Markdown denso | **Completo** (`.specs/features/token-budgeting/`) |
| 6 | Interface, MCP Server & Tooling | CLI (`clap`: init/sync/compact/search/trace/blame/diff), servidor MCP (`rmcp`) com tool surface unificado | **Completo** (`.specs/features/cli-mcp-server/`) — integração drop-in na skill `graph-spec-design` **fora do escopo deste repositório** por instrução do usuário (ver nota abaixo) |
| 7 | Dependency Edges | Import/referência entre arquivos (TS/JS primeiro): `DependsOn` arquivo→arquivo e símbolo→símbolo, resolução de `tsconfig paths`/`#/*`/workspace, herança e DI (tipos de construtor), incremental sem arestas órfãs | **Planejado** (`.specs/features/dependency-edges/`) |
| 8 | Retrieval Benchmark | Corpus de perguntas + `nexspec bench`: recall@k/MRR, tokens vs. baselines (grep, leitura de arquivos), portão de qualidade e ranking ciente de identificadores | **Planejado** (`.specs/features/retrieval-benchmark/`) |
| 9 | Performance & Scale Guard | Repositório sintético determinístico, orçamentos de tempo em CI (Windows+Linux), teto de co-change, micro-benchmarks, regressões nomeadas dos bugs reais, espera de lock do redb, higiene do WAL | **Planejado** (`.specs/features/performance-guard/`) |
| 10 | Report Command | `nexspec report`: God nodes, comunidades + coesão, cobertura de requisitos (REQ órfãos/sem implementação), saída com orçamento de tokens e tool MCP | **Planejado** (`.specs/features/report-command/`) — depende da Fase 7 |
| 11 | Graph Query Surface | `query` (BFS/DFS com orçamento e filtro de contexto), `path A B`, `explain X`, `affected X` (reverso por relação/profundidade), confiança `EXTRACTED`/`INFERRED` nas saídas, tools MCP equivalentes | **Planejado** (`.specs/features/graph-query-surface/`) — depende da Fase 7 |
| 12 | Graph Export | `export` JSON portátil versionado, HTML autocontido, árvore colapsável e wiki por comunidade (GraphML/SVG/Obsidian/Neo4j adiados) | **Planejado** (`.specs/features/graph-export/`) |
| 13 | Multi-Repo Graph | grafo global, `merge-graphs`, ligação entre repositórios (pacotes, rotas), consultas `--global`, merge driver do Git | **Planejado** (`.specs/features/multi-repo-graph/`) |
| 14 | Domain Extractors | mecanismo de *language pack*; DDL/Liquibase (tabelas, views, FKs, ponte tabela↔entidade), manifestos (`package.json`/`tsconfig`/`Cargo.toml`), introspecção Postgres opt-in | **Planejado** (`.specs/features/domain-extractors/`) |
| 15 | Work Memory | `save-result` + `reflect` determinísticos (lições, becos sem saída) e boost leve no ranking | **Planejado** (`.specs/features/work-memory/`) |
| 16 | Workflow Integration | `watch`, `hook install`, `check-update`, `install --platform` (MCP), `doctor` | **Planejado** (`.specs/features/workflow-integration/`) |
| 17 | LLM Enrichment (opt-in) | provedor plugável, rótulos de comunidade e extração semântica de docs — **atrás de portão de decisão** | **Condicional** (`.specs/features/llm-enrichment/`) |
| 18 | Semantic Annotations | `annotate`/`annotate_node` (anotações do agente com proveniência e confiança `INFERRED`, arquivo como fonte da verdade), rótulos de comunidade, `SimilarTo` por embeddings locais; alternativa à Fase 17 sem provedor de LLM | **Planejado** (`.specs/features/semantic-annotations/`) — depende das Fases 11 e 15 |

### Nota — por que a Fase 0 existe e vem antes de tudo

`redb`, o arquivo CSR, o Tantivy e o HNSW cada um faz commit próprio. Sem um
coordenador central, um crash no meio de um `sync` pode deixar esses 4 stores
divergentes entre si (ex.: edge gravado no CSR mas não no Tantivy). A Fase 0
resolve isso na base — WAL + version pointer atômico — para que as fases
seguintes (1–6) já construam sobre uma primitiva de consistência sólida, em vez
de remendar isso depois.

## Critério de "pronto" do projeto (v1)

- `nexspec init/sync/search/trace/blame/diff` funcionando via CLI estática
- Contrato de saída compatível com o que a skill `graph-spec-design` espera hoje do
  `graphify` (`.specs/graph/graph.json`, `GRAPH_REPORT.md`, staleness check)
- `graph-spec-design` consegue rodar 100% sobre `nexspec` sem `graphify` Python
  instalado (ver seção "Integração com a skill" abaixo)

## Integração com a skill graph-spec-design

**Atualização (2026-09-29): fora do escopo deste repositório**, por
instrução explícita do usuário — será feita em outro repositório/sessão,
consumindo o binário `nexspec` já pronto (Fase 6 completa) a partir daqui.

Hoje a skill chama `graphify` (Python) via `uv`/`pip`. Meta: expor `nexspec` com CLI
e contrato de I/O suficientemente compatíveis para que o Rule #1 da skill
(`references/init.md`, `references/session.md`) passe a detectar e preferir
`nexspec` no lugar de `graphify`, sem exigir mudança na skill além de trocar o
binário/comando invocado. Essa troca é uma feature própria
(`nexspec-skill-integration`), especificada e implementada fora deste
repositório.

## Ordem de trabalho sugerida

Fase 0→1→2→3 é uma cadeia rígida de pré-requisitos (consistência → storage → git →
AST); nenhuma delas faz sentido isolada das anteriores. Fases 4 e 5 podem ser
paralelizadas depois que 3 estiver de pé. Fase 6 fecha o pacote e é o ponto de
integração real com este projeto (`graph-spec-design` deixa de depender de Python).

## Evoluções pós-v1 (registradas em 2026-09-29)

Origem: avaliação "promissor → confiável" feita após dogfooding no projeto `condominium-management-system`
(oito bugs corrigidos numa sessão: O(n²) no `CsrDelta`, co-change recalculado a cada sync, `DirtyCache`,
arquivos não rastreados, `abstract class`, ids com prefixo, ligação REQ↔TASK entre arquivos e `trace` unidirecional).

| Evolução | Fase | Por quê |
|---|---|---|
| Arestas de import/referência entre arquivos | 7 | `trace CachePort` não lista os 7 usuários; sem isso não há análise de impacto |
| Benchmark de precisão e de tokens | 8 | a economia de tokens foi só estimada; o ranking errou em consulta real |
| CI com orçamento de tempo e regressões | 9 | os bugs só apareceram em repositório real; testes unitários passavam |
| `nexspec report` (God nodes, comunidades, cobertura) | 10 | a skill promete God nodes/coesão que o `nexspec` não produz |

**Ordem sugerida:** 9 e 8 primeiro (rede de proteção e régua), depois 7 (medindo o ganho no benchmark), por fim 10.
Os critérios para substituir o graphify de vez: Fase 8 com `recall@5 ≥ 0,8` em `locate` **e** Fase 7 entregue.

### Evoluções externas (repositório `graph-spec-design`, fora deste escopo)

- **Reduzir o custo fixo por sessão:** o `STATE.md` pesa ~7,6 mil tokens por sessão (limite atual 30 KB) e o índice
  não o reduz; baixar o limite para ~10–12 KB e endurecer as janelas de compactação.
- **Alinhar promessas ao que existe:** remover "God Node (degree N)"/coesão dos templates de `specify.md` e `design.md`
  até a Fase 10 existir; depois trocar por `nexspec report`.
- **Adotar ids com prefixo de feature** (`REQ-CTR-001`) — já suportado pelo `nexspec` desde 2026-09-29.

## Plano "aposentar o graphify" (registrado em 2026-09-29)

Guarda-chuva: `.specs/features/graphify-parity/spec.md` (matriz de paridade capacidade a capacidade, levantada lendo o código
do `graphifyy 0.9.61`, critério objetivo de saída e teste das 10 perguntas). Resumo das prioridades:

- **P0 (bloqueiam a troca):** Fases 7, 8, 9, 10 e 11.
- **P1 (fluxo diário):** Fase 16 (watch/hooks/MCP), Fase 14 (SQL/Liquibase e manifestos), Fase 12 (JSON + HTML + wiki).
- **P2:** Fase 13 (multi-repo), Fase 15 (memória de trabalho), Fase 18 (anotações do agente + similaridade por embeddings — a camada semântica **sem** LLM próprio).
- **P3 / condicional:** Fase 17 (LLM próprio; a Fase 18 é o caminho preferido) — só se a Fase 8 e o teste das 10 perguntas provarem que a falta de semântica dói.
- **Fora de escopo:** áudio/vídeo, PDFs/Office, Google Workspace, `add <url>`/`clone`, painel de PRs.

Critério para o graphify sair da skill: linhas P0 entregues + `recall@5 ≥ 0,8` (Fase 8) + teste das 10 perguntas aprovado +
2 semanas de uso diário sem recorrer a ele. Até lá, permanece congelado como fallback.

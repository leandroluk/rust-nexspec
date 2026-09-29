# Spec: Interface, MCP Server & Tooling (Fase 6)

## Summary

Fase 6 é a fase de composição: até aqui, cada fase anterior entregou uma
biblioteca (Sync Coordinator, storage primitives, Git, AST/lexical,
vetores, token budgeting) sem nenhum ponto de entrada executável. Esta fase
adiciona um binário `nexspec` — CLI (`clap`) com os comandos
`init/sync/compact/search/trace/blame/diff`, e um servidor MCP embutido
(`rmcp`, stdio) expondo os mesmos recursos como ferramentas para agentes.
Tudo isso é sustentado por um novo componente de composição, `Engine`, que
abre/cria a estrutura `.specs/.index/` (redb, WAL, CSR, Tantivy, e HNSW sob
`full`) e reconstrói os `SyncParticipant`s necessários a cada operação —
nenhuma fase anterior precisou decidir *onde* esses arquivos moram no disco
nem *quando* reabri-los entre execuções do processo; essa é a decisão
central desta fase.

**Escopo explicitamente reduzido por instrução do usuário**: a integração
com a skill `graph-spec-design` (trocar as chamadas a `graphify` Python
pelo binário `nexspec`) é responsabilidade de **outro repositório/sessão**,
não deste. Esta fase entrega o binário e o protocolo MCP como uma
ferramenta de uso geral; não produz o contrato de saída específico que a
skill hoje espera do `graphify` (`.specs/graph/graph.json`,
`GRAPH_REPORT.md`) — isso fica para quando aquele outro repositório vier
consumir este.

## Requirements

- REQ-601: **`Engine`** (novo componente de composição, não uma feature de
  usuário em si) — abre ou cria `.specs/.index/` (`metadata.redb`,
  `sync.wal`, `edges.bin`, `tantivy/`, e `vectors.bin` sob a feature `full`)
  para um repositório dado; mantém um `Arc<Csr>` de longa duração (visível a
  cada operação de leitura sem reabrir o CSR) e reconstrói os demais
  `SyncParticipant`s (Redb, Tantivy, Hnsw) sob demanda a cada chamada que
  precisa escrever, já que suas APIs (Fases 0-4) não foram desenhadas para
  serem mantidas abertas indefinidamente entre ciclos de sync de um
  processo CLI de vida curta.
- REQ-602: `nexspec init <path>` — cria a estrutura `.specs/.index/` acima
  se ainda não existir; idempotente (rodar de novo sobre uma estrutura já
  existente não apaga nem corrompe nada).
- REQ-603: `nexspec sync [--resume]` — roda um ciclo de sync incremental via
  `SyncOrchestrator` (Fase 2); `--resume` chama `Coordinator::resume()`
  (Fase 0) antes, para recuperação determinística pós-crash.
- REQ-604: `nexspec compact` — força a compactação da camada delta do CSR
  (Fase 1), fora do gatilho automático por threshold.
- REQ-605: `nexspec search "<query>" [--max-tokens N]` — busca híbrida:
  BM25 (Tantivy, Fase 3) sempre; HNSW (Fase 4) quando a busca semântica
  estiver disponível (`full` **e** modelo de embedding presente em disco —
  ausência de qualquer um dos dois degrada para BM25-only, nunca erro).
  Funde os dois via RRF (`hybrid::seed_discovery`), expande 1 hop
  (`hybrid::expand`), e — se `--max-tokens` for passado — poda/serializa o
  resultado através do pipeline da Fase 5 (`token::pruner`/`Budget`/
  `serialize`); sem a flag, retorna a lista de nós rankeados crua.
- REQ-606: `nexspec trace <ID>` — travessia topológica determinística a
  partir de um id/marcador (`REQ-XXX`/`ADR-XXX`/nome de símbolo), ao longo
  das arestas do CSR (`Satisfies`, `DependsOn`, `DefinedIn`, `Implements`),
  retornando a árvore de dependência com profundidade e tipo de aresta por
  nó.
- REQ-607: `nexspec blame <SYMBOL> [--full-history]` — **AST-aware blame**
  (a peça que a Fase 2 deliberadamente adiou — ver
  `.specs/features/git-integration/spec.md` → Out of Scope): localiza o
  símbolo pelo nome, resolve seu `line_start..line_end` (Fase 3), e roda
  `gix`'s blame nativo (`gix_blame::file`, já uma dependência transitiva de
  `gix`) escopado a esse intervalo de linhas — devolve, por hunk, o commit
  que introduziu aquelas linhas, autor e timestamp. `--full-history` troca
  a janela padrão do grafo de co-mudança (REQ-206, 500 commits/6 meses)
  por um walk sem limite ao listar arquivos que co-mudam com o arquivo do
  símbolo — não afeta o próprio algoritmo de blame, que sempre precisa
  andar o histórico completo para atribuir linhas corretamente.
- REQ-608: `nexspec diff --staged` — análise estrutural: symbols tocados
  pela árvore suja/staged (via `GitSource::is_dirty`/`tracked_paths_at_head`,
  Fase 2, reaproveitados sem sync completo) mais seus dependentes diretos
  (1 hop via `DependsOn`/`DependedOnBy` no CSR) — sem tocar Tantivy/HNSW,
  já que é puramente estrutural (lean por padrão, REQ-401 da Fase 4
  continua valendo: nada de embeddings carregados para este comando).
- REQ-609: **Servidor MCP embutido** (`nexspec mcp`, stdio, `rmcp`) —
  ferramentas: `query_context`, `trace_requirement`, `find_impacted_code`,
  `semantic_search`, `get_symbol_history`, `sync_workspace`. Cada uma é um
  wrapper fino sobre o método correspondente de `Engine` (mesma lógica que
  os comandos CLI acima — nenhuma regra de negócio duplicada entre CLI e
  MCP).

## Affected Components (from graph)

Sem grafo do próprio NexSpec ainda. Componentes existentes consumidos por
`Engine` (nenhuma mudança de assinatura nos existentes, exceto onde
anotado):

- `sync::{Coordinator, VersionPointer, Wal, RedbParticipant}` (Fase 0)
- `graph::csr::{Csr, CsrBase, CsrParticipant}` (Fase 1) — `CsrParticipant`
  ganha um método público `compact_now()` (thin wrapper sobre seu
  `compact()` privado existente) para REQ-604.
- `git::GitSource`, `sync_orchestrator::SyncOrchestrator` (Fase 2)
- `code::Language`, `search::{TantivyParticipant, find_by_id, search_text}`
  (Fase 3)
- `vector::{Embedder, HnswParticipant}`, `hybrid::{seed_discovery, expand}`
  (Fase 4, só sob `full`)
- `token::{Budget, Tier, TieredItem, prune_symbol, serialize}` (Fase 5)

## Out of Scope

- **Integração com a skill `graph-spec-design`** — responsabilidade de
  outro repositório/sessão, conforme instrução explícita do usuário. Esta
  fase não produz `.specs/graph/graph.json`/`GRAPH_REPORT.md` nem qualquer
  outro contrato de compatibilidade com o `graphify` Python.
- Transporte HTTP/SSE do MCP (`transport-streamable-http-*`) — só stdio.
- Autenticação/OAuth do `rmcp` (`auth`, `auth-enterprise-managed`) — sem
  sentido para um servidor MCP local via stdio.
- Submodules/`git-lfs`/sparse-checkout no `blame` — mesma exclusão já
  registrada na Fase 2, não revisitada aqui.
- Watch de sistema de arquivos (`.git/` via `notify`/inotify/FSEvents) para
  disparar `sync` automaticamente — ideia já registrada em "Deferred Ideas"
  do STATE.md; esta fase só entrega `nexspec sync` como comando explícito,
  chamado manualmente ou por CI, não um daemon.
- Re-embedding automático de todo o repositório em `nexspec init` (ideia
  registrada em "Deferred Ideas" do STATE.md sobre baixar modelo por
  padrão) — `init` só cria a estrutura de diretórios; baixar o modelo de
  embedding continua sendo responsabilidade do usuário/deploy, como desde a
  Fase 4.

## Open Questions

Nenhuma que bloqueie o início. Um ponto técnico confirmado durante a
pesquisa de design (não uma decisão do usuário): `rmcp` 3.5.0 (SDK oficial
do Model Context Protocol em Rust) e sua API de macros (`#[tool_router]`/
`#[tool]`/`#[tool_handler]`) foram inspecionados diretamente no código-fonte
baixado (`~/.cargo/registry/src/.../rmcp-3.5.0`), não de memória — evita
repetir o risco já visto com APIs de crates que mudam rápido (ex.: Tantivy
`TopDocs` na Fase 3).

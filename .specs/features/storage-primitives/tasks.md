# Tasks: Storage Primitives & Graph Topology (Fase 1)

## T-101: Dependências novas (`memmap2`, `arc-swap`, `zstd`, `comrak`) [x]
- **REQ**: (infra — pré-requisito de todos os REQs desta fase)
- **What**: `cargo add memmap2 arc-swap zstd comrak`.
- **Where**: `Cargo.toml`
- **Depends on**: none
- **Done when**: `cargo build` compila com as novas deps resolvidas.
- **Gate**: `cargo build`

## T-102: Tipos `Node`/`NodeType`/`NodePayload` [x]
- **REQ**: REQ-101, REQ-102
- **What**: Enum `NodeType` (Requirement, Task, Adr, DocSection, Symbol, File);
  enum `NodePayload` com uma variante por tipo (campos mínimos: `title`,
  `source_hash`, mais campos específicos); struct `Node { id: StableId,
  node_type: NodeType, payload: NodePayload }`. Derivar `rkyv::Archive`/
  `Serialize`/`Deserialize` no payload (consistência com WAL/CSR). Reaproveitar
  `sync::mutation::StableId` (não redefinir).
- **Where**: `src/graph/node.rs`
- **Depends on**: T-101
- **[P]**: A (paralelizável com T-103)
- **Done when**: roundtrip de serialização de `NodePayload` preserva dados
  para cada variante.
- **Gate**: `cargo test graph::node`

## T-103: Tipos `Edge`/`EdgeType` [x]
- **REQ**: REQ-101, REQ-102
- **What**: Enum `EdgeType` (Satisfies, DependsOn, Implements, DefinedIn);
  struct `Edge { id: StableId, from: StableId, to: StableId, edge_type:
  EdgeType }`. Deriva `rkyv::Archive`/`Serialize`/`Deserialize`.
- **Where**: `src/graph/edge.rs`
- **Depends on**: T-101
- **[P]**: A (paralelizável com T-102)
- **Done when**: roundtrip de serialização preserva dados.
- **Gate**: `cargo test graph::edge`

## T-104: `CsrBase` — layout binário imutável (rkyv + mmap) [x]
- **REQ**: REQ-105
- **What**: Formato do arquivo `edges.bin`: índice denso `u32 -> Vec<Edge
  offset>` por direção (out-edges) e por `EdgeType`, mapa `StableId -> u32`
  (índice físico denso, REQ-103) construído no load. `CsrBase::build(edges:
  &[Edge]) -> CsrBase` (escreve arquivo), `CsrBase::open(path) -> CsrBase`
  (mmap via `memmap2`), `CsrBase::edges_from(id: &StableId, edge_type:
  EdgeType) -> &[Edge]` (O(1) após lookup do índice denso).
- **Where**: `src/graph/csr/base.rs`
- **Depends on**: T-103
- **Done when**: `build` seguido de `open` no mesmo arquivo retorna as mesmas
  edges consultadas por `edges_from`; teste cobre múltiplos `EdgeType` e nó
  sem edges (retorna slice vazio, não erro).
- **Gate**: `cargo test graph::csr::base`

## T-105: `CsrDelta` — estrutura append-only em memória [x]
- **REQ**: REQ-105
- **What**: `CsrDelta { added: Vec<Edge>, removed: Vec<StableId> }` (edge ID
  removido). `CsrDelta::edges_from(id, edge_type)` combinando `added`
  filtrado e ignorando `removed`. `CsrDelta::merge_into(&self, base:
  &CsrBase, id, edge_type) -> Vec<Edge>` — junta resultado de `base` com o
  delta (removendo os que estão em `removed`, incluindo os de `added`).
- **Where**: `src/graph/csr/delta.rs`
- **Depends on**: T-104
- **Done when**: teste cobre (a) delta vazio = comportamento igual à base
  pura, (b) edge adicionada só no delta aparece no merge, (c) edge removida
  no delta desaparece do merge mesmo estando na base.
- **Gate**: `cargo test graph::csr::delta`

## T-106: `Csr` — fachada lock-free (`ArcSwap<CsrDelta>`) [x]
- **REQ**: REQ-106
- **What**: `Csr { base: CsrBase, delta: ArcSwap<CsrDelta> }`. `edges_from()`
  público faz `delta.load()` (snapshot atômico) + `merge_into(base, ...)`.
  `publish_delta(&self, new_delta: CsrDelta)` troca o ponteiro via
  `ArcSwap::store` — não bloqueia leitores em andamento (eles seguem com o
  `Guard` que já carregaram).
- **Where**: `src/graph/csr/mod.rs`
- **Depends on**: T-105
- **Done when**: teste com uma thread lendo em loop e outra publicando deltas
  concorrentemente não deadlocka nem panica (usar `std::thread::scope`);
  leitor nunca observa um delta parcialmente construído (invariante
  verificada por construção do tipo, não por teste de timing frágil).
- **Gate**: `cargo test graph::csr::mod`

## T-107: `CsrParticipant` — implementação de `SyncParticipant` [x]
- **REQ**: REQ-106, REQ-107, REQ-109
- **What**: `CsrParticipant` guarda `Csr` (T-106) + um `RwLock<Option<(u64,
  CsrDelta)>>` de staging. `stage()` constrói o próximo `CsrDelta` (mescla
  `EdgeMutation`s recebidas ao delta atual) sem publicar ainda;
  `committed_version()` — versão própria rastreada internamente (padrão de
  `RedbParticipant`, mas sem tabela `redb` própria — pode reaproveitar uma via
  composição, não reinventar persistência); `commit()` publica o delta via
  `Csr::publish_delta` e, se o tamanho do delta publicado exceder o threshold
  (REQ-107, default 5% do total de edges da base), dispara `compact()`
  (reconstrói `CsrBase` a partir de base+delta, escreve em
  `edges.bin.staging`, rename atômico, remapeia mmap, zera o delta);
  `abort()` descarta o staging sem publicar.
- **Where**: `src/graph/csr/participant.rs`
- **Depends on**: T-106
- **Done when**: teste replica os 3 cenários de `RedbParticipant` (T-006 da
  Fase 0): stage→commit torna dado visível, stage→abort descarta, stage→stage→
  commit não duplica; mais um teste específico: delta cresce até o threshold
  → `commit()` seguinte aciona compactação automaticamente (delta volta a
  vazio, base cresce).
- **Gate**: `cargo test graph::csr::participant`

## T-108: `markdown::extract` — parser de `.specs/*.md` via comrak [x]
- **REQ**: REQ-108
- **What**: `extract(path: &Path) -> MutationSet` — parseia um arquivo
  Markdown com `comrak`, reconhece padrões `REQ-\d+`, `TASK-\d+`, `ADR-\d+` em
  cabeçalhos/texto, gera um `NodeMutation::Upsert` por entidade encontrada
  (payload = `NodePayload::Requirement`/`Task`/`Adr`/`DocSection`, hash Blake3
  do texto canônico como `StableId`) e `EdgeMutation` quando uma seção
  referencia outra (ex.: uma spec que cita `REQ-001` gera edge `Satisfies`
  se for uma task, ou uma relação genérica de referência caso o tipo não seja
  óbvio — decisão mínima nesta fase, refinável depois).
- **Where**: `src/graph/markdown.rs`
- **Depends on**: T-102, T-103
- **Done when**: rodando `extract()` sobre um fixture de teste com um REQ, uma
  TASK que referencia esse REQ, e um ADR solto, retorna `MutationSet` com 3
  nós e ao menos 1 edge conectando a task ao requirement.
- **Gate**: `cargo test graph::markdown`

## T-109: Integração fim-a-fim via `Coordinator` [x]
- **REQ**: REQ-109
- **What**: Teste de integração (`tests/graph_storage_integration.rs`):
  `extract()` sobre um fixture real de `.specs/`, resultado passado a
  `Coordinator::stage()` com `[RedbParticipant, CsrParticipant]` como
  participantes, depois consulta via `Csr::edges_from` e `RedbParticipant::
  get_node` confirmando que specs viraram nós/edges consultáveis.
- **Where**: `tests/graph_storage_integration.rs`
- **Depends on**: T-107, T-108
- **Done when**: teste passa fim-a-fim sem mocks, usando o `Coordinator` real
  da Fase 0.
- **Gate**: `cargo test --test graph_storage_integration`

## T-110: Lint e superfície pública
- **REQ**: (todos — fechamento da fase, mesmo padrão de T-010 na Fase 0)
- **What**: Exportar `graph::{Node, NodeType, Edge, EdgeType, Csr,
  CsrParticipant}` de `src/lib.rs`. Doc comments em toda API pública.
- **Where**: `src/lib.rs`, `src/graph/mod.rs`
- **Depends on**: T-109
- **Done when**: `cargo doc --no-deps` sem warnings, `cargo clippy
  --all-targets -- -D warnings` sem erros.
- **Gate**: `cargo doc --no-deps && cargo clippy --all-targets -- -D warnings`

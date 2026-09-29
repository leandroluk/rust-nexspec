# Spec: Local Vector Engine & Hybrid Traversal (Fase 4)

## Summary

Fase 4 adiciona busca semântica (embeddings + HNSW) ao NexSpec, e a combina
com a busca léxica já existente (Tantivy, Fase 3) via Reciprocal Rank Fusion
(RRF) — a "descoberta de seeds" da estratégia de recuperação híbrida. A
partir dos nós-semente encontrados, uma expansão limitada por k-hops sobre o
CSR (Fase 1) traz o contexto topológico ao redor. Ao contrário das fases
anteriores, esta introduz uma **dependência de runtime pesada e opcional**
(ONNX Runtime + um modelo de embedding real, ~30MB) — o design central da
fase é justamente isolar essa dependência (lazy-load, flag "lean") para que
o resto do NexSpec nunca pague o custo dela sem precisar.

## Requirements

- REQ-401: Embutir `ort` (bindings ONNX Runtime) para inferência CPU,
  **lazy-initialized** — nada de `ort`/modelo carregado no startup do
  processo; só na primeira chamada que realmente precisa de embeddings
  (`semantic_search`/inserção de vetor no HNSW).
- REQ-402: Referenciar um modelo de embedding quantizado INT8
  (`bge-small-en-v1.5` ou `all-MiniLM-L6-v2`, ~30MB) mais seu tokenizer. O
  binário/crate deve **compilar e os testes deste módulo devem rodar sem o
  arquivo do modelo presente** — ausência do modelo é um erro de runtime
  limpo (`ModelNotAvailable`), nunca falha de build. Baixar o arquivo real
  do modelo é uma ação de download explícito (ver Open Questions — requer
  confirmação do usuário antes de qualquer task que precise dele).
- REQ-403: Índice vetorial HNSW (`instant-distance`, já na matriz de crates
  do `.defs/NexSpec.md`), persistido em `.specs/.index/vectors.bin`,
  participando do protocolo de staging da Fase 0 como um quarto
  `SyncParticipant` (`HnswParticipant`).
- REQ-404: Flag de compilação "lean" (feature Cargo) exclui `ort`/HNSW do
  binário inteiramente — builds só-CI (`diff --staged`, `trace`, léxico)
  nunca pagam esse peso, nem em tamanho de binário nem em tempo de startup.
- REQ-405: `hybrid::seed_discovery(query: &str) -> Vec<(StableId, f32)>` —
  consulta Tantivy (BM25, Fase 3) e HNSW (cosine) simultaneamente, combina
  os rankings via Reciprocal Rank Fusion.
- REQ-406: `hybrid::expand(seeds: &[StableId], csr: &Csr, edge_types: &[EdgeType], max_depth: u8) -> Vec<StableId>`
  — expansão limitada por profundidade (default k=1–2) a partir dos nós-
  semente, ao longo de arestas do CSR (Fase 1, base+delta já mesclados),
  filtrada por tipo de relação.

## Affected Components (from graph)

Sem grafo do próprio NexSpec ainda. Componentes existentes consumidos:

- `sync::coordinator::Coordinator` — `HnswParticipant` é o **quarto**
  `SyncParticipant` real (depois de Redb, Csr, Tantivy).
- `search::TantivyParticipant`/`search::query` (Fase 3) — a metade léxica
  do RRF em REQ-405.
- `graph::csr::Csr::edges_from` (Fase 1) — motor de travessia de REQ-406, já
  mescla base+delta, reaproveitado sem mudança.

## Out of Scope

- Treinar ou fine-tunar modelos de embedding próprios — só consumo de um
  modelo pré-treinado.
- Embeddings multi-modais ou de código especializados (ex.: modelos
  treinados em código-fonte) — o modelo é genérico de texto; símbolos de
  código são embutidos pelo texto do nome/assinatura, não por uma
  representação estrutural da AST.
- CLI (`nexspec search --hybrid`) — Fase 6.
- Re-embedding incremental inteligente (só re-embeddar o que mudou já é
  coberto pelo modelo de sync da Fase 0/2; nada especial aqui).

## Open Questions

- **[BLOQUEIA execução, não a spec] Baixar o arquivo do modelo real
  (~30MB)**: é uma ação de download explícito — preciso da confirmação do
  usuário antes de rodar qualquer task que dependa do arquivo do modelo
  chegando ao disco (nome, origem exata — ex. Hugging Face — e tamanho serão
  apresentados no momento do pedido). Até lá, a fase é implementada e
  testada com vetores sintéticos (não gerados por um modelo real) para tudo
  que não seja a própria chamada de inferência ONNX.
- **Tokenizer do modelo**: decisão adiada para o momento da task de
  inferência real — `bge-small-en-v1.5`/`all-MiniLM-L6-v2` usam tokenizers
  WordPiece; se a crate `tokenizers` (Hugging Face) for pesada demais para o
  orçamento desta fase, cai para um tokenizer mínimo compatível. Não afeta
  REQ-403/404/405/406, que operam sobre vetores já prontos (reais ou
  sintéticos), não sobre texto cru.
- **`instant-distance` vs alternativas de HNSW**: mantém a escolha já feita
  em `.specs/codebase/STACK.md` (não re-decidida aqui).

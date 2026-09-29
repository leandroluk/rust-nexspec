# Tasks: Local Vector Engine & Hybrid Traversal (Fase 4)

Nota geral: T-401..T-405, T-407..T-409 não dependem do arquivo real do
modelo de embedding — usam vetores sintéticos (`Vec<f32>` arbitrário) para
testar armazenamento/fusão/travessia. **T-406 é a única task que baixa o
modelo real e precisa de confirmação explícita do usuário antes de rodar**
(REQ-402/Open Questions do spec.md) — as demais podem (e devem) ser
executadas antes, sem bloqueio.

## T-401: Feature `lean` + dependências opcionais (`ort`, `instant-distance`) [x]
- **REQ**: REQ-401, REQ-404
- **What**: `cargo add ort instant-distance --optional`; feature `lean` em
  `Cargo.toml` que desliga essas deps (via `default = [...]` sem elas +
  feature reversa, ou `not(feature = "lean")` nas deps — decidir a forma
  exata do Cargo feature flag na implementação). `cargo build` (default) e
  `cargo build --no-default-features --features lean` (ou equivalente)
  ambos compilam.
- **Where**: `Cargo.toml`
- **Depends on**: none
- **Done when**: os dois builds acima compilam.
- **Gate**: `cargo build && cargo build --no-default-features --features lean`

## T-402: `HnswIndex` + `HnswParticipant` (vetores sintéticos) [x]
- **REQ**: REQ-403
- **What**: `HnswIndex` (wrap `instant_distance::Hnsw`), `search(vec: &[f32],
  k: usize) -> Vec<(StableId, f32)>` (cosine). `HnswParticipant` — mesmo
  formato de `CsrParticipant` (base arquivo + delta em memória,
  `SyncParticipant`). Testes com vetores sintéticos replicam os 3 cenários
  padrão (stage→commit visível, stage→abort descarta, stage→stage→commit
  não duplica).
- **Where**: `src/vector/hnsw.rs`, `src/vector/mod.rs`
- **Depends on**: T-401
- **[P]**: A (paralelizável com T-403)
- **Done when**: os 3 cenários passam; `search()` encontra o vizinho mais
  próximo correto entre vetores sintéticos conhecidos.
- **Gate**: `cargo test vector::hnsw`

## T-403: `hybrid::expand` (k-hop limitado sobre o CSR) [x]
- **REQ**: REQ-406
- **What**: `expand(seeds: &[StableId], csr: &Csr, edge_types: &[EdgeType],
  max_depth: u8) -> Vec<StableId>` — BFS a partir dos seeds via
  `Csr::edges_from` (Fase 1), respeitando `max_depth`, sem duplicar nós
  visitados.
- **Where**: `src/hybrid.rs`
- **Depends on**: none (só precisa de `Csr` da Fase 1, já pronto)
- **[P]**: A (paralelizável com T-402)
- **Done when**: fixture de grafo com 3 hops a partir do seed — `max_depth:
  1` retorna só vizinhos diretos, `max_depth: 2` inclui o segundo hop,
  nó fora do alcance não aparece.
- **Gate**: `cargo test hybrid::expand`

## T-404: `hybrid::seed_discovery` (fusão RRF) [x]
- **REQ**: REQ-405
- **What**: `seed_discovery(bm25_ranked: &[StableId], hnsw_ranked:
  &[StableId]) -> Vec<(StableId, f32)>` — Reciprocal Rank Fusion
  (`score = Σ 1/(k + rank)`, `k` configurável, default 60 — constante comum
  na literatura de RRF). Assinatura toma listas já ranqueadas (não texto
  cru) para poder testar a matemática da fusão isoladamente de Tantivy/HNSW
  reais.
- **Where**: `src/hybrid.rs`
- **Depends on**: none
- **Done when**: teste adversarial — documento bem ranqueado só no BM25 e
  outro bem ranqueado só no HNSW aparecem nos dois nos resultados fundidos,
  nenhum dos dois sinais domina sozinho; documento em 1º em ambos os
  rankings fica em 1º na fusão.
- **Gate**: `cargo test hybrid::seed_discovery`

## T-405: `Embedder` — scaffold lazy + caminho "modelo ausente"
- **REQ**: REQ-401, REQ-402
- **What**: `vector::embedder::Embedder` com `ort::Session` atrás de
  `OnceLock`, inicializado só na primeira chamada de `embed()`.
  `VectorError::ModelNotAvailable` quando os arquivos de modelo/tokenizer
  não existem no caminho configurado — **sem baixar nada**, sem exigir o
  modelo real presente para este teste passar.
- **Where**: `src/vector/embedder.rs`
- **Depends on**: T-401
- **Done when**: construir `Embedder` com um caminho de modelo inexistente e
  chamar `embed()` retorna `Err(VectorError::ModelNotAvailable)`, não panic,
  não tenta rede.
- **Gate**: `cargo test vector::embedder`

## T-406: Inferência real — modelo de embedding (REQUER CONFIRMAÇÃO DO USUÁRIO)
- **REQ**: REQ-401, REQ-402
- **What**: Baixar o modelo quantizado (`all-MiniLM-L6-v2` ou
  `bge-small-en-v1.5`, ONNX INT8, ~30MB) + tokenizer de uma fonte
  confirmada com o usuário (nome exato, origem, tamanho apresentados no
  pedido de confirmação — ação de download explícito). Ligar `Embedder` à
  inferência real; se `ort` precisar da lib nativa do ONNX Runtime via
  `download-binaries`, isso entra no mesmo pedido de confirmação.
- **Where**: `src/vector/embedder.rs` (extensão), arquivo(s) de
  modelo/tokenizer (local a definir — fora do controle de versão do Git,
  provavelmente)
- **Depends on**: T-405
- **Done when**: `embed("hello world")` retorna um `Vec<f32>` de dimensão
  esperada (384 para os dois modelos candidatos), determinístico entre
  chamadas.
- **Gate**: `cargo test vector::embedder::real_inference -- --ignored`
  (marcado `#[ignore]` por padrão — só roda quando o modelo está presente,
  não trava CI/outros ambientes sem o arquivo)

## T-407: Verificação da feature `lean`
- **REQ**: REQ-404
- **What**: Confirmar programaticamente (script/task de CI, não só
  inspeção manual) que um build com `lean` não traz `ort`/
  `instant-distance` na árvore de dependências (`cargo tree --no-default-features --features lean`
  não deve listá-las).
- **Where**: fora de `src/` — script/comando de verificação, ou teste que
  roda `cargo tree` e faz assert no output
- **Depends on**: T-401
- **Done when**: verificação confirma ausência das duas crates na árvore
  do build `lean`.
- **Gate**: comando de verificação retorna sucesso

## T-408: Integração: 4 participantes reais via `Coordinator`
- **REQ**: (todos — valida a fase inteira junto)
- **What**: Teste de integração com `Coordinator` real e `[RedbParticipant,
  CsrParticipant, TantivyParticipant, HnswParticipant]`, usando vetores
  sintéticos para o `HnswParticipant` (não depende de T-406) — confirma que
  o quarto participante convive com os outros três sem regressão.
- **Where**: `tests/four_participants_integration.rs`
- **Depends on**: T-402, T-403, T-404
- **Done when**: teste passa fim-a-fim; consulta híbrida
  (`seed_discovery` + `expand`) sobre dados sintéticos retorna o conjunto
  esperado.
- **Gate**: `cargo test --test four_participants_integration`

## T-409: Lint e superfície pública
- **REQ**: (todos — fechamento da fase, mesmo padrão de T-010/T-110/T-209/T-310)
- **What**: Exportar `vector::{HnswParticipant, Embedder, VectorError}`,
  `hybrid::{seed_discovery, expand}` de `src/lib.rs`. Doc comments em toda
  API pública.
- **Where**: `src/lib.rs`, `src/vector/mod.rs`
- **Depends on**: T-408
- **Done when**: `cargo doc --no-deps` sem warnings, `cargo clippy
  --all-targets -- -D warnings` sem erros (nos dois builds, default e
  `lean`).
- **Gate**: `cargo doc --no-deps && cargo clippy --all-targets -- -D warnings`

# State

**Last Updated:** 2026-09-29

## Current Work

**Fases 0-6 completas — todo o roadmap do NexSpec implementado neste
repositório.** Fase 6 (Interface, MCP Server & Tooling) — 12/12 tasks
(T-601..T-612), escopo reduzido por instrução explícita do usuário:
integração com a skill `graph-spec-design` fica para outro repositório/
sessão; esta fase entregou só o binário `nexspec` (CLI `clap`:
init/sync/compact/search/trace/blame/diff/mcp) e o servidor MCP embutido
(`rmcp`, stdio, 6 tools: query_context/semantic_search/trace_requirement/
find_impacted_code/get_symbol_history/sync_workspace).

Suíte completa: 93/93 testes de lib (3 ignored) + todos os binários de
integração verdes (build `full`, incluindo `tests/cli_integration.rs` que
roda o binário real via `CARGO_BIN_EXE_nexspec`); 86/86 testes de lib (1
ignored) no build `lean`. `cargo doc`/`cargo clippy -- -D warnings` limpos
nos dois builds. Tudo commitado e no GitHub (`leandroluk/rust-specdb`,
branch `main`). Produto renomeado de "SpecDB" para "NexSpec" (crate
`nexspec`) — repo/pasta local seguem com o nome antigo até o usuário
trocar por conta própria.

Modelo `all-MiniLM-L6-v2` quantizado INT8 (~23MB, `Xenova/all-MiniLM-L6-v2`
no Hugging Face) + tokenizer baixados para `.models/` (gitignored, não
versionado). `Embedder` faz inferência real via `ort` + `tokenizers`, mean
pooling + normalização L2, confirmado determinístico e semanticamente
coerente (frases parecidas rankeiam mais perto que não-relacionadas).

## Todos
- [ ] Fase 9 (performance-guard), Fase 8 (retrieval-benchmark), Fase 7 (dependency-edges), Fase 10 (report-command): specificadas em `.specs/features/*/spec.md` (2026-09-29); Design/Tasks pendentes. Ordem sugerida 9 -> 8 -> 7 -> 10 (ver ROADMAP).
- [x] T-601..T-612: Fase 6 completa (ver Feature "cli-mcp-server" abaixo)

## Active Blockers
- none

## Degraded Mode
- Grafo do próprio NexSpec NÃO construído — `.specs/graph/graph.json` não
  existe. Fora do escopo deste repositório (ver Feature "cli-mcp-server" →
  Out of Scope) — fica para quando o repositório que integra a skill
  `graph-spec-design` consumir o binário `nexspec`.

## Feature "token-budgeting" (Fase 5): COMPLETA (9/9)
`token::budget::{Tokenizer, TiktokenTokenizer, CharHeuristicTokenizer,
Budget, Tier, TieredItem}`, `token::pruner::prune_symbol`,
`token::serializer::serialize`. Pruner reusa `code::parser::Language`/
`symbol_query` (agora `pub(crate)`) para relocalizar o mesmo nó de definição
pela linha e substituir seu corpo por `{ ... }`/`...`; sem corpo
identificável, cai para o texto original inalterado. `Budget::fit` corta
por tier (`Target`/`Seed`/`Dependency`) de forma determinística: para no
primeiro item que estouraria o limite com margem (default 90%), descarta
todo o resto por inteiro (não faz best-effort tentando os próximos itens
menores). `tiktoken-rs` é dependência obrigatória (não gated por `lean`).
Teste de integração (`tests/token_budgeting_integration.rs`) confirma o
fluxo pruner→budget→serializer fim-a-fim com `CharHeuristicTokenizer`
(sem dependência de rede). Gate final: `cargo doc`/`cargo clippy -- -D
warnings` limpos em `full` e `lean`; suíte completa verde nos dois builds.

## Feature "cli-mcp-server" (Fase 6): COMPLETA (12/12)
`src/engine.rs` (`Engine`, composition root), `src/git/blame.rs`
(AST-aware blame), `src/mcp.rs` (`NexSpecMcp`, 6 tools via `rmcp`),
`src/bin/nexspec.rs` (CLI `clap`, 8 subcomandos incl. `mcp`). Novas deps
não-opcionais: `clap`, `tokio`, `rmcp` v3.5.0, `serde`/`serde_json`/
`schemars`. Escopo reduzido por instrução do usuário: sem integração com a
skill `graph-spec-design` neste repositório. Gate final: `cargo doc`/
`cargo clippy -- -D warnings` limpos em `full`/`lean`; suíte completa
verde nos dois builds, incluindo `tests/cli_integration.rs` (roda o
binário real via `CARGO_BIN_EXE_nexspec`, cobrindo init→sync→search→
trace→blame→diff fim-a-fim).

## Recent Decisions (Last 15)
- 2026-09-29 Fase 6 fechada — 5/5 tasks finais (T-608..T-612). CLI
  (`clap`) com 8 subcomandos, todos delegando a `Engine` (nenhuma lógica
  duplicada entre CLI e MCP). Servidor MCP (`src/mcp.rs`) usa DTOs
  próprios (`NodeDto`/`SearchResponse`/etc., `#[derive(Serialize)]`) em vez
  de derivar `Serialize` nos tipos internos do `Engine` — mantém `StableId`
  convertido para hex e `NodePayload` resumido (kind+summary) na borda de
  apresentação, sem acoplar os tipos ricos internos a `serde`/JSON.
  `main()` usa `std::process::ExitCode` + `Box<dyn std::error::Error>` (sem
  `anyhow`, que só chegaria como dependência transitiva não-direta). Teste
  de integração roda o binário de verdade via `CARGO_BIN_EXE_nexspec`
  (padrão do Cargo para testes de integração alcançarem um binário irmão),
  não só a biblioteca — inclui um cenário de árvore suja para validar
  `diff --staged` de ponta a ponta. Smoke test manual confirmou
  `search --max-tokens` produzindo Markdown podado real. Gate: suíte
  completa 93/93 lib (`full`, 3 ignored) + todos os binários de
  integração; 86/86 lib (`lean`, 1 ignored); `cargo doc`/`cargo clippy -- -D
  warnings` limpos nos dois builds.
- 2026-09-29 T-604 completo. `git::blame::blame_symbol` via
  `gix::Repository::blame_file` (feature `blame` do `gix` já vem habilitada
  por padrão via `extras`, confirmado lendo `gix-0.88.0/Cargo.toml`
  localmente — nenhuma mudança necessária em `Cargo.toml`) +
  `gix::blame::BlameRanges::from_one_based_inclusive_range` (conversão do
  `line_start`/`line_end` 0-indexado do crate para o formato 1-indexado do
  `gix_blame`). Retorna `BlameHunk{commit_oid, author_name, author_email,
  author_unix_seconds, lines}` por hunk. `gix::blame`/`gix::bstr` reusados
  via re-export do próprio `gix` (sem dependência direta em `gix-blame`).
  Gate: `cargo test git::blame` → 2/2.
- 2026-09-29 T-601/602/603/605/606/607 completos (`src/engine.rs` novo).
  `Engine::open` idempotente (cria metadata.redb/edges.bin/tantivy/ só se
  ausentes); `sync`/`resume`/`compact` reconstroem participantes por
  chamada, como planejado no design. Refinamentos sobre o design original:
  `diff_staged` NÃO usa `DirtyCache` (ela assume estado persistido
  entre chamadas do mesmo processo — "primeira vez que olha para um
  arquivo = mudou", errado para um comando CLI de tiro único); comparação
  direta `git.read_blob_at_head` vs `std::fs::read` do working tree, sem
  estado. `Csr` ganhou `all_edges()` (merge base+delta, mesma lógica do
  `compact()` interno do `CsrParticipant`, exposta como leitura) — CSR só
  indexava por `from`, e REQ-608 precisa da direção reversa ("quem depende
  de X"). `trace`'s resolução de `target` usa top-hit BM25 como fallback
  textual (não "exact-match" como o design sugeria) — mais simples e
  suficiente para este escopo. `search::schema` ganhou `unhex` (par de
  `hex` já existente). Suíte: 87/87 lib (`full`, 3 ignored) + todos os
  binários de integração; 80/80 lib (`lean`, 1 ignored). `cargo clippy -- -D
  warnings` limpo nos dois builds.
- 2026-09-29 Feature "cli-mcp-server" (Fase 6) especificada e desenhada.
  REQ-601..609. Decisão do usuário: integração com a skill
  `graph-spec-design` explicitamente fora do escopo deste repositório
  (outro repo/sessão fará isso) — Fase 6 aqui entrega só CLI+MCP como
  ferramenta standalone, sem contrato `.specs/graph/graph.json`/
  `GRAPH_REPORT.md` compatível com `graphify`. Decisão de arquitetura:
  `Engine` (novo, `src/engine.rs`) é o primeiro componente a compor redb+
  Csr+Tantivy+Hnsw+Git+Token ao mesmo tempo; reconstrói participantes por
  chamada em vez de mantê-los vivos (evita struct auto-referenciada com
  `Coordinator<'a>`). `blame` (REQ-607) fecha uma lacuna deliberadamente
  deixada aberta na Fase 2 (precisava de `line_start`/`line_end` da Fase
  3) via `gix::Repository::blame_file`. 12 tasks (T-601..T-612).
- 2026-09-29 Fase 5 (token-budgeting) implementada e fechada, 9/9 tasks.
  Decisão de implementação notável: `Budget::fit` usa corte "hard stop" (um
  item que não cabe interrompe a inclusão de todos os itens seguintes,
  mesmo que algum deles individualmente coubesse) — não um "melhor esforço"
  que pula o item grande e tenta o próximo menor. Isso é o que a spec
  (REQ-504) pedia ("nós além desse ponto de corte são omitidos
  inteiramente"), mas exigiu ajustar um teste inicial que assumia
  comportamento best-effort.
- 2026-09-29 Feature "token-budgeting" (Fase 5) especificada e desenhada.
  REQ-501..505. Decisões: `token::` módulo top-level novo (sibling de
  `hybrid`/`graph`/`code`); pruning é função pura sobre texto-fonte
  (sem `NodePayload` novo, sem `SyncParticipant` — recomputado sob demanda);
  `Budget::fit` recebe input já tiered/ordenado pelo chamador (não resolve
  grafo sozinho, mesmo padrão de `hybrid::seed_discovery`/`expand` tomando
  listas já rankeadas); `tiktoken-rs` é dependência obrigatória (não
  gated por `lean` — leve, sem peso de binário nativo/modelo como
  `ort`/`instant-distance`). 9 tasks (T-501..T-509).
- 2026-09-29 STATE.md compactado (25KB → ver relatório abaixo). Formato
  legado (`## Progress`/`## Decisions` sem janela) migrado para o template
  windowed. Histórico completo das Fases 0-2 em `STATE_ARCHIVE.md`.
- 2026-09-28 Feature "ast-lexical-search" (Fase 3) especificada, desenhada e
  quebrada em tasks. REQ count: 8 (REQ-301..308). Escopo `Complex`.
  Decisões: `DependsOn` só resolve chamadas dentro do mesmo arquivo (sem
  resolvedor de módulos cross-file); `TantivyParticipant` guarda um
  `IndexWriter` de vida longa (stage=add_document, commit=writer.commit,
  abort=writer.rollback); `search::` é módulo top-level novo, irmão de
  `graph`/`sync`/`git`, não aninhado em `graph::`. 10 tasks (T-301..T-310),
  ondas [P-A] T-304 e [P-B] T-307 (paralelizável com o resto a partir de
  T-301).
- 2026-09-28 Produto renomeado de "SpecDB" para "NexSpec". Repo/pasta local
  e remoto GitHub não renomeados nesta sessão (usuário faz depois).
- 2026-09-28 Feature "git-integration" (Fase 2) especificada. Submodules/
  LFS/sparse-checkout adiados; `SyncOrchestrator` fora de `sync::`/`graph::`;
  co-change vira `EdgeType::CoChanges`.
- 2026-09-28 Feature "storage-primitives" (Fase 1) especificada. CSR de duas
  camadas cobre só edges; delta lock-free via `ArcSwap`.
- Ver `STATE_ARCHIVE.md` para decisões anteriores (Fase 0 spec/design, rename
  de produto detalhado, etc.).

## Feature "vector-engine" (Fase 4): COMPLETA (9/9)
Todas as 9 tasks concluídas. `HnswParticipant` é o 4º `SyncParticipant`
real; `hybrid::{expand, seed_discovery}` prontos; `Embedder` faz inferência
real via `ort`+`tokenizers` contra `all-MiniLM-L6-v2` quantizado (baixado
em `.models/`, gitignored). Gate final: `cargo doc`/`cargo clippy -- -D
warnings` limpos em `full` e `lean`; suíte completa 85/85 (`full`) + 2
testes de inferência real, 78/78 (`lean`).

## Recent Progress (Last 10)
- 2026-09-29 Correcoes de dogfooding (commits `862ca8e`, `b49cab7`, `6fbd4a7`, `a09cbf3`, `81fbf65`, `67344c5`): `CsrDelta` O(n^2) -> indice por id (sync do repo real de >8 min sem terminar para ~25 s); `abstract class`/`enum`/`type` indexados; `dirty` usa `gix status` (nao mais todos os arquivos) e inclui nao rastreados; ids com prefixo (`REQ-TCK-001`) e sufixo (`REQ-021b`); ids de REQ/TASK/ADR so pelo marcador (links entre arquivos e `@spec` sem tabela); listas dentro de TASK contam como corpo; `trace` segue arestas de entrada (`<-`); co-change so recalculado quando o historico muda (sync sem novidades 5,8 s -> 0,9 s). Evolucoes derivadas registradas como Fases 7-10 no ROADMAP.
- 2026-09-29 T-406 completo. Baixado `all-MiniLM-L6-v2` quantizado INT8
  (~23MB, `Xenova/all-MiniLM-L6-v2` no Hugging Face) + `tokenizer.json` +
  `config.json` para `.models/` (novo, gitignored). Adicionada dependência
  `tokenizers` (feature `fancy-regex`, já que `onig` exige lib C) e
  `ort` ganhou a feature `download-binaries` (baixa a lib nativa do ONNX
  Runtime automaticamente). `Embedder::embed()` agora: tokeniza →
  `input_ids`/`attention_mask`/`token_type_ids` como tensores i64 → roda a
  sessão ONNX → mean pooling sobre tokens não-padding → normalização L2 →
  `Vec<f32>` de 384 dimensões. 2 testes `#[ignore]` (só rodam com o modelo
  presente, via `cargo test -- --ignored`) confirmam: determinístico,
  norma ≈1, e frases semanticamente parecidas rankeiam mais perto entre si
  do que frases não relacionadas — validação real de correção semântica,
  não só "não deu panic". Sugestões do usuário registradas em Deferred
  Ideas: Fase 6 deve automatizar esse download (`nexspec init` por padrão,
  `--no-model` para pular). Gate: `cargo test -- --ignored` (os 2 testes de
  inferência) → 2/2; suíte completa → 85/85 (`full`), 78/78 (`lean`);
  `cargo clippy -- -D warnings` limpo nos dois builds.
- 2026-09-29 T-409 completo. `lib.rs` exporta `hybrid::{expand,
  seed_discovery}` e, sob a feature `full`, `vector::{Embedder,
  HnswParticipant, VectorError}`. Corrigidos 2 lints clippy
  (`chunks_exact_to_as_chunks` → `as_chunks::<4>()`, `type_complexity` →
  alias `StagedPoints`) e gate de feature faltante no teste de integração
  de T-408 (`#![cfg(feature = "full")]`, já que ele usa `vector::` e não
  compilaria — nem deveria — num build `lean`). Gate: `cargo doc --no-deps`
  e `cargo clippy --all-targets -- -D warnings` limpos nos dois builds
  (`full` e `--no-default-features --features lean`); suíte completa →
  85/85 (`full`), 78/78 (`lean`).
- 2026-09-29 T-408 completo, mas revelou e corrigiu um bug real de
  robustez numérica. `tests/four_participants_integration.rs`: `Coordinator`
  real com `[Redb, Csr, Tantivy, Hnsw]` via `stage()` direto (não
  `SyncOrchestrator`, que ainda não gera vetores). Bug encontrado: bytes
  arbitrários decodificados como `f32` (convenção do `HnswParticipant`)
  produzem componentes de magnitude extrema (perto de `f32::MAX`);
  elevar ao quadrado em `f32` na função de distância cosseno estourava para
  `Infinity`, e `Infinity/Infinity = NaN` fazia o HNSW devolver o vizinho
  errado — silenciosamente, sem panic, só resultado incorreto. Corrigido:
  `EmbeddingPoint::distance` agora acumula em `f64` (produto escalar e
  normas), cosseno final clampado em `[-1,1]` antes de converter de volta
  a `f32`. Teste de regressão dedicado
  (`distance_stays_finite_for_extreme_magnitude_vectors`) adicionado a
  `vector::hnsw`. Gate: `cargo test --test four_participants_integration`
  → 1/1; suíte completa → 85/85 (era 72 antes da fase começar).
- 2026-09-29 T-407 completo. `scripts/check-lean-build.sh` roda `cargo tree
  --no-default-features --features lean` e falha se `ort`/`instant-distance`
  aparecerem na árvore — validado que o grep realmente pegaria a falha
  (testado contra a árvore `full`, onde as duas aparecem). Gate: script →
  "OK: lean build excludes ort and instant-distance."
- 2026-09-29 T-405 completo. `vector::embedder::Embedder` — construção não
  toca disco/rede; `embed()` retorna `VectorError::ModelNotAvailable`
  limpo se modelo/tokenizer não existem no path configurado. Decisão
  deliberada: **nenhum tipo do crate `ort` é tocado nesta task** — a
  inferência real (T-406) exige um modelo de verdade para poder ser
  testada/iterada de verdade contra a API real do `ort`, então escrever
  esse código agora seria "adivinhar" uma API sem conseguir compilar
  contra o caso real. `OnceLock<()>` como placeholder do slot "carregado
  uma vez" (REQ-401), preenchido de verdade em T-406. Gate: `cargo test
  vector::embedder` → 2/2 pass.
- 2026-09-29 T-404 completo. `hybrid::seed_discovery(bm25_ranked,
  hnsw_ranked) -> Vec<(StableId, f32)>` — RRF clássico (`Σ 1/(k+rank)`,
  `k=60`, constante da literatura). Teste adversarial confirma que um
  documento só em BM25 e outro só em HNSW não somem da fusão, e que
  concordância entre os dois sinais (1º em ambos) supera qualquer um dos
  dois sozinho. Gate: `cargo test hybrid::` → 5/5 pass.
- 2026-09-29 T-402/T-403 completos (onda [P-A]).
  `vector::hnsw::{HnswIndex, HnswParticipant}` — 4º `SyncParticipant` real.
  Simplificação deliberada vs. CsrParticipant: `instant-distance` só
  constrói em lote (sem inserção incremental), então em vez de um
  base+delta de verdade, `commit()` funde o staged no conjunto completo de
  pontos committed e **reconstrói o índice inteiro** — aceitável na escala
  de vetores desta fase, revisitar se perfilamento pedir. Convenção
  específica desta fase: payload de `NodeMutation::Upsert` é lido como
  vetor `f32` little-endian bruto (`encode_vector`/`decode_vector`) — não
  existe variante de embedding em `NodePayload` ainda; revisitar quando
  T-406 ligar geração real de embeddings ao `SyncOrchestrator`. Bug pego
  nos testes: `NamedTempFile::new()` cria arquivo vazio que *existe* (0
  bytes) — `path.exists()` sozinho não basta para decidir "tem pontos
  persistidos", precisa checar tamanho > 0 também.
  `hybrid::expand` — BFS limitado por `max_depth` sobre `Csr::edges_from`
  (Fase 1, inalterado). Gate: `cargo test vector::hnsw` → 4/4,
  `cargo test hybrid::` → 3/3; build `lean` seguiu compilando.
- 2026-09-29 T-401 completo. `cargo add ort instant-distance --optional`;
  `[features]` reorganizado: `default = ["full"]`, `full = ["dep:ort",
  "dep:instant-distance"]`, `lean = []` (build sem elas via
  `--no-default-features --features lean`). Ambos os builds compilam;
  suíte completa (default) continua 72/72 — nada em `src/` referencia
  `ort`/`instant-distance` ainda, então `lean` exclui automaticamente sem
  precisar de `#[cfg(...)]` nenhum por enquanto (virá em T-402/T-405).
  Gate: `cargo build` + `cargo build --no-default-features --features
  lean` → ambos sucesso.
- 2026-09-29 T-310 completo, fecha a Fase 3. `lib.rs` exporta
  `code::{Language, extract as extract_code}`, `search::{TantivyParticipant,
  find_by_id, search_text}` (renomeado para `extract_code` para não colidir
  em prosa com `markdown::extract`). Corrigido 1 doc-link quebrado
  (`[TantivyParticipant]` em `query.rs` sem import no escopo do módulo).
  Gate: `cargo doc --no-deps` e `cargo clippy --all-targets -- -D warnings`
  limpos de primeira; `cargo test` → 72/72 pass.

## Feature "ast-lexical-search" (Fase 3): COMPLETA
Todas as 10 tasks (T-301..T-310) concluídas, 72/72 testes, `cargo doc`/
`cargo clippy -- -D warnings` limpos. Tree-sitter (5 linguagens) extrai
símbolos + edges `DefinedIn`/`DependsOn`/`Satisfies`; `rayon` paraleliza
cold-start; `TantivyParticipant` é o terceiro `SyncParticipant` real,
validando o contrato da Fase 0 sob um modelo de storage bem diferente de
`redb`/CSR (buffer/commit/rollback nativo do Tantivy). `SyncOrchestrator`
agora roteia `.md` e código na mesma passagem, com `@spec`/`@adr` resolvendo
contra specs do mesmo ciclo.
- 2026-09-29 T-309 completo. `tests/three_participants_integration.rs`:
  `Coordinator` real com `[RedbParticipant, CsrParticipant,
  TantivyParticipant]`, `SyncOrchestrator::run_once()` sobre fixture com
  `.md` + `.rs` (REQ satisfeito por símbolo via `@spec`). Confirma
  consistência nos 3 stores: nós em `redb`, edge `Satisfies` no CSR,
  achável por id exato e por BM25 no Tantivy. Adicionado
  `TantivyParticipant::handle()`/`TantivyHandle`/trait `TantivyQueryable`
  (mesmo padrão do `csr_handle()` da Fase 1) — necessário porque o
  participante é movido para dentro do `Coordinator`, então quem quiser
  consultar depois precisa de um handle guardado antes do move; `find_by_id`/
  `search_text` (T-308) generalizados para aceitar `&impl TantivyQueryable`
  em vez de `&TantivyParticipant` fixo. Gate:
  `cargo test --test three_participants_integration` → 1/1; suíte completa
  → 72/72.
- 2026-09-29 T-308 completo. `search::query::{find_by_id, search_text}` —
  `find_by_id` via `TermQuery` no campo `id` (lookup de termo, não scan);
  `search_text` via `QueryParser` + BM25 sobre `text`. `id` mudou de
  parâmetro (`&str`) para `&StableId` no fast-path — mais coerente com o
  resto do crate (que só circula `StableId` bytes, não hex string, exceto
  internamente no schema). Gate: `cargo test search::query` → 3/3 pass.
- 2026-09-29 T-307 completo. `search::schema::TantivySchema` (`id`/`kind`/
  `text`/`path`) + `search::tantivy_participant::TantivyParticipant`
  (terceiro `SyncParticipant` real). `stage`/`commit`/`abort` mapeiam direto
  em `IndexWriter::add_document`/`commit`/`rollback`; idempotência via
  `staged_version: Option<u64>` (restage do mesmo ciclo não readiciona
  documentos, já que Tantivy não tem upsert-por-id nativo). Ajuste de API:
  `TopDocs::with_limit(n)` sozinho não implementa mais `Collector` nesta
  versão do tantivy (0.26) — precisa de `.order_by_score()` encadeado
  (mudança de API não documentada nos meus exemplos mentais, descoberta via
  erro de compilação). `NodeMutation::Remove` ainda não reflete no índice
  (sem `delete_term`) — limitação documentada, fora do critério de "Done"
  desta task. Gate: `cargo test search::` → 5/5 pass.
- 2026-09-29 T-306 completo. `SyncOrchestrator::run_once()` reestruturado em
  2 passagens: (1) Markdown — diff committed + dirty tree, igual antes; (2)
  código — `code::Language::from_extension(path)` roteia `.ts/.py/.go/.rs`
  etc. para `code::extract()`, usando um `known_markers` construído a
  partir dos nós `Requirement`/`Adr` já extraídos na passagem 1 do mesmo
  ciclo (permite `@spec REQ-XXX` no código resolver contra specs commitadas
  junto, REQ-304). `tests/sync_orchestrator_code_routing.rs`: commit com
  `.md` + `.rs` juntos, símbolo de código com `@spec` resolve a edge
  `Satisfies` até o REQ vindo do Markdown do mesmo ciclo. Gate:
  `cargo test --test sync_orchestrator_code_routing` → 1/1; suíte completa
  → 63/63.
- 2026-09-29 T-305 completo. `code::batch::extract_all(files,
  known_markers)` — `files.par_iter().map(extract).collect::<Result<Vec<_>,
  _>>()` (rayon), merge sequencial dos `MutationSet`s. Falha rápida no
  primeiro arquivo com erro (sem modo best-effort). Gate: `cargo test
  code::batch` → 1/1, confirmando paridade com chamar `extract()` arquivo a
  arquivo.
- 2026-09-29 T-304 completo. `extract()` ganhou parâmetro
  `known_markers: &HashMap<String, StableId>` — o id de um REQ/ADR depende
  do corpo do texto dele (ver `markdown::extract`), que o código-fonte não
  tem como recalcular sozinho, então o chamador (que já rodou
  `markdown::extract` sobre `.specs/`) fornece o mapa resolvido. Para cada
  símbolo, olha o irmão anterior na AST (`prev_sibling`); se o `kind()`
  contém "comment", escaneia por `REQ-`/`ADR-` via `find_markers`
  (reaproveitada pela terceira vez) e emite `Satisfies` só para marcadores
  presentes no mapa. Gate: `cargo test code::` → 6/6 pass.
- 2026-09-29 T-303 completo. `code::parser::extract` ganhou parâmetro `path`
  (necessário para `file_node_id`); edge `DefinedIn` de cada símbolo para o
  nó de arquivo; segunda `Query` por linguagem captura call expressions de
  nome direto (`@callee`), resolvidas por contenção de byte-range contra os
  símbolos já extraídos no mesmo arquivo → edge `DependsOn`. Chamada a
  função não resolvida no arquivo não gera edge nem erro. Gate: `cargo test
  code::` → 4/4 pass.
- 2026-09-29 T-302 completo. `code::parser::{Language, extract}` — uma
  `tree_sitter::Query` por linguagem, captures `@name`/`@def`. Ajuste: as
  queries iniciais tipavam o campo `name` com o node-kind exato
  (`identifier`/`type_identifier`), mas `class_declaration` em JS/TS rejeitou
  isso ("Impossible pattern") — trocado para `(_) @name` (wildcard) em todas
  as queries, já que só o texto do nome importa, não seu node-kind exato.
  `NodePayload::Symbol` (Fase 1) ganhou `line_start`/`line_end: u32`
  (extensão aditiva, exigida pelo REQ-302; nenhum dado persistido a migrar
  ainda). Gate: `cargo test code::` → 1/1 (5 linguagens no mesmo teste);
  suíte completa → 54/54.
- 2026-09-29 T-301 completo. `cargo add tree-sitter tree-sitter-{typescript,
  javascript,python,go,rust} rayon tantivy`. Gate: `cargo build` → sucesso
  (~39s, dependency tree bem maior que as fases anteriores — esperado dado
  4 gramáticas + motor de busca completo).
- 2026-09-29 STATE.md compactado, `STATE_ARCHIVE.md` criado com o histórico
  completo de progresso/decisões das Fases 0-2.
- 2026-09-28 Fase 2 (git-integration) COMPLETA. T-201..T-209, 53/53 testes,
  `cargo doc`/`clippy -D warnings` limpos. Ver STATE_ARCHIVE para detalhe
  por task.
- 2026-09-28 Fase 1 (storage-primitives) COMPLETA. T-101..T-110, 36/36
  testes. Ver STATE_ARCHIVE.
- 2026-09-28 Fase 0 (sync-coordinator) COMPLETA. T-001..T-010, 19/19 testes.
  Ver STATE_ARCHIVE.
- 2026-09-28 Rename SpecDB → NexSpec aplicado em todo o código/docs.

## Lessons Learned (Last 5)
- [2026-09-29] Testes unitarios passavam com todos esses bugs presentes: so o dogfooding em repositorio real (1,3 mil arquivos, 160 commits, commit de 800 arquivos) os expos. Escala e plataforma precisam de teste com orcamento de tempo (Fase 9).
- 2026-09-28 Um trait genérico (`SyncParticipant`) desenhado antes de seus
  consumidores reais existirem tende a forçar pelo menos um refino de
  assinatura/tipo quando o primeiro consumidor real chega (`Csr::base` →
  `ArcSwap`, `CsrParticipant` → `Arc<Csr>`). Não é falha de design — é o
  próprio propósito de validar cedo — mas vale orçar esse retrabalho ao
  planejar a fase seguinte que reusa um trait ainda jovem.
- 2026-09-28 Ao construir um REQ em cima de uma feature já commitada
  (REQ-204 sobre o `SyncOrchestrator` de REQ-205), checar explicitamente se
  a spec anterior foi 100% coberta antes de escrever os testes da task
  seguinte — a lacuna só apareceu ao escrever os testes de T-208. Vale um
  passo de "reler os REQs da feature inteira" antes de marcar a última task
  como pronta.

## Deferred Ideas
- Trim de features do `gix` (`default-features = false`) — footprint atual
  inclui suporte a rede/credenciais não usados nesta fase local-only.
- Corrigir/genericizar os 3 subagentes em `.claude/agents/` (po/dev/qa) —
  ainda referenciam `condominium-management-system` internamente.
- 2026-09-29 (sugestão do usuário, via Gemini) Para a Fase 6 (CLI/MCP
  Server): disparar `SyncOrchestrator::run_once()` observando eventos de
  kernel sobre `.git/` (ex. `.git/HEAD`, `.git/refs/`, `.git/index` via
  `notify`/inotify/FSEvents/ReadDirectoryChangesW) em vez de polling por
  tempo (`git diff` em loop custa CPU à toa). Não afeta o design atual —
  `run_once()` já é pull-based/on-demand, sem loop de polling embutido;
  isso é sobre *quem* e *quando* chama `run_once()`, que ainda não foi
  especificado (é exatamente o papel da Fase 6).
- 2026-09-29 (sugestão do usuário) Para a Fase 6 (CLI): comando/flag para
  baixar automaticamente modelo+tokenizer de embedding, em vez de exigir
  que o usuário baixe manualmente (como foi feito nesta sessão via `curl`
  direto em `.models/` para viabilizar T-406). Refinado numa segunda
  mensagem: `nexspec init` baixa modelo+tokenizer **por padrão**; flag
  `--no-model` pula o download e só cria a pasta `.models/` com um config
  mínimo (sem os binários) — não bloqueia o resto do `init`. O `Embedder`
  (Fase 4) já aceita caminhos de modelo/tokenizer configuráveis, então o
  comando da CLI só precisa decidir URL/destino padrão e chamar
  `curl`/`reqwest`; não exige mudança na Fase 4 em si.

# State

**Last Updated:** 2026-09-29

## Current Work

Fases 0 (Sync Coordinator), 1 (Storage Primitives/CSR), 2 (Git Integration)
e 3 (AST Parsing & Lexical Search) completas — 72/72 testes, `cargo doc`/
`cargo clippy -- -D warnings` limpos, tudo commitado e no GitHub
(`leandroluk/rust-specdb`, branch `main`). Produto renomeado de "SpecDB"
para "NexSpec" (crate `nexspec`) — repo/pasta local seguem com o nome antigo
até o usuário trocar por conta própria.

Fase 4 (Local Vector Engine & Hybrid Traversal) especificada:
`.specs/features/vector-engine/{spec,design,tasks}.md` (REQ-401..406, 9
tasks T-401..T-409). **T-406 (baixar o modelo real de embedding, ~30MB)
requer confirmação explícita do usuário antes de rodar** — todas as outras
tasks usam vetores sintéticos e não dependem disso.

## Todos
- [x] T-401: Feature `lean` + deps opcionais (`ort`, `instant-distance`)
- [x] T-402: `HnswIndex` + `HnswParticipant` (vetores sintéticos) [P-A]
- [x] T-403: `hybrid::expand` (k-hop sobre o CSR) [P-A]
- [ ] T-404: `hybrid::seed_discovery` (fusão RRF)
- [ ] T-405: `Embedder` — scaffold lazy + caminho "modelo ausente"
- [ ] T-406: Inferência real (**requer confirmação do usuário**)
- [ ] T-407: Verificação da feature `lean`
- [ ] T-408: Integração — 4 participantes reais via `Coordinator`
- [ ] T-409: Lint e superfície pública

## Active Blockers
- T-406 aguardando confirmação do usuário para baixar o modelo de embedding
  real (~30MB) — não bloqueia as outras 8 tasks da fase.

## Degraded Mode
- Grafo do próprio NexSpec NÃO construído — `.specs/graph/graph.json` não
  existe. Reindexar quando um `graphify`/`nexspec` funcional estiver
  disponível (a ironia de "a ferramenta que ainda não pode se auto-indexar"
  persiste até a Fase 6 ter CLI).

## Recent Decisions (Last 15)
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

## Recent Progress (Last 10)
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

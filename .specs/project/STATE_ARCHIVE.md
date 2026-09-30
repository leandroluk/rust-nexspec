# State Archive

<!-- Auto-generated. Never edit manually. Read with: "show state history" -->

## Archive — 2026-09-29 (compaction 1)

### Progress

- 2026-09-28 — Sessão iniciada. Skill `graph-spec-design` invocada pela primeira vez
  neste repo. Lido `.defs/NexSpec.md` (doc de arquitetura gerado com Gemini) e os
  3 subagentes em `.claude/agents/` (po, dev, qa).
- 2026-09-28 — Criada estrutura `.specs/` (project, codebase, features, quick, graph).
- 2026-09-28 — `PROJECT.md` e `ROADMAP.md` escritos a partir de `.defs/NexSpec.md`.
- 2026-09-28 — `.defs/NexSpec.md` foi reiterado externamente (outras IAs). Revisão
  reavaliada e propagada: nova Fase 0 (Sync Coordinator/WAL), CSR em duas camadas
  (base imutável + delta lock-free via ArcSwap/crossbeam-epoch), ID estável (Blake3)
  desacoplado do índice físico denso, janela de co-change limitada no blame,
  parsing paralelo (rayon), ONNX/HNSW lazy-loaded com flag "lean", tokenizer
  plugável com margem de segurança e fallback offline. `PROJECT.md`, `ROADMAP.md`
  e `STACK.md` atualizados para refletir essa revisão.
- 2026-09-28 T-001 completo. `cargo init --lib` (crate `specdb`, edition 2024),
  deps `redb`/`rkyv`/`crc32fast`/`thiserror` adicionadas, `src/lib.rs` +
  `src/sync/mod.rs` criados. Gate: `cargo build` → sucesso. Bloqueio no caminho:
  faltava linker MSVC (`lld-link.exe`) — instalado workload C++ do VS 2022
  Build Tools (usuário rodou elevado) e adicionado `C:\Program Files\LLVM\bin`
  ao PATH de usuário permanente (LLVM já estava instalado, só não estava no
  PATH).
- 2026-09-28 T-002, T-004, T-005 completos (onda [P-A]). `MutationSet` +
  `NodeMutation`/`EdgeMutation`/`DocMutation` (rkyv) com teste de roundtrip;
  `VersionPointer` sobre `redb` (tabela `meta`, chave `sync_version`) com
  default 0 e bump atômico; trait `SyncParticipant` (`stage`/`committed_version`
  /`commit`/`abort`) com mock `TestParticipant` para as tasks seguintes. Gate:
  `cargo test sync::` → 4/4 pass, sem warnings. Ajuste de API: `redb` 4.x exige
  `use redb::ReadableDatabase` para `begin_read()` (não documentado no design,
  corrigido durante implementação — não é SPEC_DEVIATION, é detalhe de versão
  de dependência).
- 2026-09-28 T-003 completo. `Wal` (`src/sync/wal.rs`): frames length-prefixed
  com tipo (mutation/commit-marker), crc32, fsync (`sync_data`) após cada
  append; `pending_frames()`/`mark_done()`. Gate: `cargo test sync::` → 7/7
  pass. Ajuste de implementação: bytes lidos do arquivo não vêm alinhados para
  os tipos archived do rkyv — decode agora copia o corpo do frame para um
  `rkyv::util::AlignedVec<16>` antes de `from_bytes` (não é SPEC_DEVIATION,
  detalhe de uso da API do rkyv 0.8).
- 2026-09-28 T-006 completo. `RedbParticipant` (`src/sync/redb_participant.rs`):
  stage grava blob de mutações + `staged_version` em tabelas de staging;
  commit desserializa o blob e aplica upsert/remove em tabelas `redb_nodes`/
  `redb_edges`/`redb_docs`, grava `committed_version`, limpa staging; abort
  limpa staging sem tocar committed. `stage`/`commit` checam
  `committed_version() >= target_version` primeiro → idempotentes por
  construção. Gate: `cargo test sync::` → 10/10 pass.
- 2026-09-28 T-007 completo. `Coordinator::stage()` (`src/sync/coordinator.rs`):
  WAL append → fan-out `stage()` → (falha ⇒ abort em todos, versão intacta,
  frame WAL permanece pendente) → fan-out `commit()` → bump `sync_version` →
  `wal.mark_done()`. Gate: `cargo test sync::` → 12/12 pass (fluxo feliz +
  falha de 1 participante no meio do fan-out, via `TestParticipant::failing()`,
  novo construtor adicionado ao mock de T-005 já que os campos internos são
  privados).
- 2026-09-28 T-008 completo. `Coordinator::resume()`: para cada frame pendente
  no WAL, reaplica (`stage`+`commit`) em cada participante com
  `committed_version() < target_version` (idempotente, cobre replay total e
  parcial no mesmo loop), depois bump de `sync_version` se necessário e
  `mark_done`. Gate: `cargo test sync::` → 15/15 pass, cobrindo os 3 cenários
  do design (nenhum aplicou / parcial / todos aplicados mas versão não
  bumpada). Adicionado `impl SyncParticipant for Arc<T>` (test-only, em
  `participant::test_support`) para permitir compartilhar o mesmo
  `TestParticipant` entre o coordinator e as asserções diretas do teste.
- 2026-09-28 T-009 completo. `tests/sync_crash_recovery.rs`: 4 testes de
  integração usando `Wal`/`RedbParticipant`/`Database` reais (não mocks),
  simulando "reinício do processo" via objetos novos sobre os mesmos arquivos
  em disco, um por ponto de crash do diagrama em `design.md` (antes do fsync
  do WAL / depois do WAL antes do fan-out / meio do fan-out com 2 participantes
  reais / depois do fan-out antes do bump de versão). Gate:
  `cargo test --test sync_crash_recovery` → 4/4 pass.
- 2026-09-28 T-010 completo. `src/lib.rs` exporta `sync::{Coordinator,
  MutationSet, SyncParticipant}`. Gate: `cargo doc --no-deps` e
  `cargo clippy --all-targets -- -D warnings` sem erros/warnings.
  Suíte completa: `cargo test` → 19/19 pass (15 unit + 4 integração).
  SPEC_DEVIATION: tasks.md previa `RedbParticipant`/internals como
  `pub(crate)` (T-010 original); mantido `pub` porque T-009 (testes de
  crash-injection) já o usa fora do crate, via `tests/sync_crash_recovery.rs`,
  para simular múltiplos participantes reais. Não conflita com REQ-006 — a
  restrição de "caminho único de escrita" é sobre código de produção
  (CSR/Tantivy/HNSW nas Fases 1/3/4), não sobre chamar a trait diretamente em
  testes para injetar cenários de crash.
- 2026-09-28 T-101 completo. `cargo add memmap2 arc-swap zstd comrak`. Ajuste:
  `comrak` com `default-features = false` — as features default incluíam CLI/
  syntect/bon (~90 crates transitivos) irrelevantes para só parsear Markdown.
  Gate: `cargo build` → sucesso.
- 2026-09-28 T-102/T-103 completos. `graph::node::{Node, NodeType,
  NodePayload}` (6 variantes) e `graph::edge::{Edge, EdgeType}` — reaproveitam
  `sync::mutation::StableId`. Gate: `cargo test graph::` → 3/3 pass.
- 2026-09-28 T-104 completo. `graph::csr::base::CsrBase`: arquivo rkyv
  mmapeado, índice `(from, edge_type) -> (start,len)` hasheado eagerly no
  `open()` (HashMap, O(1) médio) apontando para um slice de `edges`
  mmap-backed (só deserializa o range consultado, não o arquivo inteiro).
  Decisão de implementação: usei binary/hash index em vez de um índice físico
  denso `u32`-indexado diretamente (REQ-103 fala em índice denso, mas queries
  chegam por `StableId`, então algum dicionário é inevitável antes de virar
  índice denso; HashMap satisfaz o "O(1)" do REQ-105 na prática). Gate:
  `cargo test graph::csr` → 2/2 pass.
- 2026-09-28 T-105 completo. `graph::csr::delta::CsrDelta` (added/removed em
  memória, builder `upsert`/`remove`, `merge_into(base, from, edge_type)`).
  Nota: a propriedade lock-free/COW é de como `Csr` publica instâncias via
  `ArcSwap` (T-106), não do `CsrDelta` em si — este é um builder mutável
  comum. Gate: `cargo test graph::csr` → 5/5 pass.
- 2026-09-28 T-106 completo. `graph::csr::facade::Csr` (`base: CsrBase`,
  `delta: ArcSwap<CsrDelta>`): `edges_from()` faz `delta.load()` (snapshot
  atômico) + merge com a base; `publish_delta()` troca o ponteiro via
  `ArcSwap::store`. Teste de concorrência com `thread::scope` (1 thread
  publicando 200 deltas, outra lendo 500x em paralelo) confirma ausência de
  panic/deadlock. Gate: `cargo test graph::csr` → 7/7 pass.
- 2026-09-28 T-107 completo. `graph::csr::participant::CsrParticipant`
  (`SyncParticipant` real): `stage()` clona o delta publicado atual +
  reaplica `EdgeMutation`s (idempotente); `commit()` publica via
  `Csr::publish_delta`, aciona `compact()` automaticamente quando
  `delta.len()/base.edge_count() >= 5%`; `compact()` funde base+delta,
  escreve em `.staging`, rename atômico, reabre mmap, zera delta via
  `Csr::replace_base`. Refino de design: `Csr::base` virou `ArcSwap<CsrBase>`
  (era campo simples + `&mut replace_base` no T-106) — necessário porque
  `SyncParticipant` exige `&self`. Gate: `cargo test graph::csr` → 11/11 pass.
- 2026-09-28 T-108 completo. `graph::markdown::extract()`: single-pass sobre
  os blocos top-level do AST do `comrak` (headings, listas, parágrafos são
  todos irmãos no nível do documento em CommonMark). Reconhece `REQ-\d+` em
  itens de lista e `TASK-\d+`/`ADR-\d+` em headings; parágrafos logo após um
  heading TASK/ADR são escaneados por menções a REQs já vistos → edge
  `Satisfies`. Adicionada dependência `blake3`. Limitação conhecida: forward
  references não geram edge. Gate: `cargo test graph::markdown` → 2/2 pass.
- 2026-09-28 T-109 completo. `tests/graph_storage_integration.rs`:
  `markdown::extract()` → `Coordinator::stage()` com `[RedbParticipant,
  CsrParticipant]` reais → nós recuperados via `RedbParticipant::get_node`,
  edge `Satisfies` via `CsrParticipant::csr_handle().edges_from`. Refino:
  `CsrParticipant` passou a guardar `Arc<Csr>` + `csr_handle()`. Gate:
  `cargo test` → 36/36 pass.
- 2026-09-28 T-110 completo. `lib.rs` exporta `graph::{Csr, CsrParticipant,
  Edge, EdgeType, Node, NodeType}`. Corrigidos: doc-link privado, dois lints
  clippy, import não usado. Gate: `cargo doc`/`cargo clippy -- -D warnings`
  limpos; `cargo test` → 36/36 pass.
- 2026-09-28 T-201 completo. `cargo add gix` (defaults, ~125 crates
  transitivos incluindo rede/credenciais não usados — candidato a trim
  futuro). `tests/fixtures/mod.rs`: `FixtureRepo` (shell para `git` real só
  em `tests/`). Gate: `cargo build` OK; `cargo test --test fixture_smoke` →
  1/1.
- 2026-09-28 T-202 completo. `git::source::GitSource` (`open`,
  `head_commit_oid`, `is_dirty`). OIDs `[u8;20]` (SHA-1 only). Testes movidos
  para `tests/git_source.rs` (integração via `FixtureRepo`) em vez de unit
  tests, já que precisam de repos Git reais. Gate: `cargo test --test
  git_source` → 3/3, cobrindo HEAD em branch, detached, dirty.
- 2026-09-28 T-203 completo. `GitSource::diff_since(Option<[u8;20]>)` via
  `gix` tree diff, `track_rewrites(None)` explícito (rename vira Delete+Add).
  `VersionPointer` ganhou `last_indexed_commit()`/`set_last_indexed_commit()`
  em tabela `redb` nova (`meta_bytes`). Gate: `cargo test --test
  git_tree_diff` → 2/2, `cargo test sync::version` → 4/4.
- 2026-09-28 T-204/T-205 completos (onda [P-A]). `git::dirty_cache::
  DirtyCache` (Blake3 por path em memória). `EdgeType::CoChanges` (código 4)
  + `GitSource::co_change_edges()` como método (não função livre) para manter
  `repo: gix::Repository` privado — exigiu `pub(crate)` em `repo`/`op_err`.
  Gate: `cargo test git::` → 5/5.
- 2026-09-28 T-206 completo. `git::spec_link::extract_commit_links()`
  reaproveita `graph::markdown::find_markers`. `GitSource::commits_since()`
  (`since` exclusivo). Gate: `cargo test git::` (unit) → 7/7, `cargo test
  --test git_spec_link` → 1/1.
- 2026-09-28 T-207 completo. `sync_orchestrator::SyncOrchestrator::
  run_once()`: diff → `.md` via `read_blob_at_head` → `markdown::extract()`
  → `NodePayload::File` por path tocado → um `Coordinator::stage()` →
  `set_last_indexed_commit`. `co_change_edges` refinado para derivar
  `file_node_id(path)` direto (sem mapa `path_to_node_id`). Gate: `cargo
  test --test sync_orchestrator` → 1/1.
- 2026-09-28 T-208 completo, com correção de escopo antes: `SyncOrchestrator`
  não incorporava `DirtyCache` (REQ-204 ficou parcialmente implementado em
  T-207). Corrigido: `GitSource::work_dir()`/`tracked_paths_at_head()`,
  `run_once()` escaneia working tree só quando `is_dirty()`. `SyncReport`
  ganhou `files_dirty`. `tests/git_edge_cases.rs`: detached HEAD + dirty tree
  fim-a-fim. Gate: `cargo test --test git_edge_cases` → 2/2; suíte completa
  → 53/53.
- 2026-09-28 T-209 completo. `lib.rs` exporta `git::GitSource`,
  `sync_orchestrator::SyncOrchestrator`. Corrigidos `clippy::collapsible_if`
  e `dead_code` (fixtures helper, `#![allow(dead_code)]` documentado). Gate:
  `cargo doc`/`cargo clippy -- -D warnings` limpos; `cargo test` → 53/53.

### Decisions

- 2026-09-28 Feature "sync-coordinator" (Fase 0) especificada. REQ count: 7.
  Escopo `Complex`. Open questions resolvidas com defaults (WAL via rkyv,
  trait `SyncParticipant`, granularidade de versão = ciclo completo).
- 2026-09-28 Design completo para "sync-coordinator". Risco principal:
  acoplamento obrigatório — `SyncParticipant` vira dependência transitiva de
  CSR/Tantivy/HNSW; contrato tratado como não-estável até a Fase 1 validar.
- `.defs/NexSpec.md` é documento de referência externo, não editado
  diretamente — decisões relevantes promovidas para `.specs/`.
- Projeto roda em modo degradado até existir código suficiente para indexar.
- 2026-09-28 Produto renomeado de "SpecDB" para "NexSpec" — nome já em uso
  por outro projeto. Aplicado em `Cargo.toml`, código, docs. `.defs/
  SpecDB.md` → `.defs/NexSpec.md` (git mv). Repo/pasta local e remoto GitHub
  não renomeados nesta sessão (usuário faz depois). `cargo test` → 27/27
  pass sob o novo nome.
- 2026-09-28 Feature "storage-primitives" (Fase 1) especificada. REQ count:
  9 (REQ-101..109). Escopo `Complex`. CSR de duas camadas cobre só edges.
  Delta lock-free via `ArcSwap`. 10 tasks, onda [P-A] em T-102/T-103.
- 2026-09-28 Feature "git-integration" (Fase 2) especificada. REQ count: 8
  (REQ-201..208). Escopo `Complex`. Submodules/LFS/sparse-checkout adiados;
  `SyncOrchestrator` fora de `sync::`/`graph::`; co-change vira
  `EdgeType::CoChanges`; `last_indexed_commit` reaproveita tabela `meta`.
  9 tasks, onda [P-A] em T-204/T-205/T-206.

### Todos (completed)

- [x] T-001..T-010 (Fase 0, sync-coordinator) — todas completas
- [x] T-101..T-110 (Fase 1, storage-primitives) — todas completas
- [x] T-201..T-209 (Fase 2, git-integration) — todas completas

### Lessons Learned

- 2026-09-28 Quando um trait genérico (`SyncParticipant`) é desenhado antes
  de seus consumidores reais existirem, esperar que a primeira implementação
  real (não-mock) force pelo menos um refino de assinatura/tipo interno
  (`Csr::base` → `ArcSwap`, `CsrParticipant` → `Arc<Csr>`). Não é falha de
  design, é o próprio propósito de validar o contrato cedo — mas vale
  orçar esse retrabalho ao planejar a próxima fase que reusa um trait ainda
  jovem.
- 2026-09-28 Ao implementar um REQ em cima de outro já commitado (ex.:
  REQ-204 dirty-tree em cima do `SyncOrchestrator` de REQ-205), checar
  explicitamente se a spec anterior foi 100% coberta antes de escrever os
  testes da task seguinte — T-207 fechou sem notar que tinha pulado
  REQ-204; só apareceu ao escrever os testes de T-208. Vale um passo de
  "reler os REQs da feature inteira" antes de marcar a última task como
  pronta.

### Deferred Ideas (overflow)

- Trim de features do `gix` (`default-features = false` + seleção fina) —
  footprint atual (~125 crates transitivos) inclui suporte a rede/
  credenciais não usados nesta fase local-only.
- Corrigir/genericizar os 3 subagentes em `.claude/agents/` (po/dev/qa) —
  ainda referenciam `condominium-management-system` internamente.

---

## Archive — 2026-09-30 (compaction 2)

### Feature section
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

### Lesson
- 2026-09-28 Ao construir um REQ em cima de uma feature já commitada
  (REQ-204 sobre o `SyncOrchestrator` de REQ-205), checar explicitamente se
  a spec anterior foi 100% coberta antes de escrever os testes da task
  seguinte — a lacuna só apareceu ao escrever os testes de T-208. Vale um
  passo de "reler os REQs da feature inteira" antes de marcar a última task
  como pronta.

### Todo (completed)
- [x] T-601..T-612: Fase 6 completa (ver Feature "cli-mcp-server" abaixo)

---

## Archive — 2026-09-30 (compaction 3)

### Lesson
- [2026-09-29] Testes unitarios passavam com todos esses bugs presentes: so o dogfooding em repositorio real (1,3 mil arquivos, 160 commits, commit de 800 arquivos) os expos. Escala e plataforma precisam de teste com orcamento de tempo (Fase 9).
- 2026-09-28 Um trait genérico (`SyncParticipant`) desenhado antes de seus
  consumidores reais existirem tende a forçar pelo menos um refino de
  assinatura/tipo quando o primeiro consumidor real chega (`Csr::base` →
  `ArcSwap`, `CsrParticipant` → `Arc<Csr>`). Não é falha de design — é o
  próprio propósito de validar cedo — mas vale orçar esse retrabalho ao
  planejar a fase seguinte que reusa um trait ainda jovem.

---

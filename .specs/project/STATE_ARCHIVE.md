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

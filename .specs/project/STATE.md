# STATE

## Degraded Mode
- Grafo NÃO construído — `.specs/graph/graph.json` não existe. Rule #1 da skill
  segue em modo degradado: contexto lido diretamente dos arquivos.
- Já existe código Rust real (crate `nexspec`, módulos `sync`/`graph`, 36
  testes, Fase 0 e Fase 1 completas) o suficiente para valer a pena indexar,
  mas ainda não há um `graphify`/`nexspec` funcional para gerar o grafo.
  Reindexar quando `graphify` (Python) for instalado manualmente como
  alternativa temporária, ou quando o próprio `nexspec` tiver CLI (Fase 6) —
  a ironia de "a ferramenta que ainda não pode se auto-indexar" persiste.

## Progress [window: last 10]
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

## Progress [window: last 10]
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

## Feature "sync-coordinator" (Fase 0): COMPLETA
Todas as 10 tasks concluídas, 19/19 testes passando, `cargo doc`/`cargo
clippy -- -D warnings` limpos. 5 commits atômicos no histórico (scaffold+P-A,
WAL, RedbParticipant, Coordinator happy-path, resume, testes de integração —
T-010 será incluído no próximo commit). Pronta para servir de base para a
Fase 1 (Storage Primitives & Graph Topology / CSR).

## Decisions [window: last 10]
- 2026-09-28 Feature "storage-primitives" (Fase 1) especificada, desenhada e
  quebrada em tasks: `.specs/features/storage-primitives/{spec,design,tasks}.md`.
  REQ count: 9 (REQ-101..109). Escopo `Complex`. CSR de duas camadas cobre só
  edges (não nós — nós continuam via `RedbParticipant` da Fase 0). Delta
  lock-free via `ArcSwap` (não `crossbeam-epoch`, por simplicidade). 10 tasks
  (T-101..T-110), onda [P-A] em T-102/T-103.
- 2026-09-28 Design completo para "sync-coordinator" em
  `.specs/features/sync-coordinator/design.md`. Risco principal: acoplamento
  obrigatório — `sync::coordinator`/`SyncParticipant` vira dependência
  transitiva de CSR/Tantivy/HNSW nas Fases 1/3/4; contrato da trait tratado
  como não-estável até a Fase 1 validar na prática.
- 2026-09-28 Feature "sync-coordinator" (Fase 0) especificada em
  `.specs/features/sync-coordinator/spec.md`. REQ count: 7. Graph queried: não
  (modo degradado, projeto greenfield). Escopo: `Complex` — decisão de arquitetura
  nova, sem código prévio para ancorar. Open questions resolvidas com defaults
  documentados no próprio spec (formato WAL via rkyv, trait `SyncParticipant`,
  granularidade de versão = ciclo de sync completo) em vez de bloquear o início.
- `.defs/NexSpec.md` é tratado como documento de referência (gerado externamente
  com Gemini), não editado diretamente — decisões relevantes são promovidas para
  `.specs/`. Motivo: preservar a fonte original enquanto specs ficam sob controle
  da skill.
- Projeto roda em modo degradado até existir código suficiente para indexar.
  Motivo: grafo vazio não agrega valor sobre leitura direta de arquivo.
- 2026-09-28 Produto renomeado de "SpecDB" para "NexSpec" — "specdb" já estava
  em uso por outro projeto na internet. Aplicado: `Cargo.toml` (`name =
  "nexspec"`), `src/lib.rs`, `tests/sync_crash_recovery.rs` (`use nexspec::`),
  e todas as menções em `.specs/**/*.md`. `.defs/SpecDB.md` renomeado para
  `.defs/NexSpec.md` (git mv, preserva histórico). Repo/pasta local
  (`rust-specdb`) e o remoto no GitHub **não** foram renomeados nesta sessão —
  o usuário disse que troca isso por conta própria depois. Rebuild completo
  após o rename: `cargo test` → 27/27 pass sob o novo nome de crate.
- 2026-09-28 T-106 completo. `graph::csr::facade::Csr` (`base: CsrBase`,
  `delta: ArcSwap<CsrDelta>`): `edges_from()` faz `delta.load()` (snapshot
  atômico) + merge com a base; `publish_delta()` troca o ponteiro via
  `ArcSwap::store`. Teste de concorrência com `thread::scope` (1 thread
  publicando 200 deltas, outra lendo 500x em paralelo) confirma ausência de
  panic/deadlock — a garantia de "nunca ver delta parcial" é estrutural
  (`ArcSwap::load` sempre devolve um `Arc<CsrDelta>` inteiro), não algo que um
  teste de timing frágil precisasse provar. Gate: `cargo test graph::csr` →
  7/7 pass.
- 2026-09-28 T-107 completo. `graph::csr::participant::CsrParticipant`
  (`SyncParticipant` real): `stage()` clona o delta publicado atual +
  reaplica `EdgeMutation`s (idempotente); `commit()` publica via
  `Csr::publish_delta`, aciona `compact()` automaticamente quando
  `delta.len()/base.edge_count() >= 5%`; `compact()` funde base+delta,
  escreve em `.staging`, rename atômico, reabre mmap, zera delta via
  `Csr::replace_base`. Refino de design durante a implementação: `Csr::base`
  virou `ArcSwap<CsrBase>` (era campo simples + `&mut replace_base` no
  T-106) — necessário porque `SyncParticipant` exige `&self`, então a troca
  de base na compactação também precisa ser lock-free/COW, não só o delta.
  Não é SPEC_DEVIATION do REQ-106 (que só fala do delta) — é extensão natural
  do mesmo padrão para a base. Gate: `cargo test graph::csr` → 11/11 pass,
  incluindo compactação automática ao cruzar o threshold.
- 2026-09-28 T-108 completo. `graph::markdown::extract()`: single-pass sobre
  os blocos top-level do AST do `comrak` (headings, listas, parágrafos são
  todos irmãos no nível do documento em CommonMark — nenhuma travessia de
  siblings foi necessária). Reconhece `REQ-\d+` em itens de lista e
  `TASK-\d+`/`ADR-\d+` em headings; parágrafos logo após um heading
  TASK/ADR são escaneados por menções a REQs já vistos → edge `Satisfies`.
  Adicionada dependência `blake3` (REQ-102 exigia hash real, não usado até
  agora). Limitação conhecida, não bloqueante: referências forward (REQ
  citado antes de ser parseado) não geram edge — documentado no código, não
  exigido pelo critério de "Done" da task. Gate: `cargo test graph::markdown`
  → 2/2 pass.
- 2026-09-28 T-109 completo. `tests/graph_storage_integration.rs`:
  `markdown::extract()` sobre fixture real → `Coordinator::stage()` com
  `[RedbParticipant, CsrParticipant]` reais → nós recuperados via
  `RedbParticipant::get_node` (desserializados de volta a `NodePayload`) e
  edge `Satisfies` recuperada via `CsrParticipant::csr_handle().edges_from`.
  Refino de design: `CsrParticipant` passou a guardar `Arc<Csr>` (era `Csr`
  por valor) + método `csr_handle()` — necessário porque o participante é
  movido para dentro do `Coordinator` (`Box<dyn SyncParticipant>`), então
  quem quiser consultar o grafo depois precisa de um handle compartilhado
  guardado antes do move. Gate: suíte completa `cargo test` → 36/36 pass
  (31 unit + 1 integração nova + 4 crash-recovery da Fase 0).
- 2026-09-28 T-110 completo. `lib.rs` exporta `graph::{Csr, CsrParticipant,
  Edge, EdgeType, Node, NodeType}`. Corrigidos: doc-link privado
  (`CsrBaseData` não é pub — trocado por texto simples no doc comment),
  `clippy::unnecessary_sort_by` (`sort_by` → `sort_by_key`),
  `clippy::cloned_ref_to_slice_refs` (`&[x.clone()]` →
  `std::slice::from_ref(&x)`), import não usado no teste de integração.
  Gate: `cargo doc --no-deps` e `cargo clippy --all-targets -- -D warnings`
  limpos; `cargo test` → 36/36 pass.

## Feature "storage-primitives" (Fase 1): COMPLETA
Todas as 10 tasks (T-101..T-110) concluídas, 36/36 testes passando, `cargo
doc`/`cargo clippy -- -D warnings` limpos. `Node`/`Edge` tipados, CSR duas
camadas (base mmap + delta lock-free via `ArcSwap`, compactação automática
por threshold), extração de Markdown via `comrak`, tudo integrado ao
`sync::Coordinator` da Fase 0 como `SyncParticipant` real (contrato da Fase 0
validado na prática, sem precisar mudar a trait).

## Known Issues
- Os 3 subagentes em `.claude/agents/` (po.md, dev.md, qa.md) foram copiados de
  outro projeto (`condominium-management-system`) — texto interno ainda referencia
  esse nome. Não bloqueia o uso, mas vale corrigir/genericizar em algum momento
  (SPEC_DEVIATION leve, não urgente).

## Todos
- [x] T-001: Inicializar projeto Rust e dependências base — Execute
- [x] T-002: `MutationSet` e tipos de mutação (rkyv) — Execute [P-A]
- [x] T-003: WAL — frame format, append e leitura de pendências — Execute
- [x] T-004: `VersionPointer` — `sync_version` atômico em redb — Execute [P-A]
- [x] T-005: Trait `SyncParticipant` — Execute [P-A]
- [x] T-006: `RedbParticipant` — implementação real da trait — Execute
- [x] T-007: `Coordinator` — stage/commit/abort (fluxo feliz) — Execute
- [x] T-008: `Coordinator::resume()` — recovery determinístico — Execute
- [x] T-009: Testes de injeção de crash (integração) — Execute
- [x] T-010: Superfície pública e lint final — Execute

## Todos
- [x] T-101: Dependências novas (memmap2/arc-swap/zstd/comrak) — Execute
- [x] T-102: Tipos `Node`/`NodeType`/`NodePayload` — Execute [P-A]
- [x] T-103: Tipos `Edge`/`EdgeType` — Execute [P-A]
- [x] T-104: `CsrBase` — layout binário imutável — Execute
- [x] T-105: `CsrDelta` — estrutura append-only — Execute
- [x] T-106: `Csr` — fachada lock-free — Execute
- [x] T-107: `CsrParticipant` — implementação de `SyncParticipant` — Execute
- [x] T-108: `markdown::extract` — parser comrak — Execute
- [x] T-109: Integração fim-a-fim via `Coordinator` — Execute
- [x] T-110: Lint e superfície pública — Execute

## Progress [window: last 10]
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

## Decisions [window: last 10]
- 2026-09-28 Feature "git-integration" (Fase 2) especificada, desenhada e
  quebrada em tasks: `.specs/features/git-integration/{spec,design,tasks}.md`.
  REQ count: 8 (REQ-201..208). Escopo `Complex`. Decisões: cobertura de
  submodules/LFS/sparse-checkout adiada (fora de escopo, registrado como
  débito técnico, não bloqueio); `SyncOrchestrator` vive fora de `sync::`/
  `graph::` (primeiro componente que depende dos dois); co-change vira
  `EdgeType::CoChanges` reaproveitando o CSR da Fase 1 (não um novo tipo de
  store); `last_indexed_commit` reaproveita a tabela `meta` do
  `VersionPointer` (não um store novo). 9 tasks (T-201..T-209), onda [P-A]
  em T-204/T-205/T-206.

## Todos
- [x] T-201: Dependência `gix` e fixtures de teste — Execute
- [x] T-202: `GitSource` — abrir repositório e ler HEAD — Execute
- [x] T-203: `TreeDiff` — diff de árvore desde `last_indexed_commit` — Execute
- [x] T-204: `DirtyCache` — mudanças não commitadas — Execute [P-A]
- [x] T-205: `EdgeType::CoChanges` + `co_change_edges` — Execute [P-A]
- [x] T-206: `extract_commit_links` — linking temporal — Execute [P-A]
- [ ] T-207: `SyncOrchestrator::run_once()` — integração — Execute
- [ ] T-208: Testes de estados não-triviais do repositório — Execute
- [ ] T-209: Lint e superfície pública — Execute

## Progress [window: last 10]
- 2026-09-28 T-201 completo. `cargo add gix` (defaults — footprint maior que
  o resto do projeto, ~125 crates transitivos incluindo suporte a rede/
  credenciais que não usamos; candidato a `default-features = false` +
  seleção fina depois, não bloqueante agora). `tests/fixtures/mod.rs`:
  `FixtureRepo` (shell para `git` real só em `tests/`, nunca em `src/`, por
  isolamento e simplicidade — criar commits via API de escrita do `gix`
  diretamente seria mais "puro" mas bem mais código). Smoke test em
  `tests/fixture_smoke.rs`. Gate: `cargo build` → sucesso;
  `cargo test --test fixture_smoke` → 1/1 pass.

- 2026-09-28 T-202 completo. `git::source::GitSource` (`open`,
  `head_commit_oid`, `is_dirty`) — API estreita sobre `gix::Repository`.
  OIDs representados como `[u8;20]` (SHA-1); repo SHA-256 falharia com erro
  explícito (`GitError::UnsupportedHash`), não truncamento silencioso —
  limitação documentada, não tratada agora. Ajuste de escopo do gate: os
  testes ficaram em `tests/git_source.rs` (integração, via `FixtureRepo`) em
  vez de unit tests `#[cfg(test)]` dentro de `src/git/source.rs` como o
  `tasks.md` sugeria (`cargo test git::source`) — testes de `GitSource`
  precisam de repositórios Git reais, que só o helper de `tests/fixtures`
  fornece; unit tests dentro do crate não têm acesso a esse helper (crates
  de teste de integração são compilados separadamente). Gate ajustado:
  `cargo test --test git_source` → 3/3 pass, cobrindo HEAD em branch, HEAD
  destacado e detecção de dirty.

- 2026-09-28 T-203 completo. `GitSource::diff_since(Option<[u8;20]>)`:
  `None` → todos os blobs de `HEAD` via `tree.traverse().breadthfirst.files()`
  como `Added`; `Some(oid)` → `tree.changes().for_each_to_obtain_tree()` do
  `gix`, com `track_rewrites(None)` explícito (rename vira Delete+Add, que é
  exatamente o modelo remove/re-extract que REQ-205 espera — sem isso, o
  `gix` ativa detecção de rename por padrão e um arquivo renomeado
  desapareceria do diff). `VersionPointer` ganhou `last_indexed_commit()`/
  `set_last_indexed_commit()` numa tabela `redb` nova (`meta_bytes`, valor
  `&[u8]`) — a tabela `meta` original só guarda `u64`, não serve para OID de
  20 bytes. Gate: `cargo test --test git_tree_diff` → 2/2,
  `cargo test sync::version` → 4/4.

- 2026-09-28 T-204/T-205 completos (onda [P-A]). `git::dirty_cache::DirtyCache`
  (HashMap em memória `PathBuf -> Blake3`, `scan()` retorna paths mudados,
  primeira leitura conta tudo como "mudado"). `EdgeType::CoChanges` (código 4)
  + `GitSource::co_change_edges()` — implementado como **método de
  `GitSource`** (não função livre `co_change_edges(&GitSource, ...)` como o
  `tasks.md` esboçou) para manter o campo `repo: gix::Repository` privado e a
  fronteira "todo gix passa por GitSource" (REQ-201) intacta; exigiu tornar
  `repo` e `op_err` `pub(crate)` para outros módulos de `git::` acessarem.
  Janela por commits via `head_id().ancestors().all()`, corte por idade via
  `commit.time()?.seconds`, walk é newest-first então corta assim que um
  commit fica velho demais. Gate: `cargo test git::` → 5/5 pass.

- 2026-09-28 T-206 completo (fecha a onda [P-A]). `git::spec_link::
  extract_commit_links(message) -> Vec<String>` reaproveita
  `graph::markdown::find_markers` (agora `pub(crate)`) — mesma sintaxe de
  marcador reconhecida em specs e em mensagens de commit.
  `GitSource::commits_since(Option<[u8;20]>) -> Vec<CommitInfo>` (oid,
  message, author, timestamp), `since` exclusivo (para no commit indicado,
  sem incluí-lo). Gate: `cargo test git::` (unit) → 7/7,
  `cargo test --test git_spec_link` → 1/1.

## Next Steps
- T-207 (`SyncOrchestrator::run_once()`) — integra tudo (T-203..T-206) com
  o `Coordinator` da Fase 0. Depois: T-208 (estados não-triviais) e T-209
  (lint/superfície pública, fecha a Fase 2).

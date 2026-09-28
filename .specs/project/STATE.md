# STATE

## Degraded Mode
- Repositório novo, sem código-fonte ainda (`.git` só tem o commit inicial).
  `graphify`/`specdb` não têm o que indexar de verdade.
- Grafo NÃO construído — `.specs/graph/graph.json` não existe. Rule #1 da skill
  segue em modo degradado: contexto lido diretamente dos arquivos.
- Reindexar assim que houver código Rust suficiente (fim da Fase 1 do roadmap) ou
  quando um `specdb`/`graphify` funcional estiver instalável.

## Progress [window: last 10]
- 2026-09-28 — Sessão iniciada. Skill `graph-spec-design` invocada pela primeira vez
  neste repo. Lido `.defs/SpecDB.md` (doc de arquitetura gerado com Gemini) e os
  3 subagentes em `.claude/agents/` (po, dev, qa).
- 2026-09-28 — Criada estrutura `.specs/` (project, codebase, features, quick, graph).
- 2026-09-28 — `PROJECT.md` e `ROADMAP.md` escritos a partir de `.defs/SpecDB.md`.
- 2026-09-28 — `.defs/SpecDB.md` foi reiterado externamente (outras IAs). Revisão
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
- `.defs/SpecDB.md` é tratado como documento de referência (gerado externamente
  com Gemini), não editado diretamente — decisões relevantes são promovidas para
  `.specs/`. Motivo: preservar a fonte original enquanto specs ficam sob controle
  da skill.
- Projeto roda em modo degradado até existir código suficiente para indexar.
  Motivo: grafo vazio não agrega valor sobre leitura direta de arquivo.

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
- [ ] T-104: `CsrBase` — layout binário imutável — Execute
- [ ] T-105: `CsrDelta` — estrutura append-only — Execute
- [ ] T-106: `Csr` — fachada lock-free — Execute
- [ ] T-107: `CsrParticipant` — implementação de `SyncParticipant` — Execute
- [ ] T-108: `markdown::extract` — parser comrak — Execute
- [ ] T-109: Integração fim-a-fim via `Coordinator` — Execute
- [ ] T-110: Lint e superfície pública — Execute

## Progress [window: last 10]
- 2026-09-28 T-101 completo. `cargo add memmap2 arc-swap zstd comrak`. Ajuste:
  `comrak` com `default-features = false` — as features default incluíam CLI/
  syntect/bon (~90 crates transitivos) irrelevantes para só parsear Markdown.
  Gate: `cargo build` → sucesso.
- 2026-09-28 T-102/T-103 completos. `graph::node::{Node, NodeType,
  NodePayload}` (6 variantes) e `graph::edge::{Edge, EdgeType}` — reaproveitam
  `sync::mutation::StableId`. Gate: `cargo test graph::` → 3/3 pass.

## Next Steps
- Rodar Execute em T-102/T-103 em paralelo [P-A] em seguida.

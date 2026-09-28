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

## Decisions [window: last 10]
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
- [ ] T-003: WAL — frame format, append e leitura de pendências — Execute
- [x] T-004: `VersionPointer` — `sync_version` atômico em redb — Execute [P-A]
- [x] T-005: Trait `SyncParticipant` — Execute [P-A]
- [ ] T-006: `RedbParticipant` — implementação real da trait — Execute
- [ ] T-007: `Coordinator` — stage/commit/abort (fluxo feliz) — Execute
- [ ] T-008: `Coordinator::resume()` — recovery determinístico — Execute
- [ ] T-009: Testes de injeção de crash (integração) — Execute
- [ ] T-010: Superfície pública e lint final — Execute

## Next Steps
- Rodar Execute (`/graph-spec-design` "implement") começando por T-001; T-002,
  T-004 e T-005 podem ser feitos em paralelo (onda [P-A]) logo em seguida.

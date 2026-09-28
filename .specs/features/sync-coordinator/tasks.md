# Tasks: Sync Coordinator & Transactional Integrity (Fase 0)

## T-001: Inicializar projeto Rust e dependências base [x]
- **REQ**: (infra — pré-requisito de todos os REQs)
- **Graph node**: n/a (projeto greenfield, sem grafo ainda)
- **What**: `cargo init` na raiz do repo; criar `src/lib.rs`, `src/sync/mod.rs`
  vazio; adicionar dependências iniciais ao `Cargo.toml`: `redb`, `rkyv`,
  `crc32fast`, `thiserror`.
- **Where**: `Cargo.toml`, `src/main.rs` ou `src/lib.rs`, `src/sync/mod.rs`
- **Depends on**: none
- **Done when**: `cargo build` compila um crate vazio com as deps resolvidas.
- **Gate**: `cargo build`

## T-002: `MutationSet` e tipos de mutação (rkyv) [x]
- **REQ**: REQ-001
- **Graph node**: n/a
- **What**: Definir `NodeMutation`, `EdgeMutation`, `DocMutation` (enums
  insert/update/remove) e `MutationSet { nodes, edges, docs }`, derivando
  `rkyv::Archive`/`Serialize`/`Deserialize`. Incluir roundtrip test
  (serializa→desserializa→compara).
- **Where**: `src/sync/mutation.rs`
- **Depends on**: T-001
- **[P]**: A (paralelizável com T-004, T-005)
- **Done when**: roundtrip de serialização preserva os dados exatamente.
- **Gate**: `cargo test sync::mutation`

## T-003: WAL — frame format, append e leitura de pendências [x]
- **REQ**: REQ-001, REQ-004
- **Graph node**: n/a
- **What**: Implementar `Wal` com `append_frame(target_version, &MutationSet)`
  (length-prefixed + rkyv + crc32, `fsync` após escrita), `pending_frames()`
  (frames sem commit-marker), `mark_done(target_version)` (escreve
  commit-marker). Frame com crc32 inválido no final do arquivo é descartado
  silenciosamente no replay (tratado como escrita incompleta).
- **Where**: `src/sync/wal.rs`
- **Depends on**: T-002
- **Done when**: teste cobre (a) append+read normal, (b) frame truncado no fim é
  ignorado, (c) `mark_done` faz `pending_frames` parar de retornar aquele frame.
- **Gate**: `cargo test sync::wal`

## T-004: `VersionPointer` — `sync_version` atômico em redb [x]
- **REQ**: REQ-002
- **Graph node**: n/a
- **What**: Tabela `redb` `meta` (chave `"sync_version"` → `u64`). API
  `current(&self) -> u64` (default 0 se ausente) e `bump(&self, new: u64)`
  dentro de uma transação `redb` única.
- **Where**: `src/sync/version.rs`
- **Depends on**: T-001
- **[P]**: A (paralelizável com T-002, T-005)
- **Done when**: `bump` seguido de `current` reflete o novo valor; ausência de
  chave retorna 0.
- **Gate**: `cargo test sync::version`

## T-005: Trait `SyncParticipant` [x]
- **REQ**: REQ-003, REQ-005, REQ-006
- **Graph node**: n/a
- **What**: Definir trait com `stage(&self, target_version: u64, mutations:
  &MutationSet) -> Result<()>`, `committed_version(&self) -> u64`,
  `commit(&self, target_version: u64) -> Result<()>`, `abort(&self,
  target_version: u64) -> Result<()>`. Documentar contrato de idempotência
  (REQ-005) no doc comment — chamadas repetidas com o mesmo `target_version`
  não duplicam efeito. Marcar explicitamente como API não-estável (ver
  `design.md` → Risks) até a Fase 1 validar.
- **Where**: `src/sync/participant.rs`
- **Depends on**: T-001
- **[P]**: A (paralelizável com T-002, T-004)
- **Done when**: trait compila e há um mock `TestParticipant` em
  `#[cfg(test)]` usado nos testes das tasks seguintes.
- **Gate**: `cargo build`

## T-006: `RedbParticipant` — implementação real da trait [x]
- **REQ**: REQ-003, REQ-005
- **Graph node**: n/a
- **What**: Implementar `SyncParticipant` para `redb`: `stage()` grava em
  chaves de staging (prefixo `staging::`); `commit()` promove via transação
  `redb` (copia staging → chave real, versão real, remove staging); `abort()`
  limpa as chaves de staging sem tocar o estado committed. `stage()` chamado
  duas vezes com o mesmo `target_version` é no-op na segunda vez.
- **Where**: `src/sync/redb_participant.rs`
- **Depends on**: T-004, T-005
- **Done when**: teste cobre stage→commit (dado visível), stage→abort (dado
  descartado), stage→stage→commit (idempotência, sem duplicar).
- **Gate**: `cargo test sync::redb_participant`

## T-007: `Coordinator` — `stage()`/`commit()`/`abort()` (fluxo feliz) [x]
- **REQ**: REQ-001, REQ-003, REQ-006, REQ-007
- **Graph node**: n/a
- **What**: `Coordinator::new(wal, version_pointer, participants:
  Vec<Box<dyn SyncParticipant>>)`. `stage(mutations)`: grava frame no WAL
  (fsync) **antes** de chamar `stage()` em cada participante; se todos
  confirmarem, chama `commit()` em cada um e depois `version.bump()` e
  `wal.mark_done()`; se algum falhar, chama `abort()` em todos os que já
  fizeram `stage()`. Teste com `TestParticipant` (mock de T-005) simulando
  sucesso e falha de um participante no meio do fan-out.
- **Where**: `src/sync/coordinator.rs`
- **Depends on**: T-003, T-006
- **Done when**: fluxo feliz (todos ok) resulta em `sync_version` incrementado
  e WAL marcado done; falha de 1 participante resulta em abort de todos, sem
  bump de versão.
- **Gate**: `cargo test sync::coordinator::commit_flow`

## T-008: `Coordinator::resume()` — recovery determinístico [x]
- **REQ**: REQ-004, REQ-005
- **Graph node**: n/a
- **What**: No startup, ler `wal.pending_frames()`; para cada frame pendente,
  comparar `participant.committed_version()` com `target_version` de cada
  participante e decidir por participante: replay (`stage`+`commit`) se
  atrasado, no-op se já no alvo. Se todos já no alvo mas `sync_version` global
  não bumpado, apenas bumpar e marcar WAL done. Implementar exatamente a
  árvore de decisão descrita em `design.md` → "Resume — semântica
  determinística".
- **Where**: `src/sync/coordinator.rs` (extensão)
- **Depends on**: T-007
- **Done when**: 3 cenários de resume passam (nenhum participante aplicou /
  parcialmente aplicado / todos aplicados mas versão não bumpada).
- **Gate**: `cargo test sync::coordinator::resume`

## T-009: Testes de injeção de crash (integração)
- **REQ**: REQ-004, REQ-005, REQ-007
- **Graph node**: n/a
- **What**: Suíte de integração que simula crash nos 4 pontos do diagrama de
  `design.md` (antes do fsync do WAL, depois do WAL mas antes do fan-out, no
  meio do fan-out, depois do fan-out mas antes do bump de versão) e verifica
  que `resume()` sempre converge para um estado consistente — nunca deixa
  `sync_version` refletir um subconjunto parcial das mutações.
- **Where**: `tests/sync_crash_recovery.rs`
- **Depends on**: T-008
- **Done when**: os 4 cenários de crash passam sem estado parcial observável.
- **Gate**: `cargo test --test sync_crash_recovery`

## T-010: Superfície pública e lint final
- **REQ**: REQ-006
- **Graph node**: n/a
- **What**: Exportar `sync::Coordinator`, `sync::SyncParticipant`,
  `sync::MutationSet` de `src/lib.rs`; manter `RedbParticipant`/internals como
  `pub(crate)` (só o coordinator e testes tocam neles diretamente — reforça
  REQ-006 no nível de visibilidade do compilador). Doc comments em toda API
  pública.
- **Where**: `src/lib.rs`, `src/sync/mod.rs`
- **Depends on**: T-007
- **Done when**: `cargo doc` gera sem warnings e `cargo clippy` não acusa
  `missing_docs`/visibilidade indevida.
- **Gate**: `cargo doc --no-deps && cargo clippy -- -D warnings`

# Design: Sync Coordinator & Transactional Integrity (Fase 0)

## Architecture Overview

Projeto greenfield — não há grafo/código pré-existente para traçar caminhos
(`graphify path`/`explain` não aplicáveis, ver `.specs/project/STATE.md`, modo
degradado). O design abaixo é a primeira decisão estrutural real do codebase.

```
                    ┌─────────────────────────┐
   mutation set  →  │   sync::coordinator      │
   (nós/edges/docs)  │   stage() -> commit()/   │
                    │   abort()                │
                    └───────────┬──────────────┘
                                │ 1. write intent
                                ▼
                    ┌─────────────────────────┐
                    │  sync::wal (sync.wal)    │  ← frame rkyv, fsync antes
                    └───────────┬──────────────┘     de tocar participants
                                │ 2. fan-out stage()
              ┌─────────────────┼─────────────────┐
              ▼                 ▼                 ▼        (Fase 1/3/4:
     ┌────────────────┐ ┌──────────────┐ ┌───────────────┐  CSR / Tantivy / HNSW
     │ SyncParticipant │ │ SyncParticipant│ │ SyncParticipant│  implementam esta
     │  (redb, único   │ │  (futuro: CSR)│ │ (futuro: ...) │  trait quando suas
     │  participante   │ │               │ │               │  fases chegarem)
     │  real na Fase 0)│ │               │ │               │
     └────────┬────────┘ └───────┬───────┘ └───────┬───────┘
              │ 3. staging + rename atômico, cada um reporta sua própria versão
              └─────────────────┴─────────────────┘
                                │ 4. todos confirmaram target_version?
                                ▼
                    ┌─────────────────────────┐
                    │ redb: sync_version = N   │  ← só aqui a versão N
                    │ (bump final, atômico)    │     fica "visível" p/ leitores
                    └─────────────────────────┘
```

**Invariante central:** um leitor só confia em dados na versão
`redb.sync_version` ou anterior. Um participante pode estar fisicamente "à
frente" (já promoveu seu arquivo para a versão N) antes do bump final — isso é
seguro porque nenhum leitor consulta o participante diretamente sem checar
`sync_version` primeiro; o bump é o que torna N "real".

## Dependency Paths

- REQ-001 (WAL antes de tocar stores) → `sync::wal::append_frame()` chamado
  dentro de `sync::coordinator::stage()`, antes de qualquer `SyncParticipant`
  ser invocado.
- REQ-002 (version pointer) → tabela `redb` `meta` (chave `"sync_version"`),
  lida por `sync::version::current()`, escrita apenas por
  `sync::coordinator::commit()`.
- REQ-003 (staging + rename) → responsabilidade de cada implementação de
  `SyncParticipant::stage()`; o coordinator não sabe o formato interno de cada
  store, só chama a trait.
- REQ-004 (crash recovery) → `sync::wal::pending_frames()` no startup +
  `sync::coordinator::resume()`.
- REQ-005 (idempotência) → contrato da trait `SyncParticipant`: `stage()` deve
  ser seguro de chamar múltiplas vezes com o mesmo `target_version` sem efeito
  duplicado (participantes comparam `target_version` contra sua própria versão
  interna antes de aplicar).
- REQ-006 (API única) → módulo `sync` é o único ponto de escrita; enforced por
  convenção/visibilidade de módulo agora (não há outros módulos ainda), e por
  revisão de design nas Fases 1/3/4 quando CSR/Tantivy/HNSW forem implementados.
- REQ-007 (atomicidade observável) → decorre de REQ-002: leitura sempre passa
  por `sync::version::current()` antes de acessar qualquer store.

## New Components

| Component | Responsibility | Location |
|---|---|---|
| `SyncParticipant` (trait) | Contrato `fn stage(&self, target_version: u64, mutations: &MutationSet) -> Result<()>`, `fn committed_version(&self) -> u64`, `fn commit(&self, target_version: u64) -> Result<()>`, `fn abort(&self, target_version: u64) -> Result<()>` | `src/sync/participant.rs` |
| `RedbParticipant` | Único `SyncParticipant` real na Fase 0 — grava metadados em staging keys, promove via transação `redb` | `src/sync/redb_participant.rs` |
| `Wal` | Append-only frame log (`sync.wal`): `append_frame`, `pending_frames`, `mark_done` | `src/sync/wal.rs` |
| `MutationSet` | Struct serializável (rkyv) representando o lote de mutações de um ciclo (`Vec<NodeMutation>`, `Vec<EdgeMutation>`, `Vec<DocMutation>`) | `src/sync/mutation.rs` |
| `Coordinator` | Orquestra `stage()` → fan-out para participantes → `commit()`/`abort()`, expõe `resume()` para o startup | `src/sync/coordinator.rs` |
| `VersionPointer` | Wrapper de leitura/escrita do `sync_version` em `redb` (tabela `meta`) | `src/sync/version.rs` |

## Modified Components

Nenhum — greenfield, sem componentes pré-existentes.

## WAL — formato de frame

Cada frame no `sync.wal` é length-prefixed, serializado com `rkyv` (consistente
com a stack já escolhida para o CSR):

```
[u32 frame_len][u64 target_version][rkyv(MutationSet) bytes][u32 crc32]
```

- `fsync` do arquivo do WAL acontece **antes** de qualquer chamada a
  `SyncParticipant::stage()` — é isso que torna a intenção "durável" mesmo se o
  processo morrer no meio do fan-out.
- `crc32` detecta frame truncado (crash durante o próprio `append_frame`); um
  frame corrompido no final do arquivo é descartado no replay (nunca aplicado
  parcialmente), tratado como se o `append_frame` nunca tivesse terminado.
- `mark_done(target_version)` não apaga o frame fisicamente (append-only) — grava
  um frame de tipo "commit-marker" no final do WAL. O WAL é truncado/rotacionado
  só em manutenção explícita (fora de escopo desta fase), não durante operação
  normal.

## Resume — semântica determinística

No startup, `Coordinator::resume()`:

1. Lê `redb.sync_version` (última versão confirmada globalmente).
2. Lê `wal::pending_frames()` — frames com `target_version >
   redb.sync_version` sem commit-marker correspondente.
3. Para cada frame pendente (deve haver no máximo 1 em operação normal — um
   ciclo de sync por vez):
   - Para cada `SyncParticipant` registrado: se `participant.committed_version()
     < target_version`, chama `stage()` de novo com o mesmo `MutationSet` do
     frame (idempotente por REQ-005) e depois `commit(target_version)`.
   - Se **todos** os participantes já estão em `target_version` mas
     `redb.sync_version` ainda não foi bumpado, o crash aconteceu entre o passo
     3 e 4 do diagrama — resume só precisa bumpar `sync_version` e escrever o
     commit-marker.
   - Se **nenhum** participante aplicou nada ainda, resume reaplica do zero em
     todos.
4. Nunca existe estado intermediário observável — resume só termina quando
   `sync_version` reflete exatamente os participantes, ou quando decide
   `abort()` (ex.: frame corrompido pelo crc32 check) e loga a decisão.

## Risks

- **Acoplamento obrigatório:** `sync::coordinator` vira, por design, uma
  dependência transitiva de todo módulo de escrita nas Fases 1/3/4 (CSR,
  Tantivy, HNSW todos implementam `SyncParticipant`). Não é um "God Node" de
  lógica de negócio, mas é um ponto único de falha estrutural — um bug aqui
  quebra sync para todos os stores simultaneamente. Mitigação: escopo mínimo
  nesta fase (só a trait + `redb` real), superfície pequena, testada
  isoladamente antes das fases seguintes existirem.
- **Contrato da trait definido cedo demais:** `SyncParticipant` é desenhado
  antes de CSR/Tantivy/HNSW existirem (Fases 1/3/4), então há risco de o
  contrato precisar mudar quando esses stores forem implementados de verdade
  (ex.: descobrir que `stage()` precisa de mais contexto). Mitigação: tratar a
  trait como **não estável** até a Fase 1 (primeiro consumidor real) validar o
  contrato na prática; revisar antes de "congelar".
- **Sem grafo/GRAPH_REPORT.md ainda** — não há God Nodes/comunidades de baixa
  coesão para checar (nenhum código existe). Este risco desaparece
  automaticamente assim que a Fase 3+ gerar o primeiro grafo real do próprio
  NexSpec.

## Decision Log

- WAL serializado com `rkyv` (não JSON/bincode) — consistência com a escolha já
  feita para o CSR em `.specs/codebase/STACK.md`; evita introduzir uma segunda
  lib de serialização só para esta fase.
- `SyncParticipant` como trait genérica (não um enum fechado de stores) — decisão
  para permitir que Fases 1/3/4 implementem sem precisar tocar o módulo `sync`
  outra vez; custo é a superfície de trait precisar acomodar necessidades ainda
  não conhecidas (ver Risks acima).
- Granularidade de commit: **todo o ciclo de sync é uma unidade** (não por
  nó/edge individual) — alinhado com REQ-007 e com o modelo de Tree-Diff
  Incremental Sync da Fase 2 (um `nexspec sync` = um `git diff` = uma versão).
- WAL é append-only com commit-markers em vez de truncar/reescrever — mais
  simples de raciocinar sobre crash-safety (nunca há um "meio de escrita" que
  sobrescreve dado válido); custo de espaço em disco é aceitável dado o volume
  esperado (um repo local, não um sistema de alta frequência de escrita).

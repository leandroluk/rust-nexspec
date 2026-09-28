# Design: Storage Primitives & Graph Topology (Fase 1)

## Architecture Overview

```
                    .specs/**/*.md
                          │
                    [ comrak parser ]
                          │
                     MutationSet (nodes: Requirement/Task/Adr/DocSection,
                                   edges: Satisfies/DependsOn/...)
                          │
                          ▼
              sync::coordinator::Coordinator::stage()
                          │  (fan-out — Fase 0, inalterado)
           ┌──────────────┼───────────────────┐
           ▼                                   ▼
  ┌─────────────────────┐          ┌──────────────────────────┐
  │ RedbParticipant       │          │ CsrParticipant (novo)     │
  │ (Fase 0 — nós/docs em │          │ topologia (edges) — CSR   │
  │ redb::nodes/docs)     │          │ base + delta              │
  └─────────────────────┘          └──────────┬───────────────┘
                                               │
                                    ┌──────────┴───────────┐
                                    ▼                       ▼
                          ┌──────────────────┐   ┌────────────────────┐
                          │ Base layer        │   │ Delta layer         │
                          │ edges.bin (rkyv,  │   │ ArcSwap<DeltaSet>    │
                          │ mmap, imutável    │   │ (COW, lock-free)     │
                          │ entre compactações)│   │ append-only até     │
                          └──────────────────┘   │ threshold → compact │
                                                  └────────────────────┘
```

Leitura de grafo (`query`/`path`/`explain`, fora do escopo desta fase mas o
formato já precisa suportar) sempre mescla base + `delta.load()` (snapshot
atômico via `ArcSwap`) — nunca lê o delta "ao vivo" sendo mutado.

## Dependency Paths

- REQ-101/102/103 → `graph::node::{Node, NodeType, NodePayload}`,
  `graph::edge::{Edge, EdgeType}` — tipos puros, sem I/O.
- REQ-104 → `graph::node_store::NodeStore`, que reaproveita o padrão de
  `sync::redb_participant::RedbParticipant` (Fase 0) — na prática, os nós
  passam a ser gravados nas tabelas `redb_nodes`/`redb_docs` que
  `RedbParticipant` **já implementa desde a Fase 0**; esta fase não recria
  esse participante, só passa a alimentá-lo com `NodeMutation`/`DocMutation`
  reais (antes só existiam mutations genéricas de teste).
- REQ-105/106/107 → `graph::csr::{CsrBase, CsrDelta, CsrParticipant}` (novo
  módulo `src/graph/csr/`).
- REQ-108 → `graph::markdown::extract()` (novo módulo `src/graph/markdown.rs`),
  produz `MutationSet` a partir de arquivos `.md`.
- REQ-109 → `CsrParticipant` implementa `sync::SyncParticipant`; o extrator de
  Markdown **não** implementa a trait diretamente — ele é um produtor de
  `MutationSet` que é passado para `Coordinator::stage()` por quem o chama
  (o "quem" de fora — Fase 2/6 — decide quando rodar a extração; Fase 1 só
  entrega a função pura `extract()` e o participante `CsrParticipant`).

## New Components

| Component | Responsibility | Location |
|---|---|---|
| `Node`, `NodeType`, `NodePayload` | Entidade de nó tipada + payload por tipo (enum) | `src/graph/node.rs` |
| `Edge`, `EdgeType` | Entidade de edge tipada | `src/graph/edge.rs` |
| `CsrBase` | Arquivo mmap imutável (`edges.bin`, rkyv), lookups O(1) por direção/tipo | `src/graph/csr/base.rs` |
| `CsrDelta` | Estrutura append-only de edges pendentes, publicada via `ArcSwap<CsrDelta>` | `src/graph/csr/delta.rs` |
| `CsrParticipant` | Implementa `SyncParticipant`; `stage()` monta o próximo `CsrDelta` off-thread, `commit()` troca o `ArcSwap`; aciona compactação por threshold | `src/graph/csr/participant.rs` |
| `Csr` (fachada) | Combina `CsrBase` + `ArcSwap<CsrDelta>`; expõe `edges_from(id, edge_type)` mesclando as duas camadas | `src/graph/csr/mod.rs` |
| `markdown::extract` | Parseia `.specs/**/*.md` com `comrak`, retorna `MutationSet` (REQ/TASK/ADR/DocSection + edges de relação) | `src/graph/markdown.rs` |

## Modified Components

| Component | Change | Risk |
|---|---|---|
| `sync::participant::SyncParticipant` | Nenhuma mudança de assinatura esperada — `CsrParticipant` é o primeiro teste real do contrato desenhado na Fase 0 | Se o contrato não bastar (ex.: `stage()` precisar de mais contexto que `target_version`+`MutationSet`), a trait muda aqui — documentado como risco aceito desde `sync-coordinator/design.md` |
| `sync::redb_participant::RedbParticipant` | Nenhuma mudança de código — passa a receber `NodeMutation`/`DocMutation` com payloads reais (antes só bytes de teste) | Baixo — já foi desenhado para bytes opacos |

## Risks

- **Validação do contrato `SyncParticipant` sob carga real**: até agora só
  `RedbParticipant` (Fase 0) e mocks o implementam. `CsrParticipant` precisa
  de estado extra que os outros não têm (o `ArcSwap<CsrDelta>` em memória, que
  não é "reconstruível" só a partir do que está em disco entre `stage()` e
  `commit()` — precisa viver no processo inteiro). Mitigação: `CsrParticipant`
  guarda o `ArcSwap` como campo próprio (não como parte do storage do
  coordinator), e `committed_version()` reflete o que está persistido em
  `edges.bin` + o que o delta em memória já processou — se a trait não
  expressar isso bem, é o ponto onde ela precisa ser revisada (aceito desde a
  Fase 0).
- **Concorrência leitor/escritor no `ArcSwap`**: um `Coordinator::resume()`
  (crash recovery) pode reconstruir o delta a partir do WAL da Fase 0
  enquanto, teoricamente, uma query já está rodando. Mitigação: `resume()` só
  roda no startup, antes de qualquer servidor/CLI aceitar queries (invariante
  de processo, não de código — documentar isso explicitamente na Fase 6
  quando o servidor existir).
- **Compactação e mmap**: recriar `edges.bin` enquanto ele está mmapeado por
  leitores é um risco clássico de plataforma (Windows não permite reescrever
  um arquivo mmapeado da mesma forma que Unix). Mitigação: compactação escreve
  em `edges.bin.staging` (já é o padrão do coordinator — REQ-003 da Fase 0) e
  o mmap é *remapeado* (não sobrescrito in-place) depois do rename atômico —
  leitores existentes continuam com o mmap antigo válido até religarem.
- **Sem grafo/`GRAPH_REPORT.md`** — mesma situação da Fase 0, ainda não há
  código suficiente para um grafo real de si mesmo.

## Decision Log

- Delta layer cobre só **edges**, não nós — nós continuam em `redb` via
  `RedbParticipant` já existente (ver Open Questions do spec.md). Evita
  duplicar a responsabilidade transacional da Fase 0.
- `NodePayload` como enum único serializado via `rkyv` (não um formato por
  tipo) — mais simples de versionar e consistente com CSR/WAL.
- `ArcSwap` escolhido sobre `crossbeam-epoch` para o delta — API mais simples;
  revisar só se profiling exigir.
- Extração de Markdown é uma função pura (`extract() -> MutationSet`), não um
  `SyncParticipant` — quem decide *quando* rodar extração e chamar
  `Coordinator::stage()` com o resultado é responsabilidade de fases
  posteriores (Git sync na Fase 2 decide o que mudou; CLI/servidor na Fase 6
  decide o gatilho). Mantém esta fase focada em primitivas, sem orquestração.

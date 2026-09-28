# Spec: Storage Primitives & Graph Topology (Fase 1)

## Summary

Fase 1 dá ao SpecDB o modelo de dados e o motor de grafo que tudo mais (Git
sync, AST, busca híbrida, serialização de contexto) vai consumir: entidades
tipadas (`Node`, `Edge`), um armazenamento de metadados transacional (`redb`,
já com `sync_version` da Fase 0), e uma topologia CSR de duas camadas
(base imutável mmap + delta lock-free) para navegação O(1) do grafo. Também
entra aqui a extração de `.specs/*.md` (REQ/TASK/ADR) via `comrak`, que é o
que conecta specs a código — a peça central da proposta do SpecDB.
Todo módulo desta fase escreve exclusivamente através do
`sync::coordinator` (Fase 0): CSR e o extrator de Markdown se tornam
implementações de `SyncParticipant`.

## Requirements

- REQ-101: Definir `NodeType`/`EdgeType` (enums) e as entidades de domínio que
  eles cobrem: `Requirement`, `Task`, `Adr`, `DocSection`, `Symbol`, `File`
  (nós) e `Satisfies`, `DependsOn`, `Implements`, `DefinedIn` (tipos de edge).
- REQ-102: Todo `Node`/`Edge` tem um **ID estável** — Blake3 hash de 32 bytes
  do seu conteúdo canônico — reaproveitando o tipo `sync::mutation::StableId`
  já definido na Fase 0. IDs estáveis nunca mudam por causa de compactação ou
  renumeração física.
- REQ-103: Um **índice físico denso `u32`** é atribuído a cada nó apenas no
  momento de build/compactação do CSR — é puramente um detalhe de
  implementação do layout mmap, nunca referenciado por specs, edges em
  trânsito ou pelo WAL da Fase 0 (que já usa `StableId`).
- REQ-104: Metadados de nó (`NodePayload`: tipo, campos específicos do
  domínio, hash do conteúdo-fonte) são persistidos em `redb`
  (`stable_id -> NodePayload`), com payload comprimido via `zstd`.
- REQ-105: O CSR tem **duas camadas**: uma base imutável em arquivo binário
  mmap (`rkyv`, `.specs/.index/edges.bin`) com lookups O(1) por direção
  (`Satisfies`, `DependsOn`, `Implements`, `DefinedIn`), e um delta
  append-only para adições/remoções desde a última compactação. Queries
  mesclam base + delta de forma transparente.
- REQ-106: O delta é **lock-free**: publicado via `ArcSwap` (ou
  `crossbeam-epoch`), Copy-on-Write. Um leitor nunca bloqueia por causa de um
  `sync` rodando em background, nem observa um delta parcial/rasgado —
  escritores constroem a próxima versão do delta off-thread e trocam o
  ponteiro atomicamente só quando pronta.
- REQ-107: Compactação (fold do delta na base, gerando novo arquivo base) é
  automática quando o delta excede um threshold configurável (default: 5% do
  total de edges da base), ou explícita via um comando/método `compact()`.
- REQ-108: `comrak` extrai de `.specs/**/*.md`: requisitos (`REQ-XXX`),
  tasks (`TASK-XXX`), ADRs (`ADR-XXX`) e relações entre seções de documento,
  cada entidade hasheada (Blake3) para virar seu `StableId`.
- REQ-109: CSR e o extrator de Markdown implementam `sync::SyncParticipant`
  (trait da Fase 0) — nenhum dos dois escreve em disco fora do fluxo
  `stage()`/`commit()`/`abort()` do `Coordinator`.

## Affected Components (from graph)

Sem grafo ainda — este código é justamente o que vai gerar o primeiro grafo
real do próprio SpecDB (auto-hospedagem futura). Componentes existentes que
esta fase consome diretamente:

- `sync::coordinator::Coordinator` — CSR/extrator de Markdown viram
  participantes dele.
- `sync::participant::SyncParticipant` — trait implementada por ambos; **é a
  primeira consumidora real**, então valida (ou força revisão de) o contrato
  desenhado "cedo demais" na Fase 0 (ver risco já registrado em
  `.specs/features/sync-coordinator/design.md`).
- `sync::mutation::{StableId, NodeMutation, EdgeMutation, DocMutation}` —
  reaproveitados como o vocabulário de mutação entre esta fase e o
  coordinator; não são redefinidos aqui.

## Out of Scope

- Multi-linguagem AST (Tree-sitter) — Fase 3. Aqui só Markdown/specs.
- Busca léxica/vetorial (Tantivy/HNSW) — Fases 3/4.
- Git/gix e sync incremental via diff de árvore — Fase 2. Fase 1 assume que o
  conjunto de arquivos a indexar já foi determinado por quem a chama (a Fase 2
  é quem vai decidir "o que mudou" a partir do Git).
- CLI (`specdb init/sync/...`) — Fase 6. Aqui só a biblioteca.
- Paralelismo de parsing via `rayon` — mencionado na Fase 3 do roadmap
  (cold-start de Tree-sitter); o parser Markdown desta fase pode rodar
  sequencial por ora sem violar nenhum REQ.

## Open Questions

Resolvidas com decisão padrão (documentada para revisão, não bloqueiam início):

- **Formato de `NodePayload`**: um enum Rust com uma variante por `NodeType`
  (`NodePayload::Requirement { .. }`, `NodePayload::Symbol { .. }}` etc.),
  serializado com `rkyv` (consistência com CSR/WAL) antes de ir para `zstd` e
  `redb`. Evita um segundo formato de serialização na mesma fase.
- **O que exatamente vai no delta layer**: apenas edges (adição/remoção),
  não nós — nós vivem em `redb` (já transacional, já com staging via
  `RedbParticipant` da Fase 0) e não precisam de uma segunda camada de
  consistência. O CSR de duas camadas é especificamente sobre a topologia
  (edges), que é o que precisa de lookups O(1) e portanto de um layout mmap
  denso.
- **`crossbeam-epoch` vs `ArcSwap`**: começar com `ArcSwap` (API mais simples,
  suficiente para "trocar o ponteiro do delta atomicamente"); `crossbeam-epoch`
  fica como alternativa se o profiling mostrar necessidade de reclamação de
  memória mais fina. Documentado aqui para não virar decisão implícita dentro
  do código.

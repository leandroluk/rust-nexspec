# State

**Last Updated:** 2026-09-29

## Current Work

Fases 0 (Sync Coordinator), 1 (Storage Primitives/CSR) e 2 (Git Integration)
completas — 53/53 testes, `cargo doc`/`cargo clippy -- -D warnings` limpos,
tudo commitado e no GitHub (`leandroluk/rust-specdb`, branch `main`).
Produto renomeado de "SpecDB" para "NexSpec" (crate `nexspec`) — repo/pasta
local seguem com o nome antigo até o usuário trocar por conta própria.

Agora iniciando a Fase 3 (Multi-Language AST Parsing & Lexical Search):
`.specs/features/ast-lexical-search/{spec,design,tasks}.md` já escritos
(REQ-301..308, 10 tasks T-301..T-310). Próximo: Execute a partir de T-301.

## Todos
- [x] T-301: Dependências (tree-sitter + 4 gramáticas, rayon, tantivy)
- [x] T-302: `Language` + parsing de símbolos de um arquivo
- [ ] T-303: Edges `DefinedIn`/`DependsOn` (mesmo arquivo)
- [ ] T-304: Edge `Satisfies` via `@spec`/`@adr`
- [ ] T-305: `code::batch::extract_all` (paralelo via rayon)
- [ ] T-306: `SyncOrchestrator` roteia arquivos de código
- [ ] T-307: `TantivySchema` + `TantivyParticipant`
- [ ] T-308: Fast-path de busca exata + BM25
- [ ] T-309: Integração fim-a-fim (3 participantes reais)
- [ ] T-310: Lint e superfície pública

## Active Blockers
- none

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

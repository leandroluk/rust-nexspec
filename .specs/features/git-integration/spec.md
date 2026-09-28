# Spec: Git Integration & Incremental Sync (Fase 2)

## Summary

Fase 2 conecta o NexSpec ao histórico Git real do repositório que ele indexa,
usando `gix` (Gitoxide, Rust puro — sem `git` subprocess). É o componente que
decide **o que mudou** desde o último índice e traduz isso em chamadas para
`graph::markdown::extract()` (Fase 1) e, mais tarde, para o parser Tree-sitter
(Fase 3) — sem essa fase, todo `sync` seria um reprocessamento completo do
repositório. Também estabelece o vínculo temporal entre commits e specs
(`feat(auth): satisfy REQ-001`) e um grafo de co-mudança (arquivos que mudam
juntos com frequência) para sinalizar dependências implícitas.

## Requirements

- REQ-201: Embutir `gix` no processo — nenhuma chamada a `git` via subshell em
  nenhum caminho de código.
- REQ-202: Rastrear `last_indexed_commit` (OID do commit) em `metadata.redb`,
  versionado junto com o `sync_version` do Sync Coordinator (Fase 0) — o
  commit indexado é parte do estado que o coordinator torna visível/atômico.
- REQ-203: `diff_since_last_index() -> TreeDiff` — diff de árvore nativo
  (in-memory, via `gix`) entre `HEAD` e `last_indexed_commit`, classificando
  cada caminho afetado como `Added`, `Modified` ou `Deleted`.
- REQ-204: Mudanças não commitadas na working tree (dirty state) também
  entram no diff — cache efêmero de hash Blake3 por arquivo para detectar
  "sujo desde a última leitura" sem precisar de um commit.
- REQ-205: `sync_orchestrator` — para cada `.md` em `Added`/`Modified` no
  diff, chama `graph::markdown::extract()` (Fase 1) e agrega os
  `MutationSet`s resultantes num único `Coordinator::stage()` por ciclo de
  sync; para `Deleted`, emite `NodeMutation::Remove`/`EdgeMutation::Remove`
  para as entidades que vieram daquele arquivo.
- REQ-206: Grafo de co-mudança — pesos de co-ocorrência entre arquivos que
  aparecem juntos nos mesmos commits, calculados sobre uma **janela limitada**
  (default: últimos 500 commits ou 6 meses, o que for menor) para evitar custo
  O(commits × arquivos) sobre repositórios grandes. Uma flag de "full history"
  (API, não CLI — a CLI é Fase 6) opta pelo walk sem limite.
- REQ-207: Linking temporal — ao processar um commit, extrair menções a
  `REQ-XXX`/`TASK-XXX`/`ADR-XXX` na mensagem do commit e gravar essa
  associação (commit OID + timestamp + autor) como metadado ligado ao nó
  correspondente.
- REQ-208: Operar sem falhar em estados de repositório não-triviais: HEAD
  destacado (detached) e working tree suja. Submodules, `git-lfs` e sparse
  checkout ficam **fora do escopo desta fase** (ver Open Questions) — não são
  ignorados silenciosamente, mas também não são um requisito de correção
  ainda.

## Affected Components (from graph)

Sem grafo do próprio NexSpec ainda (ver `.specs/project/STATE.md`, modo
degradado). Componentes existentes que esta fase consome diretamente:

- `sync::coordinator::Coordinator` — o `sync_orchestrator` desta fase é quem
  decide *quando* chamar `stage()`, papel que a Fase 1 deixou deliberadamente
  fora de escopo (ver `.specs/features/storage-primitives/design.md`).
- `graph::markdown::extract()` — consumida diretamente, sem mudanças.
- `sync::version::VersionPointer` — `last_indexed_commit` é armazenado ao lado
  do `sync_version`, mesma tabela `meta` em `redb` (reaproveitar, não
  duplicar um segundo mecanismo de versão).

## Out of Scope

- Tree-sitter / parsing de código-fonte não-Markdown — Fase 3. Esta fase só
  decide *quais arquivos mudaram*; o que fazer com `.rs`/`.py`/etc. é da Fase
  3 em diante.
- Submodules, `git-lfs`, sparse checkout — cobertura explícita adiada (ver
  Open Questions). `gix` pode ou não lidar bem com esses casos hoje; não
  vamos escrever fixtures de CI para eles nesta fase.
- CLI (`nexspec sync`) — Fase 6. Aqui só a biblioteca (`sync_orchestrator` é
  uma função/struct chamável, não um comando).
- Blame por linha/símbolo ("AST-aware blame" do `.defs/NexSpec.md`) — precisa
  de símbolos com `line_start..line_end`, que só existem a partir da Fase 3
  (Tree-sitter). Nesta fase, o grafo de co-mudança trabalha em granularidade
  de **arquivo**, não de símbolo.

## Open Questions

Resolvidas com decisão padrão (documentada para revisão, não bloqueiam início):

- **Cobertura de casos exóticos do Git**: `.defs/NexSpec.md` original pedia
  fixtures de CI para submodules/LFS/sparse-checkout já na Fase 2. Decisão:
  adiar — cobrir apenas HEAD destacado e working tree suja nesta fase (são
  comuns e baratos de testar); os demais casos entram como débito técnico
  explícito em `.specs/codebase/CONCERNS.md` quando o código existir, não
  como bloqueio de REQ.
- **Onde mora o cache de dirty-state (REQ-204)**: em memória por instância do
  orchestrator (não persistido em `redb`) — é inerentemente efêmero (reflete
  o estado do working tree *agora*, não histórico), então não precisa
  participar do protocolo de staging/commit da Fase 0.
- **Formato do grafo de co-mudança**: reaproveita o mesmo `EdgeType`/`Edge`
  da Fase 1 — precisa de um novo `EdgeType::CoChanges` (edge não-direcional
  na prática, mas representada como duas edges `A->B` e `B->A` para caber no
  modelo de edge direcionada já existente, evitando introduzir um segundo
  tipo de aresta no CSR).

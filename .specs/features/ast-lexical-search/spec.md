# Spec: Multi-Language AST Parsing & Lexical Search (Fase 3)

## Summary

Fase 3 estende o NexSpec para além de Markdown: `Tree-sitter` extrai símbolos
(funções, tipos, classes), imports e anotações `@spec`/`@adr` de código-fonte
real (TS/JS, Python, Go, Rust), alimentando o mesmo grafo (`NodeType::Symbol`,
já definido na Fase 1) que `markdown::extract()` alimenta com specs. Em
paralelo, `Tantivy` dá busca léxica (BM25) sobre símbolos, IDs de requisito e
caminhos de arquivo — o primeiro motor de busca real do NexSpec (Fases 0-2
só tinham navegação de grafo, sem full-text). `SyncOrchestrator` (Fase 2)
passa a rotear arquivos não-`.md` para o parser Tree-sitter, do mesmo jeito
que já roteia `.md` para `markdown::extract()`.

## Requirements

- REQ-301: Gramáticas Tree-sitter linkadas estaticamente para TypeScript/
  JavaScript, Python, Go e Rust.
- REQ-302: `code::extract(source: &str, language: Language) -> MutationSet`
  — produz um `Node` (`NodeType::Symbol`) por função/método, tipo/classe/
  struct e módulo top-level, com nome, assinatura (quando aplicável) e faixa
  de linhas (`line_start..line_end`) no payload.
- REQ-303: Edges `DefinedIn` (símbolo → `NodeType::File` do arquivo que o
  contém, reaproveitando `graph::node::file_node_id` da Fase 2) e
  `DependsOn` (símbolo chamador → símbolo chamado, quando ambos resolvem
  dentro do mesmo arquivo — resolução cross-file fica fora de escopo, ver
  Open Questions).
- REQ-304: Reconhecer anotações `@spec REQ-XXX` / `@adr ADR-XXX` em
  comentários/docstrings adjacentes a um símbolo, gerando edge `Satisfies`
  do símbolo para o nó de spec correspondente — mesma sintaxe de marcador já
  usada em `.specs/*.md` (REQ-108) e mensagens de commit (REQ-207), mesma
  função `find_markers` reaproveitada uma terceira vez.
- REQ-305: Parsing de múltiplos arquivos em paralelo via `rayon` no
  cold-start (primeira indexação de um repositório) — parsing por arquivo é
  embaraçosamente paralelo.
- REQ-306: `SyncOrchestrator::run_once()` (Fase 2) roteia arquivos
  `Added`/`Modified` não-`.md` (com extensão reconhecida: `.ts`/`.tsx`/`.js`/
  `.jsx`/`.py`/`.go`/`.rs`) para `code::extract()`, do mesmo jeito que já
  roteia `.md` para `markdown::extract()` — mesmo `MutationSet` combinado,
  mesmo `Coordinator::stage()` único por ciclo.
- REQ-307: Índice léxico Tantivy (BM25) sobre símbolos, IDs de requisito
  (`REQ-XXX`/`TASK-XXX`/`ADR-XXX`) e caminhos de arquivo — consulta exata de
  identificador é o caso rápido (fast-path).
- REQ-308: `TantivyParticipant` implementa `sync::SyncParticipant` (mesmo
  contrato da Fase 0/1) — o índice léxico participa do mesmo protocolo de
  staging/commit/abort que `redb` e o CSR, nunca é escrito fora dele.

## Affected Components (from graph)

Sem grafo do próprio NexSpec ainda. Componentes existentes consumidos:

- `sync::coordinator::Coordinator` — `TantivyParticipant` se torna o
  **terceiro** `SyncParticipant` real (depois de `RedbParticipant` e
  `CsrParticipant`), mais uma validação do contrato desenhado na Fase 0.
- `graph::node::{NodeType::Symbol, file_node_id}` — reaproveitados sem
  mudança; `code::extract()` produz `NodePayload::Symbol` já definido na
  Fase 1.
- `graph::markdown::find_markers` — reaproveitada pela terceira vez (specs,
  commits, agora código).
- `sync_orchestrator::SyncOrchestrator` — ganha um segundo roteamento de
  extração (código, além de Markdown), no mesmo `run_once()`.

## Out of Scope

- Resolução de símbolos cross-file/cross-crate (REQ-303 só resolve chamadas
  dentro do mesmo arquivo) — precisaria de um resolvedor de imports/módulos
  completo por linguagem, fora de escopo aqui.
- Busca vetorial/embeddings — Fase 4.
- Orçamento de tokens / serialização de contexto — Fase 5.
- Linguagens além de TS/JS/Python/Go/Rust — mesmo conjunto do `.defs/
  NexSpec.md` original, não expandido aqui.
- Blame por símbolo (linha-precisa) — a Fase 2 já decidiu (ver
  `.specs/features/git-integration/spec.md`) que isso fica para quando
  símbolos com `line_start..line_end` existissem; eles passam a existir
  agora (REQ-302), mas ligar isso ao `git blame` é uma extensão futura da
  Fase 2, não desta fase.

## Open Questions

Resolvidas com decisão padrão (documentada para revisão, não bloqueiam início):

- **Uma gramática por vez ou todas linkadas sempre?** Todas as 4 linkadas
  estaticamente sempre (simplicidade de build; `.defs/NexSpec.md` já previa
  isso). Seleção de linguagem por extensão de arquivo, feita pelo chamador
  (`SyncOrchestrator`), não pelo parser.
- **Granularidade de símbolo**: funções/métodos, tipos (struct/class/
  interface/enum) e módulos top-level. Não desce a nível de variável/campo
  individual — ruído demais para o caso de uso (contexto de agente de IA,
  não um LSP completo).
- **`DependsOn` entre símbolos do mesmo arquivo apenas**: resolver imports
  para localizar o símbolo chamado em outro arquivo exigiria um resolvedor
  de módulos por linguagem (caminhos relativos em TS, `go.mod` em Go,
  `Cargo.toml`/`mod` em Rust, `sys.path` em Python) — cada um com regras
  próprias. Adiado explicitamente; o grafo fica "menos conectado" entre
  arquivos até uma fase futura resolver isso, mas correto dentro de cada
  arquivo.

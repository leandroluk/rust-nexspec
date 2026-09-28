# NexSpec — Roadmap

Fonte primária de escopo técnico: `.defs/NexSpec.md` (documento gerado com Gemini).
Este roadmap traduz as fases dele em milestones rastreáveis pela skill
`graph-spec-design`. Cada fase vira uma feature em `.specs/features/` quando
começar a ser trabalhada.

## Fases

| # | Fase | Entrega central | Status |
|---|------|------------------|--------|
| 0 | Sync Coordinator & Transactional Integrity | WAL (`sync.wal`), `sync_version` atômico em `redb`, staging+rename por store, `nexspec sync --resume`, apply idempotente | **Completo** (`.specs/features/sync-coordinator/`) |
| 1 | Storage Primitives & Graph Topology | `Node`/`Edge` tipados, ID estável (Blake3) vs índice físico denso (`u32`), redb+zstd, CSR **duas camadas** (base rkyv/mmap + delta lock-free via `ArcSwap`/`crossbeam-epoch`, compactação por threshold), extração Markdown (comrak) | Não iniciado |
| 2 | Git Integration & Incremental Sync | gix embutido (com fixtures para submodules/LFS/sparse checkout/detached HEAD), tree-diff incremental, blame por símbolo com janela de co-change limitada (default 500 commits/6 meses), linking commit→spec | Não iniciado |
| 3 | Multi-Language AST & Lexical Search | Tree-sitter (TS/JS, Python, Go, Rust) com parsing paralelo via `rayon` no cold-start, índice Tantivy (BM25) | Não iniciado |
| 4 | Vector Engine & Hybrid Traversal | ONNX (ort) **lazy-loaded** + modelo INT8 quantizado, flag de compilação "lean" (sem ort/HNSW), HNSW, RRF, expansão k-hop sobre base+delta | Não iniciado |
| 5 | Token Budgeting & LLM Serialization | Pruning AST, `Tokenizer` trait plugável (default tiktoken-rs) com margem de segurança (90% do budget), fallback offline (`char_count / 3.5`), serializer Markdown denso | Não iniciado |
| 6 | Interface, MCP Server & Tooling | CLI (`clap`: init/sync/compact/search/trace/blame/diff), servidor MCP (`rmcp`) com tool surface unificado, integração drop-in na skill `graph-spec-design` | Não iniciado |

### Nota — por que a Fase 0 existe e vem antes de tudo

`redb`, o arquivo CSR, o Tantivy e o HNSW cada um faz commit próprio. Sem um
coordenador central, um crash no meio de um `sync` pode deixar esses 4 stores
divergentes entre si (ex.: edge gravado no CSR mas não no Tantivy). A Fase 0
resolve isso na base — WAL + version pointer atômico — para que as fases
seguintes (1–6) já construam sobre uma primitiva de consistência sólida, em vez
de remendar isso depois.

## Critério de "pronto" do projeto (v1)

- `nexspec init/sync/search/trace/blame/diff` funcionando via CLI estática
- Contrato de saída compatível com o que a skill `graph-spec-design` espera hoje do
  `graphify` (`.specs/graph/graph.json`, `GRAPH_REPORT.md`, staleness check)
- `graph-spec-design` consegue rodar 100% sobre `nexspec` sem `graphify` Python
  instalado (ver seção "Integração com a skill" abaixo)

## Integração com a skill graph-spec-design

Hoje a skill chama `graphify` (Python) via `uv`/`pip`. Meta: expor `nexspec` com CLI
e contrato de I/O suficientemente compatíveis para que o Rule #1 da skill
(`references/init.md`, `references/session.md`) passe a detectar e preferir
`nexspec` no lugar de `graphify`, sem exigir mudança na skill além de trocar o
binário/comando invocado. Essa troca será uma feature própria
(`.specs/features/nexspec-skill-integration/`) quando a Fase 6 estiver perto.

## Ordem de trabalho sugerida

Fase 0→1→2→3 é uma cadeia rígida de pré-requisitos (consistência → storage → git →
AST); nenhuma delas faz sentido isolada das anteriores. Fases 4 e 5 podem ser
paralelizadas depois que 3 estiver de pé. Fase 6 fecha o pacote e é o ponto de
integração real com este projeto (`graph-spec-design` deixa de depender de Python).

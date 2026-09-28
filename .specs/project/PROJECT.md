# NexSpec

## O que é

Motor de contexto **in-process**, escrito em Rust, para agentes de codificação IA.
Substitui o `graphify` (Python) usado pela skill `graph-spec-design`, entregando o
mesmo tipo de índice (grafo de código + specs) várias ordens de magnitude mais
rápido e mais barato em tokens.

Unifica:
- Análise AST de código-fonte (Tree-sitter) e de `.specs/*.md` (Comrak)
- Grafo topológico em CSR (Compressed Sparse Row) de **duas camadas** (base imutável
  + delta append-only lock-free), navegação O(1) via mmap
- Busca híbrida vetorial (HNSW, lazy-loaded) + léxica (Tantivy/BM25) com fusão RRF
- Integração nativa com Git (gix — sem subshells)
- Orçamento de tokens determinístico e com tokenizer plugável para serialização de
  contexto ao LLM
- Um **Sync Coordinator** central (WAL + version pointer atômico) que garante
  consistência entre todos os stores — nenhum store é fonte de verdade sozinho

## Princípio de design central

Todo caminho de escrita passa por um único **Sync Coordinator**, dono da
consistência entre stores. `redb`, CSR, Tantivy e HNSW cada um faz commit
independente; o que decide o que é "visível" é o `sync_version` atômico do
coordinator, não o estado individual de cada store. Isso existe para que um crash
no meio de um `sync` nunca deixe os índices divergentes entre si (ver Fase 0 do
roadmap).

## Por que existe

O `graphify` atual (Python) é a dependência de índice da skill `graph-spec-design`.
Funciona, mas paga custo de startup de interpretador, GIL, e não é distribuível como
binário único. A tese do NexSpec: reimplementar o mesmo contrato de saída
(`graph.json`, `GRAPH_REPORT.md`, comandos `query`/`path`/`explain`) em Rust nativo,
como binário estático zero-instalação, embutindo storage (redb), grafo (rkyv/CSR),
Git (gix), AST (tree-sitter), léxico (tantivy) e vetorial (ort/HNSW) no mesmo
processo — sem daemons externos, sem runtime interpretado.

## Escopo

**Dentro do escopo:**
- Sync Coordinator transacional (WAL + version pointer atômico + crash recovery)
- Motor de storage/índice embarcado (redb + CSR duas-camadas + mmap)
- IDs estáveis (Blake3) desacoplados do índice físico denso (`u32`) do CSR
- Extração de entidades de `.specs/*.md` (REQ-XXX, TASK-XXX, ADR-XXX) e de código
  (símbolos, imports, call sites) via Tree-sitter multi-linguagem, parseado em
  paralelo (`rayon`) no cold-start
- Sync incremental via diff de árvore Git nativo (gix), com cobertura de casos
  exóticos (submodules, LFS, sparse checkout, detached HEAD)
- Busca híbrida (BM25 + vetorial lazy-loaded) com expansão k-hop no grafo
  (base + delta mesclados)
- Serialização de contexto compacta para LLM (Markdown denso, tokenizer plugável,
  margem de segurança, fallback offline)
- CLI (`nexspec init/sync/compact/search/trace/blame/diff`) e servidor MCP embutido
  com superfície de tools unificada
- Substituição drop-in do `graphify` dentro da skill `graph-spec-design`
  (mesmo contrato de saída em `.specs/graph/`)

**Fora do escopo (por ora):**
- UI gráfica / dashboard
- Treinamento de modelos de embedding próprios (usa modelo pré-treinado quantizado)
- Suporte a linguagens além do conjunto inicial (TS/JS, Python, Go, Rust)
- Multi-tenant / servidor remoto (é embarcado, single-repo, single-process)

## Stack

Rust. Ver `.specs/codebase/STACK.md` para a matriz de crates por módulo (fonte:
`.defs/NexSpec.md`, mantido como documento de referência gerado com Gemini — não
editar diretamente, promover decisões relevantes para `.specs/`).

## Contexto de uso

Este próprio repositório usa a skill `graph-spec-design` para se auto-organizar
(specs, tasks, execução) enquanto o NexSpec ainda não existe como binário — ou seja,
"bootstrapping": construímos a ferramenta que um dia vai indexar este mesmo
repositório. Até o binário `nexspec` (ou o `graphify` Python) estar disponível e
instalado, a skill opera em **modo degradado** (leitura direta de arquivos, sem
grafo). Assim que houver código suficiente e um `graphify`/`nexspec` funcional,
reindexar com `graph-spec-design . --update --no-viz`.

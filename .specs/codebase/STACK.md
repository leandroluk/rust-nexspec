# Stack

Rust. Matriz abaixo é a decisão de stack vinda de `.defs/NexSpec.md`,
promovida aqui como referência de arquitetura. Todos os módulos até a Fase 5
já têm crate e código correspondentes em `src/`.

| Módulo              | Crate principal                | Papel                                                        |
| ------------------- | ------------------------------ | ------------------------------------------------------------ |
| CLI & Transport     | `clap`, `tokio`, `rmcp`        | Parsing de argumentos e servidor MCP via stdio               |
| Sync Coordination   | `redb`                         | WAL + version pointer atômico `sync_version` (Fase 0)        |
| Metadata Storage    | `redb`, `zstd`                 | Tabelas key-value ACID e compressão de payload               |
| Graph Serialization | `rkyv`, `memmap2`              | CSR base zero-copy + merge com camada delta em memória       |
| Parallelism         | `rayon`                        | Parsing AST e lookups de blame em paralelo no cold-start     |
| Concurrency (delta) | `arc-swap`, `crossbeam-epoch`  | Publish/read lock-free (COW) da camada delta do CSR          |
| Git Integration     | `gix` (Gitoxide)               | Inspeção, diff e blame de Git em Rust puro                   |
| Code AST            | `tree-sitter`, `tree-sitter-*` | Gramáticas para TypeScript, Python, Go, Rust                 |
| Spec & Docs AST     | `comrak`                       | Parsing de Markdown e extração de REQ/ADR                    |
| Lexical Search      | `tantivy`                      | Índice BM25 embarcado                                        |
| Vector Engine       | `ort`, `instant-distance`      | Embeddings ONNX INT8 (lazy-loaded) e busca HNSW              |
| Token Budgeting     | `tiktoken-rs`                  | Estimativa BPE em tempo real, plugável via trait `Tokenizer` |

Detalhes de cada fase (o que cada crate resolve concretamente) estão em
`.specs/project/ROADMAP.md` e no documento de origem `.defs/NexSpec.md`.

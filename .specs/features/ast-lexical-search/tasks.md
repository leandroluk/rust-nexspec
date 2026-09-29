# Tasks: Multi-Language AST Parsing & Lexical Search (Fase 3)

## T-301: Dependências (`tree-sitter` + 4 gramáticas, `rayon`, `tantivy`) [x]
- **REQ**: REQ-301, REQ-305, REQ-307
- **What**: `cargo add tree-sitter tree-sitter-typescript tree-sitter-javascript
  tree-sitter-python tree-sitter-go tree-sitter-rust rayon tantivy`.
- **Where**: `Cargo.toml`
- **Depends on**: none
- **Done when**: `cargo build` compila com as novas deps.
- **Gate**: `cargo build`

## T-302: `Language` + parsing de símbolos de um arquivo
- **REQ**: REQ-301, REQ-302
- **What**: Enum `Language` (TypeScript, JavaScript, Python, Go, Rust) +
  `Language::from_extension(path) -> Option<Language>`. `code::extract(source:
  &str, language: Language) -> MutationSet` — usa `tree_sitter::Parser` com a
  gramática certa, percorre a AST via query/cursor, produz um
  `NodeMutation::Upsert` (`NodePayload::Symbol { name, source_hash }`, mais
  faixa de linhas — decidir campo extra ou reaproveitar `name` formatado)
  por função/método/tipo/módulo top-level encontrado.
- **Where**: `src/code/parser.rs`
- **Depends on**: T-301
- **Done when**: fixture de 1 arquivo por linguagem (função + tipo simples)
  produz os nós esperados para as 5 linguagens.
- **Gate**: `cargo test code::parser`

## T-303: Edges `DefinedIn` e `DependsOn` (mesmo arquivo)
- **REQ**: REQ-303
- **What**: Para cada símbolo extraído, edge `DefinedIn` símbolo→
  `file_node_id(path)`. Para cada call expression dentro de uma função cujo
  callee resolve a outro símbolo já extraído do mesmo arquivo, edge
  `DependsOn` chamador→chamado.
- **Where**: `src/code/parser.rs` (extensão)
- **Depends on**: T-302
- **Done when**: fixture com `fn a() { b() }` + `fn b() {}` produz edge
  `DependsOn` de `a` para `b`; chamada a função não resolvida no arquivo não
  gera edge (nem erro).
- **Gate**: `cargo test code::parser::depends_on`

## T-304: Edge `Satisfies` via `@spec`/`@adr`
- **REQ**: REQ-304
- **What**: Reconhecer comentário/docstring imediatamente anterior a um
  símbolo contendo `@spec REQ-XXX` ou `@adr ADR-XXX` (via
  `graph::markdown::find_markers`, já `pub(crate)`), gerar edge `Satisfies`
  símbolo→spec (só se o REQ/ADR já for conhecido — mesma limitação de
  forward-reference documentada em `markdown::extract`).
- **Where**: `src/code/parser.rs` (extensão)
- **Depends on**: T-302
- **[P]**: A (paralelizável com T-303)
- **Done when**: fixture com `// @spec REQ-701\nfn f() {}` gera edge
  `Satisfies` de `f` para o nó `REQ-701` (quando esse nó já existir no
  `MutationSet`/contexto de teste).
- **Gate**: `cargo test code::parser::satisfies`

## T-305: `code::batch::extract_all` (paralelo via rayon)
- **REQ**: REQ-305
- **What**: `extract_all(files: &[(PathBuf, String, Language)]) -> MutationSet`
  — `files.par_iter().map(|f| code::extract(...)).collect()`, merge
  sequencial dos `MutationSet`s resultantes.
- **Where**: `src/code/batch.rs`
- **Depends on**: T-302
- **Done when**: teste com N arquivos fixture retorna a união exata dos
  nós/edges que `extract()` produziria chamado individualmente por arquivo
  (mesmo resultado, só paralelizado).
- **Gate**: `cargo test code::batch`

## T-306: `SyncOrchestrator` roteia arquivos de código
- **REQ**: REQ-306
- **What**: `run_once()` ganha um branch `code_language(path) ->
  Option<Language>`; para `.md` usa `markdown::extract` (como já é), para
  extensão reconhecida usa `code::extract` sobre o mesmo conteúdo (lido via
  `GitSource::read_blob_at_head` ou do working tree se sujo, mesmo padrão já
  usado para Markdown) — tudo no mesmo `MutationSet` combinado.
- **Where**: `src/sync_orchestrator.rs` (extensão)
- **Depends on**: T-303, T-304
- **Done when**: teste de integração com fixture Git contendo um `.rs` e um
  `.md` no mesmo commit produz nós de ambos os tipos após `run_once()`.
- **Gate**: `cargo test --test sync_orchestrator_code_routing`

## T-307: `TantivySchema` + `TantivyParticipant` (stage/commit/abort)
- **REQ**: REQ-307, REQ-308
- **What**: Schema Tantivy (`id` stored+string fast-path, `kind`, `text`
  BM25, `path`). `TantivyParticipant::new(index_path: &Path) ->
  Result<Self, SearchError>` abre/cria o índice e um `IndexWriter` de vida
  longa. `stage()` adiciona documentos ao writer (nós do `MutationSet` viram
  documentos; edges são ignoradas — Tantivy indexa conteúdo, não topologia).
  `commit()` chama `writer.commit()`. `abort()` chama `writer.rollback()`.
  Replica os 3 cenários de teste padrão (stage→commit visível, stage→abort
  descarta, stage→stage→commit não duplica).
- **Where**: `src/search/schema.rs`, `src/search/tantivy_participant.rs`
- **Depends on**: T-301
- **[P]**: B (paralelizável com T-302..T-306, mesmo bloco de dependências
  raiz T-301)
- **Done when**: os 3 cenários passam; documento commitado é encontrável por
  busca exata de `id` e por busca BM25 em `text`.
- **Gate**: `cargo test search::tantivy_participant`

## T-308: Fast-path de busca exata + BM25
- **REQ**: REQ-307
- **What**: `search::query::find_by_id(&TantivyParticipant, id: &str) ->
  Option<...>` (fast-path, campo `id` não tokenizado) e
  `search::query::search_text(&TantivyParticipant, query: &str, limit: usize)
  -> Vec<...>` (BM25 sobre `text`, via `tantivy::collector::TopDocs`).
- **Where**: `src/search/query.rs`
- **Depends on**: T-307
- **Done when**: `find_by_id("REQ-401")` acha o documento certo em O(1)-ish
  (busca de termo exato, não scan); `search_text("stable id")` retorna
  resultados ranqueados contendo o termo, sem falso-positivo óbvio.
- **Gate**: `cargo test search::query`

## T-309: Integração fim-a-fim: 3 participantes reais
- **REQ**: (todos — valida a fase inteira junto)
- **What**: Teste de integração com `Coordinator` real e
  `[RedbParticipant, CsrParticipant, TantivyParticipant]` — um ciclo de sync
  via `SyncOrchestrator` sobre fixture com `.md` + `.rs`, depois consulta os
  3 stores confirmando consistência (mesmos nós visíveis em `redb`, mesmas
  edges no CSR, mesmo conteúdo buscável no Tantivy).
- **Where**: `tests/three_participants_integration.rs`
- **Depends on**: T-306, T-308
- **Done when**: teste passa fim-a-fim sem mocks.
- **Gate**: `cargo test --test three_participants_integration`

## T-310: Lint e superfície pública
- **REQ**: (todos — fechamento da fase, mesmo padrão de T-010/T-110/T-209)
- **What**: Exportar `code::{Language, extract}`, `search::{TantivyParticipant,
  query}` de `src/lib.rs`. Doc comments em toda API pública.
- **Where**: `src/lib.rs`, `src/code/mod.rs`, `src/search/mod.rs`
- **Depends on**: T-309
- **Done when**: `cargo doc --no-deps` sem warnings, `cargo clippy
  --all-targets -- -D warnings` sem erros.
- **Gate**: `cargo doc --no-deps && cargo clippy --all-targets -- -D warnings`

# Tasks: Interface, MCP Server & Tooling (Fase 6)

## T-601: Dependências (`clap`, `tokio`, `rmcp`, `serde`/`serde_json`/`schemars`) + `Engine` scaffold [x]
- **REQ**: REQ-601, REQ-602
- **What**: `Engine::open(index_dir, repo_root) -> Result<Engine, EngineError>`
  — cria `.specs/.index/` (metadata.redb, sync.wal, edges.bin via
  `CsrBase::build(&[], ..)` se ausente, `tantivy/` via
  `TantivyParticipant::new` uma vez e descartado, `vectors.bin` sob
  `full`), guarda `Database`, `Arc<Csr>`, paths, `repo_root`. Idempotente:
  rodar sobre estrutura já existente não recria/apaga nada
  (`CsrBase::open` se o arquivo já existir, em vez de `build` de novo).
- **Where**: `Cargo.toml`, `src/engine.rs`, `src/lib.rs`
- **Depends on**: none
- **Done when**: `Engine::open` sobre um diretório vazio cria a estrutura;
  chamado de novo sobre o mesmo diretório não falha nem reseta o `Arc<Csr>`
  para uma base vazia se já havia edges.
- **Gate**: `cargo test engine::open` (+ `cargo build`/`cargo build --no-default-features --features lean`)

## T-602: `Engine::sync`/`Engine::resume` [x]
- **REQ**: REQ-603
- **What**: Constrói `GitSource::open(&repo_root)`, participantes frescos
  (Redb/Csr/Tantivy/Hnsw-sob-`full`), `Coordinator`, `SyncOrchestrator`,
  chama `run_once()`. `Engine::resume` faz a mesma construção de
  participantes e chama `Coordinator::resume()`.
- **Where**: `src/engine.rs`
- **Depends on**: T-601
- **Done when**: teste com um repo Git fixture (`tempfile` + `gix::init`)
  sincroniza um `.md` com REQ e o resultado é consultável via `self.csr`
  depois.
- **Gate**: `cargo test engine::sync`

## T-603: `CsrParticipant::compact_now` + `Engine::compact` [x]
- **REQ**: REQ-604
- **What**: `CsrParticipant::compact_now(&self) -> Result<(), SyncError>`
  (novo método `pub`, wrapper de uma linha sobre o `compact()` privado
  existente). `Engine::compact(&self)` constrói um `CsrParticipant` sobre
  `self.csr` e chama `compact_now()`.
- **Where**: `src/graph/csr/participant.rs`, `src/engine.rs`
- **Depends on**: T-602
- **Done when**: teste força compactação fora do threshold automático e
  confirma delta zerado / base atualizada.
- **Gate**: `cargo test engine::compact`

## T-604: `git::blame::blame_symbol` [x]
- **REQ**: REQ-607
- **What**: `blame_symbol(git: &GitSource, path: &Path, line_start: u32,
  line_end: u32) -> Result<Vec<BlameHunk>, GitError>` via
  `gix::Repository::blame_file` + `gix_blame::BlameRanges::
  from_one_based_inclusive_range((line_start+1)..=(line_end+1))`.
  `BlameHunk { commit_oid: [u8;20], author_name: String, author_email:
  String, time: gix_date::Time, lines: Range<u32> }`, resolvido via
  `repo.find_commit(entry.commit_id)`.
- **Where**: `src/git/blame.rs`, `src/git/mod.rs`
- **Depends on**: none (só precisa de `GitSource`, Fase 2, inalterado)
- **[P]**: A (paralelizável com T-605/T-606)
- **Done when**: teste com um repo fixture de 2 commits (linha adicionada
  no 2º) confirma que o hunk daquela linha aponta para o commit certo.
- **Gate**: `cargo test git::blame`

## T-605: `Engine::search` [x]
- **REQ**: REQ-605
- **What**: BM25 sempre (Tantivy read handle); HNSW só sob `full` **e**
  `Embedder::embed` bem-sucedido (senão lista vazia, nunca erro). Funde via
  `hybrid::seed_discovery`, expande 1 hop via `hybrid::expand`. Se
  `max_tokens: Some(n)`, resolve payloads via Redb, poda símbolos
  (`token::prune_symbol`, lendo o arquivo-fonte via
  `GitSource::read_blob_at_head`), tiera (1º resultado = `Target`, resto do
  seed set = `Seed`, só-expansão = `Dependency`) e roda
  `Budget::with_default_margin(n).fit()` + `token::serialize`.
- **Where**: `src/engine.rs`
- **Depends on**: T-601
- **[P]**: A
- **Done when**: teste sem `max_tokens` retorna lista rankeada crua; teste
  com `max_tokens` pequeno retorna Markdown podado e cortado.
- **Gate**: `cargo test engine::search`

## T-606: `Engine::trace` [x]
- **REQ**: REQ-606
- **What**: Resolve `target` (hex de `StableId` ou lookup textual via
  Tantivy) e faz BFS sobre `Csr::edges_from` em `{Satisfies, DependsOn,
  DefinedIn, Implements}`, registrando `(depth, edge_type, node_id)` por
  nó visitado.
- **Where**: `src/engine.rs`
- **Depends on**: T-601
- **[P]**: A
- **Done when**: fixture com REQ→Symbol (`Satisfies`)→Symbol
  (`DependsOn`) retorna os 2 hops com profundidade e tipo de aresta
  corretos.
- **Gate**: `cargo test engine::trace`

## T-607: `Engine::diff_staged` [x]
- **REQ**: REQ-608
- **What**: `GitSource::is_dirty`/`tracked_paths_at_head` + `DirtyCache`
  para achar arquivos sujos, `code::extract` sobre o conteúdo *atual* da
  working tree desses arquivos, e para cada símbolo resultante busca
  dependentes diretos (`DependsOn` incoming) via `self.csr`. Não toca
  Tantivy/HNSW.
- **Where**: `src/engine.rs`
- **Depends on**: T-602 (reusa padrão de leitura de dirty tree já usado
  por `SyncOrchestrator`)
- **Done when**: teste com um arquivo sujo cujo símbolo já tem um
  dependente indexado retorna esse dependente na lista de impacto.
- **Gate**: `cargo test engine::diff_staged`

## T-608: CLI (`clap`) — `src/bin/nexspec.rs` [x]
- **REQ**: REQ-602, REQ-603, REQ-604, REQ-605, REQ-606, REQ-608
- **What**: `clap` `#[derive(Parser)]` com subcomandos `init <path>`, `sync
  [--resume]`, `compact`, `search "<query>" [--max-tokens N]`, `trace
  <ID>`, `diff --staged` (todos operando sobre `.specs/.index/` relativo ao
  cwd ou `--repo <path>`), delegando a `Engine`. `blame` fica fora deste
  task (T-609, depende de T-604 que roda em paralelo). Saída
  humano-legível por padrão.
- **Where**: `src/bin/nexspec.rs`, `Cargo.toml` (`[[bin]]`)
- **Depends on**: T-602, T-603, T-605, T-606, T-607
- **Done when**: `cargo run --bin nexspec -- sync` sobre um repo fixture
  roda sem erro; `--help` lista todos os subcomandos.
- **Gate**: `cargo build --bin nexspec`

## T-609: CLI `blame` + `Engine::blame` [x]
- **REQ**: REQ-607
- **What**: `Engine::blame(&self, symbol_name: &str, full_history: bool) ->
  Result<BlameResult, EngineError>` — resolve o símbolo, chama
  `git::blame::blame_symbol`, adiciona vizinhos de co-mudança do arquivo
  (`git::cochange`, janela padrão ou irrestrita conforme `full_history`).
  Subcomando CLI `blame <SYMBOL> [--full-history]`.
- **Where**: `src/engine.rs`, `src/bin/nexspec.rs`
- **Depends on**: T-604, T-608
- **Done when**: `nexspec blame <symbol conhecido>` imprime o(s) commit(s)
  que introduziram suas linhas.
- **Gate**: `cargo test engine::blame`

## T-610: Servidor MCP (`rmcp`) — `src/mcp.rs` + subcomando `nexspec mcp` [x]
- **REQ**: REQ-609
- **What**: `NexSpecMcp(Arc<Engine>)`, `#[tool_router]`/`#[tool_handler]`
  com 6 `#[tool]`: `query_context`, `trace_requirement`,
  `find_impacted_code`, `semantic_search`, `get_symbol_history`,
  `sync_workspace` — cada um desserializa `Parameters<...Args>` (structs
  `#[derive(Deserialize, JsonSchema)]`), chama o método `Engine`
  correspondente, retorna `Result<String, String>` (JSON via
  `serde_json::to_string` no sucesso). Subcomando `nexspec mcp` monta um
  runtime `tokio` e chama `.serve(rmcp::transport::stdio()).await?.
  waiting().await?`.
- **Where**: `src/mcp.rs`, `src/bin/nexspec.rs`
- **Depends on**: T-602, T-603, T-605, T-606, T-607, T-609
- **Done when**: teste unitário chama cada método `#[tool]` diretamente
  (sem transporte real) contra um `Engine` de fixture e confirma JSON
  válido na saída de sucesso.
- **Gate**: `cargo test mcp::`

## T-611: Integração fim-a-fim (init → sync → search/trace/blame/diff via CLI) [x]
- **REQ**: (todos — valida a fase inteira junto)
- **What**: Teste de integração que roda o binário `nexspec` via
  `std::process::Command` (ou chama `Engine` diretamente, decidido na
  implementação conforme o que for mais estável em CI) sobre um repo Git
  fixture: `init` → `sync` → `search`/`trace`/`blame`/`diff --staged`,
  confirmando saída não-vazia e coerente em cada um.
- **Where**: `tests/cli_integration.rs`
- **Depends on**: T-608, T-609
- **Done when**: teste passa fim-a-fim.
- **Gate**: `cargo test --test cli_integration`

## T-612: Lint e superfície pública [x]
- **REQ**: (todos — fechamento da fase, mesmo padrão de T-010/.../T-509)
- **What**: Exportar `Engine`, `EngineError`, tipos de resultado
  (`SearchResult`, `TraceResult`, `BlameResult`, `DiffResult`) de
  `src/lib.rs`. Doc comments em toda API pública.
- **Where**: `src/lib.rs`, `src/engine.rs`
- **Depends on**: T-611
- **Done when**: `cargo doc --no-deps` sem warnings, `cargo clippy
  --all-targets -- -D warnings` sem erros (nos dois builds, default e
  `lean`).
- **Gate**: `cargo doc --no-deps && cargo clippy --all-targets -- -D warnings`

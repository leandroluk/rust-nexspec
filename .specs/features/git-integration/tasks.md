# Tasks: Git Integration & Incremental Sync (Fase 2)

## T-201: Dependência `gix` e fixtures de teste [x]
- **REQ**: REQ-201
- **What**: `cargo add gix`. Criar `tests/fixtures/git_repo.rs` — helper que
  cria um repositório Git temporário programaticamente (via `gix` ou
  `std::process::Command` só dentro de `tests/`, nunca em `src/`) com alguns
  commits, para uso pelos testes desta feature.
- **Where**: `Cargo.toml`, `tests/fixtures/git_repo.rs`
- **Depends on**: none
- **Done when**: `cargo build` compila; helper de fixture cria um repo válido
  e retorna seu path.
- **Gate**: `cargo build`

## T-202: `GitSource` — abrir repositório e ler HEAD [x]
- **REQ**: REQ-201, REQ-208
- **What**: `GitSource::open(path: &Path) -> Result<GitSource, GitError>`
  (wrap `gix::open`). `GitSource::head_commit_oid(&self) -> Result<[u8;20], GitError>`
  (aceita HEAD normal e detached). `GitSource::is_dirty(&self) -> Result<bool, GitError>`.
- **Where**: `src/git/source.rs`
- **Depends on**: T-201
- **Done when**: teste abre o repo de fixture, lê o OID do HEAD, e outro
  teste confirma que HEAD destacado (checkout de um commit específico, não
  de uma branch) não causa erro.
- **Gate**: `cargo test git::source`

## T-203: `TreeDiff` — diff de árvore desde `last_indexed_commit`
- **REQ**: REQ-202, REQ-203
- **What**: `VersionPointer` ganha `last_indexed_commit()`/
  `set_last_indexed_commit()` (chave nova na tabela `meta`, mesmo padrão de
  `sync_version`). `GitSource::diff_since(&self, since: Option<[u8;20]>) -> Result<TreeDiff, GitError>`
  — `None` (nunca indexado) trata tudo como `Added`; caso contrário, diff de
  árvore nativo `gix` entre os dois commits, classificando `Added`/
  `Modified`/`Deleted`.
- **Where**: `src/git/source.rs`, `src/sync/version.rs` (extensão)
- **Depends on**: T-202
- **Done when**: teste com 2 commits de fixture (arquivo A criado no 1º,
  arquivo A modificado + arquivo B criado no 2º) retorna o diff correto entre
  os dois; teste com `since: None` retorna tudo como `Added`.
- **Gate**: `cargo test git::source::tree_diff`

## T-204: `DirtyCache` — mudanças não commitadas
- **REQ**: REQ-204
- **What**: `DirtyCache` (HashMap em memória `PathBuf -> [u8;32]` Blake3).
  `DirtyCache::scan(&mut self, repo_root: &Path, tracked_paths: &[PathBuf]) -> Vec<PathBuf>`
  — recalcula hash de cada caminho e retorna os que mudaram desde a última
  chamada (primeira chamada: todos com conteúdo não vazio contam como
  "sujos" na primeira leitura, para não perder estado inicial).
- **Where**: `src/git/dirty_cache.rs`
- **Depends on**: T-201
- **[P]**: A (paralelizável com T-205, T-206)
- **Done when**: teste cobre (a) primeira varredura reporta arquivos
  existentes, (b) segunda varredura sem mudanças reporta vazio, (c) editar um
  arquivo entre varreduras faz ele reaparecer.
- **Gate**: `cargo test git::dirty_cache`

## T-205: `EdgeType::CoChanges` + `co_change_edges`
- **REQ**: REQ-206
- **What**: Adicionar variante `CoChanges` a `graph::edge::EdgeType` (+
  `to_code`/`from_code`). `CoChangeWindow { max_commits: usize, max_age:
  Duration }` (default 500 / 6 meses). `co_change_edges(&GitSource, window,
  path_to_node_id: &HashMap<PathBuf, StableId>) -> Vec<EdgeMutation>` —
  percorre commits dentro da janela, para cada commit com 2+ arquivos
  modificados gera pares de edges `CoChanges` bidirecionais entre os nós
  correspondentes.
- **Where**: `src/graph/edge.rs` (extensão), `src/git/cochange.rs`
- **Depends on**: T-202
- **[P]**: A (paralelizável com T-204, T-206)
- **Done when**: fixture com 3 commits (2 deles tocando os mesmos 2
  arquivos) produz exatamente as edges `CoChanges` esperadas, direcionadas
  nos dois sentidos; janela pequena (`max_commits: 1`) exclui commits fora
  do alcance.
- **Gate**: `cargo test git::cochange`

## T-206: `extract_commit_links` — linking temporal
- **REQ**: REQ-207
- **What**: `extract_commit_links(message: &str) -> Vec<String>` — reaproveita
  o parser de marcadores já usado em `graph::markdown` (extrair para um
  helper compartilhado se fizer sentido, ou duplicar a lógica mínima — decidir
  na implementação) para achar `REQ-\d+`/`TASK-\d+`/`ADR-\d+` na mensagem do
  commit. `GitSource::commits_since(&self, since: Option<[u8;20]>) ->
  Result<Vec<CommitInfo>, GitError>` (`CommitInfo { oid, message, author,
  timestamp }`).
- **Where**: `src/git/spec_link.rs`, `src/git/source.rs` (extensão)
- **Depends on**: T-202
- **[P]**: A (paralelizável com T-204, T-205)
- **Done when**: mensagem `"feat(auth): satisfy REQ-001"` extrai
  `["REQ-001"]`; mensagem sem marcador extrai vazio.
- **Gate**: `cargo test git::spec_link`

## T-207: `SyncOrchestrator::run_once()` — integração
- **REQ**: REQ-205
- **What**: `SyncOrchestrator::new(git: GitSource, coordinator: Coordinator, ...)`.
  `run_once(&mut self) -> Result<SyncReport, SyncOrchestratorError>`: chama
  `diff_since_last_index`, para cada `.md` em `added`/`modified` roda
  `markdown::extract()` sobre o conteúdo do arquivo (lido via `gix` do blob
  na árvore do commit, não do filesystem — para funcionar mesmo em HEAD
  destacado), agrega os `MutationSet`s (mais os de `co_change_edges` e
  removes para `deleted`) num único `Coordinator::stage()`, e só então avança
  `last_indexed_commit`.
- **Where**: `src/sync_orchestrator.rs`
- **Depends on**: T-203, T-204, T-205, T-206
- **Done when**: teste de integração com 2 commits de fixture (cada um
  adicionando um `.md` com REQ/TASK) roda `run_once()` duas vezes e confirma
  que a segunda chamada não reprocessa nada do primeiro commit (diff vazio
  para ele), e que os nós/edges de ambos os commits estão no
  `RedbParticipant`/`CsrParticipant` ao final.
- **Gate**: `cargo test sync_orchestrator`

## T-208: Testes de estados não-triviais do repositório
- **REQ**: REQ-208
- **What**: Testes de integração cobrindo HEAD destacado (checkout de um
  commit específico) e working tree suja (arquivo modificado sem commit) —
  `GitSource`/`SyncOrchestrator` não devem falhar nesses estados.
- **Where**: `tests/git_edge_cases.rs`
- **Depends on**: T-207
- **Done when**: ambos os cenários rodam sem erro e produzem o diff/dirty-set
  esperado.
- **Gate**: `cargo test --test git_edge_cases`

## T-209: Lint e superfície pública
- **REQ**: (todos — fechamento da fase, mesmo padrão de T-010/T-110)
- **What**: Exportar `git::GitSource`, `sync_orchestrator::SyncOrchestrator`
  de `src/lib.rs`. Doc comments em toda API pública.
- **Where**: `src/lib.rs`, `src/git/mod.rs`
- **Depends on**: T-208
- **Done when**: `cargo doc --no-deps` sem warnings, `cargo clippy
  --all-targets -- -D warnings` sem erros.
- **Gate**: `cargo doc --no-deps && cargo clippy --all-targets -- -D warnings`

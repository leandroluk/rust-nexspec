# Tasks: Workflow Integration (Fase 16)

## T-1601: Lock nomeado e `WatchLoop` (REQ-1601) [x]
- **What**: `SyncLock` ganha `acquire_named(dir, file, timeout)`; `WatchLoop::run(root, debounce, stop, sync)` com `notify`, filtro `.gitignore` (`ignore`) e debounce; eventos só de artefatos do motor são ignorados.
- **Where**: `Cargo.toml`, `src/sync/lock.rs`, `src/workflow/watch.rs`
- **Gate**: `cargo test workflow::watch`

## T-1602: `nexspec watch` (REQ-1601) [x]
- **What**: comando com `--debounce MS`, `watch.lock` (um por repositório), Ctrl+C limpo (termina o ciclo em curso), `sync` a cada rajada.
- **Depends on**: T-1601
- **Gate**: `cargo test --test workflow_watch`

## T-1603: `hook install|uninstall|status` (REQ-1602) [x]
- **What**: bloco marcado nos hooks `post-commit`/`post-merge`/`post-checkout`; anexa; idempotente; remove só o bloco.
- **Where**: `src/workflow/hooks.rs`
- **Gate**: `cargo test --test workflow_hooks`

## T-1604: `check-update` (REQ-1603) [x]
- **What**: `up-to-date`/`stale: …`/`no-index` + códigos 0/3/4, sem escrever.
- **Where**: `src/workflow/check.rs`
- **Gate**: `cargo test --test workflow_check`

## T-1605: `install --platform` e `uninstall` (REQ-1604) [x]
- **What**: Claude Code, Gemini, Cursor, VS Code (projeto) e Codex (usuário); `--dry-run`, backup, merge sem perder outras entradas, idempotência.
- **Where**: `src/workflow/install.rs`
- **Gate**: `cargo test --test workflow_install`

## T-1606: `doctor` (REQ-1605) [x]
- **What**: versão, build, modelo, índice (formato, WAL pendente), hooks, MCP por plataforma, `.gitignore`; sugestão de correção; código 5 em falha.
- **Where**: `src/workflow/doctor.rs`
- **Depends on**: T-1603, T-1605
- **Gate**: `cargo test --test workflow_doctor`

## T-1607: `mcp --watch` (Q1) [x]
- **What**: o servidor MCP roda o `WatchLoop` numa thread, sincronizando pelo seu próprio `Engine`.
- **Depends on**: T-1601
- **Gate**: `cargo test --test workflow_watch`

## T-1608: Contrato de saída e docs (REQ-1606) [x]
- **What**: tabela de códigos e mensagens no README e em `docs/`; teste que fixa os códigos.
- **Depends on**: T-1602..T-1607
- **Gate**: `cargo test --test workflow_contract`

## T-1609: Fechamento [x]
- **What**: ROADMAP/STATE/docs; CI verde.
- **Gate**: `cargo test && cargo test --no-default-features --features lean && cargo clippy --all-targets -- -D warnings`

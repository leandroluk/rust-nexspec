# Design: Workflow Integration (Fase 16)

## Architecture Overview

Seis comandos pequenos que tiram o `sync` da memória do usuário e registram o servidor MCP nos agentes. Todos seguem as mesmas regras: **não escrevem nada que não tenham anunciado**, são **idempotentes**, deixam **backup** do que alteram e devolvem **códigos de saída estáveis**.

```
watch ──────► WatchLoop { notify + filtro .gitignore + debounce } ─► sync (Engine aberto só durante o ciclo)
mcp --watch ► o mesmo WatchLoop, sincronizando pelo Engine do servidor
hook ───────► .git/hooks/{post-commit,post-merge,post-checkout}: bloco marcado, anexado, removível
check-update► compara HEAD/árvore com o índice, sem escrever
install ────► config do MCP por plataforma (JSON/TOML), --dry-run, backup, uninstall
doctor ─────► verificações curtas + comando de correção; sai com código ≠ 0 só em falha
```

## Dependency Paths

- Lock: `SyncLock` (Fase 9) é por diretório de índice (`sync.lock`). `watch` precisa de um segundo lock **por processo watch** (um watch por repositório) e **não** pode segurar `sync.lock` entre ciclos, senão os hooks esperariam 30 s. T-1601 generaliza o lock para aceitar o nome do arquivo (`watch.lock`).
- Sync: `Engine::open` + `Engine::sync` (abre e fecha o índice por ciclo: ~50 ms de abertura, medido na Fase 9); `mcp --watch` usa o `Engine` que o servidor já segura.
- `check-update`: `VersionPointer::last_indexed_commit`, `GitSource::head_commit_oid`, `dirty_paths` (já filtrados de artefatos do motor).
- `doctor`: `Wal::pending_frames`, `INDEX_FORMAT`, `Embedder` (presença dos arquivos do modelo), `.gitignore`.
- Hooks: `gix` dá o diretório comum do repositório (worktrees compartilham `hooks/`).

## Decisões de contrato

| Comando                  | Sucesso                                 | Códigos                                                      |
| ------------------------ | --------------------------------------- | ------------------------------------------------------------ |
| todos                    | `0`                                     | `1` = erro de execução (mensagem em stderr)                  |
| `report --fail-on-cycle` | `0`                                     | `2` = ciclo de import (Fase 10)                              |
| `check-update`           | `0` = índice em dia                     | `3` = defasado (HEAD mudou ou árvore suja), `4` = sem índice |
| `doctor`                 | `0` = nenhuma falha (avisos permitidos) | `5` = ao menos uma falha                                     |

`check-update` imprime **uma linha**: `up-to-date`, `stale: <motivo>` ou `no-index`, sempre na primeira linha do stdout, para a skill consumir sem interpretar prosa.

## Componentes novos

| Componente       | Responsabilidade                                             | Local                     |
| ---------------- | ------------------------------------------------------------ | ------------------------- |
| `NamedLock`      | lock de processo por arquivo (`SyncLock` parametrizado)      | `src/sync/lock.rs`        |
| `WatchLoop`      | eventos → debounce → callback de sync; encerramento por flag | `src/workflow/watch.rs`   |
| `ignore_matcher` | `.gitignore` + `info/exclude` + pastas do motor              | `src/workflow/watch.rs`   |
| `hooks`          | bloco marcado em 3 hooks; status                             | `src/workflow/hooks.rs`   |
| `check_update`   | compara índice × `HEAD`/árvore                               | `src/workflow/check.rs`   |
| `platforms`      | escrita/remoção da config MCP por plataforma                 | `src/workflow/install.rs` |
| `doctor`         | checagens + sugestões                                        | `src/workflow/doctor.rs`  |

## Decision Log

- **D1 — `watch` abre o índice só durante o ciclo.** Segurar o lock entre ciclos trancaria o hook `post-commit` e o `sync` manual; abrir custa ~50 ms.
- **D2 — Um `watch` por repositório** com `watch.lock` (lock de SO: some se o processo morrer). Segundo `watch` falha com `1` e mensagem com o PID.
- **D3 — Filtro de eventos por `.gitignore`** (crate `ignore`, sem `git` externo) mais uma lista fixa (`.git/`, `.specs/.index/`, `.models/`, `target/`, `node_modules/`). Eventos só de artefatos do motor não disparam ciclo (evita laço: o sync escreve no índice).
- **D4 — Hooks anexam, nunca sobrescrevem.** Bloco `# >>> nexspec >>>` … `# <<< nexspec <<<`; se o hook já tem o bloco, nada muda (idempotente); `uninstall` remove só o bloco e apaga o arquivo se sobrar apenas o shebang que nós criamos. O bloco roda em segundo plano (`&`) e só se `nexspec` existir no `PATH`.
- **D5 — `install` por plataforma com escopo explícito.** Claude Code (`.mcp.json`), Gemini (`.gemini/settings.json`), Cursor (`.cursor/mcp.json`) e VS Code (`.vscode/mcp.json`) são do **projeto**; Codex só tem configuração do **usuário** (`~/.codex/config.toml`), então exige `--scope user`. Sempre `--dry-run` disponível, backup `<arquivo>.bak-<AAAAMMDDhhmmss>` antes de alterar e `uninstall`. Reescrever JSON pode perder comentários do arquivo original: dito no `--dry-run` e coberto pelo backup.
- **D6 — `check-update` não escreve.** Não cria `.specs/.index`; sem índice responde `no-index` (4).
- **D7 — `mcp --watch` (Q1):** o mesmo `WatchLoop`, chamando `Engine::sync` do servidor; sem segundo processo nem segundo lock.
- **D8 — Erros acionáveis.** Toda falha do `doctor` vem com o comando que a resolve (`nexspec hook install`, `nexspec sync`, `nexspec install --platform …`).

## Riscos

- **Tempestade de eventos** (checkout de branch grande): o debounce agrega; um único `sync` cobre tudo.
- **Hook em Windows:** o Git roda hooks com o `sh` do Git for Windows; o bloco usa só `command -v`, `&` e redirecionamentos POSIX.
- **Arquivos de configuração de terceiros** (JSON com comentários): leitura tolerante (mesmo parser do resolvedor), escrita com backup; se não der para interpretar, **não altera** e diz o motivo.
- **Teste de `watch` depende de tempo:** usa limites generosos (10 s) e verifica o resultado no índice, não a sincronização dos eventos.

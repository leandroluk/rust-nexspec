# Spec: Workflow Integration — watch, hooks e instalação em agentes (Fase 16)

> Origem: paridade com `graphify watch|update|check-update|hook|install` (`watch.py`, `hooks.py`, `install.py`).
> A skill `graph-spec-design` (outro repositório) hoje instala o hook `post-commit` à mão; a Fase 2 deixou "integração com a skill" fora deste repo.

## Summary

O `nexspec sync` é rápido e incremental, mas ainda depende de o usuário lembrar de rodá-lo e de configurar o servidor MCP à mão em cada agente. A Fase 16 entrega os comandos de fluxo diário: observar mudanças, instalar hooks de Git com segurança e registrar o MCP nos agentes.

## Requirements

- REQ-1601: **`watch [--debounce MS]`** — observa a árvore (crate `notify`, respeitando `.gitignore`) e roda `sync` incremental após debounce; um único processo por repositório (usa o lock do REQ-907); encerra limpo com Ctrl+C sem deixar WAL pendente.
- REQ-1602: **`hook install|uninstall|status`** — instala/remove hooks `post-commit`, `post-merge`, `post-checkout` chamando `nexspec sync` em segundo plano com espera de lock; não sobrescreve hooks existentes (anexa e marca o bloco); idempotente; funciona no Git Bash do Windows.
- REQ-1603: **`check-update`** — retorna código de saída distinto e mensagem curta quando o índice está defasado em relação a `HEAD`/árvore suja (para cron e para a skill), sem alterar nada.
- REQ-1604: **`install --platform <p>`** — escreve/atualiza a configuração do servidor MCP `nexspec` (`command`, `args: ["--repo", ".", "mcp"]`) nos agentes suportados (Claude Code, Gemini/Antigravity, Cursor, Codex, VS Code), com `--dry-run`, backup do arquivo anterior e `uninstall`. Não instala a skill (fica em `graph-spec-design`).
- REQ-1605: **`doctor`** — diagnóstico curto: versão, build (`full`/`lean`), modelo ONNX presente, integridade do índice (WAL pendente, versão), hooks instalados, MCP registrado por plataforma, `.gitignore` com `.specs/.index`. Sugere o comando de correção.
- REQ-1606: **Mensagens e códigos de saída estáveis** — contrato documentado para a skill consumir sem interpretar texto livre.

## Out of Scope

- Instalação da skill em plataformas (responsabilidade do repositório `graph-spec-design`).
- Daemon residente compartilhado entre repositórios.

## Open Questions

- Q1: `watch` como processo separado ou modo do servidor MCP (`mcp --watch`)? Recomendação: ambos, compartilhando o mesmo código.

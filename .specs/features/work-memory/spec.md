# Spec: Work Memory — save-result e reflect (Fase 15)

> Origem: paridade com `graphify save-result` e `graphify reflect` (`reflect.py`): laço de feedback determinístico entre as
> respostas dadas e o grafo, sem LLM.

## Summary

Cada consulta que o agente responde pode ser registrada com um resultado (`useful`, `dead_end`, `corrected`) e os nós citados.
Um agregador determinístico transforma esses sinais em "lições" (fontes preferidas, becos sem saída, correções) que o agente
carrega no início da sessão e que também ajustam o ranking. O objetivo é **economizar tokens evitando repetir becos sem saída**.

## Requirements

- REQ-1501: **`save-result`** — grava uma nota (`--question`, `--answer`, `--type`, `--nodes …`, `--outcome`, `--correction`) em `.specs/.memory/` (versionável, Markdown com frontmatter), com ids estáveis e timestamp.
- REQ-1502: **`reflect`** — agrega as notas em `LESSONS.md` determinístico: *Preferred* (nós corroborados por ≥ N respostas `useful`), *Tentative* (1 sinal), *Contested* (sinais mistos; a recência decide), *Dead ends* e *Corrections*; peso do sinal decai com meia-vida configurável (default 30 dias); nós que não existem mais no grafo são descartados.
- REQ-1503: **Sobreposição no ranking** — `search`/`query` aplicam um *boost* leve a nós preferidos e uma penalidade a becos sem saída (configurável, desligável com `--no-memory`); o efeito aparece no `nexspec bench` (Fase 8) para não degradar precisão.
- REQ-1504: **Carga por sessão com orçamento** — `reflect --max-tokens` emite só o resumo curto para a skill carregar no início; nunca o histórico bruto.
- REQ-1505: **Privacidade** — as notas ficam no repositório do usuário; nenhuma telemetria; `save-result` recusa gravar conteúdo marcado como segredo (padrões comuns de chave/token).

## Out of Scope

- Resumos gerados por LLM das lições.
- Memória compartilhada entre usuários/máquinas (usa Git, se o usuário versionar).

## Open Questions

- Q1: Guardar `.specs/.memory/` no Git por padrão? Recomendação: sim para lições (`LESSONS.md`), não para as notas brutas.

# Spec: Report Command — God nodes, comunidades e cobertura de requisitos (Fase 10)

> Origem: avaliação de 2026-09-29 (item 4). O graphify entregava `GRAPH_REPORT.md` com God nodes, comunidades e coesão; a skill
> `graph-spec-design` ainda cita "God Node (degree N)" e coesão em `design.md`/`specify.md`, mas o `nexspec` não produz nada disso.
> Depende da Fase 7 (sem arestas de dependência, grau e comunidades não têm significado).

## Summary

Um comando `nexspec report` que resume a estrutura do grafo em Markdown/JSON, com orçamento de tokens: nós de maior grau (God nodes), comunidades com coesão, requisitos sem implementação e código sem requisito. Substitui o `GRAPH_REPORT.md` e alimenta as verificações de drift/risco da skill sem exigir leitura de código.

## Requirements

- REQ-1001: **Resumo do grafo** — contagem de nós/arestas por tipo, arquivos por linguagem, idade do índice (último commit indexado) e tamanho em disco.
- REQ-1002: **God nodes** — top-N símbolos/arquivos por grau (entrada + saída de `DependsOn`/`Satisfies`/`Implements`, excluindo `CoChanges`), com grau, dependentes diretos e caminho; N configurável (default 10).
- REQ-1003: **Comunidades e coesão** — detecção determinística (propagação de rótulos/Louvain com seed fixa) sobre `DependsOn` com peso de `CoChanges`; reporta tamanho, principais arquivos e **coesão** = arestas internas ÷ arestas totais do grupo. Comunidades de baixa coesão (< 0,3) são sinalizadas como frágeis.
- REQ-1004: **Cobertura de rastreabilidade** — lista `REQ-…` sem nenhum `Satisfies` de código/task (não implementado ou sem task), tasks sem REQ, e `@spec` apontando para REQ inexistente (órfãos). Saída também com `--format json` para a skill.
- REQ-1005: **Orçamento de tokens** — `--max-tokens` aplica o mesmo pruner/serializer da Fase 5; sem a flag, imprime completo; ordenação por relevância determinística.
- REQ-1006: **Superfície MCP** — tool `graph_report` equivalente, mantendo o conjunto de tools unificado.
- REQ-1007: **Compatibilidade de contrato** — seções e nomes estáveis (`God Nodes`, `Communities`, `Requirement Coverage`) para a skill consumir; documentados no README.
- REQ-1008: **Conexões inesperadas** — arestas entre comunidades distintas com baixa probabilidade estrutural (ex.: módulo de UI dependendo de infraestrutura), ordenadas por surpresa (equivalente a `surprising_connections` do graphify), com explicação de uma linha.
- REQ-1009: **Perguntas sugeridas** — até N perguntas geradas de forma determinística a partir da estrutura (God nodes sem REQ, comunidades frágeis, ciclos, REQs órfãos) para guiar a exploração.
- REQ-1010: **Ciclos de import** — lista os ciclos do grafo de `Imports` (Fase 7) com o menor conjunto de arestas a remover; opção `--fail-on-cycle` para CI.
- REQ-1011: **`report --diff <rev>`** — compara o grafo atual com o de uma revisão (`graph_diff`): nós/arestas adicionados e removidos, God nodes que mudaram, comunidades que se fundiram/dividiram; base para revisão de PR.

## Skill (fora deste repositório — ver ROADMAP, "Evoluções externas")

Enquanto este comando não existir, `graph-spec-design` deve deixar de prometer God nodes/coesão; quando existir, `design.md`/`drift.md` passam a usar `nexspec report`.

## Out of Scope

- Visualização interativa (o `graph.html` do graphify).
- Semântica gerada por LLM (rótulos/resumos de comunidades).

## Open Questions

- Q1: Louvain vs. propagação de rótulos: qual estável/rápido o bastante para ~7 mil nós? Decidir com o benchmark da Fase 8/9.
- Q2: Incluir `CoChanges` no grau? Recomendação: não (ruído), só no peso das comunidades.

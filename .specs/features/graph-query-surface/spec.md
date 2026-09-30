# Spec: Graph Query Surface — query, path, explain, affected (Fase 11)

> Origem: paridade com `graphify query|path|explain|affected|god-nodes` (`cli.py`, `affected.py`, `serve.py`).
> Depende das arestas da Fase 7 (sem elas, `path`/`affected` não têm o que percorrer).

## Summary

Hoje o `nexspec` responde com `search` (ranking híbrido, com orçamento) e `trace` (vizinhança sem orçamento nem filtros). O graphify oferece quatro consultas de grafo com semânticas distintas e todas com orçamento de tokens. A Fase 11 entrega as equivalentes, unifica orçamento e filtros, e as expõe também no MCP.

## Requirements

- REQ-1101: **`query "<pergunta>"`** — encontra sementes por busca híbrida e expande por BFS (padrão) ou `--dfs`, respeitando `--budget N` (tokens, padrão 2000, margem de 90% da Fase 5) e `--context C` (repetível) para filtrar por contexto/relação de aresta. Saída em Markdown denso, ordenada de forma determinística.
- REQ-1102: **`path "A" "B"`** — caminho mínimo entre dois nós (símbolo, arquivo ou `REQ-…`), resolvidos por nome/ID com desambiguação explícita (lista candidatos quando ambíguo). Mostra cada salto com tipo de aresta e confiança; "sem caminho" é resultado válido, não erro.
- REQ-1103: **`explain "X"`** — descrição compacta de um nó: tipo, arquivo/linhas, assinatura podada (Fase 5), dependências e dependentes principais (limitados por orçamento), REQs/tasks que o satisfazem, comunidade (Fase 10) e últimos autores (`blame`).
- REQ-1104: **`affected "X" [--relation R]… [--depth N]`** — travessia **reversa** (quem depende de X) filtrável por relação (`imports`, `calls`, `extends`, `satisfies`…), profundidade padrão 2; agrupa por arquivo/comunidade e informa "+N omitidos" acima do teto. Substitui e generaliza o raio de impacto do `diff --staged`.
- REQ-1105: **Confiança e contexto nas saídas** — cada aresta exibida carrega `EXTRACTED` (derivada diretamente do código) ou `INFERRED` (heurística/fallback) e o contexto (`runtime`, `type-only`, `test`, `spec`); `--min-confidence` filtra.
- REQ-1106: **Orçamento uniforme** — `trace`, `search`, `query`, `path`, `explain`, `affected` aceitam `--max-tokens/--budget` e usam o mesmo serializador; sem a flag, saída completa com teto de segurança por hop.
- REQ-1107: **MCP** — tools `query_graph`, `find_path`, `explain_node`, `find_affected` (além das 6 existentes), com os mesmos parâmetros e o mesmo orçamento; nomes estáveis documentados.
- REQ-1108: **Resolução de alvo compartilhada** — um único resolvedor de alvo (`nome`, `arquivo:símbolo`, `REQ-…`, hex) para todas as consultas, com o mesmo comportamento de ambiguidade.

## Out of Scope

- Linguagem de consulta própria (Cypher/SPARQL).
- Respostas geradas por LLM (as consultas devolvem fatos do grafo; interpretação é do agente).

## Open Questions

- Q1: `trace` continua existindo ou vira alias de `affected`/`explain`? Recomendação: manter por compatibilidade (skill), documentando o mapeamento.
- Q2: Contexto de aresta como enum fixo ou string livre? Recomendação: enum pequeno definido na Fase 7.

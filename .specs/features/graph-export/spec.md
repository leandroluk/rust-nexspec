# Spec: Graph Export — JSON portátil, HTML e wiki (Fase 12)

> Origem: paridade com `graphify` (`export.py`, `exporters/html.py`, `tree_html.py`, `callflow_html.py`, `wiki.py`).
> O índice do `nexspec` é binário (redb/CSR/Tantivy) e local; um formato portátil também é pré-requisito da Fase 13.

## Summary

O graphify produzia `graph.json`, `graph.html` (visualização interativa), `GRAPH_TREE.html`, `callflow` e uma wiki navegável por agentes. O `nexspec` não exporta nada. A Fase 12 entrega o mínimo útil: JSON portátil e determinístico, um visualizador HTML estático de arquivo único e uma wiki em Markdown por comunidade. Formatos de nicho ficam adiados.

## Requirements

- REQ-1201: **`export --format json`** — grafo completo (nós, arestas, tipos, confiança, contexto, comunidades quando existirem) num JSON versionado (`schema_version`), ordenação estável (diff limpo em Git), com `--out` e `--filter` (glob de caminho, tipo de nó). Importável de volta (`nexspec import`) para a Fase 13.
- REQ-1202: **`export --format html`** — arquivo único e autocontido (sem CDN), com busca, filtro por tipo/comunidade, painel de detalhes e destaque de vizinhança; degrada para os N nós mais conectados acima de um teto (default 5000) em vez de travar o navegador.
- REQ-1203: **Árvore colapsável** — `export --format tree`: hierarquia diretório → arquivo → símbolo com as arestas de saída principais no inspetor (equivalente ao `GRAPH_TREE.html`).
- REQ-1204: **Wiki para agentes** — `export --format wiki --out <dir>`: `index.md` + um artigo por comunidade (Fase 10) com arquivos, símbolos principais, dependências entre comunidades e REQs relacionados; links relativos entre artigos.
- REQ-1205: **Determinismo e incrementalidade** — mesma entrada → mesma saída byte a byte; `--check` retorna erro se o export existente estiver defasado (para CI e hooks).
- REQ-1206: **Adiados (P3)** — GraphML, SVG, Obsidian vault, Cypher para Neo4j/FalkorDB: aceitos apenas como exportadores plugáveis sobre o JSON do REQ-1201, sem alterar o núcleo.

## Out of Scope

- Edição do grafo pela interface HTML.
- Servir o HTML por HTTP (arquivo estático).

## Open Questions

- Q1: Biblioteca de renderização do HTML (implementação própria mínima em canvas/SVG vs. embutir uma lib pequena)? Recomendação: própria e mínima, para manter o binário sem dependências de front-end.

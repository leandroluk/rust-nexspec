# Design: Graph Export (Fase 12)

## Architecture Overview

```
Engine ─► GraphSnapshot (nós + arestas) + comunidades (Fase 10) ─► ExportGraph (modelo estável, filtrado, ordenado)
                                                                       ├─ json  ─► arquivo versionado (schema_version), importável
                                                                       ├─ html  ─► arquivo único: JSON embutido + layout/render em canvas (sem CDN)
                                                                       ├─ tree  ─► arquivo único: diretório → arquivo → símbolo, inspetor de arestas
                                                                       └─ wiki  ─► index.md + um artigo por comunidade
```

Todos os formatos nascem do mesmo `ExportGraph`; HTML, árvore e wiki **não** consultam o índice de novo. Nada carrega data/hora: mesma entrada → mesma saída, byte a byte.

## Dependency Paths

- Fonte: `Engine::snapshot()` (nós e arestas) e `report::communities::communities()` (grupos de arquivos; o mapa arquivo → comunidade vem de `Community.files`).
- Nomes de relação/contexto/confiança: `query::filter::{relation_name, context_name}` e `graph::edge::decode_meta` (mesmos nomes da saída das consultas).
- Filtro de caminho: crate `globset` (já na árvore de dependências via `ignore`).
- CLI: `export --format json|html|tree|wiki`; `--check` compara com o que já existe em `--out`.

## Decisões

| Id | Decisão | Motivo |
|---|---|---|
| D1 | **Um modelo de exportação** (`ExportGraph`) com payload completo por nó (`ExportPayload`, espelho de `NodePayload`, hashes em hex) e arestas `{from, to, type, confidence, context}`. | O JSON tem que reconstruir o grafo (`import`, Fase 13) sem perder nada. |
| D2 | Ordenação: nós por id (hex), arestas por `(from, to, type)`, comunidades por id. Sem timestamps, sem commit, sem versão da ferramenta no arquivo (só `schema_version`). | Diff limpo no Git; o arquivo só muda quando o grafo muda. |
| D3 | Filtro `--path GLOB` (repetível) e `--kind K` (repetível): um nó passa se o tipo está em `--kind` (ou não há `--kind`) **e**, havendo `--path`, o nó tem caminho (arquivo, símbolo, pacote, tabela definida em arquivo) que casa; aresta só se os dois extremos passam. | REQ-1201. |
| D4 | HTML: implementação própria mínima (Q1). JSON embutido em `<script type="application/json">`, layout de forças com grade espacial (repulsão só com vizinhos da célula) e resfriamento, canvas 2D com pan/zoom, busca, filtro por tipo e comunidade, painel de detalhes, destaque de vizinhança. Acima do teto (`--max-nodes`, default 5000) ficam os N nós de maior grau e o arquivo **diz** quantos foram omitidos. | REQ-1202; sem dependência de front-end. |
| D5 | Árvore: `<details>` aninhados gerados no cliente a partir de uma lista plana (diretório → arquivo → símbolo); o inspetor mostra as arestas de saída e de entrada do nó selecionado. | REQ-1203. |
| D6 | Wiki: um artigo por comunidade listada (Fase 10) com arquivos, símbolos principais (por grau), dependências entre comunidades (contagem de arestas que cruzam) e requisitos relacionados (`Satisfies`); links relativos; `index.md` com a tabela de comunidades. Nome do arquivo = `NN-slug.md`. | REQ-1204. |
| D7 | `--check`: gera em memória e compara byte a byte com o destino (arquivo, ou diretório da wiki: conjunto de arquivos `.md` e conteúdo); sai com 7 se defasado (contrato de saída, README). | REQ-1205. |
| D8 | Exportadores extras (GraphML, Cypher…) ficam fora: o JSON é a interface (REQ-1206). | P3. |

## Componentes

| Componente | Local |
|---|---|
| modelo, filtro, JSON | `src/export/mod.rs` |
| HTML do grafo | `src/export/html.rs` |
| árvore | `src/export/tree.rs` |
| wiki | `src/export/wiki.rs` |
| API do motor | `Engine::export_graph` |

## Riscos

- **Grafos grandes no navegador**: teto de nós e layout com grade; o aviso de truncamento fica visível na página.
- **Segurança do HTML**: todo texto do grafo entra por `textContent`/JSON escapado (`</script>` neutralizado); nada de `innerHTML` com dados do repositório.
- **Payload grande no JSON**: corpos de requisitos vão inteiros (necessário para importar); `--kind`/`--path` reduzem.

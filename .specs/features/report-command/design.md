# Design: Report Command (Fase 10)

## Architecture Overview

O relatório é **análise pura sobre um retrato do grafo**. Um `GraphSnapshot` (nós decodificados + arestas + arquivo de cada símbolo) é montado uma vez a partir do `Engine`; todas as seções são funções puras sobre ele, o que as torna testáveis com grafos escritos à mão e independentes do armazenamento.

```
Engine ──snapshot()──► GraphSnapshot { nodes, edges, file_of, index info }
                              │
   ┌──────────┬───────────┬──────────┬───────────┬──────────┬───────────┐
 summary   god_nodes   communities  coverage   surprising   cycles    questions
   └──────────┴───────────┴──────────┴───────────┴──────────┴───────────┘
                              │ Report (serde)
                 render: Markdown | JSON  (+ orçamento de tokens)
                              │
              CLI `nexspec report`   MCP `graph_report`   `report --diff <rev>`
```

## Dependency Paths

- Dados: `Csr::all_edges()` (arestas com `meta` desde a Fase 7), `RedbParticipant` (payloads; **falta** uma iteração de todos os nós: T-1001), `Engine::node_payload`, `graph::cycles` (Fase 7, SCCs de `Imports`).
- Arquivo de um símbolo: aresta `DefinedIn` (símbolo → arquivo). Comunidades e "conexões inesperadas" trabalham em **nível de arquivo** (símbolos colapsam no arquivo que os define): é onde a estrutura de módulos aparece e mantém o grafo pequeno (~1,3 mil nós no repo de referência, não ~5 mil).
- Orçamento de tokens: `token::budget::{Budget, CharHeuristicTokenizer}` (Fase 5).
- `--diff <rev>`: hoje `GitSource` só enxerga `HEAD`; T-1010 adiciona uma "revisão fixa" a ele (sem `git` externo: REQ-201).

## Modelo e regras

| Seção | Regra |
|---|---|
| Summary | contagem por `NodeType` e por tipo de aresta; arquivos por linguagem (extensão); último commit indexado + data; tamanho do `index_dir` |
| God Nodes | grau = arestas de entrada + saída de `{Satisfies, Implements} ∪ dependências`; **sem** `CoChanges` (Q2) nem `DefinedIn` (ruído estrutural). Top-N (10), com grau de entrada (dependentes diretos), de saída, caminho e se tem REQ |
| Communities | grafo **não dirigido de arquivos**, peso 1 por aresta de dependência agregada entre o par de arquivos, + 0,25 por co-change (Q2: só como peso). **Propagação de rótulos determinística** (ordem por id, empate → menor rótulo, máx. 30 rodadas), sem aleatoriedade. Coesão = peso interno ÷ (peso interno + peso que sai do grupo); < 0,3 → `fragile` |
| Requirement Coverage | REQs sem `Satisfies` vindo de símbolo/task; tasks sem `Satisfies` para REQ; `Satisfies` cujo destino não existe (`@spec` órfão) |
| Surprising Connections | aresta de dependência entre comunidades distintas; surpresa = `1/(1 + arestas entre as duas comunidades)` + 0,5 se os diretórios de topo diferem + 0,25 se a origem é teste/spec e o destino é runtime (ou vice-versa não); explicação de uma linha; ordenadas por surpresa, empates por id |
| Import Cycles | SCCs de `Imports` da Fase 7 + menor conjunto **aproximado** de arestas a remover (feedback arc set guloso). Exato é NP-difícil; o relatório diz "aproximado" |
| Suggested Questions | geradas por gabarito a partir das seções anteriores, em ordem fixa, até N (5) |

## New Components

| Component | Responsibility | Location |
|---|---|---|
| `GraphSnapshot` | retrato somente-leitura do grafo | `src/report/snapshot.rs` |
| análises | summary, god nodes, cobertura, ciclos, perguntas | `src/report/analysis.rs` |
| `communities` | propagação de rótulos + coesão + conexões inesperadas | `src/report/communities.rs` |
| `render` | Markdown/JSON + orçamento | `src/report/render.rs` |
| `nexspec report` / `graph_report` | CLI e MCP | `src/bin/nexspec.rs`, `src/mcp.rs` |
| `GitSource::at_revision` | ler outra revisão sem `git` externo | `src/git/source.rs` |
| `report::diff` | compara dois relatórios | `src/report/diff.rs` |

## Decision Log

- **D1 — Análise em nível de arquivo** para comunidades/surpresas (símbolos colapsam no arquivo).
- **D2 — Propagação de rótulos, não Louvain** (Q1): determinística por construção, O(E) por rodada, poucas linhas; Louvain exige otimização de modularidade e desempates mais delicados. Reavaliar só se a qualidade for ruim nos dois repos de referência.
- **D3 — Co-change fora do grau, dentro do peso das comunidades** (Q2), com peso 0,25.
- **D4 — Contrato estável (REQ-1007):** seções `## Summary`, `## God Nodes`, `## Communities`, `## Requirement Coverage`, `## Surprising Connections`, `## Import Cycles`, `## Suggested Questions`; JSON com as mesmas chaves em `snake_case`. Documentado no README.
- **D5 — `--max-tokens` corta por seção, por prioridade** (cabeçalhos e God Nodes primeiro; listas longas truncam com "+N omitidos"), usando o mesmo estimador da Fase 5.
- **D6 — `--diff <rev>` reindexa a revisão num diretório temporário** (mesma estratégia do `bench`: o repo não é tocado) e compara relatórios; o custo é um sync por revisão.
- **D7 — `--fail-on-cycle`:** código de saída 2 (distinto de erro = 1) para CI.

## Risks

- **Qualidade das comunidades** em grafos esparsos: muitos arquivos isolados viram comunidades de tamanho 1; o relatório ignora comunidades < 3 arquivos na seção principal e as conta num resumo.
- **Determinismo** com `HashMap`: toda ordenação usa chaves explícitas (id, caminho).
- **Custo em repos grandes:** uma varredura de `all_edges()` + uma iteração de nós; medido em T-1011.

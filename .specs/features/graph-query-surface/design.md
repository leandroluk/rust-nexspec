# Design: Graph Query Surface (Fase 11)

## Architecture Overview

Quatro consultas novas (`query`, `path`, `explain`, `affected`) sobre **uma visão única do grafo** e **um único resolvedor de alvo**. Nada de armazenamento novo: a visão é o `GraphSnapshot` da Fase 10 mais listas de adjacência, e cada consulta é uma função pura sobre ela (testável com grafos escritos à mão). O `Engine` só monta a visão, resolve o alvo e adiciona o que depende de I/O (busca híbrida, assinatura podada, `blame`).

```
alvo (nome | arquivo:símbolo | REQ-… | hex) ──► TargetResolver ──► StableId | candidatos ambíguos
                                                        │
Engine ──view()──► GraphView { snapshot, fwd, rev } ──┬─► affected(reverso, filtros, agrupado)
                                                      ├─► path(menor caminho, saltos com tipo+confiança)
                                                      ├─► explain(nó + vizinhança + REQs + comunidade + autores)
                                                      └─► query(sementes da busca híbrida → BFS/DFS sob orçamento)
                                                                   │
                                          Markdown denso ─ fit_lines(orçamento de tokens, "+N omitidos")
                               CLI: query | path | explain | affected      MCP: query_graph | find_path | explain_node | find_affected
```

## Dependency Paths

- Visão: `Engine::snapshot()` (Fase 10) já entrega nós decodificados, arestas com `meta` e `file_of`. A visão acrescenta `fwd: HashMap<StableId, Vec<usize>>` e `rev` (índices em `edges`), construídos numa passada.
- Sementes do `query`: `Engine::search` (híbrida, Fases 4/8) → `SearchHit` → ids.
- `explain`: `Engine::prune_symbol_source` (hoje privado, Fase 5) para a assinatura; `Engine::blame` (Fase 2/6) para autores; comunidades de `report::communities` (Fase 10) para o rótulo; `Satisfies` do snapshot para REQs.
- Orçamento: estimador `CharHeuristicTokenizer` e margem de 90% (mesmo de `search --max-tokens` e `report --max-tokens`).
- `trace` (Fase 7) e `diff --staged` seguem existindo (Q1); `affected` generaliza o raio de impacto sem removê-los.

## Modelo

**Filtro de arestas (REQ-1101/1104/1105).** `EdgeFilter { relations: Vec<Relation>, min_confidence: Confidence, contexts: Vec<EdgeContext> }`. Nomes de relação na CLI (minúsculas): `imports`, `reexports`, `calls`, `instantiates`, `extends`, `references`, `satisfies`, `implements`, `defined_in`, `depends_on`, `cochanges`; o alias `dependencies` expande para o conjunto de dependência. Padrão: todas as de dependência + `satisfies` + `implements` (nunca `cochanges`/`defined_in`, que só entram se pedidas). `--min-confidence extracted` remove arestas `INFERRED`; `--context` (repetível) mantém só os contextos dados. Enum fixo vindo da Fase 7 (Q2).

**Resolvedor de alvo (REQ-1108).** Ordem: (1) 64 hex; (2) marcador `REQ-`/`TASK-`/`ADR-` exato; (3) `caminho/parcial:Símbolo` (sufixo de caminho + nome exato); (4) caminho de arquivo (sufixo); (5) nome exato de símbolo; (6) nome exato sem diferenciar maiúsculas; (7) nada → erro com sugestões por similaridade de prefixo. Mais de um candidato no mesmo nível → `Ambiguous(Vec<Candidate>)` (id curto, rótulo, tipo): **nunca** escolhe em silêncio; os comandos imprimem a lista e saem com erro de uso. Candidatos ordenados de forma determinística.

**`affected` (REQ-1104).** BFS reverso a partir do alvo; profundidade 2 por padrão; só arestas que passam no filtro; teto de 25 nós novos por nível (reuso da regra de confiança/contexto da Fase 7: extraídas e de runtime primeiro). Resultado agrupado por arquivo e, dentro, por comunidade (quando disponível), com "+N omitidos" por nível.

**`path` (REQ-1102).** BFS **não dirigido** (uma dependência inversa também liga dois nós), mas cada salto mostra a direção real (`->`/`<-`), o tipo e a confiança. Custa 1 por salto; arestas `cochanges` ficam de fora por padrão. Sem caminho é um resultado (`found: false`), com os dois componentes conexos descritos em uma linha (tamanhos).

**`explain` (REQ-1103).** Seções fixas: *Node* (tipo, arquivo:linhas), *Signature* (podada, só símbolo), *Depends on* / *Depended on by* (top-N por confiança, total e "+N"), *Requirements* (REQs ligados), *Community* (rótulo + coesão do arquivo), *Recent authors* (até 3, por `blame`). Cada seção degrada sozinha (sem `blame` disponível, omite a seção).

**`query` (REQ-1101).** Sementes: até 5 hits da busca híbrida (ordem do ranking). Expansão BFS (padrão) ou `--dfs` por arestas que passam no filtro, até esgotar o orçamento. Saída: sementes primeiro (com o trecho podado quando símbolo), depois vizinhos por profundidade, cada linha com relação, direção, confiança e contexto. Ordem determinística (profundidade, confiança, contexto, rótulo).

**Orçamento uniforme (REQ-1106).** Um utilitário `fit_lines(lines, budget_tokens)` mantém o prefixo que cabe e acrescenta `… +N omitted`. `trace`, `query`, `path`, `explain`, `affected` e `report` o usam; `search` continua com o pipeline da Fase 5 (mesma margem). Sem flag, saída completa com os tetos por hop já existentes.

## New Components

| Component | Responsibility | Location |
|---|---|---|
| `GraphView` | snapshot + adjacência direta/reversa | `src/query/view.rs` |
| `TargetResolver` | um resolvedor para todas as consultas | `src/query/target.rs` |
| `EdgeFilter`, `Relation` | filtros e nomes estáveis | `src/query/filter.rs` |
| `affected`, `find_path`, `explain`, `query_graph` | as quatro consultas | `src/query/{affected,path,explain,query}.rs` |
| `fit_lines` | orçamento uniforme | `src/query/budget.rs` |
| CLI + MCP | `query`/`path`/`explain`/`affected`; `query_graph`/`find_path`/`explain_node`/`find_affected` | `src/bin/nexspec.rs`, `src/mcp.rs` |

## Modified Components

| Component | Change | Risk |
|---|---|---|
| `Engine` | `view()`, `resolve_target()`, `signature_of()` (extrai de `prune_symbol_source`) | God node; só adições e uma extração |
| `trace` (CLI) | aceita `--max-tokens`, `--depth`, filtros | compatível: sem flags, igual a hoje |

## Decision Log

- **D1 — Consultas como funções puras sobre `GraphView`**, com I/O só no `Engine`: testes com grafos de 5 nós, sem repositório Git.
- **D2 — `path` não dirigido com direção exibida:** "como A se liga a B" costuma atravessar dependências em sentidos opostos (A usa C, B usa C).
- **D3 — Ambiguidade nunca é resolvida em silêncio** (REQ-1102/1108): erro com candidatos numerados; `arquivo:símbolo` e `--pick N` (índice 1-based da lista) resolvem.
- **D4 — `trace` fica** (Q1), documentado como a visão "vizinhança completa"; `affected` é "quem depende", `explain` é "o que é isto".
- **D5 — Enum fixo de contexto e relação** (Q2), nomes em minúsculas estáveis na CLI/MCP.
- **D6 — Dependência de comunidade só onde já existe:** `explain`/`affected` pedem a análise de comunidades uma vez por chamada (≈100 ms no repo de referência); se falhar, omitem o agrupamento.

## Risks

- **Nomes comuns** (`index`, `Config`, `Service`): o resolvedor devolve candidatos, e o aviso aparece no texto, não como erro mudo.
- **Custo por chamada:** montar a visão (nós + arestas) a cada consulta; medido em ~20–60 ms no repo de referência. Se pesar no servidor MCP, cachear por `sync_version` (mesma técnica do HNSW).
- **Saídas longas:** todo comando tem teto por hop e aceita orçamento; o teste de aceite exige que nenhuma consulta no repo de referência passe de 2 000 tokens com o padrão.

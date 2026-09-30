# Spec: Graphify Parity — plano para aposentar o graphify (guarda-chuva)

> Origem: decisão de 2026-09-29 — "matar o graphify". Levantamento feito lendo o código do `graphifyy 0.9.61`
> (`site-packages/graphify`, ~47 mil linhas Python) e o `--help` do CLI. Este documento **não implementa nada**: fixa
> a matriz de paridade, as prioridades e o critério objetivo de "graphify pode sair".
> Fases 7–10 já cobrem o núcleo (ver `dependency-edges`, `retrieval-benchmark`, `performance-guard`, `report-command`); as
> Fases 11–17 abaixo cobrem o restante.

## Summary

O graphify é um produto amplo: extração AST multi-linguagem + extração semântica por LLM (docs, papers, imagens, áudio),
grafo com comunidades (Leiden/Louvain), relatório, consulta (`query`/`path`/`explain`/`affected`/`god-nodes`), exportadores
(HTML, wiki, Obsidian, GraphML, Neo4j), grafo global entre repositórios, `watch`/hooks, memória de trabalho
(`save-result`/`reflect`) e instaladores para ~19 plataformas de agentes. O `nexspec` tem o núcleo determinístico e barato
(sync transacional, busca híbrida com orçamento de tokens, rastreabilidade REQ↔TASK↔código). Para aposentar o graphify é
preciso cobrir **o que o fluxo do projeto realmente usa**, e decidir conscientemente o que não será portado.

## Matriz de paridade

Legenda de prioridade: **P0** bloqueia a troca · **P1** desejável logo após · **P2** valor claro, sem urgência · **P3** só sob demanda · **Fora** decisão de não portar.

| Capacidade do graphify | Estado no nexspec | Destino | Prio |
|---|---|---|---|
| Extração AST (5 linguagens hoje; graphify ~30) | TS/JS/Python/Go/Rust | mecanismo de *language pack*; linguagens sob demanda (Fase 14) | P2 |
| Resolução entre arquivos: imports, re-exports, aliases, `calls`, `instantiates`, herança, `EXTRACTED`/`INFERRED` | só `DefinedIn` + chamadas no mesmo arquivo | **Fase 7** (ampliada) | **P0** |
| `query` BFS/DFS com orçamento e filtro de contexto de aresta | `search` + `trace` (orçamento só no `search`) | **Fase 11** | **P0** |
| `path A B` (caminho mínimo) | ausente | **Fase 11** | **P0** |
| `explain X` (nó + vizinhos em linguagem natural) | ausente | **Fase 11** | P1 |
| `affected X` (travessia reversa por relação/profundidade) | `trace` bidirecional; `diff --staged` | **Fase 11** | **P0** |
| `god-nodes`, comunidades (Leiden/Louvain), coesão, `GRAPH_REPORT.md` | ausente | **Fase 10** | **P0** |
| Conexões "surpresa" (entre comunidades), perguntas sugeridas, ciclos de import, `graph_diff` | ausente | **Fase 10** (ampliada) | P1 |
| `benchmark` (redução de tokens vs. corpus inteiro) | ausente | **Fase 8** (baseline "corpus inteiro" adicionado) | **P0** |
| Servidor MCP (`serve.py`) | 6 tools | estender com `path`/`explain`/`affected`/`report` (Fase 11) | P1 |
| `update`/`watch`/`check-update`/hooks | `sync` incremental + hook `post-commit` manual | **Fase 16** | P1 |
| `install --platform` (skill/MCP por plataforma) | ausente | **Fase 16** (config de MCP; a skill vive em `graph-spec-design`) | P1 |
| Extratores SQL / `pg_introspect` / JSON de config / `cargo` | ausentes | **Fase 14** (Liquibase/DDL + `package.json`/`tsconfig`/`Cargo.toml`) | P1 |
| Exportadores: `graph.json`, HTML interativo, `tree`, `callflow`, wiki | ausentes | **Fase 12** (JSON + HTML + wiki) | P1 |
| Exportadores: Obsidian, SVG, GraphML, Neo4j/FalkorDB | ausentes | **Fase 12** (adiados) | P3 |
| `merge-graphs`, grafo global, chamadas entre repositórios, merge driver do git | ausentes | **Fase 13** | P2 |
| Memória de trabalho: `save-result` + `reflect` | ausente | **Fase 15** | P2 |
| Extração semântica por LLM (docs, papers, imagens), rótulos de comunidade por LLM, `dedup` semântico | ausente (por design: determinístico) | **Fase 18** (anotações do agente + `SimilarTo` por embeddings) como caminho preferido; **Fase 17** (LLM próprio) opt-in atrás de portão de decisão | P2 / P3 |
| Transcrição de áudio/vídeo, Google Workspace, PDFs/Office, `add <url>`, `clone`, `prs` | ausentes | **Fora** — não pertencem a um índice de código+specs; existem ferramentas próprias | Fora |
| *Shrink guard* (recusa reescrever grafo com menos nós) | n/a | **Fora** — o sync é transacional/incremental (WAL); não há rebuild destrutivo | Fora |

## Requirements

- REQ-P01: **Matriz viva** — esta tabela é atualizada a cada fase entregue (estado e destino); linhas P0 e P1 sem dono/fase são erro de planejamento.
- REQ-P02: **Critério de aposentadoria** — o graphify só sai da skill `graph-spec-design` (precedência, referências e `.specs/graph`) quando **todos** forem verdadeiros: (a) linhas P0 entregues; (b) Fase 8 com `recall@5 ≥ 0,8` em `locate` e economia de tokens medida ≥ a estimada; (c) teste de aceitação abaixo aprovado; (d) 2 semanas de uso diário sem recorrer ao graphify.
- REQ-P03: **Teste de aceitação das 10 perguntas** — 10 perguntas reais sobre o repositório `condominium-management-system` (impacto, caminho entre módulos, God nodes, quem implementa REQ, onde está X, quem usa `CachePort`, ciclos de import, comunidades do módulo `tenant`, o que muda ao alterar `TypeORMRepositoryFor`, tabelas ligadas a `ContractService`), respondidas com `nexspec` e com graphify; cada resposta é classificada *melhor/igual/pior* por revisão humana. Aprovado com ≥ 8 *igual ou melhor* e nenhuma *pior* em perguntas P0.
- REQ-P04: **Sem regressão de fluxo** — o fluxo Specify→Design→Tasks→Execute da skill roda 100% sobre `nexspec` num projeto real (feito parcialmente em `tenant-contract`/`system-async`).
- REQ-P05: **Rollback seguro** — até REQ-P02, o graphify permanece instalável e congelado (sem rodadas de LLM) como fallback; nenhuma migração apaga `.specs/graph` antes da aprovação.

## Prioridade de execução sugerida

1. Rede de proteção e régua: Fase 9 → Fase 8.
2. Núcleo de paridade P0: Fase 7 → Fase 10 → Fase 11.
3. Fluxo diário (P1): Fase 16, Fase 14 (SQL/Liquibase), Fase 12 (JSON + HTML + wiki).
4. Depois: Fase 13, Fase 15. Fase 17 só se a Fase 8 mostrar que a falta de semântica dói.

## Out of Scope

- Reproduzir formatos internos do graphify (`graph.json` do graphify, manifest, cache): o `nexspec` define o próprio contrato.
- Compatibilidade com os *skills* de 19 plataformas do graphify.

## Open Questions

- Q1: Algum uso real do graphify no fluxo atual depende da camada semântica (LLM)? Revisar o `GRAPH_REPORT.md`/consultas passadas antes de decidir a Fase 17.
- Q2: O teste das 10 perguntas usa quais perguntas exatas? Propor a lista na Fase 8 e congelá-la em `bench/`.

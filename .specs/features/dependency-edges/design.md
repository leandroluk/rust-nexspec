# Design: Dependency Edges (Fase 7)

## Architecture Overview

O grafo hoje só sabe "este símbolo está neste arquivo" e chamadas dentro do mesmo arquivo. A Fase 7 acrescenta o que responde "quem usa X?": imports e referências entre arquivos, resolvidos para nós que já existem no índice.

```
arquivo .ts/.js ──parse (tree-sitter)──► FileFacts { declarações, imports, re-exports, usos de identificadores }
                                              │
           specifier ──► Resolver (caminhos rastreados + tsconfig/package.json) ──► caminho destino (ou nenhum)
                                              │
        ┌─────────────────────────────────────┴────────────────────────────────┐
        ▼                                                                      ▼
 Imports / ReExports  (arquivo → arquivo)                      Calls / Instantiates / Extends / References
                                                               (símbolo → símbolo; fallback arquivo → arquivo)
        └────────────────── EdgeMutation + meta (confiança, contexto) ──────────────────┘
                                              │
   Orquestrador: para cada arquivo alterado, remove as arestas/símbolos antigos que partiam dele e não foram regerados
```

Princípios:
- **Nada de "fatos do destino".** O id de um símbolo passa a ser função de `(caminho, nome, ordinal)`. Logo, dado o caminho resolvido de um import e o nome importado, o id do símbolo de destino é calculável sem ler o arquivo de destino. Se o destino não declara esse nome (re-export, tipo ambiente), a aresta aponta para um nó inexistente e é ignorada nas consultas; o `Imports` arquivo→arquivo sempre existe como rede de segurança.
- **Uma única passada por arquivo** sobre identificadores, atribuindo cada uso ao menor símbolo que o contém (o mesmo `CallResolver` já usado para chamadas), em vez de uma query Tree-sitter por construção sintática.
- **Resolução conservadora** (REQ-711): só liga com destino único; ambíguo vira `INFERRED` ou nenhuma aresta.

## Dependency Paths

Lidos no código:

- `SyncOrchestrator::run_once` (`src/sync_orchestrator.rs`) → pass 2: `code::extract_all` (rayon) → `code::extract` (`src/code/parser.rs`) → `MutationSet`. Hoje `extract` só conhece o texto de **um** arquivo; a resolução precisa também do conjunto de caminhos rastreados e de `tsconfig.json`/`package.json`, que o orquestrador tem (`GitSource::tracked_paths_at_head`, `read_blob_at_head`).
- Símbolos: `stable_id(format!("{name}@{line_start}"))` (`parser.rs`): **instável** — editar linhas acima muda o id e deixa o nó antigo no índice (nada o remove). A Fase 7 precisa trocar isso (T-702).
- Arestas: `Edge { id, from, to, edge_type }` (`src/graph/edge.rs`), sem payload; `EdgeMutation::Upsert` tem `payload`, que o `CsrParticipant` descarta. Confiança/contexto (REQ-710) exigem um campo novo em `Edge` (T-701).
- Consumidores: `Engine::trace` (`src/engine.rs`) segue `{Satisfies, DependsOn, DefinedIn, Implements}` e inverte só `Satisfies/DependsOn/Implements`; `search` expande 1 salto pelos mesmos tipos; `diff_staged` usa `DependsOn` para o raio de impacto. Todos precisam conhecer o vocabulário novo (T-707).
- Remoção de arestas antigas: `CsrDelta::remove` e `EdgeMutation::Remove` existem; o que falta é saber *quais* arestas saíam de um arquivo. `Csr::all_edges()` dá isso com uma varredura (dezenas de milhares de arestas: milissegundos).

## Vocabulário de arestas (REQ-709/710)

| Tipo           | De → Para                 | Quando                                                |
| -------------- | ------------------------- | ----------------------------------------------------- |
| `Imports`      | arquivo → arquivo         | `import`/`require`/`import()` resolvido               |
| `ReExports`    | arquivo → arquivo         | `export … from`, `export * from`                      |
| `Calls`        | símbolo → símbolo/arquivo | `f()` para nome importado ou do mesmo arquivo         |
| `Instantiates` | símbolo → símbolo         | `new X()`                                             |
| `Extends`      | símbolo → símbolo         | `extends` / `implements`                              |
| `References`   | símbolo → símbolo         | tipo de parâmetro/propriedade, decorator, demais usos |

`DependsOn` **deixa de ser emitido** e vira o agregado de consulta (`EdgeType::is_dependency()` = qualquer um dos seis acima + o próprio `DependsOn` para índices antigos/sintéticos). As chamadas do mesmo arquivo passam a ser `Calls`.

`Edge.meta: u8`: bit 0 = confiança (`0` EXTRACTED, `1` INFERRED); bits 1–2 = contexto (`0` runtime, `1` type-only, `2` test, `3` spec). Viaja em `EdgeMutation::payload[0]`.

## Resolução de especificadores (REQ-702)

Ordem: (1) relativo (`./`, `../`) + extensões `.ts .tsx .js .jsx .mjs .cjs .mts .cts` e `index.*`; (2) `paths`/`baseUrl` do `tsconfig.json` mais próximo (com `extends` simples); (3) `imports` do `package.json` (`#/*`); (4) pacote do workspace (`@scope/pkg` → `package.json` do pacote → `exports`/`source`/`main` → arquivo em `src/`); (5) qualquer outra coisa: externo, **ignorado**. Leitura direta de JSON (com comentários e vírgulas finais tolerados), sem dependência nova (Q1).

## Incrementalidade (REQ-706)

Para cada arquivo de código adicionado/modificado/apagado F, o orquestrador:
1. levanta, numa varredura de `all_edges()`, as arestas de dependência que saem do nó-arquivo de F e dos símbolos de F (símbolos = origens de `DefinedIn → F`);
2. regera as de F e remove (`EdgeMutation::Remove`) as antigas não regeradas; remove também símbolos que deixaram de existir (e suas arestas `DefinedIn`/`Satisfies`);
3. para arquivo **apagado**: remove nó, símbolos e as arestas de dependência que **chegavam** neles.

Limite conhecido (D5): um import que não resolvia e passa a resolver porque *outro* arquivo foi criado só é ligado quando o importador for reprocessado (mudar ou reindexar). Na prática renomeações vêm acompanhadas da edição dos importadores no mesmo commit.

## New Components

| Component                                      | Responsibility                                                    | Location                   |
| ---------------------------------------------- | ----------------------------------------------------------------- | -------------------------- |
| `EdgeMeta`, `Confidence`, `EdgeContext`        | bits de confiança/contexto                                        | `src/graph/edge.rs`        |
| `symbol_node_id(path, name, ordinal)`          | id estável de símbolo                                             | `src/graph/node.rs`        |
| `FileFacts` + extração de imports/exports/usos | fatos de um arquivo TS/JS                                         | `src/code/facts.rs`        |
| `SpecifierResolver`                            | especificador → caminho rastreado                                 | `src/code/resolve.rs`      |
| `dependency_edges`                             | fatos + resolver → `EdgeMutation`s                                | `src/code/deps.rs`         |
| Reconciliação incremental                      | arestas/símbolos antigos → `Remove`                               | `src/sync_orchestrator.rs` |
| Fixture `ts_workspace`                         | alias `paths`, barrel, re-export, type-only, ciclo, não resolvido | `tests/fixtures/`          |

## Modified Components

| Component                          | Change                                                                                       | Risk                                                                                |
| ---------------------------------- | -------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| `EdgeType`                         | seis tipos novos; `is_dependency()`                                                          | códigos novos no fim (5–10); índices antigos continuam legíveis                     |
| `Edge` (CSR)                       | campo `meta`                                                                                 | toca 16 construções; `INDEX_FORMAT` → 5                                             |
| `code::extract`                    | ids de símbolo estáveis; chamadas do mesmo arquivo viram `Calls`; aceita contexto do arquivo | God node do pipeline de código; testes existentes mudam de `DependsOn` para `Calls` |
| `SyncOrchestrator::run_once`       | resolver + reconciliação por arquivo                                                         | God node de escrita                                                                 |
| `Engine::trace/search/diff_staged` | usam `is_dependency()`; teto por hop com "+N omitidos"                                       | muda saída do `trace`                                                               |

## Decision Log

- **D1 — Id de símbolo por `(caminho, nome, ordinal)`** em vez de `nome@linha`: estável a edições acima e permite calcular o destino sem ler o arquivo. Custo: ordinal muda se a *ordem* de homônimos no arquivo mudar (raro: sobrecargas). `INDEX_FORMAT` sobe.
- **D2 — Sem `DependsOn` emitido**; agregado em consulta. Evita duplicar ~10 mil arestas e mantém os tipos informativos para a Fase 11.
- **D3 — `meta` em `Edge`**, não em arquivo lateral: consultas filtram sem segunda leitura.
- **D4 — Contexto `test`/`spec`** decidido pelo caminho do arquivo de origem (`*.spec.*`, `*.test.*`, `/test/`, `/__tests__/`, `e2e`); `type-only` por `import type`/`export type` ou uso só em posição de tipo.
- **D5 — Não reprocessa importadores não alterados** (limite acima); reindexar resolve.
- **D6 — Segunda entrega (Python/Go/Rust, REQ-705)** só arquivo→arquivo, pelo mesmo caminho `Imports`; primeiro fecha-se TS/JS contra o benchmark.
- **D7 — Medida de sucesso:** no benchmark da Fase 8, `structure` sai de 0,00 (corpus próprio e condominium); aceite do spec: `trace CachePort` lista os 7 usuários e nenhuma aresta falsa numa amostra de 30.

## Risks

- **Nomes comuns** (`index`, `Config`) casando com símbolos errados: resolução só por import resolvido (nunca por nome solto) e confiança `INFERRED` quando o destino não declara o nome.
- **Explosão em God nodes** (um tipo importado por centenas de arquivos): teto por hop no `trace` (REQ-707) e contagem de omitidos.
- **Custo do primeiro sync** (REQ-708, ≤ +20%): extração já roda em rayon; o resolver usa um `HashSet` de caminhos e cache por diretório.
- **Identificadores em posições que não são referência** (propriedades `obj.name`, chaves de objeto): só se consideram identificadores cujo texto é um nome **importado** ou declarado no arquivo e que não são nome de propriedade/chave.

## Resultados (T-708, 2026-09-30)

Medido no clone local do condominium-management-system (1.267 arquivos), `--release`.

- **Aceite do spec.** `trace CachePort` lista todos os usuários no código: `CacheRedisAdapter` (`Extends`), os 3 interceptors, `redis.adapter.ts` (nível de arquivo) e os 2 specs (`[spec]`); `git grep` dá exatamente esses 6 arquivos mais a definição (o "7" do spec contava a definição). `trace AccessUserPersonaReader` lista os 10 usuários + `TypeORMAccessUserPersonaReader`, igual ao `git grep` (12 arquivos − 1 definição − 1 menção em `STATE.md`).
- **Amostra de arestas.** 40 arquivos sorteados, `Imports` do `trace` contra uma leitura independente dos imports relativos: 9 esperadas, 9 achadas, 0 faltando. As 11 arestas "a mais" eram imports de pacote que o leitor independente não cobria e foram conferidas à mão: `#/oidc.port` (`package.json#imports`) e `@pkgs/nest-core` (pacote do workspace). Nenhum falso positivo encontrado.
- **Custo (REQ-708).** `sync` a frio, mediana de 5 execuções alternadas, antes (`0b23c99`) × depois: 1,30 s × 1,18 s (razão 0,91, dentro do teto de 1,20). `sync` sem mudanças 19 ms. Teste de orçamento no repo sintético: a frio 2,9 s, 1 arquivo 0,25 s.
- **Benchmark (BM25), kind `structure`:** condominium recall@5 0,00 → **1,00**; corpus próprio (Rust, só arestas de arquivo) recall@5 0,00 → 0,25 e recall@10 0,00 → **1,00**. Nos demais `kind`s, sem mudança.
- **Decisão de busca.** Expandir a busca pelas arestas de dependência *para frente* inflava todas as respostas (+35% de tokens no condominium) sem ganho de recall; a expansão ficou nos tipos semânticos de antes. Dependentes ("quem usa X") entram na busca **só quando a pergunta pede** (`what uses`, `callers of`, `quem usa`, …): mesma economia de tokens nas demais perguntas.
- **Não feito (por escopo):** símbolo-level em Rust/Python/Go (só arquivo→arquivo, como o spec pede); religar importadores não alterados quando um arquivo novo passa a resolver (limite D5).

# Design: Multi-Repo Graph (Fase 13)

## Architecture Overview

```
repo A index ─► export (Fase 12) ─┐                                   ┌─► merge-graphs a.json b.json --out g.json
repo B index ─► export ───────────┼─► tag + prefix ids ─► merge ─────┤
export.json (arquivo) ────────────┘      + cross-repo links            └─► global add ─► ~/.nexspec/global/repos/<tag>.json ─► rebuild ─► índice global
                                                                                                                              │
                                                              query/path/explain/affected --global [--repo tag] ◄─────────────┘
```

O grafo global **é um índice nexspec comum** (mesmo formato, resolve Q1), reconstruído por inteiro a partir dos exports guardados em `~/.nexspec/global/repos/`. Reconstruir é barato (dezenas de milhares de nós, segundos), idempotente e torna `add` (mesmo `--as` substitui), `remove` e a ligação entre repositórios triviais: não há contabilidade de posse por nó.

## Dependency Paths

- Export (Fase 12): fonte de tudo; ganha o campo opcional `repo` por nó/aresta (REQ-1301) e `Package.dependencies`.
- Índice: `Engine::open(<global>/index, <global>)` (sem git; só consultas de grafo) e `Engine::apply_mutations` (novo) para gravar o grafo reconstruído por um ciclo do `Coordinator`.
- Consultas (Fase 11): `query_view`/`TargetResolver` já trabalham sobre `GraphSnapshot`; `--repo` restringe a resolução do alvo a um repositório.
- Hooks (Fase 16): `hook install` passa a registrar o driver de merge.

## Decisões

| Id | Decisão | Motivo |
|---|---|---|
| D1 | **Id global = blake3(`repo:<tag>:<id hex>`)**; arestas recebem id derivado dos extremos já mapeados. Sem colisão mesmo para caminhos iguais (`package.json` existe em todo repositório). | REQ-1301. |
| D2 | **A origem fica visível pelo próprio nome do nó**: caminho de arquivo e diretório de pacote ganham o prefixo `<tag>/`; `schema` de tabela vira `<tag>:<schema>`; título de requisito/tarefa/ADR vira `<tag>:<título>`. Símbolos herdam pelo arquivo. Nenhuma consulta existente precisa mudar e todo resultado mostra de que repositório veio (REQ-1305). | `NodePayload` não tem campo de repositório; acrescentar um mudaria todo o armazenamento. |
| D3 | `merge-graphs` = **tag + união**: cada entrada ganha o tag (`--as` ou nome do arquivo), nós e arestas são unidos por id global, um tag repetido é substituído (**o argumento mais tardio vence**: exports não trazem commit, por decisão D2 da Fase 12 — mantê-los sem data/commit é o que dá diff limpo). | REQ-1303; regra documentada. |
| D4 | Ligação por **pacote**: `Package.dependencies` (todas as dependências declaradas, inclusive externas) × nomes de pacotes de outros repositórios → `DependsOn` entre `Package`, `INFERRED` (baseado em nome). | REQ-1304. |
| D5 | Ligação por **HTTP declarado**: nós `Endpoint{method, path, operation_id, external}`. `external = false` vem de OpenAPI (`openapi.json/yaml`, `swagger.json`); `external = true` vem de chamadas de cliente com caminho literal (`fetch('/x')`, `axios.get(...)`, `http.get(...)`, `requests.get(...)`), ligadas ao símbolo/arquivo que chama (`Calls`). No grafo global, endpoint externo e endpoint definido em outro repositório com o mesmo método e caminho normalizado (`{id}`, `:id`, `<id>` → `{}`; host e prefixo de versão ignorados só se o caminho restante casa) geram `Calls` inferida do consumidor para o provedor. | REQ-1304 sem executar nem inferir tipos. |
| D6 | Pasta global: `$NEXSPEC_HOME/global` ou `~/.nexspec/global`; `repos/<tag>.json` (export guardado) + `index/` (reconstruído). `global path` imprime a pasta. | REQ-1302. |
| D7 | `global add <repo|export.json> [--as TAG]`: diretório → abre o índice local dele (`<repo>/.specs/.index`) e exporta; arquivo → lê o JSON. Tag padrão = nome da pasta/arquivo (sem extensão). | REQ-1302. |
| D8 | `--global` troca o índice consultado pelo global; `--repo TAG` (só com `--global`) restringe a **resolução do alvo** ao repositório (a travessia atravessa repositórios — é o objetivo). | REQ-1305. |
| D9 | Driver de merge: `nexspec merge-driver %O %A %B` faz a união (nós e arestas por chave) de A e B e grava em A; não representa remoções (união, como a spec pede). `hook install` acrescenta `*.graph.json merge=nexspec` a `.gitattributes` e a seção `[merge "nexspec"]` ao `.git/config`, em blocos marcados e removíveis. | REQ-1306. |

## Componentes

| Componente | Local |
|---|---|
| tag, prefixo, união, ligação entre repositórios | `src/global/merge.rs` |
| pasta global, `add/list/remove/path`, reconstrução | `src/global/store.rs` |
| endpoints (OpenAPI e chamadas de cliente) | `src/domain/http.rs` |
| driver de merge | `src/global/driver.rs` |
| consultas globais | `src/bin/nexspec.rs`, `src/query/target.rs` |

## Riscos

- **Ligação por nome gera falsos positivos** (dois pacotes com o mesmo nome em repositórios diferentes): sempre `INFERRED`, e as consultas já filtram por `--min-confidence extracted`.
- **Caminhos HTTP genéricos** (`/health`, `/:id`): só casam método + caminho completo normalizado; caminhos de um segmento só são ignorados.
- **Reconstrução a cada `add`**: aceitável no tamanho esperado; o custo é medido e registrado.

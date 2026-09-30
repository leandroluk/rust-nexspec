# Design: Domain Extractors (Fase 14)

## Architecture Overview

```
tracked files + dirty ──► candidates ──► domain pass (whole-repo, deterministic)
   *.sql, changelog/changeset XML|YAML      ├─ SchemaBuilder (SQL statements in file order) ─► Table/View/Column/Constraint
   package.json, tsconfig*, pnpm-workspace, ├─ manifests ─► Package, DependsOn(workspace)
   Cargo.toml, entity-looking code          └─ ORM bridge ─► Implements(entity symbol → Table)
                                            ▼
                     full domain graph ──► diff against the domain nodes already indexed ──► upserts + removals
```

Um esquema é cumulativo (o changeset 050 altera a tabela criada pelo 005), então o passe de domínio não é por arquivo: relê **todos** os candidatos quando qualquer um deles — ou qualquer arquivo de código alterado que mencione um marcador de entidade — mudou, monta o grafo inteiro e o reconcilia com o que já está indexado. Tudo é determinístico e idempotente (ids estáveis, saída ordenada).

## Dependency Paths

- Nós/arestas: `src/graph/{node,edge}.rs` ganham quatro payloads (`Table`, `Column`, `Constraint`, `Package`) e os `NodeType`s correspondentes; **nenhum tipo de aresta novo** (`DefinedIn`, `References`, `Implements`, `DependsOn` já existem).
- Orquestrador: `src/sync_orchestrator.rs` ganha um passe de domínio depois do passe de código; `Engine::sync` entrega os ids dos nós de domínio já indexados (de `RedbParticipant::all_nodes`).
- Manifestos reaproveitam os parsers JSONC/tsconfig/package de `src/code/resolve.rs` (uma única fonte para `paths`, `extends`, `exports`, `imports`, workspaces).
- Tantivy/busca: `describe()` em `src/search/schema.rs` torna tabelas, colunas, constraints e pacotes achados por nome (ciente de identificadores: `tb_contract_reminder` casa com "contract reminder").
- Consultas: `explain`, `affected` e `path` já funcionam em qualquer nó; só rótulos/tipos precisam das novas variantes (`report/snapshot.rs`, `mcp.rs`, `engine.rs`).

## Decisões

| Id | Decisão | Motivo |
|---|---|---|
| D1 | **Parser SQL mínimo próprio**, subconjunto Postgres (`CREATE TABLE`, `CREATE [OR REPLACE] VIEW`, `CREATE [UNIQUE] INDEX`, `ALTER TABLE … ADD/DROP COLUMN/CONSTRAINT`, `DROP TABLE/VIEW/INDEX`), sem dependência `sqlparser`; o que não for reconhecido é ignorado, nunca fatal (Q1). | O DDL do projeto de referência é regular; uma dependência soma tempo de compilação e uma gramática de que usamos 5 %. O que não entendemos custa uma aresta faltando, não um sync quebrado. |
| D2 | Liquibase XML: blocos `<sql>` (CDATA) e elementos `createTable`/`column`/`constraints` lidos por um scanner de tags pequeno; YAML: blocos `sql:` e `createTable`; arquivos `.sql` (puros ou Liquibase formatted) inteiros. | Sem crate de XML/YAML para o subconjunto que importa. |
| D3 | Arquivos processados em ordem de caminho (a numeração dos changesets é a ordem de aplicação). | O esquema depois do último changeset é o que o código enxerga. |
| D4 | Tabelas, views, colunas e constraints são nós distintos; coluna/constraint `DefinedIn` tabela; tabela `DefinedIn` o arquivo que a criou primeiro. Nomes preservados sem aspas (`ix_tb_…`, `uq_tb_…`, `fk_tb_…`). | REQ-1402. |
| D5 | FK → `References` tabela→tabela (extraída); view → `References` para tabelas achadas depois de `FROM`/`JOIN` que existam no esquema (inferida: textual). | REQ-1402. |
| D6 | Ponte ORM: `@Entity({name:'x'})`/`@Entity('x')` (TypeORM), `__tablename__ = "x"` (SQLAlchemy) e `@@map("x")` no `.prisma`; símbolo da classe → tabela `Implements`, **extraída** para nome literal. Candidatos são arquivos de código rastreados que contêm o marcador. | REQ-1403 sem executar nada. |
| D7 | Manifestos: `package.json`, `tsconfig*.json`, `pnpm-workspace.yaml`, `Cargo.toml` (+ membros do workspace). Nós `Package` para membros do workspace; `DependsOn` só entre pacotes do workspace (dependências externas ficam listadas no nó, não como nós). | REQ-1404; mantém o grafo sobre o repositório. |
| D8 | Reconciliação: upsert do grafo de domínio inteiro; remove nós de domínio que sumiram e toda aresta que tocava um nó removido. | Idempotente, pequeno (centenas de nós), sem contabilidade de posse por arquivo. |
| D9 | Banco vivo (`extract --postgres DSN`): crate `postgres` sem TLS, sessão somente leitura, só `SELECT` em catálogos; o resultado é comparado com o esquema dos changesets e impresso como relatório de divergência; nunca escreve no índice por padrão. | REQ-1405; opt-in; nunca DDL. |
| D10 | Language packs (REQ-1406) seguem sob demanda: `code::Language` já é a tabela de extensões, gramáticas e queries; uma linguagem nova é uma variante + uma linha por tabela, sem mudar o orquestrador. Nenhuma é adicionada agora (P3: um projeto real precisa pedir). | A spec diz isso. |
| D11 | Orçamento (REQ-1407): o passe só lê arquivos candidatos; medido no clone do condominium contra o número da Fase 9. | REQ-1407. |

## Componentes

| Componente | Local |
|---|---|
| payloads de nó, ids | `src/graph/node.rs`, `src/domain/ids.rs` |
| parser de instruções SQL + esquema | `src/domain/sql.rs`, `src/domain/schema.rs` |
| leitor Liquibase | `src/domain/liquibase.rs` |
| ponte ORM | `src/domain/orm.rs` |
| manifestos | `src/domain/manifest.rs` |
| montagem do grafo + reconciliação | `src/domain/graph.rs`, `src/sync_orchestrator.rs` |
| banco vivo + drift | `src/domain/postgres.rs` |

## Riscos

- **SQL não entendido** (funções, blocos DO, tipos exóticos): instruções ignoradas e contadas (`extract --verbose`).
- **Views** sobre CTEs ou subconsultas: nomes achados por texto podem incluir nomes de CTE; só nomes que existem no esquema viram aresta.
- **Custo do primeiro sync**: o passe lê arquivos de código rastreados só para dois marcadores de entidade (busca de bytes), sem parsear.

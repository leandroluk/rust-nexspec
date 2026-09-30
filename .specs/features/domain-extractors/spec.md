# Spec: Domain Extractors — SQL/Liquibase, configs e language packs (Fase 14)

> Origem: paridade com `extractors/sql.py`, `pg_introspect.py`, `json_config.py`, `cargo_introspect.py` e ~25 linguagens extras
> do graphify. Motivação local: o projeto `condominium-management-system` tem 130 changesets Liquibase (`tb_*`, views, FKs)
> e entidades TypeORM que espelham as tabelas — hoje invisíveis para o grafo.

## Summary

O grafo só entende código nas 5 linguagens suportadas e Markdown de specs. A Fase 14 adiciona extratores por domínio (esquema de
banco, manifestos de pacote/config) e um mecanismo para incluir linguagens novas com custo baixo, ligando **tabela ↔ entidade
↔ repositório ↔ usecase** e **pacote ↔ dependência**.

## Requirements

- REQ-1401: **Mecanismo de *language pack*** — cada linguagem/formato declara extensões, gramática Tree-sitter (ou parser dedicado), query de símbolos, query de imports e resolvedor de módulos; adicionar uma linguagem não toca o orquestrador. Cobertura mínima de testes por pack (símbolos, imports, arestas).
- REQ-1402: **DDL/SQL e Liquibase** — extrai de `*.sql` e de changesets XML/YAML (`<sql>`/`createTable`/`CREATE TABLE|VIEW|INDEX`): nós `Table`, `View`, `Column` (nome/tipo/nulo), `Index`/`Constraint`; arestas `References` (FK) tabela→tabela e view→tabela. Nomes de índice/constraint preservados (`ix_tb_…`, `uq_tb_…`, `fk_tb_…`).
- REQ-1403: **Ponte tabela ↔ código** — liga `Table` a entidades ORM por nome (`@Entity({name: 'tb_x'})`, TypeORM/Prisma/SQLAlchemy quando houver) com aresta `Implements`/`MapsTo` `INFERRED`→`EXTRACTED` quando o nome é literal; `affected tb_contract_reminder` alcança entidade, repositório e usecases.
- REQ-1404: **Manifestos** — `package.json` (name, dependências, `exports`/`imports`, workspaces), `tsconfig*.json` (`paths`, `extends`), `pnpm-workspace.yaml`, `Cargo.toml`/workspace: nós `Package` e arestas `DependsOn` entre pacotes do workspace. Os mesmos dados alimentam a resolução de imports da Fase 7 (fonte única).
- REQ-1405: **Introspecção opcional de banco vivo** — `extract --postgres <DSN>` (somente leitura, opt-in) adiciona tabelas/views/funções/FKs reais e sinaliza divergência entre changesets e banco (`drift`). Nunca executa DDL.
- REQ-1406: **Linguagens sob demanda** — prioridade: Java, C#, PHP, Ruby, Kotlin, Swift, C/C++, Shell. Cada uma entra só quando um projeto real a pedir (P3), seguindo REQ-1401.
- REQ-1407: **Orçamento de sync** — extratores novos respeitam os limites da Fase 9 (primeiro sync do repositório de referência não excede +25%).

## Out of Scope

- Semântica de negócio inferida por LLM sobre o esquema.
- Geração de migrações ou DDL.

## Open Questions

- Q1: Liquibase em XML com SQL embutido (`<sql><![CDATA[…]]></sql>`, como neste projeto): parser SQL próprio mínimo (`CREATE TABLE/VIEW/INDEX`) ou dependência (`sqlparser`)? Recomendação: `sqlparser` restrito ao dialeto Postgres, com fallback regex para blocos não reconhecidos.

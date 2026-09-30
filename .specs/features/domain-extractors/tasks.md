# Tasks: Domain Extractors (Fase 14)

## T-1401: Modelo de nós de domínio (REQ-1401, REQ-1402) [x]
- **What**: payloads `Table`/`Column`/`Constraint`/`Package`, `NodeType`, ids estáveis, documentos de busca, `INDEX_FORMAT` 7, todos os `match` do crate.
- **Where**: `src/graph/node.rs`, `src/domain/ids.rs`, `src/search/schema.rs`, `src/engine.rs`, `src/mcp.rs`, `src/report/snapshot.rs`, `src/query/target.rs`
- **Gate**: `cargo test graph:: search::`

## T-1402: SQL, esquema acumulado e Liquibase (REQ-1402) [x]
- **What**: parser do subconjunto Postgres, `SchemaBuilder` (CREATE/ALTER/DROP em ordem), leitor de XML/YAML/`.sql`; FK e views.
- **Where**: `src/domain/{sql,schema,liquibase}.rs`
- **Gate**: `cargo test domain::`

## T-1403: Ponte tabela ↔ entidade (REQ-1403) [x]
- **What**: TypeORM, SQLAlchemy e Prisma por nome literal; `Implements` extraída.
- **Where**: `src/domain/orm.rs`
- **Depends on**: T-1401
- **Gate**: `cargo test domain::orm`

## T-1404: Manifestos (REQ-1404) [x]
- **What**: `package.json`, `tsconfig*`, `pnpm-workspace.yaml`, `Cargo.toml`; `Package` + `DependsOn` entre membros do workspace, reaproveitando os parsers do resolvedor.
- **Where**: `src/domain/manifest.rs`, `src/code/resolve.rs`
- **Depends on**: T-1401
- **Gate**: `cargo test domain::manifest`

## T-1405: Passe de domínio no sync (REQ-1402..1404, REQ-1407) [x]
- **What**: montagem do grafo, reconciliação, gatilho (arquivo de domínio ou marcador de entidade alterado), integração no orquestrador e no `Engine`; consultas (`explain`/`affected`) funcionando; medição de orçamento.
- **Where**: `src/domain/graph.rs`, `src/sync_orchestrator.rs`, `src/engine.rs`
- **Depends on**: T-1402..T-1404
- **Gate**: `cargo test --test domain_sync`

## T-1406: Banco vivo e drift (REQ-1405) [ ]
- **What**: `extract --postgres DSN` somente leitura, comparação com o esquema dos changesets, relatório de divergência; teste vivo `#[ignore]`.
- **Where**: `Cargo.toml`, `src/domain/postgres.rs`, `src/bin/nexspec.rs`
- **Depends on**: T-1402
- **Gate**: `cargo test domain::postgres`

## T-1407: Fechamento [ ]
- **What**: README/docs/ROADMAP/STATE; REQ-1406 registrado como sob demanda; CI verde.
- **Gate**: `cargo test && cargo test --no-default-features --features lean && cargo clippy --all-targets -- -D warnings`

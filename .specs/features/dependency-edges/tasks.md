# Tasks: Dependency Edges (Fase 7)

## T-701: Vocabulário de arestas e `meta` (REQ-709, REQ-710) [x]
- **REQ**: REQ-709, REQ-710
- **What**: `EdgeType` ganha `Imports, ReExports, Calls, Instantiates, Extends, References` (códigos 5–10) e `is_dependency()`. `Edge.meta: u8` com `EdgeMeta` (confiança/contexto). `CsrParticipant` lê `payload[0]` → `meta`. `INDEX_FORMAT` 5.
- **Where**: `src/graph/edge.rs`, `src/graph/csr/*`, `src/engine.rs`, testes que constroem `Edge`
- **Depends on**: none
- **Done when**: roundtrip de `Edge` com `meta` (rkyv e CSR base/delta); `from_code`/`to_code` dos novos tipos; `meta` sobrevive a compactação.
- **Gate**: `cargo test graph::`

## T-702: Ids de símbolo estáveis e sem símbolos órfãos (REQ-706 parcial) [x]
- **REQ**: REQ-706
- **What**: `symbol_node_id(path, name, ordinal)`; `code::extract` usa. Orquestrador remove símbolos (e suas arestas `DefinedIn`/`Satisfies`) que sumiram de um arquivo modificado ou apagado.
- **Where**: `src/graph/node.rs`, `src/code/parser.rs`, `src/sync_orchestrator.rs`
- **Depends on**: T-701
- **Done when**: editar linhas acima de uma função mantém o mesmo id; remover uma função do arquivo remove o nó e as arestas; apagar o arquivo remove todos os símbolos dele.
- **Gate**: `cargo test --test sync_orchestrator --test regressions && cargo test code::`

## T-703: Fatos de arquivo TS/JS (REQ-701, REQ-711) [ ]
- **REQ**: REQ-701, REQ-711
- **What**: `FileFacts { imports[{specifier, names[{imported, local, type_only}], namespace, kind}], reexports[...], usages[{local name, byte, role}] }` via Tree-sitter: `import … from`, `import type`, `export … from`, `export * [as ns] from`, `import("x")`, `require("x")`; usos de identificadores com o papel sintático (heritage, `new`, chamada, decorator, tipo, outro), ignorando nomes de propriedade/chave.
- **Where**: `src/code/facts.rs`
- **Depends on**: none
- **[P]**: A (paralelo com T-701/T-702)
- **Done when**: testes por construção (cada forma de import/export; alias; type-only; namespace; `require`; dynamic import; `obj.name` não conta como uso).
- **Gate**: `cargo test code::facts`

## T-704: Resolução de especificadores (REQ-702) [ ]
- **REQ**: REQ-702
- **What**: `SpecifierResolver::new(tracked_paths, read_file)` resolve relativo + extensões + `index.*`, `tsconfig` (`paths`, `baseUrl`, `extends`), `package.json#imports` (`#/*`) e pacotes do workspace; externo → `None`.
- **Where**: `src/code/resolve.rs`
- **Depends on**: none
- **[P]**: A
- **Done when**: fixture com alias `paths`, barrel `index.ts`, `.js` apontando para `.ts`, workspace `@scope/pkg`, pacote externo (→ `None`), tsconfig com comentários.
- **Gate**: `cargo test code::resolve`

## T-705: Arestas de dependência (REQ-703, REQ-704, REQ-710) [ ]
- **REQ**: REQ-703, REQ-704, REQ-710
- **What**: `dependency_edges(path, facts, resolver, symbols)` emite `Imports/ReExports` (arquivo→arquivo) e `Calls/Instantiates/Extends/References` (símbolo→símbolo, fallback arquivo→arquivo), com `meta` (confiança por destino declarado; contexto por caminho/posição). Chamadas do mesmo arquivo passam a `Calls`. Integra no pass 2 do orquestrador.
- **Where**: `src/code/deps.rs`, `src/code/parser.rs`, `src/sync_orchestrator.rs`
- **Depends on**: T-701, T-702, T-703, T-704
- **Done when**: fixture `ts_workspace` produz as arestas esperadas (incl. `extends`, `implements`, `new`, tipo de parâmetro de construtor, decorator, `import type` = type-only, arquivo `*.spec.ts` = test, não resolvido = nenhuma aresta).
- **Gate**: `cargo test --test dependency_edges`

## T-706: Incrementalidade (REQ-706) [ ]
- **REQ**: REQ-706
- **What**: por arquivo de código alterado/apagado: varredura de `all_edges()` → `Remove` das arestas de dependência antigas que partiam dele e não foram regeradas; apagado remove também as que chegavam. Documentar o limite D5.
- **Where**: `src/sync_orchestrator.rs`
- **Depends on**: T-705
- **Done when**: remover um `import` remove a aresta; renomear/apagar arquivo não deixa aresta órfã; segundo sync sem mudanças não emite nada.
- **Gate**: `cargo test --test dependency_edges_incremental`

## T-707: Consulta (REQ-707, REQ-712) [ ]
- **REQ**: REQ-707, REQ-712
- **What**: `trace`, `search` (expansão) e `diff --staged` usam `is_dependency()`; teto de nós por hop com "+N omitidos"; detecção de ciclos no grafo `Imports` (função pública para a Fase 10).
- **Where**: `src/engine.rs`, `src/bin/nexspec.rs`, `src/graph/cycles.rs`
- **Depends on**: T-705
- **Done when**: `trace <símbolo>` lista dependentes (`<-`) e dependências; God node respeita o teto e mostra omitidos; ciclo A→B→A é detectado.
- **Gate**: `cargo test --test dependency_trace`

## T-708: Aceite no repositório real e no benchmark (REQ-708) [ ]
- **REQ**: REQ-708
- **What**: no condominium: `trace CachePort` (7 usuários), `trace AccessUserPersonaReader` (11), amostra de 30 arestas revisada à mão; tempo do 1º sync ≤ +20% e sync sem mudanças < 2 s (`perf_budget`); `structure` do benchmark deixa de ser 0,00; baseline atualizado.
- **Where**: medições; `.specs/features/dependency-edges/design.md`; `bench/baseline.json`
- **Depends on**: T-706, T-707
- **Done when**: números registrados no design.md; `cargo test --release --test perf_budget -- --ignored` verde.
- **Gate**: `cargo test && nexspec bench --corpus bench/self.toml --no-vector --check`

## T-709: Python, Go e Rust — arquivo→arquivo (REQ-705) [ ]
- **REQ**: REQ-705
- **What**: imports de módulo: Python (`import`, `from … import`, relativos), Go (`import "path"` via `go.mod`), Rust (`use`, `mod` dentro do crate). Só `Imports`.
- **Where**: `src/code/facts.rs`, `src/code/resolve.rs`
- **Depends on**: T-708
- **Done when**: fixture por linguagem com import resolvido, relativo e externo (ignorado).
- **Gate**: `cargo test --test dependency_edges_langs`

## T-710: Fechamento da fase [ ]
- **REQ**: todos
- **What**: ROADMAP/STATE/docs; `cargo test` (full e lean) e clippy; CI verde.
- **Depends on**: T-701…T-709
- **Gate**: `cargo test && cargo test --no-default-features --features lean && cargo clippy --all-targets -- -D warnings`

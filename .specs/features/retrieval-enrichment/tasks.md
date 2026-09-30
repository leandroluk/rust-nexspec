# Tasks: Retrieval Enrichment (Fase 19)

## T-1901: Campos de resumo no Tantivy e busca ponderada (REQ-1907) [x]
- **What**: `summary_<iso>` pré-declarados; `document_for` aceita resumos; `Upsert` faz `delete_term` antes de adicionar; `search_text` consulta `text` + `summary_*` com pesos; `INDEX_FORMAT` sobe.
- **Where**: `src/search/{schema,query,tantivy_participant}.rs`, `src/engine.rs`
- **Gate**: `cargo test search::`

## T-1902: Cache de enriquecimento (REQ-1905) [x]
- **What**: JSONL em `.specs/.cache/`, ordenado, escrita atômica, `stale` por hash, visão por arquivo/idioma.
- **Where**: `src/enrich/cache.rs`
- **Gate**: `cargo test enrich::cache`

## T-1903: Seleção, segredos e importância (REQ-1903, REQ-1906) [x]
- **What**: elegíveis (linguagens + Markdown, `.gitignore`, nunca sensíveis), varredura de segredo, recorte, ordem por importância, `--top`.
- **Where**: `src/enrich/select.rs`
- **Depends on**: T-1902
- **Gate**: `cargo test enrich::select`

## T-1904: Provedor (REQ-1902, REQ-1912) [ ]
- **What**: trait, `FakeProvider`, `GeminiProvider` (ureq, cabeçalho de chave, retry 429/5xx, JSON por schema, `usageMetadata`); teste `#[ignore]` com chave real.
- **Where**: `Cargo.toml`, `src/enrich/provider.rs`
- **Gate**: `cargo test enrich::provider`

## T-1905: Custo (REQ-1908) [x]
- **What**: estimativa offline por idioma e curva 20/50/100 %; preços configuráveis; total real por lote; retorno do investimento.
- **Where**: `src/enrich/cost.rs`
- **Depends on**: T-1903
- **Gate**: `cargo test enrich::cost`

## T-1906: `enrich` ponta a ponta (REQ-1901, REQ-1909, REQ-1913) [ ]
- **What**: `Engine::apply_enrichment`, lotes com concorrência limitada, persistência por lote, `--dry-run/--status/--clear/--yes/--token-budget`, falhas parciais (exit 6), confirmação do primeiro uso.
- **Where**: `src/enrich/run.rs`, `src/engine.rs`, `src/bin/nexspec.rs`
- **Depends on**: T-1901..T-1905
- **Gate**: `cargo test --test enrich_cli`

## T-1907: Sync materializa o cache, sinalização e doctor (REQ-1907, REQ-1910) [x]
- **What**: `sync` recupera resumos de um índice reconstruído sem API; linha informativa; `doctor` avisa chave/stale/.gitignore; `init` garante `.specs/.cache/` no `.gitignore`.
- **Depends on**: T-1906
- **Gate**: `cargo test --test enrich_sync`

## T-1908: Benchmark lado a lado (REQ-1911) [x]
- **What**: `bench --compare-enrich` (sem × com), perguntas `behavior` em prosa, critérios e saída; validação real fica a cargo do usuário com `GEMINI_API_KEY`.
- **Depends on**: T-1906
- **Gate**: `cargo test --test bench_enrich`

## T-1909: Fechamento [x]
- **What**: README, docs, ROADMAP/STATE, Fase 17 referenciando `EnrichProvider`, CI verde.
- **Gate**: `cargo test && cargo test --no-default-features --features lean && cargo clippy --all-targets -- -D warnings`

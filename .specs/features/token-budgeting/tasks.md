# Tasks: Token Budgeting & LLM Serialization (Fase 5)

## T-501: Dependência `tiktoken-rs` + módulo `token::` scaffold
- **REQ**: REQ-502
- **What**: `cargo add tiktoken-rs` (dependência obrigatória, não opcional —
  ver Decision Log em design.md). Criar `src/token/mod.rs`,
  `src/token/budget.rs`, `src/token/pruner.rs`, `src/token/serializer.rs`
  vazios/scaffold (`pub mod` em `mod.rs`).
- **Where**: `Cargo.toml`, `src/token/mod.rs`
- **Depends on**: none
- **Done when**: `cargo build` (default) e `cargo build --no-default-features --features lean` compilam ambos.
- **Gate**: `cargo build && cargo build --no-default-features --features lean`

## T-502: `Tokenizer` trait + `CharHeuristicTokenizer`
- **REQ**: REQ-502
- **What**: `trait Tokenizer { fn estimate(&self, text: &str) -> u32; }`;
  `CharHeuristicTokenizer` — `estimate` = `(text.chars().count() as f32 /
  3.5).ceil() as u32`. Testes: string vazia → 0; string conhecida →
  resultado esperado (ex. 7 chars → `ceil(7/3.5) = 2`).
- **Where**: `src/token/budget.rs`
- **Depends on**: T-501
- **Done when**: testes de `CharHeuristicTokenizer::estimate` passam para
  string vazia, string curta e string com caracteres multi-byte (UTF-8).
- **Gate**: `cargo test token::budget::char_heuristic`

## T-503: `TiktokenTokenizer`
- **REQ**: REQ-502
- **What**: `TiktokenTokenizer` — `new() -> Result<Self, TokenError>`
  (`TokenError::Unavailable(String)` se `tiktoken_rs::cl100k_base()`
  falhar), `estimate` via `.encode_ordinary(text).len()`. Teste principal
  marcado `#[ignore]` (precisa de rede/cache na primeira execução, mesmo
  padrão dos testes de inferência real de T-406 em `vector-engine`); um
  teste não-ignorado cobre só que `TiktokenTokenizer::new()` não gera panic
  (erro tratado como `Result`, independente de rede disponível).
- **Where**: `src/token/budget.rs`
- **Depends on**: T-502
- **Done when**: teste não-ignorado passa sempre (com ou sem rede); teste
  `#[ignore]` passa quando a rede está disponível (validado manualmente
  nesta task, igual T-406).
- **Gate**: `cargo test token::budget::tiktoken` (+ `-- --ignored` se rede disponível)

## T-504: `Budget` — margem de segurança + `Tier`/`TieredItem`
- **REQ**: REQ-503, REQ-504
- **What**: `enum Tier { Target, Seed, Dependency }` (ordem de prioridade =
  ordem de declaração, `Target < Seed < Dependency` via `#[derive(PartialOrd,
  Ord)]` ou comparação manual). `struct TieredItem { tier: Tier, text:
  String }`. `struct Budget { max_tokens: u32, margin: f32 }`;
  `Budget::new(max_tokens, margin)`, `Budget::with_default_margin(max_tokens)`
  (`margin = 0.9`), `Budget::effective_limit() -> u32`. Testes:
  `effective_limit` arredonda para baixo; `margin` fora de `(0.0, 1.0]`
  rejeitado (`Budget::new` retorna `Result` ou `panic` documentado — decidir
  na implementação, mantendo consistência com o resto do crate que prefere
  `Result` a panic em entrada de usuário).
- **Where**: `src/token/budget.rs`
- **Depends on**: T-502
- **Done when**: testes de `effective_limit` e validação de `margin` passam.
- **Gate**: `cargo test token::budget::budget`

## T-505: `Budget::fit` — corte por prioridade
- **REQ**: REQ-504
- **What**: `Budget::fit(&self, items: Vec<TieredItem>, tokenizer: &impl
  Tokenizer) -> Vec<TieredItem>` — ordena por `tier` (estável, preserva
  ordem relativa dentro do mesmo tier), acumula tokens estimados, para antes
  do item que estouraria `effective_limit()`, descarta os restantes por
  inteiro (nunca corta o texto de um item no meio). Teste principal:
  fixture com 1 item por tier, orçamento que cabe só `Target`+`Seed` →
  `Dependency` fica de fora; item individual maior que o orçamento inteiro é
  descartado sozinho (não quebra o corte dos itens anteriores que já
  couberam).
- **Where**: `src/token/budget.rs`
- **Depends on**: T-504
- **Done when**: os 2 cenários acima (corte por orçamento parcial, item
  individual maior que o orçamento) passam.
- **Gate**: `cargo test token::budget::fit`

## T-506: `token::pruner::prune_symbol`
- **REQ**: REQ-501
- **What**: `prune_symbol(source: &str, language: code::parser::Language,
  line_start: u32, line_end: u32) -> String` — reparsa `source` com
  Tree-sitter (mesma gramática de `Language::ts_language`, hoje privada em
  `code::parser`; promover a `pub(crate)` se necessário), localiza o nó cuja
  linha inicial/final contém `line_start..=line_end`, acha o último filho
  desse nó cujo texto começa com `{` (ou `:` seguido de bloco indentado para
  Python), substitui o intervalo de bytes desse filho por um marcador curto
  (`{ ... }` / `: ...`) preservando o texto antes dele (assinatura). Sem
  filho de bloco identificável → retorna o texto original da linha inteira
  sem alteração. Testes: uma função com corpo por linguagem (5 casos, igual
  ao padrão de `code::parser::extract`) confirma que o corpo desaparece mas
  a assinatura permanece; um `struct`/`type alias` sem bloco retorna
  inalterado.
- **Where**: `src/token/pruner.rs`, `src/code/parser.rs` (visibilidade de
  `Language::ts_language`, se necessário)
- **Depends on**: T-501
- **Done when**: os testes descritos acima passam para as 5 linguagens.
- **Gate**: `cargo test token::pruner`

## T-507: `token::serializer::serialize`
- **REQ**: REQ-505
- **What**: `serialize(items: &[TieredItem]) -> String` — para cada item,
  emite `### <tier ou label>\n\`\`\`\n<text>\n\`\`\`\n\n` (linguagem do
  fence: decidir se `TieredItem` ganha um campo opcional `lang: Option<
  &'static str>` nesta task, já que REQ-505 pede "linguagem detectada" no
  bloco de código). Teste: 3 items (um por tier) → saída contém as 3 seções
  na ordem de entrada, cada uma com fence de code block.
- **Where**: `src/token/serializer.rs`, `src/token/budget.rs` (campo `lang`
  em `TieredItem`, se adotado)
- **Depends on**: T-505
- **Done when**: teste de saída determinística passa (snapshot de string
  exata, não só "contém").
- **Gate**: `cargo test token::serializer`

## T-508: Integração fim-a-fim (pruner → budget → serializer)
- **REQ**: (todos — valida a fase inteira junto)
- **What**: Teste de integração combinando os 3 módulos: fixture com um
  símbolo de código real (arquivo `.rs` de exemplo), um `Requirement` e um
  `Adr` sintéticos, tiers atribuídos manualmente (`Target`/`Seed`/
  `Dependency`), orçamento pequeno o bastante para forçar corte de pelo
  menos um item, `CharHeuristicTokenizer` (evita dependência de rede no
  teste). Confirma: corpo do símbolo aparece podado na saída, item cortado
  não aparece, Markdown resultante é bem formado.
- **Where**: `tests/token_budgeting_integration.rs`
- **Depends on**: T-506, T-507
- **Done when**: teste passa fim-a-fim.
- **Gate**: `cargo test --test token_budgeting_integration`

## T-509: Lint e superfície pública
- **REQ**: (todos — fechamento da fase, mesmo padrão de T-010/T-110/T-209/T-310/T-409)
- **What**: Exportar `token::{Tokenizer, TiktokenTokenizer,
  CharHeuristicTokenizer, Budget, Tier, TieredItem, TokenError,
  prune_symbol, serialize}` de `src/lib.rs`. Doc comments em toda API
  pública.
- **Where**: `src/lib.rs`, `src/token/mod.rs`
- **Depends on**: T-508
- **Done when**: `cargo doc --no-deps` sem warnings, `cargo clippy
  --all-targets -- -D warnings` sem erros (nos dois builds, default e
  `lean`).
- **Gate**: `cargo doc --no-deps && cargo clippy --all-targets -- -D warnings`

# Design: Work Memory (Fase 15)

## Architecture Overview

```
save-result ──► .specs/.memory/notes/<id>.md   (nota Markdown com frontmatter; bruta, local por padrão)
                        │
reflect ────────────────┼─► .specs/.memory/LESSONS.md       (resumo determinístico; versionável)
                        └─► .specs/.cache/memory.json        (id do nó → preferido/beco sem saída; regenerável, ignorado pelo Git)
                                              │
search / query ◄── sobreposição leve no ranking (--no-memory desliga) ◄─┘
reflect --max-tokens N ──► resumo curto no stdout para a skill carregar no início da sessão
```

Nada de LLM e nada de rede. Tudo é função das notas, do grafo e do relógio (a meia-vida), e a saída de `reflect` não carrega data: só muda quando uma classificação muda.

## Dependency Paths

- Nós citados: `query::target::resolve` (o mesmo resolvedor das consultas) grava `id` + rótulo; `reflect` confere o `id` no `GraphSnapshot` atual e descarta o que sumiu (REQ-1502).
- Segredos: `enrich::select::find_secret` (Fase 19) — uma lista só de padrões.
- Ranking: `Engine::search` (fusão BM25 + vetor) ganha o ajuste; `query_graph` usa `search` para os sementes.
- Orçamento: `query::budget::fit_lines` para `reflect --max-tokens`.
- Bench: `bench --compare-memory` (mesma forma do `--compare-enrich`).

## Decisões

| Id | Decisão | Motivo |
|---|---|---|
| D1 | **Nota = um arquivo** `.specs/.memory/notes/<id>.md`: frontmatter (`id`, `at`, `type`, `outcome`, lista de nós com `id` e rótulo) + corpo (`Question:`, `Answer:`, `Correction:`). `id` = 10 primeiros hex de `blake3(pergunta ␊ desfecho ␊ ids ordenados)`: gravar a mesma coisa de novo **substitui** a nota e renova o `at` (recência). | REQ-1501: ids estáveis, idempotente, legível e diffável. |
| D2 | `save-result` **resolve cada `--nodes` pelo resolvedor único**; ambíguo ou inexistente = erro com as sugestões de sempre (a memória não guarda alvos que não apontam para nada). `corrected` exige `--correction`. | Memória limpa na entrada; `reflect` não precisa adivinhar. |
| D3 | **Notas brutas fora do Git, lições no Git** (Q1): `init` acrescenta `.specs/.memory/notes/` ao `.gitignore`; `LESSONS.md` fica versionável. | Notas têm perguntas e respostas em prosa (ruído e privacidade); a lição é o que vale compartilhar. |
| D4 | **Sinais com decaimento**: peso `0,5^(idade / meia-vida)` (meia-vida 30 dias, `--half-life-days`); um sinal com peso < 0,25 já não conta (duas meias-vidas). Desfechos: `useful` (+), `dead_end` e `corrected` (−). | REQ-1502. |
| D5 | **Classificação por nó** (sinais vivos): *Preferred* = ≥ N `useful` (default 2, `--min-useful`) e nenhum negativo; *Tentative* = 1 `useful` e nenhum negativo; *Contested* = `useful` e negativos juntos, e **o sinal mais recente decide** o lado em que aparece (e é o que o ranking aplica); *Dead end* = só negativos. *Corrections* = pares pergunta → correção das notas `corrected` vivas. | REQ-1502. |
| D6 | `reflect` sem opções grava `LESSONS.md` (e o cache do ranking); `reflect --max-tokens N` **só imprime** o resumo curto (preferidos, becos sem saída e correções, nessa ordem de valor) cortado pelo orçamento, sem escrever nada. | REQ-1504: a skill carrega o resumo, nunca o histórico. |
| D7 | **Sobreposição no ranking**: nós *Preferred* (e *Contested* inclinados a útil) ganham `× (1 + 0,25)`, *Dead end* (e contestados inclinados a negativo) `× 0,5`, sobre a pontuação da fusão; `NEXSPEC_MEMORY_BOOST`/`NEXSPEC_MEMORY_PENALTY` ajustam, `--no-memory` desliga, sem cache não muda nada. | REQ-1503: leve e reversível. |
| D8 | O cache do ranking é `.specs/.cache/memory.json` (regenerável com `reflect`, ignorado pelo Git). O `bench` **não** o usa por padrão (reprodutível); `bench --compare-memory` roda com e sem e reprova se algum tipo perder mais de 2 pontos de `recall@5`. | REQ-1503: o efeito aparece no bench sem degradar precisão. |
| D9 | Ferramentas MCP `save_result` e `reflect` (a skill grava e lê pelo servidor que já usa). | Uso real da Fase 15. |
| D10 | Segredos: `save-result` recusa pergunta, resposta ou correção com padrão de chave/token/senha, nomeando o tipo (nunca o valor). | REQ-1505. |

## Componentes

| Componente | Local |
|---|---|
| formato, leitura e escrita de notas | `src/memory/note.rs` |
| agregação e `LESSONS.md` | `src/memory/reflect.rs` |
| cache e ajuste de ranking | `src/memory/overlay.rs` |
| CLI, MCP | `src/bin/nexspec.rs`, `src/mcp.rs` |

## Riscos

- **Viés de confirmação**: um nó preferido sobe e é citado de novo; o decaimento e o teto de 25 % limitam, e `--no-memory` mostra o ranking sem a memória.
- **Nota sobre nó renomeado**: o `id` muda e a nota é descartada no `reflect` (registrado na contagem de descartados).
- **Churn em `LESSONS.md`**: sem datas na saída e ordem estável; só muda quando uma classificação muda.

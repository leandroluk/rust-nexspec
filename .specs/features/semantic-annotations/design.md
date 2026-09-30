# Design: Semantic Annotations (Fase 18)

## Architecture Overview

```
annotate / annotate_node ──► .specs/.memory/annotations.jsonl   (fonte da verdade, versionável, ordem estável)
                                      │
sync ── depois do ciclo de git ───────┤ materializa: nós Annotation + arestas AnnotatedBy / relação anotada (INFERRED, contexto annotation)
                                      │ estado por anotação: fresh | stale (hash do alvo mudou) | dangling (alvo sumiu)
explain / query / affected ◄──────────┤ mostram as anotações do nó (autor, data, estado)
search ◄──────────────────────────────┤ desfecho useful/dead_end/corrected ajusta o ranking (mesmo nudge da Fase 15; --no-memory)
report / wiki ◄───────────────────────┘ rótulo de comunidade = anotação mais recente não obsoleta

sync (opt-in) ── vizinhos mais próximos no HNSW ──► arestas SimilarTo (INFERRED, contexto embedding, pontuação quantizada)
```

O arquivo é a fonte da verdade; o índice é derivado e reconstruível: apagar `.specs/.index/` não perde anotação nenhuma. Nada de LLM: a camada semântica vem de quem consulta (anotações) e do modelo local (embeddings).

## Dependency Paths

- Resolução do alvo: `query::target::resolve` (Fase 11) para `annotate <alvo>`; a chave estável gravada vem do nó resolvido.
- Materialização: `Engine::materialize_annotations` (um ciclo do `Coordinator`, depois do `sync`), a partir de `Engine::snapshot()`.
- Ranking: o `Overlay` da Fase 15 (`memory::overlay`) também recebe os desfechos das anotações frescas.
- Comunidades: `report::communities` (rótulos) e `export::wiki`.
- Grafo: `NodePayload::Annotation`, `EdgeType::{AnnotatedBy, SimilarTo}`, `EdgeContext::{Annotation, Embedding}`, pontuação de similaridade nos 4 bits altos de `Edge.meta` (`INDEX_FORMAT` 9).
- Vetores: `HnswParticipant` (build `full`, modelo presente).

## Decisões

| Id | Decisão | Motivo |
|---|---|---|
| D1 | **Chave estável do alvo**: arquivo = caminho; símbolo = `caminho::Nome`; requisito/tarefa/ADR = marcador; tabela = `table:schema.nome`; pacote = `package:nome`; endpoint = `endpoint:MÉTODO caminho`; comunidade = `community:<rótulo derivado>`. O id interno nunca é gravado. | REQ-1803: sobrevive a reconstruir o índice e a mudar de máquina. |
| D2 | **Uma anotação = uma linha JSON** `{key, target, label?, note?, relation?, to?, outcome?, author, model?, at, source_hash, confidence}`, ordenadas por `(target, at, key)`; `key` = hash curto de alvo+rótulo+nota+relação+destino, então repetir a mesma anotação a substitui e renova o `at`. | REQ-1802: diff limpo, idempotente. |
| D3 | **Estado**: `fresh` (hash atual do alvo = `source_hash`), `stale` (hash mudou: continua visível e marcada, sai do ranking, da busca e das arestas de relação), `dangling` (alvo não existe: só aparece em `list`/`lint`, não entra no grafo). O hash do alvo é o do **arquivo** que o contém (símbolo → seu arquivo; requisito → título + corpo; demais: sem hash, nunca `stale`). | REQ-1803. |
| D4 | **Materialização depois do `sync`** (não dentro do orquestrador): lê o arquivo, resolve as chaves contra o grafo já atualizado, e só abre um ciclo se algo mudou (nós/arestas de anotação comparados com os existentes). Sem arquivo de anotações e sem nós antigos, não custa nada. | O orquestrador não conhece o grafo inteiro; aqui a resolução é exata e o custo só existe para quem anota. |
| D5 | **Nunca vence o determinístico**: nó de anotação novo (`Annotation`) ligado por `AnnotatedBy` (alvo → anotação; não é aresta de dependência, então `affected`/impacto ignoram); relação anotada (`--relation T --to B`) é aresta própria, `INFERRED`, contexto `annotation`, id próprio — nunca toca uma aresta `EXTRACTED`. | REQ-1804. |
| D6 | **Rótulo de comunidade**: a anotação mais recente, não obsoleta, do alvo `community:<rótulo derivado>`; `report` e a wiki a usam no lugar do rótulo derivado. Comunidades não são nós: esse rótulo vive só no arquivo. | REQ-1805. |
| D7 | **Consultas**: `explain` lista as anotações do nó (autor, data, estado, texto); `query` e `affected` acrescentam uma linha `notes:` por nó anotado. O ranking usa o `Overlay`: `useful` sobe, `dead_end`/`corrected` desce (anotações frescas), junto com a memória da Fase 15; `--no-memory` desliga os dois. | REQ-1806. |
| D8 | **Governança**: `annotate list` (filtra por alvo/estado), `show <id>`, `remove <id>`, `lint` (stale, dangling, duplicadas de mesmo alvo+texto, acima de 500 caracteres); `annotate` recusa segredo, recusa nota > 500 caracteres e `--max-tokens` limita o que `list` carrega. Saída 0/1 do `lint`: 0 sem achados, 8 com achados. | REQ-1807. |
| D9 | **SimilarTo**: desligado por padrão; `sync --similar` (ou `NEXSPEC_SIMILAR=1`), build `full` com modelo: para cada documento/símbolo indexado, até K=3 vizinhos do HNSW com similaridade ≥ 0,75, aresta `INFERRED`/`embedding` com a pontuação em 4 bits (`score()` devolve 0..1 em passos de 1/15); a cada rodada o conjunto anterior é substituído. O benchmark decide se vira padrão (Q2). | REQ-1808. |
| D10 | MCP `annotate_node` com a orientação de quando anotar; a skill é outro repositório. | REQ-1809. |

## Componentes

| Componente | Local |
|---|---|
| modelo (nó, arestas, contextos, pontuação) | `src/graph/{node,edge}.rs` |
| armazenamento, chaves, lint | `src/annotate/store.rs` |
| estados e materialização | `src/annotate/materialize.rs` |
| CLI, MCP | `src/bin/nexspec.rs`, `src/mcp.rs` |
| similaridade | `src/annotate/similar.rs` |

## Riscos

- **Ruído**: o limite de 500 caracteres, o `lint` e a saída do ranking só para anotações frescas seguram o custo de anotar demais.
- **Alvo com ordinal** (dois símbolos de mesmo nome no arquivo): a chave `caminho::Nome` resolve o primeiro; `lint` avisa quando há ambiguidade.
- **Custo do `snapshot` a cada sync**: só quando existem anotações (ou nós de anotação antigos).

# Design: Retrieval Enrichment (Fase 19)

## Architecture Overview

```
enrich ──► select (eligibles + segredos + importância) ──► lotes ──► EnrichProvider ──► cache .specs/.cache/enrichment.jsonl
                                                                                          │
sync ───────────────────────────────────────────────────────────── materializa o cache ───┤
enrich ─► Engine::apply_enrichment(paths) ─► TantivyParticipant (delete+add do doc do arquivo) ◄┘
search/query ─► BM25 em text + summary_<lang> (pesos) ──► fusão existente
```

O enriquecimento é **um campo a mais nos documentos de arquivo do Tantivy**. Não cria nós, arestas nem ids; o grafo determinístico não é tocado.

## Dependency Paths

- Tantivy: `src/search/{schema,query,tantivy_participant}.rs`. O esquema é fixo por índice, então os campos por idioma são **pré-declarados** para o conjunto de idiomas com stemmer no Tantivy (`summary_<iso>`); campos vazios não custam nada.
- Busca: `Engine::search` chama `search_text`; passa a consultar `text` + `summary_*` com pesos (`NEXSPEC_ENRICH_WEIGHT`).
- Sync: `sync_orchestrator` já produz `NodeMutation::Upsert` de `File`; o participante consulta a `EnrichmentView` (cache carregado) ao montar o documento.
- Importância: `GraphView`/grau (Fase 10/11), arestas `Satisfies` (REQ/TASK), co-change (Fase 2).
- Tokens: `Tokenizer` da Fase 5 (`TiktokenTokenizer`/heurístico) para estimativa offline.
- Rede: `ureq` (bloqueante, rustls) — sem runtime assíncrono; concorrência por `std::thread::scope`.

## Decisões

| Id | Decisão | Motivo |
|---|---|---|
| D1 | Campos `summary_<iso>` pré-declarados para os idiomas do Tantivy (ar, da, nl, en, fi, fr, de, el, hu, it, no, pt, ro, ru, es, sv, ta, tr). Idiomas fora (zh/ja/ko) são recusados com mensagem clara na v1 (resolve Q4). | Esquema dinâmico exigiria reindexar ao mudar `--lang`; campos vazios são gratuitos. |
| D2 | `Upsert` no Tantivy passa a fazer `delete_term(id)` antes de `add_document`. | Sem isso, reaplicar o resumo duplicaria o documento do arquivo; é também correção geral de upsert. |
| D3 | Pesos por campo: `text` 1.0, `summary_*` 0.5 (env `NEXSPEC_ENRICH_WEIGHT`; 0 desliga). Sem detecção do idioma da pergunta. | REQ-1907. Calibrado pelo benchmark (Q1). |
| D4 | Cache JSONL ordenado por (path, lang); reescrito atomicamente (tmp + rename) após cada lote. | REQ-1905/1906: interrompível sem perda. |
| D5 | Hash do conteúdo = blake3 do arquivo inteiro; entrada com hash diferente é `stale` e não entra no índice. | REQ-1906. |
| D6 | O trecho enviado é `path` + primeiros N caracteres (corte em fronteira de linha); arquivo com padrão de segredo é omitido. | REQ-1903. |
| D7 | Importância v1 = grau (imports/calls/…) + 50 por vínculo `Satisfies` + co-change; `--top` corta. | Q2: começar simples e medir saturação. |
| D8 | Provedor atrás do trait `EnrichProvider`; Gemini por `x-goog-api-key`; chave nunca em URL/log/erro. Testes usam `FakeProvider` com respostas gravadas. | REQ-1902/1912. |
| D9 | Preços por variáveis (`NEXSPEC_ENRICH_PRICE_IN/OUT`, USD por 1M tokens) com default documentado para o modelo padrão e aviso quando o modelo é desconhecido (Q3). | Preço muda. |
| D10 | `enrich` não é tool MCP nem roda em hook. | REQ-1901/1910. |
| D11 | Saída de `--status`/`--dry-run`: primeira linha curta e estável; códigos: 0 ok, 1 erro, 6 sucesso parcial (itens falharam). | REQ-1909/1913. |

## Componentes novos

| Componente | Local |
|---|---|
| `EnrichmentCache`, `EnrichmentView` | `src/enrich/cache.rs` |
| seleção, varredura de segredos, recorte | `src/enrich/select.rs` |
| `EnrichProvider`, `FakeProvider`, `GeminiProvider` | `src/enrich/provider.rs` |
| estimativa, preços, relatório de custo | `src/enrich/cost.rs` |
| orquestração (`run`, `status`, `clear`) | `src/enrich/run.rs` |
| campos de resumo e busca ponderada | `src/search/{schema,query}.rs` |
| `Engine::apply_enrichment` | `src/engine.rs` |

## Riscos

- **Segredo vazando no trecho**: varredura por padrões + exclusão de arquivos sensíveis por nome; confirmação antes do primeiro envio.
- **Ganho não comprovado**: critério do REQ-1911; sem ele os pesos ficam 0 (o recurso existe, mas inerte).
- **Ruído de resumos em inglês para perguntas em outro idioma**: campos separados por idioma, pesos menores que `text`.
- **Custo inesperado**: `--dry-run` offline, `--token-budget`, recusa acima de 500 mil tokens sem `--yes`.

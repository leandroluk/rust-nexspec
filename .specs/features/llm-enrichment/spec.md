# Spec: LLM Enrichment (opt-in) — rótulos de comunidade e extração semântica (Fase 17)

> Origem: a parte do graphify que o `nexspec` **não** portará por padrão: extração semântica de docs/papers/imagens
> (`llm.py`, `semantic_cleanup.py`, `dedup.py`), rotulagem de comunidades por LLM (`label`, `cluster-only`) e `provider`.
> Prioridade **P3** e atrás de um **portão de decisão** (ver abaixo). Nada aqui pode ser exigido para o fluxo padrão.

## Summary

O diferencial do `nexspec` é ser determinístico, barato e offline. A camada semântica do graphify é cara, não determinística e, neste
projeto, já se mostrou frágil (rebuild recusado pela *shrink guard* porque o LLM omitiu mais de 50 documentos). Esta fase só existe para
quem quiser opt-in explícito, e só deve ser construída se a Fase 8 provar que a ausência de semântica reduz a precisão das respostas.

> **Ordem preferida (2026-09-29):** primeiro a Fase 18 (`semantic-annotations`) — anotações feitas pelo próprio agente e similaridade por embeddings locais. Um provedor de LLM dentro do `nexspec` só se justifica para execuções sem agente (CI gerando wiki com rótulos) ou para grandes corpora de texto livre.

## Portão de decisão (antes de qualquer implementação)

- Executar a Fase 8 e o teste das 10 perguntas (`graphify-parity`, REQ-P03). Construir esta fase **somente se** ≥ 3 perguntas P0 forem
  classificadas *pior* por falta de contexto semântico que specs em Markdown (REQ/TASK/ADR) e o grafo estrutural não fornecem.

## Requirements (condicionais)

- REQ-1701: **Provedor plugável** — `LlmProvider` (trait; a generalização do `EnrichProvider` da Fase 19, `src/enrich/provider.rs`, e reaproveita a contabilidade de tokens e custo de `src/enrich/cost.rs`) com implementações para endpoints compatíveis com OpenAI e com Anthropic (URL base e modelo por configuração/variável de ambiente, incluindo servidores locais); nenhuma chamada de rede sem `--llm` explícito e chave configurada.
- REQ-1702: **Rótulos de comunidade** — `label [--missing-only]` nomeia comunidades (Fase 10) a partir de arquivos/símbolos principais, com cache por *hash* do conteúdo da comunidade, lote e concorrência configuráveis; rótulos são **anotações** (não alteram ids nem arestas) e o relatório funciona sem elas.
- REQ-1703: **Extração semântica de documentos (Markdown/texto)** — conceitos e relações `INFERRED` ligando documentos a símbolos/REQs, com validação e limpeza do fragmento (contrato de schema), cache incremental por arquivo e **limite de custo** (`--token-budget`, `--max-concurrency`); resultado nunca sobrescreve arestas `EXTRACTED`.
- REQ-1704: **Honestidade de proveniência** — toda aresta semântica é marcada `INFERRED` com modelo, data e hash da fonte; `--min-confidence EXTRACTED` (Fase 11) a exclui.
- REQ-1705: **Falha segura** — erro de provedor, resposta incompleta ou schema inválido descarta apenas o fragmento afetado e registra aviso; o grafo determinístico nunca é degradado (sem *rebuild* destrutivo).

## Out of Scope

- PDFs, Office, imagens, áudio/vídeo, Google Workspace (mantidos fora — ver `graphify-parity`).
- Treinamento/ajuste fino de modelos.

## Open Questions

- Q1: Vale a pena manter esta fase no roadmap ou apenas registrar a decisão de não fazê-la? Reavaliar após o teste das 10 perguntas.

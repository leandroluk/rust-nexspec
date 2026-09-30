# Spec: Semantic Annotations — anotações do agente e similaridade por embeddings (Fase 18)

> Origem: decisão de 2026-09-29. Em vez de o `nexspec` chamar um LLM próprio (Fase 17, condicional), a camada semântica vem de
> (1) **anotações que o próprio agente registra** sobre o que já entendeu e (2) **similaridade por embeddings locais** (o modelo
> MiniLM/ONNX da Fase 4). Ambas são baratas, offline (a segunda) e sem provedor, chave ou custo por chamada.
> Relaciona-se com a Fase 15 (`work-memory`: usa o mesmo armazenamento) e com a Fase 11 (as anotações aparecem em `explain`/`query`).

## Summary

O agente que consulta o `nexspec` já leu arquivos e chegou a conclusões (o que uma comunidade faz, que um doc explica um símbolo, que
um caminho era beco sem saída). Hoje isso se perde ao fim da sessão. `annotate` grava essas conclusões como **anotações rastreáveis**,
com proveniência e confiança `INFERRED`, e o grafo as usa nas consultas seguintes. A similaridade por embeddings adiciona arestas
`SimilarTo` determinísticas entre documentos e símbolos, sem LLM.

## Requirements

- REQ-1801: **`annotate` (CLI) e `annotate_node` (MCP)** — `annotate <alvo> [--label TEXTO] [--note TEXTO] [--relation TIPO --to <alvo2>] [--outcome useful|dead_end|corrected]`; `<alvo>` resolvido pelo resolvedor único da Fase 11 (símbolo, arquivo, `REQ-…`, comunidade). Toda anotação grava proveniência: `author` (`agent`/`user`), `model` opcional, `at`, `source_hash` do alvo e `confidence = INFERRED`.
- REQ-1802: **Fonte da verdade em arquivo** — anotações em `.specs/.memory/annotations.jsonl` (uma por linha, ordenação estável, versionável no Git); o índice é derivado e reconstruível (`sync` lê o arquivo e materializa nós/arestas `Annotation`/`AnnotatedBy`). Apagar `.specs/.index/` nunca perde anotações.
- REQ-1803: **Chave estável e obsolescência** — o alvo é guardado por chave estável (`arquivo::símbolo` ou id de REQ), não por id interno; quando o `source_hash` do alvo muda, a anotação vira `stale` (continua visível, marcada) e some do ranking; alvo inexistente vira `dangling`.
- REQ-1804: **Nunca vence o determinístico** — anotação não altera nós nem sobrescreve arestas `EXTRACTED`; relações anotadas entram como arestas `INFERRED` com `context = annotation`, filtráveis por `--min-confidence` (Fase 11).
- REQ-1805: **Rótulos de comunidade** — `annotate <comunidade> --label "…"` nomeia comunidades (Fase 10); `report` e a wiki (Fase 12) usam o rótulo mais recente não obsoleto e caem no placeholder quando não há.
- REQ-1806: **Uso nas consultas** — `explain`, `query` e `affected` (Fase 11) mostram anotações do nó com autor/data/estado; `search` aplica *boost* leve a nós com desfecho `useful` e penalidade a `dead_end`, desligável com `--no-memory` e validado no benchmark (Fase 8) antes de vir ligado por padrão.
- REQ-1807: **Governança** — `annotate list|show|remove|lint`; `lint` reporta `stale`, `dangling`, duplicadas e notas acima do limite (500 caracteres); `annotate` recusa conteúdo com padrões de segredo (chave/token) e limita o total de anotações carregadas por sessão pelo orçamento de tokens (`--max-tokens`).
- REQ-1808: **Similaridade por embeddings (`SimilarTo`)** — no `sync`, para nós novos ou alterados (documentos/specs e símbolos), calcula o vizinho mais próximo no HNSW existente e cria arestas `SimilarTo` (`INFERRED`, `context = embedding`) com pontuação, até K por nó (default 3) e acima de um limiar (default 0,75); registra `model_id` e versão; removidas/recalculadas quando o nó muda. Ausente no build `lean`. Desligado por padrão até o benchmark da Fase 8 mostrar ganho.
- REQ-1809: **Contrato para a skill** — descrição das tools MCP orienta *quando* anotar (após responder com sucesso, ao nomear comunidade, ao descartar caminho) e *como* manter curto; a skill `graph-spec-design` usa `annotate` no fim das tarefas de exploração (fora deste repositório).

## Out of Scope

- Qualquer chamada a LLM feita pelo `nexspec` (ver Fase 17, condicional).
- Anotação automática em massa; anotações são pontuais e feitas por quem consulta.
- Sincronização de anotações entre usuários além do Git.

## Open Questions

- Q1: `annotate` grava direto em `.specs/.memory/` ou passa por uma etapa de revisão? Recomendação: grava direto (é barato de reverter com Git) e o `lint` sinaliza problemas.
- Q2: Limiar/K de `SimilarTo` — calibrar com o benchmark; valores iniciais são hipóteses.

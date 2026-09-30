# Spec: Retrieval Benchmark — precisão da busca e economia de tokens medidas (Fase 8)

> Origem: avaliação de 2026-09-29 (item 2). A economia de tokens é a premissa do projeto, mas foi estimada à mão em 3–4 consultas
> (bytes ÷ 4, sem A/B). Observado: a busca "outbox decorator repository dispatch" trouxe `*.decorator.ts` genéricos e não
> `outbox.decorator.ts` entre os 10 primeiros.

## Summary

Um benchmark reprodutível que mede (a) **qualidade** do ranking (o arquivo/símbolo esperado aparece no top-k?) e (b) **custo em tokens** de responder cada pergunta com `nexspec` versus os baselines sem índice. Serve como critério objetivo para trocar o graphify, guiar melhorias de ranking e barrar regressões.

## Requirements

- REQ-801: **Formato do corpus** — `bench/queries.toml` (ou `.yaml`) com entradas `{id, kind, query, expect: [caminhos|símbolos], notes}`; `kind` ∈ `locate` (onde está X), `structure` (quem depende/implementa), `behavior` (como funciona — exige leitura posterior), `traceability` (REQ↔código).
- REQ-802: **Runner** — `nexspec bench --corpus <arquivo> [--repo <path>] [--k 5,10]` executa cada consulta contra o índice e emite JSON + Markdown com `recall@k`, `MRR`, tokens da resposta (mesmo `Tokenizer` do Fase 5) e latência por consulta.
- REQ-803: **Baselines de custo** — para cada consulta: (i) saída de `grep -rn` do termo-chave; (ii) leitura integral dos N arquivos esperados. Reporta economia = `1 − nexspec / baseline` por `kind`, sempre com a ressalva de que `behavior` exige leitura adicional (custo somado, não omitido).
- REQ-804: **Corpora versionados** — (a) este repositório (corpus próprio em `bench/self.toml`, ~20 perguntas); (b) corpus externo opcional via `--repo` apontando para um projeto real (ex.: `condominium-management-system`), mantido fora deste repo.
- REQ-805: **Portões de qualidade** — `recall@5 ≥ 0,8` em `locate` no corpus próprio; regressão > 5 pontos vs. o último resultado gravado em `bench/baseline.json` falha o CI.
- REQ-806: **Melhorias de ranking rastreadas pelo benchmark** — tokenização ciente de identificadores (camelCase/snake_case/PascalCase), boost por nome de símbolo e por caminho (`outbox.decorator.ts`), penalização de arquivos genéricos de mesmo padrão, e revisão do peso BM25 × vetor (RRF). Cada mudança de ranking apresenta delta no benchmark.
- REQ-807: **Relatório honesto** — separa economia "de localização" de custo fixo do fluxo (ex.: contexto carregado por sessão pela skill), para não superestimar o ganho total.

## Out of Scope

- Medir cobrança real de tokens de um provedor (usa o `Tokenizer` local como proxy).
- Avaliação humana de qualidade de respostas de LLM.

## Open Questions

- Q1: Formato do corpus: TOML (sem dependência nova, já usada?) ou YAML? Recomendação: TOML.
- Q2: Congelar o commit do repositório-alvo para reprodutibilidade (baseline muda quando o alvo muda)?

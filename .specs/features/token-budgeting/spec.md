# Spec: Token Budgeting & LLM Serialization (Fase 5)

## Summary

Fase 5 transforma resultados brutos de recuperação (nós rankeados vindos do
`hybrid::seed_discovery` + `hybrid::expand` da Fase 4) em um payload de texto
denso, cabendo dentro de um orçamento de tokens explícito. Duas
responsabilidades novas: (1) **podar** corpos de símbolos de código,
mantendo só assinatura/contrato (REQ-501), e (2) **serializar** o conjunto
podado em Markdown compacto, cortando por prioridade quando o orçamento não
cabe tudo (REQ-503/504). A contagem de tokens em si é abstraída atrás de um
trait plugável (REQ-502), já que agentes consumidores (Claude Code, Cursor,
Windsurf) não compartilham o mesmo tokenizer, e o crate precisa continuar
funcional offline se o vocabulário BPE não puder ser carregado.

## Requirements

- REQ-501: **AST Signature Pruner** — dado um nó `Symbol` (Fase 3: `name`,
  `line_start`/`line_end`) e o texto-fonte do arquivo, produzir uma versão
  "podada" contendo só a assinatura (nome, parâmetros, tipo de retorno,
  cabeçalho de `struct`/`interface`/`class`/`trait`) sem o corpo de
  implementação. Reusa Tree-sitter (já uma dependência da Fase 3) para achar
  o nó de corpo (`block`/`function_body`/equivalente) e substitui-lo por um
  marcador (`{ ... }`/`: ...`), preservando o resto do texto original
  (assinatura, decorators/atributos, docstring imediatamente anterior).
  Símbolos sem corpo identificável (ex.: `struct`/`interface` sem chaves de
  bloco) retornam o texto original sem alteração.
- REQ-502: **`Tokenizer` trait plugável** — `estimate(text: &str) -> u32`.
  Implementação default via `tiktoken-rs` (BPE, `cl100k_base`). **Fallback
  offline**: se o vocabulário BPE falhar ao carregar (sem rede no primeiro
  uso, ambiente restrito), degrada automaticamente para uma heurística
  `char_count as f32 / 3.5` (arredondada para cima) — nunca falha o comando
  por causa disso.
- REQ-503: **Orçamento com margem de segurança** — um `Budget` recebe
  `max_tokens: u32` e aplica margem de segurança configurável (default 90%)
  antes de qualquer decisão de corte, absorvendo o desvio entre a estimativa
  BPE local (OpenAI/tiktoken) e o tokenizer real do modelo consumidor
  (Claude, Gemini etc.) — o limite efetivo usado internamente é
  `max_tokens * margin` (default `margin = 0.9`), nunca o valor bruto.
- REQ-504: **Corte por prioridade determinístico** — ao montar o payload
  final, nós entram nesta ordem até o orçamento (já com margem aplicada)
  estourar: (1) Spec/ADR alvo da consulta, (2) símbolo(s)-semente
  (`seed_discovery`), (3) dependências diretas (1º hop de `hybrid::expand`).
  Nós além desse ponto de corte são omitidos inteiramente (não truncados
  parcialmente) — a serialização de um nó individual nunca é cortada no
  meio.
- REQ-505: **High-Density Markdown Serializer** — serializa o conjunto de
  nós (já podado e cortado por orçamento) em Markdown compacto: cabeçalhos
  curtos por nó (`### <tipo> <nome/título>`), corpo podado em bloco de
  código com a linguagem detectada, sem metadados verbosos (sem tabelas
  YAML/JSON por nó) — otimizado para densidade de informação por token, não
  para leitura humana longa.

## Affected Components (from graph)

Sem grafo do próprio NexSpec ainda. Componentes existentes consumidos:

- `code::parser::Language` (Fase 3) — reusado para escolher a gramática
  Tree-sitter do pruner (REQ-501); mesma enum, sem extensão de variantes.
- `graph::node::NodePayload::Symbol` (Fase 3, `line_start`/`line_end`) —
  entrada do pruner; o payload não guarda o texto-fonte, então o pruner
  recebe o texto do arquivo já lido pelo chamador (ex.: `GitSource::
  read_blob_at_head`, Fase 2) e a linha inicial/final para fatiar.
- `hybrid::seed_discovery`/`hybrid::expand` (Fase 4) — produtores da lista
  rankeada que REQ-504 consome; nenhuma mudança de assinatura nelas.

## Out of Scope

- CLI (`nexspec search --max-tokens N`) — Fase 6; esta fase só entrega as
  funções de biblioteca (pruner, `Tokenizer`, `Budget`, serializer).
- Tokenizers reais de terceiros (ex.: tokenizer exato do Claude/Gemini) —
  fora do trait plugável em si (que qualquer chamador pode implementar), o
  crate só fornece a implementação `tiktoken-rs` + fallback offline.
- Pruning de corpos de Markdown (`Requirement`/`Task`/`Adr`/`DocSection`) —
  REQ-501 é especificamente sobre símbolos de código (`NodeType::Symbol`);
  nós de spec/doc já são compactos por natureza e entram inteiros no
  serializer.
- Cache/memoização de estimativas de token — cada chamada a `estimate` é
  recalculada; não há budget de performance identificado que justifique
  isso nesta fase.

## Open Questions

- Nenhuma que bloqueie a spec. Um ponto técnico a confirmar na implementação
  de REQ-502 (não uma decisão do usuário): `tiktoken-rs` baixa o arquivo de
  ranks BPE (`cl100k_base.tiktoken`) de uma URL pública na primeira
  construção do tokenizer, cacheando localmente depois — é exatamente por
  isso que REQ-502 exige fallback offline (rede indisponível no primeiro
  uso não pode quebrar o comando). Isso não é uma ação de "download
  explícito" no sentido da Fase 4 (não é um artefato de ~30MB decidido uma
  vez por sessão) — é comportamento padrão da própria biblioteca em uso
  normal, então não bloqueia a task correspondente como T-406 bloqueava.

# Spec: Retrieval Enrichment — `enrich` (resumos por arquivo via LLM, opt-in) (Fase 19)

> Origem: decisão de 2026-09-30, após testar o `nexspec` no `condominium-management-system` (1282 arquivos, 778 `.ts`, 201 `.tsx`).
> O graphify resolvia perguntas em prosa chamando uma API de LLM; o `nexspec` só acha o que bate com identificadores.
> Objetivo: **igualar ou superar o graphify** em assertividade de prosa, com menos tokens, menos latência e custo transparente.
> Relaciona-se com a Fase 17 (`llm-enrichment`, condicional) e a Fase 18 (`semantic-annotations`, sem LLM); ver "Relação com as Fases 17 e 18".
> Decisões de 2026-09-30 (Q1–Q5 e complementos) já incorporadas abaixo; ver "Decisões tomadas".

## Summary

Perguntas em prosa ("como o contrato do inquilino é criado", "como as faturas são cobradas dos moradores") não batem com nomes de classes
e funções, então o BM25 devolve só arquivos ou ruído. `nexspec enrich` pede a um LLM **uma descrição curta por arquivo, em cada idioma que o
usuário escolher** (o que faz, termos de negócio, sinônimos que um dev pesquisaria), guarda em cache local e **indexa essas descrições como
campos próprios no Tantivy**, um por idioma. É estritamente opt-in: sem `enrich` executado, nada muda, e o grafo determinístico nunca é tocado.

## Evidência que motivou a fase (condominium, 2026-09-30)

- `query "how is tenant contract created and validated"` → só 5 arquivos `page.tsx`/`spec.md`, nenhum símbolo nem usecase.
- `query "how are invoices charged to residents"` → usecases de endereço/sessão sem relação (ruído).
- `query "where do we hash passwords"` → correto (`hash` em `nest-hasher`): o nome do símbolo aparece na pergunta.
- Com o modelo ONNX presente (build `full`), o resultado das duas primeiras não mudou. O benchmark da Fase 8 já havia mostrado que o MiniLM ranqueia
  texto cheio de identificadores mal (peso do vetor 0,1). Portanto os embeddings locais sozinhos não resolvem.
- Cumpre o portão de decisão da Fase 17 (falta de contexto semântico piora respostas P0) para o caso **ranking de prosa**.

## Decisões tomadas (2026-09-30)

| # | Decisão | Consequência |
|---|---|---|
| Q1 | Os resumos vão **direto para o Tantivy** (campos próprios), não para um ranking em memória. | Bump de `INDEX_FORMAT`; o índice passa a materializar o cache de enriquecimento (REQ-1907). |
| Q2 | Enriquecimento **não é versionado no Git por padrão** (o graphify também mantém o cache local); prioridade é performance e economia de tokens, não compartilhamento. | Cache local git-ignorado em `.specs/.cache/` (REQ-1905). Compartilhar entre o time fica fora de escopo. |
| Q3 | **Um campo por idioma, nos idiomas que o usuário escolher** (`en,ru`, `en,pt` …). Misturar idiomas no mesmo campo prejudica o BM25 (estatísticas de termos e pesos). | `--lang en,ru`; campos `summary_<lang>` (REQ-1904, REQ-1907). |
| Q4 | Modelo padrão = o mais barato que passe no REQ-1911; subir só se reprovar. | `model` gravado por entrada; trocar não perde nada. |
| Q5 | Custo em tokens **com e sem provedor**, como o graphify: estimativa local antes (sem rede) **e** uso real reportado pelo provedor depois, com preço em USD. | REQ-1908. |
| — | **Enriquecer primeiro o que importa** (ordem por importância), porque melhora a DX: o primeiro uso já cobre o que mais se consulta e pode ser interrompido sem perda. | REQ-1906. |
| — | **`enrich` nunca roda no hook.** Só `sync` é automático; o estado do enriquecimento é apenas sinalizado. | REQ-1910. |

## Requirements

- REQ-1901: **Comando `enrich`** — `nexspec enrich [--provider gemini] [--model M] [--lang L1,L2,…] [--batch N] [--max-concurrency N] [--top P%|N] [--token-budget T] [--dry-run] [--status] [--clear] [--yes]`. Nenhuma chamada de rede sem executar `enrich` explicitamente; `sync`, `search`, `query` e os hooks nunca chamam provedor. Não é exposto como tool MCP (ação com custo e saída de código não deve ser invocável pelo agente sem o usuário).
- REQ-1902: **Provedor plugável, Gemini primeiro** — trait `EnrichProvider` (entrada: lote de `{path, trecho}`; saída: por item, um resumo por idioma pedido, mais o uso de tokens do lote). Implementação inicial: Gemini `generateContent` (`--model` ou `NEXSPEC_ENRICH_MODEL`), resposta JSON validada por schema. A chave vem **somente** de `GEMINI_API_KEY` (ou `GOOGLE_API_KEY`), é enviada por cabeçalho (`x-goog-api-key`) e **nunca** em URL, log, mensagem de erro, cache, índice ou relatório. O trait permite um provedor compatível com OpenAI depois (REQ-1701).
- REQ-1903: **Alvos e o que sai da máquina** — somente arquivos já indexados em linguagens suportadas (TS/JS, Python, Go, Rust) e Markdown; respeita `.gitignore`; nunca envia `.env*`, chaves/certificados (`*.pem`, `*.key`, `id_*`), binários ou gerados (`dist/`, `node_modules/`). Envia caminho + primeiros N caracteres (default 2500, configurável), não o arquivo inteiro. Antes de enviar, varre o trecho com os padrões de segredo do REQ-1807 (chave/token/senha literais) e **omite o arquivo** se houver match (registrado em `--status`). O primeiro uso exige confirmação explícita (`--yes` em modo não interativo), mostrando arquivos/caracteres/idiomas que serão enviados e para qual provedor.
- REQ-1904: **Resumo por idioma, útil para busca** — o prompt pede, **para cada idioma de `--lang`**, 1–2 frases (≤ 40 palavras) com: o que o arquivo faz, conceitos de domínio/negócio e sinônimos que um desenvolvedor pesquisaria, escritos **nativamente naquele idioma** (não traduzidos palavra a palavra do inglês). Um único pedido devolve todos os idiomas (`{"en": "...", "ru": "..."}`), evitando reenviar o código por idioma. Default `--lang en`; `en` não é obrigatório mas é recomendado (código e specs em inglês). Adicionar um idioma depois enriquece **somente o idioma novo**. O prompt tem versão (`prompt_version`) gravada em cada entrada.
- REQ-1905: **Cache local, regenerável do código mas caro** — entradas em `.specs/.cache/enrichment.jsonl` (uma por linha, ordenação estável por caminho): `{path, content_hash, lang, summary, model, prompt_version, at, confidence: "INFERRED"}`, uma linha por arquivo × idioma. **Git-ignorado** (`doctor` e `init` garantem `.specs/.cache/` no `.gitignore`). Fica **fora** de `.specs/.index/`: apagar o índice não perde enriquecimento. Não é versionado nem compartilhado por padrão.
- REQ-1906: **Ordem por importância, incremental e retomável** — `enrich` processa os arquivos em ordem decrescente de importância: grau no grafo/God nodes (Fase 10), arquivos ligados a REQ/TASK, arquivos mais alterados (co-change, Fase 2); `--top 20%` ou `--top 300` limita a passada aos mais importantes. Chave de cache `blake3(conteúdo) + idioma + model + prompt_version`: pula o que está em dia, **persiste após cada lote** e pode ser interrompido (Ctrl+C, `--token-budget`) sem perder nem repetir trabalho — o que já foi feito é exatamente o mais importante. Entradas de arquivos removidos são descartadas; arquivo cujo `content_hash` mudou fica `stale` (aparece em `--status`) e **sai do ranking** até ser reenriquecido (mesma regra do REQ-1803).
- REQ-1907: **Campos por idioma no Tantivy** — o esquema ganha um campo `summary_<lang>` por idioma configurado (tokenização ciente de identificadores da Fase 8 + stemming/folding do idioma quando disponível; `ru`, `pt`, `es`, `fr`, `de`… via analisadores do Tantivy, com *fallback* para tokenizador Unicode simples), além do `text` atual. A busca consulta `text` + todos os `summary_<lang>` ativos com pesos por campo (`text` mais alto que resumos; hipótese inicial `text`×1,0, `summary_*`×0,5, calibrada no benchmark, `NEXSPEC_ENRICH_WEIGHT` para comparar sem rebuild) — **sem detectar o idioma da pergunta**. `INDEX_FORMAT` sobe; índices antigos são reconstruídos automaticamente (já suportado). O `sync` materializa o cache nos documentos dos arquivos cujo `content_hash` ainda bate, então um índice reconstruído recupera o enriquecimento **sem chamar a API**; `enrich` aplica cada lote ao índice de forma incremental (Coordinator/WAL) e **não exige `sync` depois**. Não cria nós nem arestas, não altera ids. `--no-enrich` (em `search`/`query`) consulta só o `text`. O resumo nunca substitui o trecho de código na resposta; pode aparecer como linha de contexto rotulada `(inferido)`.
- REQ-1908: **Contabilidade de tokens e custo, com e sem provedor** — (a) **sem provedor** (`--dry-run`, offline): arquivos, caracteres, tokens de entrada e de saída **estimados** com o `Tokenizer` local (Fase 5), por idioma e para cada ponto da curva de cobertura (20 %, 50 %, 100 %) na ordem de importância; (b) **com provedor**: soma `input_tokens`/`output_tokens` **reais** reportados pelo provedor (`usageMetadata` no Gemini) por lote e total, e custo em USD pela tabela de preços do modelo (configurável, `NEXSPEC_ENRICH_PRICE_IN/OUT` por 1M tokens); (c) **retorno do investimento**: `--status` e o fim do `enrich` mostram tokens gastos no enriquecimento × economia média por consulta medida pelo benchmark (Fase 8), ou seja, em quantas consultas o custo se paga. `--token-budget T` interrompe com saída limpa; se a estimativa passar de 500 mil tokens de entrada sem `--yes`, recusa. Concorrência limitada (default 4) com *retry* exponencial em 429/5xx (máx. 3).
- REQ-1909: **Falha segura** — erro de rede, resposta incompleta ou JSON fora do schema descarta apenas os itens afetados do lote, registra aviso com o caminho (nunca o trecho nem a chave) e segue; nunca apaga entradas válidas; código de saída distinto quando houve falhas parciais (contrato estável, como REQ-1606). O grafo e o índice determinísticos nunca são degradados.
- REQ-1910: **Nunca no hook; sinalização barata** — `hook install` (Fase 16) **não** inclui `enrich`. O `sync` termina com uma linha informativa só quando já existe cache (`enrichment: N stale, M pendentes — nexspec enrich`), sem rede. `enrich --status` mostra elegíveis, em dia, `stale`, pendentes, omitidos por segredo, falhados, idiomas, modelo, data, tokens reais acumulados e custo. `doctor` avisa da ausência de `GEMINI_API_KEY` somente se já existe cache, de muitas entradas `stale` e de `.specs/.cache/` fora do `.gitignore`.
- REQ-1911: **Validação antes de virar padrão do fluxo** — o corpus do benchmark (Fase 8) do condominium ganha perguntas `behavior` em prosa (incluindo as da evidência, nos idiomas de `--lang` testados, p. ex. inglês e português). O `bench` passa a reportar **lado a lado** (sem enriquecimento × com enriquecimento) `recall@k`, `MRR`, tokens da resposta e latência, e o baseline "corpus inteiro" (REQ-808). Critérios: `recall@5` das perguntas em prosa melhora ≥ 15 pontos **e** `MRR` de `locate` não cai mais de 2 pontos (hoje 0,87) **e** a latência de `search` não piora mais que 20 %. Sem isso o peso dos campos `summary_*` fica 0 e o recurso permanece experimental.
- REQ-1912: **Testabilidade sem rede** — testes usam um `EnrichProvider` falso com respostas gravadas (*fixtures*, inclusive multi-idioma e com `usageMetadata`); nenhum teste chama a API. O provedor real é coberto por um teste `#[ignore]` que exige `GEMINI_API_KEY`.
- REQ-1913: **Contrato para a skill** — códigos de saída e saída de `--status`/`--dry-run` estáveis (texto curto na primeira linha, como `check-update`). A skill `graph-spec-design` (outro repositório) oferece `enrich` como passo **opcional** no Phase 0, mostrando a curva de custo do `--dry-run`, e trata "busca em prosa sem resultado útil" assim: tentar `search` com termos de domínio → ler `explain` dos candidatos → só então sugerir `enrich`.

## Relação com as Fases 17 e 18

- **Fase 17 (`llm-enrichment`)**: o REQ-1701 (provedor plugável) é o mesmo trait; esta fase o implementa primeiro para um único caso de uso (ranking). Rótulos de comunidade (REQ-1702) e extração de relações em docs (REQ-1703) continuam lá, condicionais. Ao implementar esta fase, reescrever o REQ-1701 para referenciar `EnrichProvider` e reaproveitar a contabilidade do REQ-1908.
- **Fase 18 (`semantic-annotations`)**: não usa LLM do `nexspec`; é complementar. Reaproveitar o detector de segredos (REQ-1807) e o conceito `stale`/`dangling`. O cache desta fase **não** usa `.specs/.memory/` (que é versionável). Se um arquivo tiver anotação do agente **e** resumo, a anotação do agente vence na exibição.

## Out of Scope

- Resumos por símbolo/função (v1 é por arquivo; símbolos herdam o contexto pelo arquivo via `DefinedIn`).
- Rótulos de comunidade e relações inferidas entre documentos (Fase 17).
- Provedores além do Gemini nesta entrega (o trait deixa o caminho aberto).
- Compartilhar o cache de enriquecimento entre pessoas (versionamento no Git, cache remoto).
- Detectar o idioma da pergunta ou traduzi-la em tempo de busca; chamar LLM a cada consulta (custo e latência por consulta; o agente chamador já reformula a pergunta de graça, ver REQ-1913).
- Qualquer envio de código sem execução explícita de `enrich`.

## Open Questions

- Q1: Peso inicial por campo e *boost* de caminho: os valores do REQ-1907 são hipóteses; calibrar com o benchmark antes de travar.
- Q2: Como compor a "importância" do REQ-1906 (pesos de grau, vínculo a REQ/TASK, co-change)? Começar por grau + vínculo a REQ e medir se a curva de recall × % enriquecido satura cedo (se 20 % já entrega ~80 % do ganho, o default de `--top` pode ser 30 %).
- Q3: Tabela de preços dos modelos: embutida com data de referência ou só via variável/configuração? Preço muda; preferir configuração com *default* documentado e aviso quando o modelo não estiver na tabela.
- Q4: Stemming/folding por idioma: quais idiomas o Tantivy cobre bem (ru, pt, es, fr, de…) e como tratar os que não cobre (zh, ja, ko precisam de tokenizador próprio)?

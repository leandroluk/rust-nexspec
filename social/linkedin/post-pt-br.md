**Título do documento (limite de 58 caracteres):** NexSpec: contexto para agentes de código, em Rust

---

Agentes de código leem arquivos. Projetos são feitos de relações.

Quando um agente precisa entender "qual função satisfaz o REQ-105?" ou "o que quebra se eu mexer nesse símbolo?", ele geralmente despeja arquivos inteiros no prompt. Tokens vão embora em código que a tarefa nem precisava, e a ligação entre spec e código continua na cabeça de quem escreveu.

Por isso estou construindo o NexSpec: um motor de contexto para agentes de código, em Rust, num único binário.

O que ele faz:
→ indexa código (TS/JS, Python, Go, Rust) e specs em Markdown num único grafo: requisitos, tasks, ADRs, símbolos e arquivos, ligados por Satisfies / DependsOn / Implements / DefinedIn
→ busca híbrida: BM25 (Tantivy) + vetores (HNSW, opcional) com fusão RRF, e expansão de vizinhança no grafo
→ poda o corpo das funções (fica só a assinatura) e entrega Markdown denso dentro de um orçamento de tokens, com corte determinístico
→ sync incremental via Git nativo (gix, sem subprocess) e `blame` que entende AST
→ CLI + servidor MCP, pra usar direto no Claude Code, Cursor e afins

Uma decisão de design de que eu gosto: toda escrita passa por um único Sync Coordinator (WAL + versão atômica). Redb, CSR, Tantivy e HNSW commitam cada um por conta própria, então o que decide o que é visível é a versão do coordinator. Um crash no meio do sync nunca deixa os índices divergentes.

O projeto foi construído spec-first (as specs em `.specs/` são a fonte da verdade) e é open source, Apache-2.0. As fases 0 a 6 do roadmap estão implementadas. Ainda é cedo, então feedback, issues e críticas são muito bem-vindos.

Repo: https://github.com/leandroluk/rust-nexspec
Docs: https://leandroluk.github.io/rust-nexspec

Se você usa agentes de código no dia a dia, o que mais te falta hoje em contexto?

#Rust #AI #OpenSource #MCP #DeveloperTools #LLM #AICodingAgents

---

## Versão curta (se quiser algo mais enxuto)

Agentes de código leem arquivos inteiros. Projetos são feitos de relações entre specs e código.

Estou construindo o NexSpec: um motor de contexto em Rust, num único binário, que indexa código e specs num só grafo, faz busca híbrida (BM25 + vetores) e entrega contexto podado dentro de um orçamento de tokens. CLI e servidor MCP incluídos.

Open source (Apache-2.0), feito spec-first. Feedback é muito bem-vindo.

https://github.com/leandroluk/rust-nexspec

#Rust #AI #OpenSource #MCP

---

## Sugestões para o post

- Anexe o `nexspec-carousel.pdf` como documento: o LinkedIn renderiza como carrossel.
- Coloque o link do repo no primeiro comentário, além do texto, se quiser mais alcance.
- A pergunta final ajuda a puxar comentários.

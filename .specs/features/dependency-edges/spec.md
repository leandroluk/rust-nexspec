# Spec: Dependency Edges — grafo de import e referência entre arquivos (Fase 7)

> Origem: avaliação de 2026-09-29 ("o que faria para transformar promissor em confiável", item 1). Medido no projeto
> `condominium-management-system`: `nexspec trace CachePort` devolve só `DefinedIn` embora 7 arquivos o usem.
> Prioridade: **alta** — é a maior lacuna frente ao graphify (12 mil arestas) e destrava `trace` reverso útil, `diff --staged` e o `report` (Fase 10).

## Summary

Hoje o grafo tem `DefinedIn` (símbolo → arquivo), `Satisfies` (símbolo/task → REQ), `CoChanges` (arquivo ↔ arquivo) e chamadas **só dentro do mesmo arquivo** (REQ-303). Falta o que responde "quem depende de X?": imports e referências **entre arquivos**. A Fase 7 extrai essas relações com Tree-sitter, resolve o destino para um arquivo/símbolo indexado e emite arestas `DependsOn`, mantendo o sync incremental.

## Requirements

- REQ-701: **Extração de imports TS/JS** — `import … from`, `import type`, `export … from`, `export * from`, `import("literal")` e `require("literal")`, capturando o especificador e os nomes importados (com alias).
- REQ-702: **Resolução de especificadores** — relativos (`./x`, `../y`, com extensões `.ts/.tsx/.js/.mjs/.cts/.mts` e `index.*`), aliases de `tsconfig` (`paths`, `baseUrl`), `imports` do `package.json` (`#/*`) e pacotes do workspace (`@scope/pkg` → `exports`/condição `source` → arquivo em `src/`). Especificador de pacote externo não resolvido é ignorado (sem aresta), nunca erro.
- REQ-703: **Arestas `DependsOn`** — de arquivo para arquivo sempre que resolvido; de símbolo para símbolo quando o nome importado casa com um `Symbol` exportado do arquivo de destino (classe, função, interface, enum, type — inclui `abstract class`); fallback para arquivo↔arquivo quando o símbolo não é identificável.
- REQ-704: **Referências de tipo e herança** — `extends`, `implements`, `new X`, decorators e **tipos de parâmetro de construtor** (injeção de dependência NestJS) geram `DependsOn` do símbolo que referencia para o símbolo referenciado (via imports resolvidos do arquivo).
- REQ-705: **Outras linguagens (segunda entrega)** — imports de módulo em Python (`import`, `from … import`), Go (`import "path"`, resolvido por `go.mod`) e Rust (`use`, `mod`, dentro do crate). Escopo file-level; símbolo-level só se houver ganho claro.
- REQ-706: **Incrementalidade** — arestas são chaveadas pelo caminho de origem: alterar/renomear/apagar um arquivo remove as arestas antigas que partem dele (`EdgeMutation::Remove`) e reavalia dependentes cujo destino deixou de resolver. Renomeações e deleções não deixam arestas órfãs.
- REQ-707: **Consulta** — `trace <símbolo|arquivo>` lista dependentes (`<-DependsOn`) e dependências; `diff --staged` usa as novas arestas para o raio de impacto; profundidade e volume limitados (teto de nós por hop, com contagem "+N omitidos") para não explodir em God nodes.
- REQ-708: **Custo** — extração paralela (rayon) e ≤ 20% de aumento no tempo do primeiro `sync` do repositório de referência (ver Fase 9); sync sem mudanças continua < 2 s.

## Acceptance

- No repositório `condominium-management-system`: `nexspec trace CachePort` lista os 7 arquivos que o usam; `nexspec trace AccessUserPersonaReader` lista os 11 arquivos que o referenciam; nenhum falso positivo em uma amostra de 30 arestas revisada à mão.
- Teste com fixture TS cobrindo: alias `paths`, barrel `index.ts`, re-export, import só de tipo, import circular, especificador não resolvido.

## Out of Scope

- Análise de chamadas de método (`obj.method()`) e inferência de tipos (exigiria o compilador TS).
- Resolução por Language Server (LSP).
- Grafo de chamadas entre pacotes externos em `node_modules`.

## Open Questions

- Q1: Resolver `tsconfig`/`package.json` por leitura direta (JSON5 leve) ou reaproveitar crate existente? Recomendação: leitura direta, sem dependência nova, cobrindo `extends` simples.
- Q2: Aresta símbolo→símbolo por nome exportado pode errar com re-exports em cadeia; aceitar fallback arquivo↔arquivo na v1?

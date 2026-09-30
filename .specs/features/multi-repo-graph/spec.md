# Spec: Multi-Repo Graph — grafo global e chamadas entre repositórios (Fase 13)

> Origem: paridade com `graphify merge-graphs`, `global add|remove|list|path`, `cross_repo_calls.py` e `merge-driver`.
> Motivação do projeto: o monorepo do condomínio deve poder ser quebrado em microservices/lambdas (premissa do usuário);
> o conhecimento precisa atravessar repositórios sem reindexar tudo.

## Summary

Cada repositório tem seu índice local (`.specs/.index/`, git-ignorado). Para responder "quem chama este serviço?" quando o
código vive em repositórios diferentes, é preciso um grafo global que una índices de vários repositórios, preservando a
origem de cada nó e ligando chamadas/imports que cruzam a fronteira.

## Requirements

- REQ-1301: **Repositório de origem em todo nó** — nós e arestas exportados carregam `repo` (tag) e caminho relativo; ids permanecem estáveis dentro do repositório e são prefixados por `repo` no grafo global (sem colisão).
- REQ-1302: **`global add <repo|export.json> [--as <tag>]`, `global list`, `global remove <tag>`, `global path`** — grafo global em `~/.nexspec/global/` (um índice próprio), alimentado a partir do JSON do REQ-1201 ou diretamente de um índice local. Atualizar o mesmo `--as` substitui os nós daquela origem (idempotente).
- REQ-1303: **`merge-graphs <a.json> <b.json>… --out <arquivo>`** — união determinística de exports; conflitos de mesmo id resolvidos por regra documentada (último `commit` indexado vence).
- REQ-1304: **Ligação entre repositórios** — pacotes publicados/consumidos (`package.json` name ↔ dependência, `Cargo.toml`, imports por nome de pacote) e chamadas HTTP declaradas (OpenAPI/rotas) geram arestas `DependsOn` cruzando `repo`, marcadas `INFERRED` quando baseadas em nome.
- REQ-1305: **Consultas globais** — `query`, `path`, `affected` (Fase 11) aceitam `--global` e `--repo <tag>`; resultados identificam o repositório de cada nó.
- REQ-1306: **Driver de merge do Git** — `nexspec merge-driver` para exports versionados (`*.graph.json`), resolvendo conflitos por união; instalado via `nexspec hook install` (Fase 16).

## Out of Scope

- Sincronização automática entre máquinas/nuvem (o grafo global é local; o export é o meio de compartilhamento).
- Resolução de chamadas por inferência de tipos entre repositórios.

## Open Questions

- Q1: O grafo global deve reutilizar o mesmo formato de índice (redb/CSR) ou ser só o JSON mesclado? Recomendação: mesmo formato, para reaproveitar todas as consultas.

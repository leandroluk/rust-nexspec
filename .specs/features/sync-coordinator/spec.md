# Spec: Sync Coordinator & Transactional Integrity (Fase 0)

## Summary

NexSpec persiste o mesmo grafo lógico em quatro stores fisicamente independentes:
`redb` (metadados), o arquivo CSR (topologia), Tantivy (léxico) e HNSW (vetorial).
Cada um faz commit por conta própria. Sem coordenação central, um crash no meio de
um `sync` pode deixar esses stores divergentes entre si (ex.: uma edge gravada no
CSR mas nunca indexada no Tantivy). O Sync Coordinator resolve isso: é o único
caminho de escrita para os quatro stores, garantindo que um ciclo de sync seja
visto por qualquer leitor como "tudo aconteceu" ou "nada aconteceu" — nunca um
estado parcial. É a fundação de todas as fases seguintes: Fase 1 (CSR) já assume
um `sync_version` disponível em `redb`.

## Requirements

- REQ-001: Todo ciclo de sync grava primeiro a intenção de mutação (conjunto de
  nós/edges/docs a inserir, atualizar ou remover) num Write-Ahead Log em
  `.specs/.index/sync.wal` antes de tocar qualquer store.
- REQ-002: Existe um `sync_version: u64` monotônico, persistido em `metadata.redb`,
  que representa a última versão totalmente committada. Leitores só devem
  enxergar dados na versão `sync_version` ou anterior.
- REQ-003: Cada store grava sua mutação num caminho de staging próprio (ex.:
  `edges.bin.staging`) e só é promovido ao caminho real (rename atômico) depois
  que os quatro stores confirmarem sucesso para aquela versão.
- REQ-004: Se o processo cair com entradas não confirmadas no WAL, `nexspec sync
  --resume` no próximo start detecta isso e decide deterministicamente: reaplica
  (se a mutação nunca foi promovida em nenhum store) ou descarta (se já foi
  promovida em todos) — nunca deixa o índice servindo estado parcial.
- REQ-005: Toda operação de mutação (inserção/atualização/remoção de nó, edge ou
  doc) é idempotente — reaplicar a mesma entrada do WAL após um resume não deve
  duplicar edges no CSR nem documentos no Tantivy.
- REQ-006: O coordinator expõe uma API interna única (`stage(mutations) ->
  commit() | abort()`) que os módulos das fases seguintes (1–5) usam para
  qualquer escrita — nenhum módulo escreve diretamente em `redb`/CSR/Tantivy/HNSW
  fora desse caminho.
- REQ-007: `nexspec sync` normal (sem crash) deve ser observável de fora como uma
  transação atômica: um leitor concorrente nunca vê metade das mutações de um
  ciclo aplicadas e a outra metade não.

## Affected Components (from graph)

Sem grafo disponível ainda (projeto greenfield, modo degradado — ver
`.specs/project/STATE.md`). Este é o primeiro módulo real do codebase; não há
componentes pré-existentes a considerar. Módulos que esta feature cria:

- `sync::coordinator` — orquestra staging/commit/abort entre os 4 stores
- `sync::wal` — leitura/escrita do write-ahead log e replay determinístico
- `sync::version` — o ponteiro `sync_version` atômico em `redb`

Fases futuras (1–6) dependem diretamente desta: qualquer módulo que grave em
`redb`, CSR, Tantivy ou HNSW passa a ser, por definição, um chamador de
`sync::coordinator` — não um God Node em si, mas um ponto de acoplamento
obrigatório de todo o sistema (vale registrar em `CONCERNS.md` quando o código
existir).

## Out of Scope

- Implementação real de `redb`/CSR/Tantivy/HNSW (Fases 1, 3, 4) — aqui só a
  interface de staging que eles vão implementar (trait/contrato), não o dado em
  si.
- Compactação do CSR delta layer (é consumidor deste coordinator, especificado na
  Fase 1).
- Replicação ou sync entre múltiplas máquinas — NexSpec é single-process,
  single-repo; "sync" aqui é sempre local (índice ↔ Git working tree).
- Métricas/observabilidade do coordinator (pode entrar como feature própria
  depois, não bloqueia a Fase 0).

## Open Questions

Resolvidas com decisão padrão (autor pode revisar antes do Design, mas não
bloqueiam o início):

- **Formato do WAL**: proposto formato binário simples (length-prefixed frames,
  cada um um mutation-set serializado com `rkyv` para consistência com o resto do
  projeto) em vez de um formato textual. Decisão: usar `rkyv`, mesma stack de
  serialização já escolhida para o CSR.
- **Quantos stores realmente existem no momento em que a Fase 0 é implementada?**
  Na Fase 0 isolada, só `redb` existe de fato (metadados + WAL + version pointer).
  Decisão: a Fase 0 implementa o coordinator com uma trait `SyncParticipant`
  (`stage`/`commit`/`abort`) e o único participante real inicialmente é o próprio
  `redb`; CSR/Tantivy/HNSW passam a implementar essa trait quando suas fases
  chegarem. Isso evita bloquear a Fase 0 esperando as Fases 1/3/4 existirem.
- **Granularidade de uma "versão" de sync**: um `sync_version` por `nexspec sync`
  completo (todas as mudanças de um `git diff` desde o último índice), não por
  arquivo individual. Alinhado com o modelo de Tree-Diff Incremental Sync da
  Fase 2.

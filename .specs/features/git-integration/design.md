# Design: Git Integration & Incremental Sync (Fase 2)

## Architecture Overview

```
                    Git repository (.git/)
                            │
                    [ gix::Repository ]
                            │
              ┌─────────────┴──────────────┐
              ▼                             ▼
     GitSource::diff_since_last_index   GitSource::dirty_paths
     (HEAD vs last_indexed_commit,      (working tree vs Blake3
      via gix tree diff)                 cache, in-memory only)
              │                             │
              └──────────────┬──────────────┘
                              ▼
                      TreeDiff { added, modified, deleted }
                              │
                              ▼
                   SyncOrchestrator::run_once()
                              │
            for each .md in added/modified:
              graph::markdown::extract() -> MutationSet
            for each path in deleted:
              MutationSet{ removes for that file's known entities }
            + co-change edges (REQ-206) + commit->spec links (REQ-207)
                              │
                              ▼
                 sync::coordinator::Coordinator::stage()
                    (Fase 0 — inalterado; RedbParticipant +
                     CsrParticipant já implementam SyncParticipant)
                              │
                              ▼
              on success: GitSource::advance(new_head_oid)
              (writes last_indexed_commit — via VersionPointer's
               same redb `meta` table, own key)
```

**Key decision:** `SyncOrchestrator` is *not* a `SyncParticipant`. It is the
caller that decides when and with what `MutationSet` to invoke
`Coordinator::stage()` — matching the boundary `storage-primitives/design.md`
already drew for `markdown::extract()`. Git integration doesn't change who
writes to the stores; it changes who decides *what* gets staged.

## Dependency Paths

- REQ-201 → `git::source::GitSource` (new module `src/git/source.rs`), wraps
  `gix::open(path)`.
- REQ-202 → `sync::version::VersionPointer` gets a second key
  (`"last_indexed_commit"`) in the same `meta` table — no new table, no new
  participant. A small extension method `VersionPointer::last_indexed_commit()`
  / `set_last_indexed_commit()`.
- REQ-203 → `GitSource::diff_since_last_index(&self) -> TreeDiff` — walks
  `gix`'s tree diff between two commit trees.
- REQ-204 → `DirtyCache` (in-memory `HashMap<PathBuf, [u8;32]>` of last-seen
  Blake3 per working-tree file), owned by `SyncOrchestrator`, not persisted.
- REQ-205 → `sync::orchestrator::SyncOrchestrator::run_once()` (new module
  `src/sync_orchestrator.rs` — deliberately outside both `sync::` and
  `graph::` since it depends on both plus `git::`, and is itself not a
  storage primitive).
- REQ-206 → `git::cochange::co_change_edges(&GitSource, window: CoChangeWindow) -> Vec<EdgeMutation>`,
  new `EdgeType::CoChanges` variant added to `graph::edge::EdgeType`
  (extends the Fase 1 enum — not a breaking change, `from_code`/`to_code`
  gain one more case).
- REQ-207 → `git::spec_link::extract_commit_links(commit) -> Vec<DocMutation>`
  (or a dedicated small payload type — decided in Open Questions of spec.md
  as "linked metadata", not a new `NodeType`; implementation attaches to the
  existing node via a `CommitLink` doc, TBD at Tasks time, not re-litigated
  here since it doesn't change any Fase 0/1 contract).
- REQ-208 → `GitSource` construction and `diff_since_last_index` must not
  panic/error on detached HEAD or a dirty tree — covered by fixture repos in
  tests, created programmatically (via `gix`'s own commit-creation API or
  shelling to a fixture-setup script that itself uses `git` only inside
  `tests/fixtures/` setup code, never inside `src/`).

## New Components

| Component              | Responsibility                                                                 | Location                   |
| ---------------------- | ------------------------------------------------------------------------------ | -------------------------- |
| `GitSource`            | Wraps `gix::Repository`; diff since last index, dirty-path detection           | `src/git/source.rs`        |
| `TreeDiff`             | `{ added: Vec<PathBuf>, modified: Vec<PathBuf>, deleted: Vec<PathBuf> }`       | `src/git/source.rs`        |
| `DirtyCache`           | In-memory Blake3-per-path cache for working-tree changes                       | `src/git/dirty_cache.rs`   |
| `CoChangeWindow`       | `{ max_commits: usize, max_age: Duration }`, default 500/6mo                   | `src/git/cochange.rs`      |
| `co_change_edges`      | Computes `EdgeType::CoChanges` edges over a bounded commit window              | `src/git/cochange.rs`      |
| `extract_commit_links` | Parses commit messages for `REQ-`/`TASK-`/`ADR-` mentions                      | `src/git/spec_link.rs`     |
| `SyncOrchestrator`     | Ties `GitSource` diff → `markdown::extract` → `Coordinator::stage()` → advance | `src/sync_orchestrator.rs` |

## Modified Components

| Component                       | Change                                                  | Risk                                                                                                         |
| ------------------------------- | ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `sync::version::VersionPointer` | New key `last_indexed_commit` in the same `meta` table  | Low — additive, same transactional path already tested in Fase 0                                             |
| `graph::edge::EdgeType`         | New variant `CoChanges`; `to_code`/`from_code` extended | Low — additive; any code matching `EdgeType` exhaustively (none does outside this crate yet) needs a new arm |

## Risks

- **`gix` API surface maturity**: Gitoxide's tree-diff and commit-walk APIs
  are still evolving faster than `redb`/`rkyv`. Mitigation: wrap all `gix`
  calls behind `GitSource`'s narrow interface (REQ-201's boundary) so an API
  break is a one-file fix, never a ripple through `sync_orchestrator` or the
  graph layer.
- **Co-change cost on large repos**: even bounded to 500 commits, computing
  pairwise file co-occurrence is O(commits × files²) in the naive form.
  Mitigation: only pair files that appear in the *same* commit (already
  small per-commit), not the full file universe — effectively O(commits ×
  avg-files-per-commit²), acceptable for the default window.
- **Detached HEAD / dirty tree edge cases**: `last_indexed_commit` assumes a
  linear notion of "since last time" that gets fuzzy on a detached HEAD
  (no branch to compare forward from). Mitigation: `diff_since_last_index`
  always diffs against the *stored OID*, never a branch ref, so detached
  HEAD is actually the simpler case — the ambiguity would only bite a
  hypothetical branch-tracking feature we don't have.
- **Submodules/LFS/sparse-checkout unhandled**: explicitly out of scope (see
  spec.md Open Questions). Risk is silent incorrect behavior if a consumer's
  repo uses these — mitigated by *not* claiming support; `CONCERNS.md` gets
  a note once code exists.

## Decision Log

- `SyncOrchestrator` lives outside `sync::`/`graph::` (own top-level module)
  — it's the first component that depends on both, and Fase 0/1 deliberately
  kept that composition-root responsibility out of themselves.
- Co-change stored as regular directed `Edge`s (`A->B` and `B->A` for one
  co-occurrence) rather than inventing an undirected edge concept in the CSR
  — reuses REQ-105/106/107 machinery from Fase 1 unchanged.
- `last_indexed_commit` reuses `VersionPointer`'s existing `redb` table/
  transaction path instead of a new store — same rationale as Fase 0's Sync
  Coordinator: fewer independently-committing stores, fewer consistency
  edges to reason about.
- Dirty-tree cache (REQ-204) is explicitly *not* persisted — it answers "what
  changed since the last time *this process* looked", which is inherently
  process-lifetime scoped; persisting it would imply a durability guarantee
  we don't need (a restart re-scanning the working tree once is cheap).

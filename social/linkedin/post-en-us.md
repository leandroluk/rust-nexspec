**Document title (58-character limit):** NexSpec: a Rust context engine for AI coding agents

---

Coding agents read files. Projects are made of relationships.

When an agent needs to answer "which function satisfies REQ-105?" or "what breaks if I change this symbol?", it usually dumps whole files into the prompt. Tokens get burned on code the task never needed, and the link between spec and code stays in the head of whoever wrote it.

So I'm building NexSpec: a context engine for AI coding agents, written in Rust, shipped as a single binary.

What it does:
→ indexes source code (TS/JS, Python, Go, Rust) and Markdown specs into one graph: requirements, tasks, ADRs, symbols and files, linked by Satisfies / DependsOn / Implements / DefinedIn
→ hybrid search: BM25 (Tantivy) + vectors (HNSW, optional) fused with RRF, then k-hop expansion over the graph
→ prunes function bodies down to signatures and serves dense Markdown inside an explicit token budget, with a deterministic cut
→ incremental sync from native Git (gix, no subprocesses) and AST-aware blame
→ CLI + MCP server, so it plugs straight into Claude Code, Cursor and friends

One design choice I like: every write goes through a single Sync Coordinator (WAL + atomic version pointer). redb, the CSR graph, Tantivy and HNSW each commit on their own, so visibility is decided by the coordinator's version, not by any one store. A crash mid-sync can never leave the indexes disagreeing with each other.

It was built spec-first (the specs in `.specs/` are the source of truth) and it's open source under Apache-2.0. Phases 0–6 of the roadmap are implemented. It's early, so feedback, issues and criticism are very welcome.

Repo: https://github.com/leandroluk/rust-nexspec
Docs: https://leandroluk.github.io/rust-nexspec

If you use coding agents daily, what's the biggest thing missing from their context today?

#Rust #AI #OpenSource #MCP #DeveloperTools #LLM #AICodingAgents

---

## Short version (for groups / feeds)

Coding agents read whole files, but projects are made of relationships between specs and code.

I'm building NexSpec: a Rust context engine (single binary) that indexes code + specs into one graph, does hybrid search (BM25 + vectors), and serves pruned context within a token budget. Ships with a CLI and an MCP server.

Open source (Apache-2.0), built spec-first. Feedback welcome.

https://github.com/leandroluk/rust-nexspec

#Rust #AI #OpenSource #MCP

---

## Posting tips

- Upload `nexspec-carousel.pdf` as a document: LinkedIn renders it as a swipeable carousel.
- Group rules often discourage links in the body; if so, drop the link and add it as the first comment.
- The closing question helps drive replies.

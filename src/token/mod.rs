//! Token budgeting & LLM serialization (Fase 5, REQ-501..505 in
//! `.specs/features/token-budgeting/spec.md`). Turns ranked graph nodes
//! into a dense, budget-bounded Markdown payload: prune code-symbol bodies
//! down to signatures ([`pruner`]), estimate token cost via a pluggable
//! [`budget::Tokenizer`], cut by priority tier when the budget doesn't fit
//! everything ([`budget::Budget`]), and serialize the result
//! ([`serializer`]).

pub mod budget;
pub mod pruner;
pub mod serializer;

//! Graph queries (Fase 11, `.specs/features/graph-query-surface/`): a shared
//! view of the graph, one target resolver and edge filters; the queries
//! themselves are pure functions over them.

pub mod affected;
pub mod api;
pub mod budget;
pub mod expand;
pub mod explain;
pub mod filter;
pub mod path;
pub mod target;
pub mod view;

pub use filter::{EdgeFilter, FilterError};
pub use target::{Candidate, Resolved};
pub use view::GraphView;

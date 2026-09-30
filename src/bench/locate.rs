//! Search result -> comparable locations (T-802).

use crate::bench::metrics::{Location, Ranked};
use crate::engine::{Engine, EngineError, SearchResult};

/// Every location behind `result`'s hits, in hit order.
pub fn locations_for(engine: &Engine, result: &SearchResult) -> Result<Vec<Location>, EngineError> {
    let mut all = Vec::new();
    for hit in &result.hits {
        all.extend(engine.hit_locations(hit)?);
    }
    Ok(all)
}

/// De-duplicated file/symbol/marker rankings for `result`.
pub fn ranked_for(engine: &Engine, result: &SearchResult) -> Result<Ranked, EngineError> {
    Ok(Ranked::from_locations(&locations_for(engine, result)?))
}

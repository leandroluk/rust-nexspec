//! `nexspec report` (Fase 10, `.specs/features/report-command/`): a summary of
//! the graph's structure computed from a [`GraphSnapshot`].

pub mod analysis;
pub mod communities;
pub mod questions;
pub mod render;
pub mod snapshot;

use serde::Serialize;

pub use analysis::{Coverage, CycleReport, GodNode, IndexInfo, Summary};
pub use communities::{Communities, Community, Surprise};
pub use snapshot::GraphSnapshot;

/// How much of each list the report includes.
#[derive(Debug, Clone, Copy)]
pub struct ReportOptions {
    /// God nodes listed (REQ-1002).
    pub top: usize,
    /// Suggested questions (REQ-1009).
    pub questions: usize,
}

impl Default for ReportOptions {
    fn default() -> Self {
        Self { top: 10, questions: 5 }
    }
}

/// The whole report. Field names are part of the JSON contract (design.md D4).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Report {
    pub summary: Summary,
    pub god_nodes: Vec<GodNode>,
    /// Barrel files (`index.ts` re-exporting modules) left out of `god_nodes`.
    pub barrel_files_excluded: usize,
    /// Includes the surprising connections under `communities.surprising`.
    pub communities: Communities,
    pub requirement_coverage: Coverage,
    pub import_cycles: Vec<CycleReport>,
    pub suggested_questions: Vec<String>,
}

pub fn build(snapshot: &GraphSnapshot, index: IndexInfo, options: ReportOptions) -> Report {
    let god_nodes = analysis::god_nodes(snapshot, options.top);
    let communities = communities::communities(snapshot, 5, 10);
    let requirement_coverage = analysis::coverage(snapshot);
    let import_cycles = analysis::import_cycle_reports(snapshot);
    let suggested_questions =
        questions::suggested_questions(&god_nodes, &communities, &requirement_coverage, &import_cycles, options.questions);
    Report {
        summary: analysis::summary(snapshot, index),
        god_nodes,
        barrel_files_excluded: analysis::barrel_files(snapshot).len(),
        communities,
        requirement_coverage,
        import_cycles,
        suggested_questions,
    }
}

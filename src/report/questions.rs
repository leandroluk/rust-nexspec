//! Suggested questions (REQ-1009): generated from a fixed template for each
//! kind of finding, in a fixed priority order, so the same graph always yields
//! the same questions.

use crate::report::analysis::{Coverage, CycleReport, GodNode};
use crate::report::communities::Communities;

pub fn suggested_questions(
    god_nodes: &[GodNode],
    communities: &Communities,
    coverage: &Coverage,
    cycles: &[CycleReport],
    limit: usize,
) -> Vec<String> {
    let mut questions: Vec<String> = Vec::new();

    for god in god_nodes.iter().filter(|g| !g.has_requirement).take(3) {
        questions.push(format!(
            "`{}` has {} dependents and is not linked to any requirement: which requirement does it serve?",
            god.label, god.dependents
        ));
    }
    for requirement in coverage.unimplemented_requirements.iter().take(3) {
        questions.push(format!("{requirement} has no implementation or task: is it done elsewhere, or still to do?"));
    }
    for cycle in cycles.iter().take(2) {
        let shown: Vec<&str> = cycle.files.iter().take(3).map(String::as_str).collect();
        let more = cycle.files.len().saturating_sub(shown.len());
        let suffix = if more > 0 { format!(" and {more} more") } else { String::new() };
        match cycle.suggested_removals.first() {
            Some((from, to)) => questions.push(format!(
                "{}{suffix} import each other: is `{from}` -> `{to}` the dependency to cut?",
                shown.join(", ")
            )),
            None => questions.push(format!("{}{suffix} import each other: why?", shown.join(", "))),
        }
    }
    for community in communities.listed.iter().filter(|c| c.fragile).take(2) {
        questions.push(format!(
            "The {} files around `{}` are weakly connected to each other (cohesion {:.2}): one module, or several?",
            community.size, community.label, community.cohesion
        ));
    }
    if !coverage.orphan_references.is_empty() {
        questions.push(format!(
            "{} code references point at requirements that do not exist (for example in `{}`): renamed or removed?",
            coverage.orphan_references.len(),
            coverage.orphan_references[0]
        ));
    }
    for surprise in communities.surprising.iter().take(1) {
        questions.push(format!("Is it intended that `{}` depends on `{}`?", surprise.from, surprise.to));
    }
    questions.truncate(limit);
    questions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::communities::{Community, Surprise};

    fn god(label: &str, dependents: usize, has_requirement: bool) -> GodNode {
        GodNode {
            id: String::new(),
            label: label.into(),
            kind: "symbol".into(),
            path: None,
            degree: dependents,
            dependents,
            dependencies: 0,
            has_requirement,
        }
    }

    fn sample() -> (Vec<GodNode>, Communities, Coverage, Vec<CycleReport>) {
        let gods = vec![god("Hub (a.ts)", 12, false), god("Known (b.ts)", 9, true)];
        let communities = Communities {
            listed: vec![Community { id: 1, label: "pkgs/x".into(), size: 7, cohesion: 0.12, fragile: true, top_files: vec![], files: vec![] }],
            surprising: vec![Surprise {
                from: "ui/a.ts".into(),
                to: "db/b.ts".into(),
                from_community: 1,
                to_community: 2,
                score: 1.2,
                explanation: String::new(),
            }],
            ..Communities::default()
        };
        let coverage = Coverage {
            requirements_total: 4,
            unimplemented_requirements: vec!["REQ-7".into()],
            tasks_without_requirement: vec![],
            orphan_references: vec!["Stray (c.ts)".into()],
        };
        let cycles = vec![CycleReport {
            files: vec!["a.ts".into(), "b.ts".into()],
            suggested_removals: vec![("b.ts".into(), "a.ts".into())],
        }];
        (gods, communities, coverage, cycles)
    }

    #[test]
    fn questions_come_out_in_priority_order_from_each_kind_of_finding() {
        let (gods, communities, coverage, cycles) = sample();
        let q = suggested_questions(&gods, &communities, &coverage, &cycles, 10);
        assert_eq!(q.len(), 6);
        assert!(q[0].contains("Hub (a.ts)") && q[0].contains("12 dependents"), "{}", q[0]);
        assert!(q[1].starts_with("REQ-7 has no implementation"), "{}", q[1]);
        assert!(q[2].contains("a.ts, b.ts import each other") && q[2].contains("`b.ts` -> `a.ts`"), "{}", q[2]);
        assert!(q[3].contains("cohesion 0.12") && q[3].contains("pkgs/x"), "{}", q[3]);
        assert!(q[4].contains("1 code references"), "{}", q[4]);
        assert!(q[5].contains("`ui/a.ts` depends on `db/b.ts`"), "{}", q[5]);
        assert!(!q.iter().any(|s| s.contains("Known (b.ts)")), "a god node that has a requirement raises no question");
    }

    #[test]
    fn the_limit_keeps_the_highest_priority_questions() {
        let (gods, communities, coverage, cycles) = sample();
        let q = suggested_questions(&gods, &communities, &coverage, &cycles, 2);
        assert_eq!(q.len(), 2);
        assert!(q[0].contains("Hub") && q[1].contains("REQ-7"));
    }

    #[test]
    fn a_clean_graph_asks_nothing() {
        assert!(suggested_questions(&[], &Communities::default(), &Coverage::default(), &[], 5).is_empty());
    }
}

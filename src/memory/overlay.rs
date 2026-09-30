//! A light nudge for ranking (REQ-1503 in `.specs/features/work-memory/spec.md`; decisions D7, D8).
//!
//! `reflect` leaves `.specs/.cache/memory.json`: the ids to prefer and the ids to avoid. `search` and `query`
//! multiply the fused score of those nodes (by default ×1.25 and ×0.5) and re-sort; without the file, or with
//! `--no-memory`, nothing changes. The file is derived data (regenerate with `nexspec reflect`) and stays out of Git.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::memory::reflect::{Class, Reflection};
use crate::search::{hex, unhex};
use crate::sync::mutation::StableId;

pub const DEFAULT_BOOST: f32 = 1.25;
pub const DEFAULT_PENALTY: f32 = 0.5;

pub fn cache_path(repo: &Path) -> PathBuf {
    repo.join(".specs").join(".cache").join("memory.json")
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Stored {
    boost: Vec<String>,
    penalty: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct Overlay {
    boost: HashSet<StableId>,
    penalty: HashSet<StableId>,
}

fn factor_from_env(name: &str, default: f32, valid: impl Fn(f32) -> bool) -> f32 {
    std::env::var(name).ok().and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite() && valid(*v)).unwrap_or(default)
}

impl Overlay {
    /// Preferred nodes and contested ones leaning useful are boosted; dead ends and contested ones leaning
    /// the other way are penalised. Tentative nodes (one signal) do not move the ranking.
    pub fn from_reflection(reflection: &Reflection) -> Self {
        let mut overlay = Self::default();
        for lesson in &reflection.lessons {
            let Some(id) = unhex(&lesson.id) else { continue };
            match lesson.class {
                Class::Preferred | Class::Contested { leaning_useful: true } => {
                    overlay.boost.insert(id);
                }
                Class::DeadEnd | Class::Contested { leaning_useful: false } => {
                    overlay.penalty.insert(id);
                }
                Class::Tentative => {}
            }
        }
        overlay
    }

    pub fn is_empty(&self) -> bool {
        self.boost.is_empty() && self.penalty.is_empty()
    }

    pub fn save(&self, repo: &Path) -> std::io::Result<()> {
        let path = cache_path(repo);
        std::fs::create_dir_all(path.parent().expect("has a parent"))?;
        let mut stored = Stored { boost: self.boost.iter().map(hex).collect(), penalty: self.penalty.iter().map(hex).collect() };
        stored.boost.sort();
        stored.penalty.sort();
        std::fs::write(path, serde_json::to_string_pretty(&stored).expect("serialises"))
    }

    /// The overlay `reflect` saved; a missing or damaged file is no overlay.
    pub fn load(repo: &Path) -> Option<Self> {
        let stored: Stored = serde_json::from_str(&std::fs::read_to_string(cache_path(repo)).ok()?).ok()?;
        let ids = |list: &[String]| list.iter().filter_map(|h| unhex(h)).collect::<HashSet<_>>();
        let overlay = Self { boost: ids(&stored.boost), penalty: ids(&stored.penalty) };
        (!overlay.is_empty()).then_some(overlay)
    }

    /// Multiplies the scores of remembered nodes and re-sorts (highest first; equal scores keep their order).
    pub fn apply(&self, fused: &mut [(StableId, f32)]) {
        let boost = factor_from_env("NEXSPEC_MEMORY_BOOST", DEFAULT_BOOST, |v| v >= 1.0);
        let penalty = factor_from_env("NEXSPEC_MEMORY_PENALTY", DEFAULT_PENALTY, |v| (0.0..=1.0).contains(&v));
        for (id, score) in fused.iter_mut() {
            if self.boost.contains(id) {
                *score *= boost;
            } else if self.penalty.contains(id) {
                *score *= penalty;
            }
        }
        fused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::reflect::Lesson;

    fn lesson(n: u8, class: Class) -> Lesson {
        Lesson { id: hex(&[n; 32]), label: format!("n{n}"), class, useful: 1, negative: 0 }
    }

    fn overlay() -> Overlay {
        Overlay::from_reflection(&Reflection {
            lessons: vec![
                lesson(1, Class::Preferred),
                lesson(2, Class::DeadEnd),
                lesson(3, Class::Tentative),
                lesson(4, Class::Contested { leaning_useful: true }),
                lesson(5, Class::Contested { leaning_useful: false }),
            ],
            ..Reflection::default()
        })
    }

    #[test]
    fn preferred_rise_dead_ends_sink_and_a_single_signal_changes_nothing() {
        let mut fused = vec![([2u8; 32], 1.0), ([3u8; 32], 0.9), ([1u8; 32], 0.8), ([9u8; 32], 0.7)];
        overlay().apply(&mut fused);
        let order: Vec<u8> = fused.iter().map(|(id, _)| id[0]).collect();
        assert_eq!(order, [1, 3, 9, 2], "1 rose above 3; the dead end 2 fell below everything");
        assert!((fused[0].1 - 1.0).abs() < 1e-6, "0.8 x 1.25");
        assert_eq!(overlay().boost.len() + overlay().penalty.len(), 4, "the tentative node is in neither set");
    }

    #[test]
    fn the_nudge_is_bounded_so_a_clearly_better_hit_stays_first() {
        let mut fused = vec![([9u8; 32], 1.0), ([1u8; 32], 0.5)];
        overlay().apply(&mut fused);
        assert_eq!(fused[0].0[0], 9, "x1.25 of 0.5 does not overtake 1.0");
    }

    #[test]
    fn the_cache_round_trips_and_a_missing_or_empty_one_is_no_overlay() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(Overlay::load(dir.path()).is_none());
        overlay().save(dir.path()).unwrap();
        let loaded = Overlay::load(dir.path()).unwrap();
        assert_eq!((loaded.boost.len(), loaded.penalty.len()), (2, 2));
        Overlay::default().save(dir.path()).unwrap();
        assert!(Overlay::load(dir.path()).is_none(), "nothing to apply");
        std::fs::write(cache_path(dir.path()), "{ broken").unwrap();
        assert!(Overlay::load(dir.path()).is_none());
    }
}

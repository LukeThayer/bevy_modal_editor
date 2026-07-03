//! `ValidationReport` — Task 6 stub.
//!
//! Task 8 owns the real `ValidationRegistry` (spec §3.3: dangling `trigger_skill`, a
//! Lifecycle-target missing a timeline (blocking), a hit-phase-target missing a timeline
//! (warning only — the packet path is legal, spec D4), a timeline-target condition with
//! `additional == false` (blocking — D4 requires `additional = true` for any trigger whose
//! target has a real timeline), unknown Effect/anim preset names, acquisition fallback dead
//! ends, `EveryNthHit` on a timeline target). None of that runs yet — this module exists purely
//! so Task 6's region signatures (`draw_rules_region(.., report: &ValidationReport)` and its
//! siblings in Tasks 7/9) can take their real final parameter type today instead of a
//! placeholder that would need call-site churn later. `validate_skill_stub` always returns an
//! empty report; every region must treat an absent problem as "not yet checked", not "known
//! clean".
use super::library::SkillLibrary;

/// One skill's validation problems. `problems` is `(target_id, message)`: `target_id` names
/// what the problem is ABOUT — today always a `SkillCondition` slot, encoded
/// `"condition:{index}"` (Task 8 may widen this to name windows/cues/etc. as those regions grow
/// their own validation) — so a region can filter down to just the rows it renders (e.g. the
/// Rules region's trigger cards only care about `"condition:*"` entries).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValidationReport {
    pub problems: Vec<(String, String)>,
}

impl ValidationReport {
    /// Problems whose `target_id` is `"condition:{index}"` for the given trigger-card index —
    /// the lookup `draw_rules_region`'s trigger cards will want once Task 8 populates real
    /// problems (unused today since `validate_skill_stub` never returns any, but the shape is
    /// exercised by `condition_problems_filters_by_target_id` below so the contract holds).
    pub fn for_condition(&self, index: usize) -> impl Iterator<Item = &str> {
        let target = format!("condition:{index}");
        self.problems
            .iter()
            .filter(move |(id, _)| *id == target)
            .map(|(_, msg)| msg.as_str())
    }

    /// Problems whose `target_id` is `"window:{window_id}"` — the lookup Task 7's Behavior
    /// region window cards use (the doc comment above calls out "widen this to name
    /// windows/... as those regions grow their own validation"; this is that widening).
    /// Unused by `validate_skill_stub` today (still always empty) — exercised by
    /// `window_problems_filters_by_target_id` below so the contract holds, same shape as
    /// `for_condition`.
    pub fn for_window<'a>(&'a self, window_id: &str) -> impl Iterator<Item = &'a str> {
        let target = format!("window:{window_id}");
        self.problems
            .iter()
            .filter(move |(id, _)| *id == target)
            .map(|(_, msg)| msg.as_str())
    }
}

/// Stub: always returns an empty report. Task 8 replaces the internals with the real
/// `ValidationRegistry` sweep described above; the SIGNATURE (`&str` id + `&SkillLibrary` in,
/// `ValidationReport` out) is this task's contract with every panel region.
pub fn validate_skill_stub(_id: &str, _library: &SkillLibrary) -> ValidationReport {
    ValidationReport::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_is_always_empty() {
        let library = SkillLibrary::default();
        let report = validate_skill_stub("anything", &library);
        assert!(report.problems.is_empty());
    }

    #[test]
    fn condition_problems_filters_by_target_id() {
        let report = ValidationReport {
            problems: vec![
                ("condition:0".to_string(), "dangling trigger_skill".to_string()),
                ("condition:1".to_string(), "additional must be true".to_string()),
                ("window:bolt".to_string(), "unrelated".to_string()),
            ],
        };
        let msgs: Vec<&str> = report.for_condition(0).collect();
        assert_eq!(msgs, vec!["dangling trigger_skill"]);
        assert!(report.for_condition(2).next().is_none());
    }

    #[test]
    fn window_problems_filters_by_target_id() {
        let report = ValidationReport {
            problems: vec![
                ("window:bolt".to_string(), "anchors on CastPoint".to_string()),
                ("condition:0".to_string(), "unrelated".to_string()),
            ],
        };
        let msgs: Vec<&str> = report.for_window("bolt").collect();
        assert_eq!(msgs, vec!["anchors on CastPoint"]);
        assert!(report.for_window("zone").next().is_none());
    }
}

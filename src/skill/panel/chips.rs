//! Relationship chips (Task 12): a COMPACT causality-navigation strip, distinct from Task 6's
//! Rules-region trigger CARDS (`panel::rules::draw_trigger_cards`, which show full editable
//! condition/target detail). The chips row is a glance-and-jump affordance: one "→ {target}" chip
//! per DISTINCT `conditions[].trigger_skill`, plus one "↺ ×{chain_count}" chip when
//! `damage.can_chain` — clicking a Trigger chip switches `SkillLibrary.open` to the target (with a
//! dirty-check prompt if the currently open entry is unsaved, handled by the caller — see
//! `crate::skill::draw_skill_panel`'s `ChipSwitchPrompt`).
//!
//! ## Chain chip semantics (brief: "decide + document")
//!
//! `damage.can_chain`/`chain_count` describe a RULES-level behavior — "this skill's damage
//! resolution may re-strike up to `chain_count` additional nearby targets"
//! (`obelisk_bevy::timeline::advance::end_hitboxes`'s chain-hop arm). A chain hop re-executes
//! THIS SAME skill's own collision windows against a new target; it never casts, opens, or
//! references a different skill's timeline the way a `conditions[].trigger_skill` does. There is
//! therefore no OTHER skill for a Chain chip to navigate to.
//!
//! Rather than giving `Chip` an `Option<String>` target (special-casing the one variant that
//! can't navigate, and pushing a "did this chip have a real target" check onto every caller), the
//! Chain chip's `target_id` is set to the skill's OWN id — a chain chip always self-targets. This
//! keeps `Chip`'s shape uniform (every chip names a target id). [`draw_chips_row`] enforces the
//! "never navigates" half of this decision directly: it only ever returns a target id for a
//! `ChipKind::Trigger` click; a Chain chip renders (with an explanatory tooltip) but a click on it
//! never becomes a returned target.

use bevy_egui::egui;

use crate::skill::library::{SkillEntry, SkillLibrary};
use crate::ui::theme::colors;

/// What kind of causality edge a chip represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipKind {
    /// A `conditions[].trigger_skill` edge — navigable (see the module doc comment).
    Trigger,
    /// `damage.can_chain` — NOT navigable; `target_id` is the skill's own id (self-targeting, by
    /// construction — see the module doc comment for why this isn't `Option<String>` instead).
    Chain,
}

/// One relationship chip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    pub target_id: String,
    pub label: String,
    pub kind: ChipKind,
}

/// Derive `entry`'s causality chips from its CURRENT (live-edited) rules — see the module doc
/// comment for the Chain chip's self-targeting/non-navigating semantics.
///
/// `library` isn't consulted by the derivation itself (every chip's label/target comes straight
/// off `entry.rules` — even a `trigger_skill` that doesn't presently exist anywhere in `library`
/// still gets a chip; a dangling reference is `crate::skill::validation`'s concern, not this
/// one's). It's taken for symmetry with `crate::skill::library::skills_referencing` and every
/// other Rules-adjacent helper in this module family, and so a future revision that wants to
/// e.g. only show chips for targets that still resolve has it on hand with no signature change.
pub fn causality_chips(entry: &SkillEntry, _library: &SkillLibrary) -> Vec<Chip> {
    let mut chips = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for cond in &entry.rules.conditions {
        if cond.trigger_skill.is_empty() {
            continue;
        }
        // First-occurrence order (authoring order), not sorted — `seen` only dedups membership.
        if seen.insert(cond.trigger_skill.clone()) {
            chips.push(Chip {
                target_id: cond.trigger_skill.clone(),
                label: format!("\u{2192} {}", cond.trigger_skill),
                kind: ChipKind::Trigger,
            });
        }
    }

    if entry.rules.damage.can_chain {
        chips.push(Chip {
            target_id: entry.rules.id.clone(),
            label: format!("\u{21ba} \u{d7}{}", entry.rules.damage.chain_count),
            kind: ChipKind::Chain,
        });
    }

    chips
}

/// Draw the chips row. Returns `Some(target_id)` when a **Trigger** chip was clicked this frame
/// (the caller — `crate::skill::draw_skill_panel` — decides whether to switch `SkillLibrary.open`
/// immediately or stage a dirty-check confirm first; see that fn's `ChipSwitchPrompt` handling).
/// A Chain chip renders with an explanatory tooltip but never contributes to the return value —
/// see the module doc comment. Draws nothing (returns `None`) when there are no chips.
pub fn draw_chips_row(ui: &mut egui::Ui, entry: &SkillEntry, library: &SkillLibrary) -> Option<String> {
    let chips = causality_chips(entry, library);
    if chips.is_empty() {
        return None;
    }

    let mut clicked_target = None;
    ui.horizontal_wrapped(|ui| {
        for chip in &chips {
            let (fill, text_color) = match chip.kind {
                ChipKind::Trigger => (colors::BG_MEDIUM, colors::ACCENT_ORANGE),
                ChipKind::Chain => (colors::BG_DARKEST, colors::ACCENT_CYAN),
            };
            let resp = ui.add(
                egui::Button::new(egui::RichText::new(&chip.label).small().color(text_color))
                    .fill(fill)
                    .corner_radius(egui::CornerRadius::same(10)),
            );
            match chip.kind {
                ChipKind::Trigger => {
                    if resp.on_hover_text(format!("Open '{}'", chip.target_id)).clicked() {
                        clicked_target = Some(chip.target_id.clone());
                    }
                }
                ChipKind::Chain => {
                    resp.on_hover_text(
                        "This skill's damage may re-strike additional nearby targets \
                         (Rules \u{2192} Costs & Damage \u{2192} Can Chain / Chain Count) — a \
                         chain hop re-runs THIS skill's own windows against a new target, it \
                         doesn't cast a different skill, so there's nothing to navigate to.",
                    );
                }
            }
        }
    });

    clicked_target
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::templates::strike_template;
    use std::path::PathBuf;

    fn entry_with(mutate: impl FnOnce(&mut SkillEntry)) -> SkillEntry {
        let (rules, timeline) = strike_template("fireball");
        let mut entry = SkillEntry {
            rules,
            timeline,
            rules_path: PathBuf::new(),
            timeline_path: PathBuf::new(),
            dirty_rules: false,
            dirty_timeline: false,
            disk_hash: (0, 0),
        };
        mutate(&mut entry);
        entry
    }

    fn condition(target: &str, condition: stat_core::TriggerCondition) -> stat_core::SkillCondition {
        stat_core::SkillCondition {
            trigger_skill: target.to_string(),
            additional: true,
            condition,
        }
    }

    #[test]
    fn fireball_pair_produces_one_trigger_chip_to_the_explosion() {
        let entry = entry_with(|e| {
            e.rules.conditions = vec![condition("fireball_explosion", stat_core::TriggerCondition::OnImpact)];
        });
        let library = SkillLibrary::default();

        let chips = causality_chips(&entry, &library);

        assert_eq!(chips.len(), 1);
        assert_eq!(chips[0].target_id, "fireball_explosion");
        assert_eq!(chips[0].label, "\u{2192} fireball_explosion");
        assert_eq!(chips[0].kind, ChipKind::Trigger);
    }

    #[test]
    fn can_chain_skill_produces_a_chain_chip() {
        let entry = entry_with(|e| {
            e.rules.damage.can_chain = true;
            e.rules.damage.chain_count = 3;
        });
        let library = SkillLibrary::default();

        let chips = causality_chips(&entry, &library);

        assert_eq!(chips.len(), 1);
        assert_eq!(chips[0].kind, ChipKind::Chain);
        assert_eq!(chips[0].label, "\u{21ba} \u{d7}3");
        // Self-targeting, not `Option` — see the module doc comment.
        assert_eq!(chips[0].target_id, entry.rules.id);
    }

    #[test]
    fn repeated_trigger_targets_dedup_to_one_chip() {
        let entry = entry_with(|e| {
            e.rules.conditions = vec![
                condition("explosion", stat_core::TriggerCondition::OnImpact),
                condition("explosion", stat_core::TriggerCondition::OnCrit),
                condition("explosion", stat_core::TriggerCondition::OnKill),
            ];
        });
        let library = SkillLibrary::default();

        let chips = causality_chips(&entry, &library);

        assert_eq!(chips.len(), 1, "three conditions targeting the same skill must dedup: {chips:?}");
        assert_eq!(chips[0].target_id, "explosion");
    }

    #[test]
    fn distinct_targets_each_get_their_own_chip_in_authoring_order() {
        let entry = entry_with(|e| {
            e.rules.conditions = vec![
                condition("zeta", stat_core::TriggerCondition::OnImpact),
                condition("alpha", stat_core::TriggerCondition::OnCrit),
            ];
        });
        let library = SkillLibrary::default();

        let chips = causality_chips(&entry, &library);

        assert_eq!(chips.len(), 2);
        assert_eq!(chips[0].target_id, "zeta", "authoring order, not sorted: {chips:?}");
        assert_eq!(chips[1].target_id, "alpha");
    }

    #[test]
    fn empty_trigger_target_is_skipped_not_a_none_chip() {
        let entry = entry_with(|e| {
            e.rules.conditions = vec![condition("", stat_core::TriggerCondition::Always)];
        });
        let library = SkillLibrary::default();

        let chips = causality_chips(&entry, &library);

        assert!(chips.is_empty(), "{chips:?}");
    }

    #[test]
    fn no_triggers_and_no_chain_produces_no_chips() {
        let entry = entry_with(|_| {});
        let library = SkillLibrary::default();

        assert!(causality_chips(&entry, &library).is_empty());
    }

    #[test]
    fn triggers_and_chain_coexist_as_separate_chips() {
        let entry = entry_with(|e| {
            e.rules.conditions = vec![condition("explosion", stat_core::TriggerCondition::OnImpact)];
            e.rules.damage.can_chain = true;
            e.rules.damage.chain_count = 2;
        });
        let library = SkillLibrary::default();

        let chips = causality_chips(&entry, &library);

        assert_eq!(chips.len(), 2);
        assert_eq!(chips[0].kind, ChipKind::Trigger);
        assert_eq!(chips[1].kind, ChipKind::Chain);
    }
}

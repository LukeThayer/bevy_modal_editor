//! Live, always-computed readouts for the Rules region (Task 6) — what the authored numbers
//! MEAN, at a glance: per-hit damage, how many strikes a cast can land, the full-chain range,
//! damage-per-mana. Pure functions over one skill's rules + timeline, unit-tested here.
//!
//! Ports `obelisk-arena @ f6472e4`'s `crates/arena_editor/src/derived.rs` (`max_strikes` /
//! `skill_readout`), ADAPTED to schema v2 per the Task 6 brief. v1 counted `WindowPhase::Chained`
//! windows reachable via an authored `EndReaction::Chain`/`Retarget` — both are gone in v2
//! (spec D3/D4/D5, `docs/superpowers/specs/2026-07-02-skill-editor-reimplementation-design.md`):
//! causality now lives entirely in rules triggers and `DamageConfig.can_chain`/`chain_count`. The
//! v2 strike count is therefore:
//!
//! - one strike per `CollisionWindow` that's both `spawn: Scheduled { .. }` (on the phase
//!   schedule, not `Template` — a template only ever fires via an emitter) AND `strikes: true`
//!   (a carrier volume with `strikes: false` can never produce a `HitConfirmed`);
//! - **+ `chain_count`** when `damage.can_chain` (spec D5 — the sim re-strikes the same skill at
//!   up to `chain_count` more targets; radius is behavior, count is rules);
//! - **+ 1 per `conditions[]` entry whose `trigger_skill` resolves to a skill with a real
//!   (non-flagged) timeline** (spec D4 — a triggered skill with a timeline executes spatially as
//!   its own free sub-cast, so it lands its own strike). This is intentionally shallow: it does
//!   NOT recurse into the triggered skill's own conditions/chains, so a triggered skill that
//!   itself chains or triggers further undercounts. That's the deliberate "one level deep"
//!   approximation the brief calls for — [`StrikeBreakdown::approximate`] is `true` whenever this
//!   contribution is nonzero, and the panel renders it with a "≈" marker.
//!
//! **Emitters are excluded from the count entirely, not approximated**: a `Template` window
//! rained by an `Emitter` (Task 11, spec §3.2) spawns for as long as its parent hitbox lives —
//! genuinely unbounded, not a fixed "N more strikes" the way a trigger or chain hop is.
//! [`StrikeBreakdown::has_unbounded_emitter`] flags this so the panel can append a "+ shards"
//! note instead of folding a fake number into the total.

#[cfg(test)]
use std::collections::BTreeMap;

use obelisk_bevy::assets::{CastTimeline, WindowSpawn};
use stat_core::Skill;

use super::library::{SkillEntry, SkillLibrary};

/// How a skill's max-strike count breaks down — kept as separate fields (rather than one
/// collapsed `u32`) so the panel can render "N scheduled + M chain + K triggered (≈)" instead of
/// just a total, and so tests can assert each contribution independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StrikeBreakdown {
    /// Scheduled, striking collision windows — the deterministic floor.
    pub scheduled: u32,
    /// `damage.chain_count` when `damage.can_chain` (spec D5), else 0. Deterministic — the sim
    /// always attempts exactly this many hops (it just may run out of valid targets).
    pub chain: u32,
    /// One per trigger condition whose target resolves to a skill with a real timeline (spec
    /// D4). Approximate: see the module docs — always render with [`StrikeBreakdown::approximate`].
    pub triggered: u32,
    /// True when any collision window on this timeline carries an `Emitter` — the emitted
    /// `Template` instances are unbounded and deliberately excluded from every count above.
    pub has_unbounded_emitter: bool,
}

impl StrikeBreakdown {
    /// The glanceable total: scheduled + chain + triggered, floored at 1 so a skill that lands
    /// no scheduled strikes (a pure buff, say) doesn't read as "deals damage zero times" — this
    /// is a conservative display heuristic, not a combat simulator (same caveat the v1 port
    /// carried).
    pub fn total(&self) -> u32 {
        (self.scheduled + self.chain + self.triggered).max(1)
    }

    /// True when the total includes a triggered-skill contribution — the one-level-deep
    /// approximation the module docs describe. The panel should render the total with a "≈".
    pub fn approximate(&self) -> bool {
        self.triggered > 0
    }
}

/// The computed summary the Rules region prints at the top: what the authored numbers mean.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillReadout {
    /// Sum of base damage lines (min, max) for ONE strike, uncharged.
    pub per_hit: (f64, f64),
    pub strikes: StrikeBreakdown,
    /// `per_hit × strikes.total()` — the full-chain range if every strike lands. Approximate
    /// whenever `strikes.approximate()` is.
    pub full_chain: (f64, f64),
    /// `full_chain / mana_cost`, or `None` for free skills (`mana_cost <= 0.0`).
    pub per_mana: Option<(f64, f64)>,
    pub crit_chance: f64,
}

/// Compute [`StrikeBreakdown`] for `skill`/`timeline`. `target_has_timeline` resolves a
/// `trigger_skill` id to "does that skill have a real (non-flagged) timeline in the library" —
/// passed as a closure rather than a `&SkillLibrary` so the math stays testable against hand
/// fixtures with no library/content-root machinery involved (see the tests below);
/// [`strike_breakdown_in_library`] is the library-backed convenience wrapper the panel calls.
pub fn strike_breakdown(
    skill: &Skill,
    timeline: &CastTimeline,
    target_has_timeline: impl Fn(&str) -> bool,
) -> StrikeBreakdown {
    let scheduled = timeline
        .collision_windows
        .iter()
        .filter(|w| w.strikes && matches!(w.spawn, WindowSpawn::Scheduled { .. }))
        .count() as u32;

    let chain = if skill.damage.can_chain {
        skill.damage.chain_count
    } else {
        0
    };

    let triggered = skill
        .conditions
        .iter()
        .filter(|c| target_has_timeline(&c.trigger_skill))
        .count() as u32;

    let has_unbounded_emitter = timeline
        .collision_windows
        .iter()
        .any(|w| w.emitter.is_some());

    StrikeBreakdown {
        scheduled,
        chain,
        triggered,
        has_unbounded_emitter,
    }
}

/// Compute the full [`SkillReadout`] from `skill` + `timeline`. See [`strike_breakdown`] for the
/// `target_has_timeline` closure's contract.
pub fn skill_readout(
    skill: &Skill,
    timeline: &CastTimeline,
    target_has_timeline: impl Fn(&str) -> bool,
) -> SkillReadout {
    let (min, max) = skill
        .damage
        .base_damages
        .iter()
        .fold((0.0, 0.0), |(lo, hi), d| (lo + d.min, hi + d.max));

    let strikes = strike_breakdown(skill, timeline, target_has_timeline);
    let n = strikes.total() as f64;
    let full = (min * n, max * n);
    let per_mana = (skill.mana_cost > 0.0).then(|| (full.0 / skill.mana_cost, full.1 / skill.mana_cost));

    SkillReadout {
        per_hit: (min, max),
        strikes,
        full_chain: full,
        per_mana,
        crit_chance: skill.damage.base_crit_chance,
    }
}

/// Whether `entry`'s timeline is real, spatial content — deliberately NOT
/// `!entry.timeline_flagged()`. `SkillEntry::timeline_flagged` answers "did the last disk scan
/// find a matching `.cast.ron` file"; a freshly template-created skill that hasn't been saved
/// yet (Task 8's territory — `insert_new_skill` sets a `timeline_path` nothing has been written
/// to) would read as "flagged" under that check even though its in-memory `timeline` is a fully
/// populated, playable template. What D4 and the triggered-strike contribution above actually
/// care about is whether the timeline describes spatial behavior at all, so this checks content
/// instead: `blank_timeline()` (the placeholder substituted in when a scan finds no matching
/// file) has empty `collision_windows`; every archetype template and every real authored
/// timeline has at least one.
pub fn entry_has_real_timeline(entry: &SkillEntry) -> bool {
    !entry.timeline.collision_windows.is_empty()
}

/// Library-backed convenience wrapper: resolves `id`'s entry and every trigger target against
/// `library`'s own skills (a trigger target counts as having "a real timeline" via
/// [`entry_has_real_timeline`]). Returns `None` if `id` isn't in the library.
pub fn skill_readout_in_library(id: &str, library: &SkillLibrary) -> Option<SkillReadout> {
    let entry = library.skills.get(id)?;
    Some(skill_readout(&entry.rules, &entry.timeline, |target_id| {
        library.skills.get(target_id).is_some_and(entry_has_real_timeline)
    }))
}

/// Build a lookup closure from a plain id->has_timeline map — a lighter-weight test fixture
/// helper than standing up a whole [`SkillLibrary`] when a test only cares about the trigger
/// contribution.
#[cfg(test)]
fn lookup<'a>(map: &'a BTreeMap<&str, bool>) -> impl Fn(&str) -> bool + 'a {
    move |id: &str| map.get(id).copied().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::templates::{projectile_template, zone_template};
    use obelisk_bevy::assets::{
        CollisionShape, CollisionWindow, Emitter, HitFilter, HitMode, VolumeMotion, WindowAnchor,
        WindowPhase,
    };
    use stat_core::{BaseDamage, DamageType, SkillCondition, TriggerCondition};

    /// fireball pair (brief, Step 1): a projectile bolt (1 scheduled+striking window) with an
    /// `on_impact` condition triggering "fireball_explosion", which has its own real timeline in
    /// the library ⇒ 1 (bolt) + 1 (triggered explosion) = 2 strikes, flagged approximate.
    #[test]
    fn fireball_pair_counts_bolt_plus_triggered_explosion() {
        let (mut rules, timeline) = projectile_template("fireball");
        rules.damage.base_damages = vec![BaseDamage::new(DamageType::Fire, 20.0, 20.0)];
        rules.conditions.push(SkillCondition {
            trigger_skill: "fireball_explosion".to_string(),
            additional: true,
            condition: TriggerCondition::OnImpact,
        });

        let known: BTreeMap<&str, bool> = [("fireball_explosion", true)].into_iter().collect();
        let strikes = strike_breakdown(&rules, &timeline, lookup(&known));

        assert_eq!(strikes.scheduled, 1, "the bolt is one scheduled, striking window");
        assert_eq!(strikes.chain, 0);
        assert_eq!(strikes.triggered, 1, "on_impact targets a skill with a real timeline");
        assert_eq!(strikes.total(), 2);
        assert!(strikes.approximate(), "triggered contribution must be flagged approximate");
        assert!(!strikes.has_unbounded_emitter);

        let readout = skill_readout(&rules, &timeline, lookup(&known));
        assert_eq!(readout.per_hit, (20.0, 20.0));
        assert_eq!(readout.full_chain, (40.0, 40.0));
    }

    /// A trigger condition whose target has NO real timeline (rules-only / dangling) does not
    /// contribute a strike — D4: only timeline targets execute spatially as a sub-cast.
    #[test]
    fn trigger_without_a_timeline_target_does_not_count() {
        let (mut rules, timeline) = projectile_template("bolt");
        rules.conditions.push(SkillCondition {
            trigger_skill: "packet_only_proc".to_string(),
            additional: false,
            condition: TriggerCondition::OnCrit,
        });

        let known: BTreeMap<&str, bool> = [("packet_only_proc", false)].into_iter().collect();
        let strikes = strike_breakdown(&rules, &timeline, lookup(&known));

        assert_eq!(strikes.scheduled, 1);
        assert_eq!(strikes.triggered, 0);
        assert_eq!(strikes.total(), 1);
        assert!(!strikes.approximate());
    }

    /// chain_bolt-style (brief, Step 1): a scheduled beam-ish window (1) plus `can_chain = true`,
    /// `chain_count = 3` ⇒ 1 + 3 = 4 strikes, deterministic (not approximate — chain hops are an
    /// authored, guaranteed-attempt count per spec D5, unlike the shallow trigger contribution).
    #[test]
    fn chain_skill_counts_scheduled_plus_chain_count() {
        let (mut rules, timeline) = projectile_template("chain_bolt");
        rules.damage.can_chain = true;
        rules.damage.chain_count = 3;

        let strikes = strike_breakdown(&rules, &timeline, |_| false);

        assert_eq!(strikes.scheduled, 1);
        assert_eq!(strikes.chain, 3);
        assert_eq!(strikes.triggered, 0);
        assert_eq!(strikes.total(), 4);
        assert!(!strikes.approximate(), "chain hops are deterministic, not the shallow approx");
    }

    /// zone-with-emitter: the zone's own scheduled window counts once; the `Template` window it
    /// emits is NEVER counted (spawn: Template is excluded by construction) and instead flags
    /// `has_unbounded_emitter` so the panel can render "+ shards" rather than a fake number.
    #[test]
    fn zone_with_emitter_excludes_shards_from_the_count() {
        let (rules, mut timeline) = zone_template("blizzard");
        timeline.collision_windows[0].emitter = Some(Emitter {
            rate: 4.0,
            jitter: 1.5,
            window: "shard".to_string(),
        });
        timeline.collision_windows.push(CollisionWindow {
            id: "shard".to_string(),
            spawn: WindowSpawn::Template,
            anchor: WindowAnchor::Caster,
            anchor_offset: Default::default(),
            strikes: true,
            active_duration: 1.0,
            shape: CollisionShape::Sphere { radius: 0.5 },
            motion: VolumeMotion::Static,
            motion_direction: Default::default(),
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::OncePerTarget,
            rehit_interval: None,
            emitter: None,
            paints: None,
        });

        let strikes = strike_breakdown(&rules, &timeline, |_| false);

        assert_eq!(strikes.scheduled, 1, "only the zone's own scheduled window counts");
        assert_eq!(strikes.total(), 1);
        assert!(strikes.has_unbounded_emitter, "the emitter must be flagged, not folded into the count");
    }

    /// A `strikes: false` carrier window (e.g. a cosmetic-only volume) never produces a
    /// `HitConfirmed` and must not inflate the scheduled count.
    #[test]
    fn non_striking_scheduled_window_is_excluded() {
        let (rules, mut timeline) = projectile_template("carrier");
        timeline.collision_windows[0].strikes = false;

        let strikes = strike_breakdown(&rules, &timeline, |_| false);

        assert_eq!(strikes.scheduled, 0);
        assert_eq!(strikes.total(), 1, "total is still floored at 1 for display");
    }

    #[test]
    fn per_mana_is_none_for_free_skills() {
        let (mut rules, timeline) = projectile_template("free_bolt");
        rules.mana_cost = 0.0;
        rules.damage.base_damages = vec![BaseDamage::new(DamageType::Physical, 5.0, 10.0)];

        let readout = skill_readout(&rules, &timeline, |_| false);
        assert!(readout.per_mana.is_none());
    }

    #[test]
    fn per_mana_divides_the_full_chain_range() {
        let (mut rules, timeline) = projectile_template("costed_bolt");
        rules.mana_cost = 10.0;
        rules.damage.base_damages = vec![BaseDamage::new(DamageType::Physical, 20.0, 40.0)];

        let readout = skill_readout(&rules, &timeline, |_| false);
        assert_eq!(readout.full_chain, (20.0, 40.0));
        let (lo, hi) = readout.per_mana.expect("costs mana");
        assert!((lo - 2.0).abs() < 1e-9 && (hi - 4.0).abs() < 1e-9);
    }

    /// The WindowPhase import is exercised only to keep parity with the module's doc comment
    /// (Scheduled windows name a phase); guards against an unused-import warning if the
    /// scheduled-window construction above ever stops referencing it directly.
    #[test]
    fn scheduled_windows_name_a_phase() {
        let (_, timeline) = projectile_template("phase_check");
        match timeline.collision_windows[0].spawn {
            WindowSpawn::Scheduled { phase, .. } => assert_eq!(phase, WindowPhase::Active),
            WindowSpawn::Template => panic!("projectile_template's window must be Scheduled"),
        }
    }
}

//! `ValidationReport` — the real sweep (Task 8), replacing Task 6's always-empty stub.
//!
//! `validate_skill` runs every rule from spec §3.3 against one [`SkillEntry`] (the currently
//! open/edited copy — NOT necessarily what's on disk yet) and its cross-reference libraries
//! (`SkillLibrary` for trigger targets, `EffectLibrary`/`VfxLibrary` for cue-binding presets,
//! `AnimationLibrary` for cue-binding anim clips). Every problem is tagged `blocking: bool`:
//! blocking problems gate the Save button (`crate::skill::save::save_skill` doesn't itself see
//! a `ValidationReport` — see that module's doc comment for why the gate lives in the panel);
//! warnings are surfaced but never block.
//!
//! Where a rule's logic already lives in obelisk-bevy (the runtime's own defensive checks),
//! this module REUSES it rather than re-deriving the same semantics by hand:
//! - `obelisk_bevy::assets::validate_timeline` — structural timeline checks (emitter targets,
//!   Template reachability, CastPoint-anchor-vs-acquisition reachability). Its `Err` message is
//!   surfaced verbatim (blocking), tagged to the offending window when the message names one
//!   (same `"'{id}'"` substring match `panel::behavior::draw_windows` already uses for its own
//!   live inline error).
//! - `obelisk_bevy::combat::system::{is_invalid_lifecycle_target, is_invalid_timeline_target,
//!   is_unsupported_timeline_condition}` — the exact runtime predicates `on_hit_confirmed` uses
//!   to decide whether a trigger condition is a content bug, evaluated here against a
//!   `CastTimelineHandles` fabricated from `SkillLibrary` (every skill whose CURRENT entry has a
//!   non-empty `collision_windows` — same "real timeline" notion `readouts::entry_has_real_timeline`
//!   already uses, not `SkillEntry::timeline_flagged`'s disk-existence notion — validation should
//!   reflect in-memory edits, not just what's saved). The fabricated handles only need to
//!   satisfy `HashMap::contains_key`, so their `Handle<CastTimeline>` values are inert
//!   `Handle::default()`s — nothing here ever resolves them through an `AssetServer`.
//!
//! Two rules are NOT obelisk-bevy predicates because obelisk-bevy has no load-time concept of
//! them at all (both are pure content-authoring checks, spec §3.3):
//! - a `trigger_skill` naming no skill anywhere (dangling reference — blocking);
//! - a hit-phase (non-`Lifecycle`) condition whose target exists but has no timeline (legal —
//!   it resolves as an inline packet — but flagged so the author notices it won't get a spatial
//!   resolution; warning only, per spec D4).
//!
//! Acquisition-fallback dead ends (a `WindowAnchor::CastPoint` window whose acquisition chain can
//! never produce a point) are NOT a separate rule here — `validate_timeline` already rejects that
//! case structurally (see its doc comment), so it's covered by the "surfaced verbatim" rule below
//! rather than re-checked by hand.
use std::collections::HashMap;

use bevy::asset::Handle;
use bevy::math::Vec3;

use obelisk_bevy::assets::{
    AcqFallback, Acquisition, CastTimeline, CastTimelineHandles, CueAttach, PaintMode, WindowAnchor,
    WindowSpawn,
};
use obelisk_bevy::combat::system::{
    is_invalid_lifecycle_target, is_invalid_timeline_target, is_unsupported_timeline_condition,
};
use obelisk_bevy::surfaces::SurfaceRegistry;
use stat_core::{ConditionPhase, Skill};

use bevy_editor_game::AnimationLibrary;
use bevy_vfx::VfxLibrary;

use crate::effects::EffectLibrary;

use super::library::{SkillEntry, SkillLibrary};
use super::readouts::entry_has_real_timeline;

/// Hard cap on trigger-graph walk depth (spec §3.3) — mirrors
/// `obelisk_bevy::combat::system::MAX_TRIGGER_RESOLUTIONS`'s "8" (that constant is private to
/// obelisk-bevy, so this is its own copy, not a shared one — same NUMBER, different concern: that
/// one bounds a runtime worklist, this one bounds an author-time graph walk). A reachable skill
/// at depth 9+ (i.e. more than 8 hops from the skill being validated) is blocking.
const MAX_TRIGGER_DEPTH: u32 = 8;

/// One validation problem. `target` names what the problem is ABOUT, so a panel region can
/// filter down to just the rows it renders:
/// - `"condition:{index}"` — a `SkillCondition` slot (Rules region trigger cards).
/// - `"window:{window_id}"` — a `CollisionWindow` (Behavior region window cards).
/// - `"cue:{slot_key}"` — a `CueBinding` slot (Presentation region, Task 9 — unconsumed today,
///   same "widen as regions grow their own validation" story `for_window` was added under).
/// - `"acquisition"` — a `GroundPoint` `on_surface` surface-gate problem (Behavior region
///   Acquisition card; surfaced in the header blocker list today, same "unconsumed until the
///   region grows an inline lookup" story as `cue:`).
/// - `"skill"` — not about any one card; shown only in the panel header's blocker list.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub target: String,
    pub message: String,
    pub blocking: bool,
}

/// One skill's validation problems.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValidationReport {
    pub problems: Vec<Problem>,
}

impl ValidationReport {
    /// Problems whose `target` is `"condition:{index}"` for the given trigger-card index.
    pub fn for_condition(&self, index: usize) -> impl Iterator<Item = &Problem> {
        let target = format!("condition:{index}");
        self.problems.iter().filter(move |p| p.target == target)
    }

    /// Problems whose `target` is `"window:{window_id}"`.
    pub fn for_window<'a>(&'a self, window_id: &str) -> impl Iterator<Item = &'a Problem> {
        let target = format!("window:{window_id}");
        self.problems.iter().filter(move |p| p.target == target)
    }

    /// Problems whose `target` is `"cue:{slot_key}"` — Task 9's Presentation region lookup.
    pub fn for_cue<'a>(&'a self, slot_key: &str) -> impl Iterator<Item = &'a Problem> {
        let target = format!("cue:{slot_key}");
        self.problems.iter().filter(move |p| p.target == target)
    }

    /// `true` when any problem is blocking — gates the Save button (see `crate::skill::save`).
    pub fn has_blocking(&self) -> bool {
        self.problems.iter().any(|p| p.blocking)
    }

    /// Every blocking problem's message — the Save button's disabled-tooltip content.
    pub fn blocking_messages(&self) -> impl Iterator<Item = &str> {
        self.problems.iter().filter(|p| p.blocking).map(|p| p.message.as_str())
    }
}

/// Run every validation rule (spec §3.3) against `entry` — see the module doc comment for the
/// full rule list and which ones reuse obelisk-bevy's own runtime predicates.
pub fn validate_skill(
    entry: &SkillEntry,
    library: &SkillLibrary,
    effects: &EffectLibrary,
    vfx: &VfxLibrary,
    surfaces: &SurfaceRegistry,
    anim: Option<&AnimationLibrary>,
) -> ValidationReport {
    let mut problems = Vec::new();
    let self_id = entry.rules.id.as_str();

    // Fabricated `CastTimelineHandles`: every skill (this entry's own live-edited copy takes
    // priority over the library's possibly-stale one) whose CURRENT timeline is non-blank. Only
    // `contains_key` is ever called against it, so a `Handle::default()` placeholder is enough —
    // see the module doc comment.
    let mut handles = CastTimelineHandles(HashMap::new());
    for (id, other) in &library.skills {
        if id != self_id && entry_has_real_timeline(other) {
            handles.0.insert(id.clone(), Handle::<CastTimeline>::default());
        }
    }
    if !entry.timeline.collision_windows.is_empty() {
        handles.0.insert(self_id.to_string(), Handle::<CastTimeline>::default());
    }

    // --- Trigger conditions (dangling / Lifecycle / hit-phase / additional / EveryNthHit) ---
    for (i, cond) in entry.rules.conditions.iter().enumerate() {
        let target = format!("condition:{i}");
        let trigger_skill = cond.trigger_skill.as_str();
        let target_known = trigger_skill == self_id || library.skills.contains_key(trigger_skill);

        if !target_known {
            let message = if trigger_skill.is_empty() {
                "trigger has no target skill selected".to_string()
            } else {
                format!("trigger_skill '{trigger_skill}' does not exist")
            };
            problems.push(Problem { target, message, blocking: true });
            continue;
        }

        if is_invalid_lifecycle_target(cond, &handles) {
            problems.push(Problem {
                target: target.clone(),
                message: format!(
                    "'{trigger_skill}' is triggered on {:?}, but has no timeline — \
                     OnImpact/OnExpire triggers require the target to have a real timeline",
                    cond.condition.phase()
                ),
                blocking: true,
            });
        } else if cond.condition.phase() != ConditionPhase::Lifecycle && !handles.0.contains_key(trigger_skill) {
            problems.push(Problem {
                target: target.clone(),
                message: "has no timeline — this trigger resolves as an inline packet (legal), \
                          not a spatial cast"
                    .to_string(),
                blocking: false,
            });
        }

        if is_invalid_timeline_target(cond, &handles) {
            problems.push(Problem {
                target: target.clone(),
                message: "has a timeline — timeline-target triggers must be additional = true".to_string(),
                blocking: true,
            });
        }

        if is_unsupported_timeline_condition(cond, &handles) {
            problems.push(Problem {
                target,
                message: "EveryNthHit is not supported on timeline-target triggers (its counter \
                          lives inside stat_core's calc path, which a timeline-target trigger \
                          never reaches)"
                    .to_string(),
                blocking: true,
            });
        }
    }

    // --- Cue bindings: unknown Effect preset / unknown anim clip ---
    for (slot, binding) in &entry.timeline.cues {
        let target = format!("cue:{slot}");
        if let Some(effect_name) = &binding.effect {
            let known = effects.effects.contains_key(effect_name) || vfx.effects.contains_key(effect_name);
            if !known {
                problems.push(Problem {
                    target: target.clone(),
                    message: format!("cue '{slot}' references unknown Effect preset '{effect_name}'"),
                    blocking: true,
                });
            }
        }
        if let Some(anim_name) = &binding.anim
            && let Some(anim_lib) = anim
            && !anim_lib.clips.contains_key(anim_name)
        {
            problems.push(Problem {
                target,
                message: format!("cue '{slot}' references unknown animation clip '{anim_name}'"),
                blocking: true,
            });
        }
    }

    // --- Charge tiers (charge_cues): mirror obelisk validate_timeline (blocking — a violating
    // file fails the game asset load) + editor lookups (unknown effect/anim), targeted
    // "cue:charge" so the Charge region renders them inline.
    {
        let target = "cue:charge".to_string();
        let mut prev_threshold: Option<f32> = None;
        for (i, tier) in entry.timeline.charge_cues.iter().enumerate() {
            if !(0.0..=1.0).contains(&tier.threshold) {
                problems.push(Problem {
                    target: target.clone(),
                    message: format!("charge tier {i} threshold must be within 0-100%"),
                    blocking: true,
                });
            }
            if let Some(prev) = prev_threshold
                && tier.threshold <= prev
            {
                problems.push(Problem {
                    target: target.clone(),
                    message: format!("charge tier {i} threshold must be greater than tier {}'s", i - 1),
                    blocking: true,
                });
            }
            prev_threshold = Some(tier.threshold);
            match &tier.cue.attach {
                CueAttach::Follow => problems.push(Problem {
                    target: target.clone(),
                    message: format!("charge tier {i} may not use Follow attach"),
                    blocking: true,
                }),
                CueAttach::Bone { socket, .. } if socket.is_empty() => problems.push(Problem {
                    target: target.clone(),
                    message: format!("charge tier {i} Bone attach needs a socket name"),
                    blocking: true,
                }),
                _ => {}
            }
            if let Some(effect_name) = &tier.cue.effect {
                let known = effects.effects.contains_key(effect_name)
                    || vfx.effects.contains_key(effect_name);
                if !known {
                    problems.push(Problem {
                        target: target.clone(),
                        message: format!("charge tier {i} references unknown Effect preset '{effect_name}'"),
                        blocking: true,
                    });
                }
            }
            if let Some(anim_name) = &tier.cue.anim
                && let Some(anim_lib) = anim
                && !anim_lib.clips.contains_key(anim_name)
            {
                problems.push(Problem {
                    target: target.clone(),
                    message: format!("charge tier {i} references unknown animation clip '{anim_name}'"),
                    blocking: true,
                });
            }
        }
        if !entry.timeline.charge_cues.is_empty() && !entry.timeline.chargeable {
            problems.push(Problem {
                target,
                message: "charge tiers are authored but Chargeable is off - they never play".to_string(),
                blocking: false,
            });
        }
    }

    // --- Surface paints (window `paints`) + acquisition `on_surface` ---
    // Registry-membership lookups (editor-only BLOCKING gate: obelisk warns-and-skips an unknown
    // surface at runtime rather than rejecting it, same author-time/runtime split as cue Effect
    // presets) PLUS a mirror of obelisk `validate_timeline`'s numeric paint rules (blocking — a
    // violating file fails the game asset load, same rationale as the charge-tier mirror above).
    // The paint numerics do overlap the `validate_timeline` verbatim surface below, but that stops
    // at its FIRST error; this loop tags EVERY offending window `window:{id}` so each renders
    // inline on its own card.
    for w in &entry.timeline.collision_windows {
        let Some(paints) = &w.paints else { continue };
        let target = format!("window:{}", w.id);
        if paints.surface.is_empty() {
            problems.push(Problem {
                target: target.clone(),
                message: format!("window '{}' paints an empty surface id", w.id),
                blocking: true,
            });
        } else if !surfaces.0.contains_key(&paints.surface) {
            problems.push(Problem {
                target: target.clone(),
                message: format!(
                    "window '{}' paints unknown surface '{}' (not a loaded surface type)",
                    w.id, paints.surface
                ),
                blocking: true,
            });
        }
        if paints.radius <= 0.0 {
            problems.push(Problem {
                target: target.clone(),
                message: format!("window '{}' paints radius must be > 0", w.id),
                blocking: true,
            });
        }
        if let PaintMode::Trail { step } = paints.mode
            && step <= 0.0
        {
            problems.push(Problem {
                target: target.clone(),
                message: format!("window '{}' paints Trail step must be > 0", w.id),
                blocking: true,
            });
        }
        if let Some(lt) = paints.lifetime
            && lt <= 0.0
        {
            problems.push(Problem {
                target,
                message: format!("window '{}' paints lifetime override must be > 0", w.id),
                blocking: true,
            });
        }
    }
    check_on_surface(&entry.timeline.acquisition, surfaces, &mut problems);

    // --- Template windows authoring non-default anchor/anchor_offset (follow-ups ticket 3) ---
    for w in &entry.timeline.collision_windows {
        if w.spawn == WindowSpawn::Template && (w.anchor != WindowAnchor::default() || w.anchor_offset != Vec3::ZERO) {
            problems.push(Problem {
                target: format!("window:{}", w.id),
                message: format!(
                    "window '{}' is a Template — its anchor/anchor_offset are never read \
                     (emitted instances spawn at the emitting hitbox's position); author them \
                     on the Scheduled window that emits it instead",
                    w.id
                ),
                blocking: false,
            });
        }
    }

    // --- Trigger-cycle depth walk ---
    let mut skills_by_id: HashMap<&str, &Skill> =
        library.skills.iter().map(|(id, e)| (id.as_str(), &e.rules)).collect();
    skills_by_id.insert(self_id, &entry.rules);
    if max_trigger_depth(self_id, &skills_by_id, 0) > MAX_TRIGGER_DEPTH {
        problems.push(Problem {
            target: "skill".to_string(),
            message: format!(
                "trigger graph exceeds depth {MAX_TRIGGER_DEPTH} from this skill (possible cycle)"
            ),
            blocking: true,
        });
    }

    // --- Structural `validate_timeline` errors, surfaced verbatim ---
    if let Err(message) = obelisk_bevy::assets::validate_timeline(&entry.timeline) {
        let target = entry
            .timeline
            .collision_windows
            .iter()
            .find(|w| message.contains(&format!("'{}'", w.id)))
            .map(|w| format!("window:{}", w.id))
            .unwrap_or_else(|| "skill".to_string());
        problems.push(Problem { target, message, blocking: true });
    }

    ValidationReport { problems }
}

/// Walk `acq` and its `AcqFallback::Then` chain for `GroundPoint` `on_surface` gates whose
/// `surface` names no loaded surface type, pushing a blocking `"acquisition"`-targeted problem for
/// each. Editor-only (obelisk resolves `on_surface` against the live `SurfaceRegistry` at cast
/// time and paid-fizzles a miss — an unknown id simply never matches; the editor is the
/// author-time gate). The walk mirrors `panel::behavior::draw_fallback`'s authoring reach: both
/// the top-level acquisition and each nested fallback `GroundPoint` are editable, so both are
/// checked.
fn check_on_surface(acq: &Acquisition, surfaces: &SurfaceRegistry, problems: &mut Vec<Problem>) {
    let (on_surface, fallback) = match acq {
        Acquisition::GroundPoint { on_surface, fallback, .. } => (on_surface.as_ref(), Some(fallback)),
        Acquisition::HitscanEntity { fallback, .. } => (None, Some(fallback)),
        Acquisition::Aim | Acquisition::SelfPoint => (None, None),
    };
    if let Some(req) = on_surface
        && !surfaces.0.contains_key(&req.surface)
    {
        problems.push(Problem {
            target: "acquisition".to_string(),
            message: format!(
                "acquisition requires surface '{}' (on_surface), which is not a loaded surface type",
                req.surface
            ),
            blocking: true,
        });
    }
    if let Some(AcqFallback::Then(inner)) = fallback {
        check_on_surface(inner, surfaces, problems);
    }
}

/// The deepest hop count reachable from `id` by walking `trigger_skill` edges, capped at
/// `MAX_TRIGGER_DEPTH + 1` (recursion always terminates — a true cycle just walks straight into
/// the cap rather than looping forever, which is exactly the "blocking" outcome we want). `depth`
/// is the caller's own hop count (root call: `0`).
fn max_trigger_depth(id: &str, skills: &HashMap<&str, &Skill>, depth: u32) -> u32 {
    if depth > MAX_TRIGGER_DEPTH {
        return depth;
    }
    let Some(skill) = skills.get(id) else {
        return depth;
    };
    skill
        .conditions
        .iter()
        .map(|c| max_trigger_depth(&c.trigger_skill, skills, depth + 1))
        .max()
        .unwrap_or(depth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use obelisk_bevy::assets::{
        AcqFallback, Acquisition, CollisionShape, CollisionWindow, CueAttach, CueBinding, CueParam,
        Emitter, HitFilter, HitMode, ParamSource, PaintMode, PaintSpec, PhaseDurations,
        SurfaceRequirement, VolumeMotion, WindowPhase,
    };
    use obelisk_bevy::surfaces::{SurfaceRegistry, SurfaceType};
    use stat_core::{SkillCondition, TriggerCondition};

    use crate::effects::EffectMarker;
    use crate::skill::templates::strike_template;

    fn blank_entry(id: &str) -> SkillEntry {
        let (rules, _) = strike_template(id);
        SkillEntry {
            rules,
            timeline: blank_timeline(id),
            rules_path: PathBuf::new(),
            timeline_path: PathBuf::new(),
            dirty_rules: false,
            dirty_timeline: false,
            disk_hash: (0, 0),
        }
    }

    fn blank_timeline(id: &str) -> CastTimeline {
        CastTimeline {
            skill_id: id.to_string(),
            phase_durations: PhaseDurations { windup: 0.0, active: 0.0, recovery: 0.0 },
            collision_windows: Vec::new(),
            acquisition: Acquisition::default(),
            vfx_cues: Default::default(),
            chain_radius: 6.0,
            chargeable: false,
            max_hold: 1.0,
            cues: Default::default(),
            charge_cues: Vec::new(),
        }
    }

    fn real_timeline(id: &str) -> CastTimeline {
        let mut tl = blank_timeline(id);
        tl.collision_windows.push(CollisionWindow {
            id: "bolt".to_string(),
            spawn: WindowSpawn::Scheduled { phase: WindowPhase::Active, offset: 0.0 },
            anchor: WindowAnchor::Caster,
            anchor_offset: Vec3::ZERO,
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
        tl
    }

    fn insert(library: &mut SkillLibrary, id: &str, timeline: CastTimeline, conditions: Vec<SkillCondition>) {
        let mut entry = blank_entry(id);
        entry.timeline = timeline;
        entry.rules.conditions = conditions;
        library.skills.insert(id.to_string(), entry);
    }

    fn empty_libs() -> (EffectLibrary, VfxLibrary) {
        (EffectLibrary::default(), VfxLibrary::default())
    }

    fn surface_type(id: &str) -> SurfaceType {
        SurfaceType {
            id: id.to_string(),
            lifetime: 180.0,
            merge_radius: 0.25,
            max_patches: 64,
            patch_radius: 0.45,
            standing: None,
            on_skill_contact: Vec::new(),
            visuals: None,
        }
    }

    /// A `SurfaceRegistry` holding exactly `ids` (empty slice = the "no surfaces loaded" case).
    fn registry_with(ids: &[&str]) -> SurfaceRegistry {
        let mut reg = SurfaceRegistry::default();
        for id in ids {
            reg.0.insert((*id).to_string(), surface_type(id));
        }
        reg
    }

    fn painting_window(id: &str, paints: PaintSpec) -> CollisionWindow {
        CollisionWindow {
            id: id.to_string(),
            spawn: WindowSpawn::Scheduled { phase: WindowPhase::Active, offset: 0.0 },
            anchor: WindowAnchor::Caster,
            anchor_offset: Vec3::ZERO,
            strikes: true,
            active_duration: 1.0,
            shape: CollisionShape::Sphere { radius: 0.5 },
            motion: VolumeMotion::Static,
            motion_direction: Default::default(),
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::OncePerTarget,
            rehit_interval: None,
            emitter: None,
            paints: Some(paints),
        }
    }

    // --- dangling trigger_skill ---

    #[test]
    fn dangling_trigger_skill_is_blocking() {
        let mut library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        entry.rules.conditions = vec![SkillCondition {
            trigger_skill: "nowhere".to_string(),
            additional: true,
            condition: TriggerCondition::Always,
        }];
        library.skills.insert("a".to_string(), entry.clone());

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        let probs: Vec<_> = report.for_condition(0).collect();
        assert_eq!(probs.len(), 1);
        assert!(probs[0].blocking);
        assert!(probs[0].message.contains("does not exist"));
    }

    #[test]
    fn known_trigger_skill_is_not_dangling() {
        let mut library = SkillLibrary::default();
        insert(&mut library, "b", blank_timeline("b"), vec![]);
        let mut entry = blank_entry("a");
        entry.rules.conditions = vec![SkillCondition {
            trigger_skill: "b".to_string(),
            additional: true,
            condition: TriggerCondition::OnImpact,
        }];
        library.skills.insert("a".to_string(), entry.clone());

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        assert!(
            report.for_condition(0).all(|p| !p.message.contains("does not exist")),
            "{:?}",
            report.problems
        );
    }

    // --- Lifecycle-target missing timeline (blocking) ---

    #[test]
    fn lifecycle_target_missing_timeline_is_blocking() {
        let mut library = SkillLibrary::default();
        insert(&mut library, "b", blank_timeline("b"), vec![]); // no real timeline
        let mut entry = blank_entry("a");
        entry.rules.conditions = vec![SkillCondition {
            trigger_skill: "b".to_string(),
            additional: true,
            condition: TriggerCondition::OnImpact,
        }];
        library.skills.insert("a".to_string(), entry.clone());

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        let probs: Vec<_> = report.for_condition(0).collect();
        assert_eq!(probs.len(), 1);
        assert!(probs[0].blocking);
    }

    #[test]
    fn lifecycle_target_with_timeline_is_clean() {
        let mut library = SkillLibrary::default();
        insert(&mut library, "b", real_timeline("b"), vec![]);
        let mut entry = blank_entry("a");
        entry.rules.conditions = vec![SkillCondition {
            trigger_skill: "b".to_string(),
            additional: true,
            condition: TriggerCondition::OnImpact,
        }];
        library.skills.insert("a".to_string(), entry.clone());

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        assert!(report.for_condition(0).next().is_none(), "{:?}", report.problems);
    }

    // --- hit-phase target missing timeline (warning) ---

    #[test]
    fn hit_phase_target_missing_timeline_is_warning_not_blocking() {
        let mut library = SkillLibrary::default();
        insert(&mut library, "b", blank_timeline("b"), vec![]);
        let mut entry = blank_entry("a");
        entry.rules.conditions = vec![SkillCondition {
            trigger_skill: "b".to_string(),
            additional: true,
            condition: TriggerCondition::OnCrit,
        }];
        library.skills.insert("a".to_string(), entry.clone());

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        let probs: Vec<_> = report.for_condition(0).collect();
        assert_eq!(probs.len(), 1);
        assert!(!probs[0].blocking);
    }

    #[test]
    fn hit_phase_target_with_timeline_and_additional_true_is_clean() {
        let mut library = SkillLibrary::default();
        insert(&mut library, "b", real_timeline("b"), vec![]);
        let mut entry = blank_entry("a");
        entry.rules.conditions = vec![SkillCondition {
            trigger_skill: "b".to_string(),
            additional: true,
            condition: TriggerCondition::OnCrit,
        }];
        library.skills.insert("a".to_string(), entry.clone());

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        assert!(report.for_condition(0).next().is_none(), "{:?}", report.problems);
    }

    // --- timeline-target additional == false (blocking) ---

    #[test]
    fn timeline_target_additional_false_is_blocking() {
        let mut library = SkillLibrary::default();
        insert(&mut library, "b", real_timeline("b"), vec![]);
        let mut entry = blank_entry("a");
        entry.rules.conditions = vec![SkillCondition {
            trigger_skill: "b".to_string(),
            additional: false,
            condition: TriggerCondition::OnCrit,
        }];
        library.skills.insert("a".to_string(), entry.clone());

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        let probs: Vec<_> = report.for_condition(0).collect();
        assert!(probs.iter().any(|p| p.blocking && p.message.contains("additional = true")));
    }

    #[test]
    fn timeline_target_additional_true_is_not_flagged() {
        let mut library = SkillLibrary::default();
        insert(&mut library, "b", real_timeline("b"), vec![]);
        let mut entry = blank_entry("a");
        entry.rules.conditions = vec![SkillCondition {
            trigger_skill: "b".to_string(),
            additional: true,
            condition: TriggerCondition::OnCrit,
        }];
        library.skills.insert("a".to_string(), entry.clone());

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        assert!(report.for_condition(0).all(|p| !p.message.contains("additional = true")));
    }

    // --- EveryNthHit on timeline target (blocking) ---

    #[test]
    fn every_nth_hit_on_timeline_target_is_blocking() {
        let mut library = SkillLibrary::default();
        insert(&mut library, "b", real_timeline("b"), vec![]);
        let mut entry = blank_entry("a");
        entry.rules.conditions = vec![SkillCondition {
            trigger_skill: "b".to_string(),
            additional: true,
            condition: TriggerCondition::EveryNthHit { n: 3 },
        }];
        library.skills.insert("a".to_string(), entry.clone());

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        let probs: Vec<_> = report.for_condition(0).collect();
        assert!(probs.iter().any(|p| p.blocking && p.message.contains("EveryNthHit")));
    }

    #[test]
    fn every_nth_hit_on_packet_target_is_not_flagged() {
        let mut library = SkillLibrary::default();
        insert(&mut library, "b", blank_timeline("b"), vec![]);
        let mut entry = blank_entry("a");
        entry.rules.conditions = vec![SkillCondition {
            trigger_skill: "b".to_string(),
            additional: true,
            condition: TriggerCondition::EveryNthHit { n: 3 },
        }];
        library.skills.insert("a".to_string(), entry.clone());

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        assert!(report.for_condition(0).all(|p| !p.message.contains("EveryNthHit")));
    }

    // --- cue bindings: unknown Effect preset ---

    #[test]
    fn cue_unknown_effect_preset_is_blocking() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        let mut cues = HashMap::new();
        cues.insert(
            "on_cast".to_string(),
            CueBinding { effect: Some("Nonexistent".to_string()), attach: CueAttach::World, anim: None, params: vec![], duration: None },
        );
        entry.timeline.cues = cues;

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        let probs: Vec<_> = report.for_cue("on_cast").collect();
        assert_eq!(probs.len(), 1);
        assert!(probs[0].blocking);
    }

    #[test]
    fn cue_known_effect_preset_in_effect_library_is_clean() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        let mut cues = HashMap::new();
        cues.insert(
            "on_cast".to_string(),
            CueBinding { effect: Some("Muzzle".to_string()), attach: CueAttach::World, anim: None, params: vec![], duration: None },
        );
        entry.timeline.cues = cues;

        let (mut effects, vfx) = empty_libs();
        effects.effects.insert("Muzzle".to_string(), EffectMarker::default());
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        assert!(report.for_cue("on_cast").next().is_none(), "{:?}", report.problems);
    }

    #[test]
    fn cue_known_effect_preset_in_vfx_library_is_clean() {
        use bevy_vfx::VfxSystem;
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        let mut cues = HashMap::new();
        cues.insert(
            "on_cast".to_string(),
            CueBinding { effect: Some("Spark".to_string()), attach: CueAttach::World, anim: None, params: vec![], duration: None },
        );
        entry.timeline.cues = cues;

        let (effects, mut vfx) = empty_libs();
        vfx.effects.insert("Spark".to_string(), VfxSystem::default());
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        assert!(report.for_cue("on_cast").next().is_none(), "{:?}", report.problems);
    }

    // --- cue bindings: unknown anim clip ---

    #[test]
    fn cue_unknown_anim_clip_is_blocking_when_anim_library_present() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        let mut cues = HashMap::new();
        cues.insert(
            "on_cast".to_string(),
            CueBinding { effect: None, attach: CueAttach::World, anim: Some("missing::clip".to_string()), params: vec![], duration: None },
        );
        entry.timeline.cues = cues;

        let (effects, vfx) = empty_libs();
        let anim = AnimationLibrary::default();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), Some(&anim));
        let probs: Vec<_> = report.for_cue("on_cast").collect();
        assert_eq!(probs.len(), 1);
        assert!(probs[0].blocking);
    }

    #[test]
    fn cue_anim_clip_unchecked_when_no_anim_library() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        let mut cues = HashMap::new();
        cues.insert(
            "on_cast".to_string(),
            CueBinding { effect: None, attach: CueAttach::World, anim: Some("missing::clip".to_string()), params: vec![], duration: None },
        );
        entry.timeline.cues = cues;

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        assert!(report.for_cue("on_cast").next().is_none(), "{:?}", report.problems);
    }

    // --- Template window non-default anchor/offset (warning) ---

    fn template_window(id: &str, anchor: WindowAnchor, offset: Vec3) -> CollisionWindow {
        CollisionWindow {
            id: id.to_string(),
            spawn: WindowSpawn::Template,
            anchor,
            anchor_offset: offset,
            strikes: true,
            active_duration: 1.0,
            shape: CollisionShape::Sphere { radius: 0.3 },
            motion: VolumeMotion::Static,
            motion_direction: Default::default(),
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::OncePerTarget,
            rehit_interval: None,
            emitter: None,
            paints: None,
        }
    }

    fn emitting_scheduled_window(id: &str, template_target: &str) -> CollisionWindow {
        CollisionWindow {
            id: id.to_string(),
            spawn: WindowSpawn::Scheduled { phase: WindowPhase::Active, offset: 0.0 },
            anchor: WindowAnchor::Caster,
            anchor_offset: Vec3::ZERO,
            strikes: true,
            active_duration: 1.0,
            shape: CollisionShape::Sphere { radius: 0.5 },
            motion: VolumeMotion::Static,
            motion_direction: Default::default(),
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::OncePerTarget,
            rehit_interval: None,
            emitter: Some(Emitter { window: template_target.to_string(), rate: 5.0, jitter: 0.5 }),
            paints: None,
        }
    }

    #[test]
    fn template_window_non_default_anchor_is_warning() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        // Non-zero `anchor_offset` alone (anchor left at its Caster default) — deliberately NOT
        // `WindowAnchor::CastPoint`, which would additionally trip the unrelated structural
        // "CastPoint anchor unreachable" `validate_timeline` rule on this same window.
        entry.timeline.collision_windows = vec![
            emitting_scheduled_window("storm", "shard"),
            template_window("shard", WindowAnchor::Caster, Vec3::new(0.0, 3.0, 0.0)),
        ];

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        let probs: Vec<_> = report.for_window("shard").collect();
        assert_eq!(probs.len(), 1, "{:?}", report.problems);
        assert!(!probs[0].blocking);
    }

    #[test]
    fn template_window_default_anchor_is_clean() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        entry.timeline.collision_windows = vec![
            emitting_scheduled_window("storm", "shard"),
            template_window("shard", WindowAnchor::Caster, Vec3::ZERO),
        ];

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        assert!(report.for_window("shard").next().is_none(), "{:?}", report.problems);
    }

    // --- trigger-cycle depth ---

    #[test]
    fn trigger_cycle_beyond_depth_8_is_blocking() {
        let mut library = SkillLibrary::default();
        // a -> b -> a -> b -> ... true cycle: walk immediately exceeds depth 8.
        insert(
            &mut library,
            "b",
            blank_timeline("b"),
            vec![SkillCondition {
                trigger_skill: "a".to_string(),
                additional: true,
                condition: TriggerCondition::Always,
            }],
        );
        let mut entry = blank_entry("a");
        entry.rules.conditions = vec![SkillCondition {
            trigger_skill: "b".to_string(),
            additional: true,
            condition: TriggerCondition::Always,
        }];
        library.skills.insert("a".to_string(), entry.clone());

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        let probs: Vec<_> = report.problems.iter().filter(|p| p.target == "skill").collect();
        assert!(probs.iter().any(|p| p.blocking && p.message.contains("depth")), "{:?}", report.problems);
    }

    #[test]
    fn trigger_chain_within_depth_8_is_clean() {
        let mut library = SkillLibrary::default();
        insert(&mut library, "b", blank_timeline("b"), vec![]);
        let mut entry = blank_entry("a");
        entry.rules.conditions = vec![SkillCondition {
            trigger_skill: "b".to_string(),
            additional: true,
            condition: TriggerCondition::Always,
        }];
        library.skills.insert("a".to_string(), entry.clone());

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        assert!(
            report.problems.iter().filter(|p| p.target == "skill").all(|p| !p.message.contains("depth")),
            "{:?}",
            report.problems
        );
    }

    // --- structural validate_timeline errors surfaced verbatim ---

    #[test]
    fn structural_timeline_error_is_blocking_and_tagged_to_window() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        // CastPoint anchor with an acquisition that can never produce a point (Aim, the
        // default) — `validate_timeline` rejects this structurally.
        let mut window = real_timeline("a").collision_windows.remove(0);
        window.anchor = WindowAnchor::CastPoint;
        entry.timeline.collision_windows = vec![window];

        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        let probs: Vec<_> = report.for_window("bolt").collect();
        assert_eq!(probs.len(), 1);
        assert!(probs[0].blocking);
        assert!(probs[0].message.contains("CastPoint"));
    }

    #[test]
    fn structurally_valid_timeline_has_no_structural_error() {
        let library = SkillLibrary::default();
        let entry = blank_entry("a");
        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        assert!(report.problems.iter().all(|p| !p.message.contains("CastPoint")), "{:?}", report.problems);
    }

    // `ParamSource`/`CueParam` are imported for API completeness (cue bindings can carry
    // charge-driven params — not exercised by these rules, but keeps the import block honest
    // about the full `CueBinding` shape this module reads).
    #[test]
    fn cue_param_source_charge_does_not_affect_validation() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        let mut cues = HashMap::new();
        cues.insert(
            "on_cast".to_string(),
            CueBinding {
                effect: None,
                attach: CueAttach::World,
                anim: None,
                params: vec![CueParam { param: "scale".to_string(), source: ParamSource::Charge }],
                duration: None,
            },
        );
        entry.timeline.cues = cues;
        let (effects, vfx) = empty_libs();
        let report = validate_skill(&entry, &library, &effects, &vfx, &SurfaceRegistry::default(), None);
        assert!(report.for_cue("on_cast").next().is_none());
    }

    // --- Surfaces: window `paints` (unknown surface lookup + numeric mirror) ---

    #[test]
    fn paints_unknown_surface_is_blocking() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        entry.timeline.collision_windows = vec![painting_window(
            "splat",
            PaintSpec { surface: "ghost".to_string(), radius: 0.45, mode: PaintMode::OnEnd, lifetime: None },
        )];
        let (effects, vfx) = empty_libs();
        let surfaces = registry_with(&[]); // no surfaces loaded -> every id is unknown
        let report = validate_skill(&entry, &library, &effects, &vfx, &surfaces, None);
        // obelisk's `validate_timeline` doesn't check registry membership (it warns-and-skips at
        // runtime), so ONLY the editor's lookup rule fires here — exactly one problem.
        let probs: Vec<_> = report.for_window("splat").collect();
        assert_eq!(probs.len(), 1, "{:?}", report.problems);
        assert!(probs[0].blocking);
        assert!(probs[0].message.contains("ghost"));
    }

    #[test]
    fn paints_known_surface_valid_numerics_is_clean() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        entry.timeline.collision_windows = vec![painting_window(
            "splat",
            PaintSpec {
                surface: "frost".to_string(),
                radius: 0.45,
                mode: PaintMode::Trail { step: 0.8 },
                lifetime: Some(5.0),
            },
        )];
        let (effects, vfx) = empty_libs();
        let surfaces = registry_with(&["frost"]);
        let report = validate_skill(&entry, &library, &effects, &vfx, &surfaces, None);
        assert!(report.for_window("splat").next().is_none(), "{:?}", report.problems);
    }

    #[test]
    fn paints_empty_surface_id_is_blocking() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        entry.timeline.collision_windows = vec![painting_window(
            "splat",
            PaintSpec { surface: String::new(), radius: 0.45, mode: PaintMode::OnEnd, lifetime: None },
        )];
        let (effects, vfx) = empty_libs();
        let surfaces = registry_with(&[]);
        let report = validate_skill(&entry, &library, &effects, &vfx, &surfaces, None);
        assert!(
            report.for_window("splat").any(|p| p.blocking && p.message.contains("empty surface")),
            "{:?}",
            report.problems
        );
    }

    #[test]
    fn paints_radius_zero_is_blocking() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        entry.timeline.collision_windows = vec![painting_window(
            "splat",
            PaintSpec { surface: "frost".to_string(), radius: 0.0, mode: PaintMode::OnEnd, lifetime: None },
        )];
        let (effects, vfx) = empty_libs();
        let surfaces = registry_with(&["frost"]);
        let report = validate_skill(&entry, &library, &effects, &vfx, &surfaces, None);
        assert!(
            report.for_window("splat").any(|p| p.blocking && p.message.contains("radius")),
            "{:?}",
            report.problems
        );
    }

    #[test]
    fn paints_trail_step_zero_is_blocking() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        entry.timeline.collision_windows = vec![painting_window(
            "splat",
            PaintSpec {
                surface: "frost".to_string(),
                radius: 0.45,
                mode: PaintMode::Trail { step: 0.0 },
                lifetime: None,
            },
        )];
        let (effects, vfx) = empty_libs();
        let surfaces = registry_with(&["frost"]);
        let report = validate_skill(&entry, &library, &effects, &vfx, &surfaces, None);
        assert!(
            report.for_window("splat").any(|p| p.blocking && p.message.contains("Trail step")),
            "{:?}",
            report.problems
        );
    }

    #[test]
    fn paints_lifetime_override_zero_is_blocking() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        entry.timeline.collision_windows = vec![painting_window(
            "splat",
            PaintSpec {
                surface: "frost".to_string(),
                radius: 0.45,
                mode: PaintMode::OnEnd,
                lifetime: Some(0.0),
            },
        )];
        let (effects, vfx) = empty_libs();
        let surfaces = registry_with(&["frost"]);
        let report = validate_skill(&entry, &library, &effects, &vfx, &surfaces, None);
        assert!(
            report.for_window("splat").any(|p| p.blocking && p.message.contains("lifetime")),
            "{:?}",
            report.problems
        );
    }

    // --- Surfaces: acquisition `on_surface` (unknown surface lookup) ---

    #[test]
    fn on_surface_unknown_surface_is_blocking() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        entry.timeline.acquisition = Acquisition::GroundPoint {
            range: 20.0,
            fallback: AcqFallback::Fizzle,
            on_surface: Some(SurfaceRequirement { surface: "ghost".to_string(), snap: true, consume: false }),
        };
        let (effects, vfx) = empty_libs();
        let surfaces = registry_with(&[]);
        let report = validate_skill(&entry, &library, &effects, &vfx, &surfaces, None);
        let probs: Vec<_> = report.problems.iter().filter(|p| p.target == "acquisition").collect();
        assert_eq!(probs.len(), 1, "{:?}", report.problems);
        assert!(probs[0].blocking);
        assert!(probs[0].message.contains("ghost"));
    }

    #[test]
    fn on_surface_known_surface_is_clean() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        entry.timeline.acquisition = Acquisition::GroundPoint {
            range: 20.0,
            fallback: AcqFallback::Fizzle,
            on_surface: Some(SurfaceRequirement { surface: "frost".to_string(), snap: true, consume: false }),
        };
        let (effects, vfx) = empty_libs();
        let surfaces = registry_with(&["frost"]);
        let report = validate_skill(&entry, &library, &effects, &vfx, &surfaces, None);
        assert!(report.problems.iter().all(|p| p.target != "acquisition"), "{:?}", report.problems);
    }

    /// `on_surface` nested one level deep in a `GroundPoint` fallback chain is still checked
    /// (the panel edits that inner arm too — see `panel::behavior::draw_fallback`).
    #[test]
    fn on_surface_unknown_in_fallback_chain_is_blocking() {
        let library = SkillLibrary::default();
        let mut entry = blank_entry("a");
        entry.timeline.acquisition = Acquisition::HitscanEntity {
            range: 20.0,
            filter: HitFilter::Enemies,
            fallback: AcqFallback::Then(Box::new(Acquisition::GroundPoint {
                range: 20.0,
                fallback: AcqFallback::Fizzle,
                on_surface: Some(SurfaceRequirement {
                    surface: "ghost".to_string(),
                    snap: true,
                    consume: false,
                }),
            })),
        };
        let (effects, vfx) = empty_libs();
        let surfaces = registry_with(&["frost"]);
        let report = validate_skill(&entry, &library, &effects, &vfx, &surfaces, None);
        assert!(
            report.problems.iter().any(|p| p.target == "acquisition" && p.blocking && p.message.contains("ghost")),
            "{:?}",
            report.problems
        );
    }
}

//! Archetype skill templates (`strike`/`projectile`/`zone`/`beam`) + starter Effect
//! presets (Task 5).
//!
//! Each archetype produces a playable v2 `CastTimeline` (passes
//! `obelisk_bevy::assets::validate_timeline`) paired with a minimal rules
//! `stat_core::Skill` (one basic damage line). Their cue bindings reference ONLY the
//! two starter Effect presets `ensure_starter_effects` guarantees exist, so a fresh
//! install (no content roots registered yet) never authors a dangling cue.

use bevy::prelude::*;

use obelisk_bevy::assets::{
    AcqFallback, Acquisition, CastTimeline, CollisionShape, CollisionWindow, CueAttach, CueBinding,
    HitFilter, HitMode, PhaseDurations, VolumeMotion, WindowAnchor, WindowPhase, WindowSpawn,
};
use stat_core::{BaseDamage, DamageConfig, DamageType, Delivery, Skill};

use crate::effects::{EffectAction, EffectLibrary, EffectMarker, EffectStep, EffectTrigger, SpawnLocation};

/// Name of the starter "cast" cue Effect preset every template's `on_cast` binding
/// references — see `ensure_starter_effects`.
pub const STARTER_MUZZLE_EFFECT: &str = "Skill Muzzle";
/// Name of the starter "hit" cue Effect preset every template's `on_hit` binding
/// references — see `ensure_starter_effects`.
pub const STARTER_IMPACT_EFFECT: &str = "Skill Impact";

/// The four skill archetypes the "New Skill" palette rows offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillArchetype {
    Strike,
    Projectile,
    Zone,
    Beam,
}

impl SkillArchetype {
    pub const ALL: [SkillArchetype; 4] = [
        SkillArchetype::Strike,
        SkillArchetype::Projectile,
        SkillArchetype::Zone,
        SkillArchetype::Beam,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            SkillArchetype::Strike => "Strike",
            SkillArchetype::Projectile => "Projectile",
            SkillArchetype::Zone => "Zone",
            SkillArchetype::Beam => "Beam",
        }
    }

    /// Build this archetype's `(rules, timeline)` pair for a fresh skill named `id`.
    pub fn build(&self, id: &str) -> (Skill, CastTimeline) {
        match self {
            SkillArchetype::Strike => strike_template(id),
            SkillArchetype::Projectile => projectile_template(id),
            SkillArchetype::Zone => zone_template(id),
            SkillArchetype::Beam => beam_template(id),
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn basic_rules(id: &str, name: &str, delivery: Delivery) -> Skill {
    Skill {
        id: id.to_string(),
        name: name.to_string(),
        delivery,
        damage: DamageConfig {
            base_damages: vec![BaseDamage::new(DamageType::Physical, 10.0, 20.0)],
            weapon_effectiveness: 0.0,
            damage_effectiveness: 1.0,
            ..DamageConfig::default()
        },
        ..Default::default()
    }
}

fn phase_durations() -> PhaseDurations {
    PhaseDurations {
        windup: 0.2,
        active: 0.3,
        recovery: 0.3,
    }
}

fn cast_and_hit_cues() -> std::collections::HashMap<String, CueBinding> {
    let mut cues = std::collections::HashMap::new();
    cues.insert(
        "on_cast".to_string(),
        CueBinding {
            effect: Some(STARTER_MUZZLE_EFFECT.to_string()),
            attach: CueAttach::World,
            anim: None,
            params: Vec::new(),
            duration: None,
        },
    );
    cues.insert(
        "on_hit".to_string(),
        CueBinding {
            effect: Some(STARTER_IMPACT_EFFECT.to_string()),
            attach: CueAttach::World,
            anim: None,
            params: Vec::new(),
            duration: None,
        },
    );
    cues
}

// ---------------------------------------------------------------------------
// Archetype templates
// ---------------------------------------------------------------------------

/// Melee strike: one short-lived sphere at the caster, `Aim` acquisition (no cast
/// point needed — the window anchors on the caster).
pub fn strike_template(id: &str) -> (Skill, CastTimeline) {
    let rules = basic_rules(id, "New Strike", Delivery::Melee);
    let timeline = CastTimeline {
        skill_id: id.to_string(),
        phase_durations: phase_durations(),
        collision_windows: vec![CollisionWindow {
            id: "strike".to_string(),
            spawn: WindowSpawn::Scheduled {
                phase: WindowPhase::Active,
                offset: 0.0,
            },
            anchor: WindowAnchor::Caster,
            anchor_offset: Vec3::new(0.0, 1.0, 1.5),
            strikes: true,
            active_duration: 0.15,
            shape: CollisionShape::Sphere { radius: 1.5 },
            motion: VolumeMotion::Static,
            motion_direction: Default::default(),
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::OncePerTarget,
            rehit_interval: None,
            emitter: None,
        }],
        acquisition: Acquisition::Aim,
        vfx_cues: Default::default(),
        chain_radius: 6.0,
        chargeable: false,
        max_hold: 1.0,
        cues: cast_and_hit_cues(),
        charge_cues: Vec::new(),
    };
    (rules, timeline)
}

/// Projectile: a sphere that flies forward from the caster on `Aim`.
pub fn projectile_template(id: &str) -> (Skill, CastTimeline) {
    let rules = basic_rules(id, "New Projectile", Delivery::Projectile);
    let timeline = CastTimeline {
        skill_id: id.to_string(),
        phase_durations: phase_durations(),
        collision_windows: vec![CollisionWindow {
            id: "bolt".to_string(),
            spawn: WindowSpawn::Scheduled {
                phase: WindowPhase::Active,
                offset: 0.0,
            },
            anchor: WindowAnchor::Caster,
            anchor_offset: Vec3::ZERO,
            strikes: true,
            active_duration: 2.0,
            shape: CollisionShape::Sphere { radius: 0.4 },
            motion: VolumeMotion::Linear { speed: 25.0 },
            motion_direction: Default::default(),
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::FirstOnly,
            rehit_interval: None,
            emitter: None,
        }],
        acquisition: Acquisition::Aim,
        vfx_cues: Default::default(),
        chain_radius: 6.0,
        chargeable: false,
        max_hold: 1.0,
        cues: cast_and_hit_cues(),
        charge_cues: Vec::new(),
    };
    (rules, timeline)
}

/// Ground-targeted zone: a static sphere at the acquired cast point.
/// `GroundPoint { fallback: Then(SelfPoint) }` — a ground-aimed cast lands where
/// aimed; an unaimed one still resolves to "above the caster" via the fallback.
pub fn zone_template(id: &str) -> (Skill, CastTimeline) {
    let rules = basic_rules(id, "New Zone", Delivery::Instant);
    let timeline = CastTimeline {
        skill_id: id.to_string(),
        phase_durations: phase_durations(),
        collision_windows: vec![CollisionWindow {
            id: "zone".to_string(),
            spawn: WindowSpawn::Scheduled {
                phase: WindowPhase::Active,
                offset: 0.0,
            },
            anchor: WindowAnchor::CastPoint,
            anchor_offset: Vec3::ZERO,
            strikes: true,
            active_duration: 2.0,
            shape: CollisionShape::Sphere { radius: 3.0 },
            motion: VolumeMotion::Static,
            motion_direction: Default::default(),
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::EveryTick,
            rehit_interval: Some(0.5),
            emitter: None,
        }],
        acquisition: Acquisition::GroundPoint {
            range: 20.0,
            fallback: AcqFallback::Then(Box::new(Acquisition::SelfPoint)),
        },
        vfx_cues: Default::default(),
        chain_radius: 6.0,
        chargeable: false,
        max_hold: 1.0,
        cues: cast_and_hit_cues(),
        charge_cues: Vec::new(),
    };
    (rules, timeline)
}

/// Beam: an instantaneous link to the designated (hitscan) target.
/// `HitscanEntity { fallback: Fizzle }` — no target in range/filter means the cast
/// simply fizzles (paid rejection), matching a beam's "must have someone to hit"
/// nature.
pub fn beam_template(id: &str) -> (Skill, CastTimeline) {
    let rules = basic_rules(id, "New Beam", Delivery::Projectile);
    let timeline = CastTimeline {
        skill_id: id.to_string(),
        phase_durations: phase_durations(),
        collision_windows: vec![CollisionWindow {
            id: "beam".to_string(),
            spawn: WindowSpawn::Scheduled {
                phase: WindowPhase::Active,
                offset: 0.0,
            },
            anchor: WindowAnchor::Caster,
            anchor_offset: Vec3::ZERO,
            strikes: true,
            active_duration: 0.1,
            shape: CollisionShape::Sphere { radius: 0.5 },
            motion: VolumeMotion::Beam,
            motion_direction: Default::default(),
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::FirstOnly,
            rehit_interval: None,
            emitter: None,
        }],
        acquisition: Acquisition::HitscanEntity {
            range: 30.0,
            filter: HitFilter::Enemies,
            fallback: AcqFallback::Fizzle,
        },
        vfx_cues: Default::default(),
        chain_radius: 6.0,
        chargeable: false,
        max_hold: 1.0,
        cues: cast_and_hit_cues(),
        charge_cues: Vec::new(),
    };
    (rules, timeline)
}

// ---------------------------------------------------------------------------
// Starter Effect presets
// ---------------------------------------------------------------------------

/// Ensure the starter Effect presets every template's cues reference exist in
/// `library` (inserted only if absent — never clobbers an author's own edits).
pub fn ensure_starter_effects(library: &mut EffectLibrary) {
    for (name, marker) in starter_effect_presets() {
        library.effects.entry(name.to_string()).or_insert(marker);
    }
}

fn starter_effect_presets() -> Vec<(&'static str, EffectMarker)> {
    vec![
        (STARTER_MUZZLE_EFFECT, single_spawn_particle_effect("Fire")),
        (STARTER_IMPACT_EFFECT, single_spawn_particle_effect("Sparks")),
    ]
}

/// A one-step effect: spawn a single `bevy_vfx` built-in particle preset on spawn, at
/// the effect entity's own position.
fn single_spawn_particle_effect(vfx_preset: &str) -> EffectMarker {
    EffectMarker {
        steps: vec![EffectStep {
            name: "spawn".to_string(),
            trigger: EffectTrigger::OnSpawn,
            actions: vec![EffectAction::SpawnParticle {
                tag: "fx".to_string(),
                preset: vfx_preset.to_string(),
                at: SpawnLocation::Offset(Vec3::ZERO),
            }],
        }],
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use obelisk_bevy::assets::validate_timeline;

    #[test]
    fn every_archetype_validates_and_round_trips() {
        for archetype in SkillArchetype::ALL {
            let (rules, timeline) = archetype.build("test_skill");

            assert!(
                validate_timeline(&timeline).is_ok(),
                "{:?} template failed validate_timeline: {:?}",
                archetype,
                validate_timeline(&timeline)
            );

            // RON round-trip.
            let ron_str = ron::ser::to_string_pretty(&timeline, Default::default())
                .unwrap_or_else(|e| panic!("{:?} timeline failed to serialize: {e}", archetype));
            let reparsed: CastTimeline = ron::de::from_str(&ron_str)
                .unwrap_or_else(|e| panic!("{:?} timeline failed to re-parse: {e}", archetype));
            assert_eq!(reparsed.skill_id, timeline.skill_id);
            assert_eq!(reparsed.collision_windows.len(), timeline.collision_windows.len());

            // TOML round-trip for the rules half.
            let toml_str = toml::to_string(&rules)
                .unwrap_or_else(|e| panic!("{:?} rules failed to serialize: {e}", archetype));
            let reparsed_rules: Skill = toml::from_str(&toml_str)
                .unwrap_or_else(|e| panic!("{:?} rules failed to re-parse: {e}", archetype));
            assert_eq!(reparsed_rules.id, rules.id);
            assert_eq!(reparsed_rules.damage.base_damages.len(), 1);
        }
    }

    #[test]
    fn template_projectile_validates() {
        let (_, timeline) = projectile_template("bolt");
        assert!(validate_timeline(&timeline).is_ok());
    }

    #[test]
    fn templates_reference_only_starter_effects() {
        let mut names = std::collections::HashSet::new();
        for archetype in SkillArchetype::ALL {
            let (_, timeline) = archetype.build("t");
            for binding in timeline.cues.values() {
                if let Some(effect) = &binding.effect {
                    names.insert(effect.clone());
                }
            }
        }
        assert_eq!(
            names,
            [STARTER_MUZZLE_EFFECT.to_string(), STARTER_IMPACT_EFFECT.to_string()]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn ensure_starter_effects_inserts_but_does_not_clobber() {
        let mut library = EffectLibrary::default();
        ensure_starter_effects(&mut library);
        assert!(library.effects.contains_key(STARTER_MUZZLE_EFFECT));
        assert!(library.effects.contains_key(STARTER_IMPACT_EFFECT));

        // Author edits the preset; a second call must not clobber it.
        library
            .effects
            .get_mut(STARTER_MUZZLE_EFFECT)
            .unwrap()
            .steps
            .clear();
        ensure_starter_effects(&mut library);
        assert!(library.effects[STARTER_MUZZLE_EFFECT].steps.is_empty());
    }
}

//! Task 10 headless integration tests for the deterministic preview stage — ported/adapted from
//! `arena_editor`'s `tests/preview_play.rs` (obelisk-arena @ `f6472e4`).
//!
//! Verified on a from-scratch headless app (`MinimalPlugins` + the preview's own plugins), NOT
//! the full editor (which needs a real render backend to even build — see
//! `bevy_modal_editor`'s own `build_editor_app` precedent in obelisk-arena's `arena_editor`). The
//! deterministic obelisk sim resolves real damage on the dummy — proving what you author is what
//! a game built on this content would play.

#![cfg(feature = "obelisk")]

use std::path::PathBuf;
use std::time::Duration;

use bevy::prelude::*;

use bevy_editor_game::{AnimationLibrary, GameResetEvent, GameStartedEvent, GameState};
use bevy_vfx::VfxLibrary;

use obelisk_bevy::assets::{
    AcqFallback, Acquisition, CastTimeline, CollisionShape, CollisionWindow, HitFilter, HitMode,
    MotionDirection, PhaseDurations, VolumeMotion, WindowAnchor, WindowPhase, WindowSpawn,
};
use obelisk_bevy::testkit::{EventRecorder, EventRecorderPlugin};
use stat_core::{BaseDamage, DamageConfig, Delivery, Skill, SkillCondition};

use bevy_modal_editor::effects::EffectLibrary;
use bevy_modal_editor::skill::library::{SkillEntry, SkillLibrary};
use bevy_modal_editor::skill::preview::{
    stage::{PreviewCaster, PreviewDummy, PreviewSimPlugin, PreviewControllerPlugin, SPAWN_MARKERS},
    rig::PreviewRigPlugin,
    cosmetics::{PreviewCosmetic, PreviewCosmeticsPlugin},
    sockets::{index_rig_sockets, RigSockets},
};
use bevy_modal_editor::skill::templates::SkillArchetype;

fn test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::asset::AssetPlugin {
            file_path: ".".into(),
            ..default()
        })
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::scene::ScenePlugin)
        .add_plugins(bevy::state::app::StatesPlugin)
        .add_plugins(EventRecorderPlugin)
        .init_state::<GameState>()
        .add_message::<GameStartedEvent>()
        .add_message::<GameResetEvent>()
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs_f64(1.0 / 60.0),
        ))
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .init_resource::<SkillLibrary>()
        .init_resource::<EffectLibrary>()
        .init_resource::<VfxLibrary>()
        // The rig attach graph needs `AnimationLibrary`/`AnimationGraph` registered even though
        // no rig scene ever resolves headlessly (no GltfPlugin) — mirrors arena_editor's own
        // `preview_play.rs` test harness exactly.
        .init_resource::<AnimationLibrary>()
        .init_asset::<AnimationGraph>()
        .add_plugins(PreviewSimPlugin)
        .add_plugins(PreviewControllerPlugin)
        .add_plugins(PreviewRigPlugin)
        .add_plugins(PreviewCosmeticsPlugin)
        .init_resource::<RigSockets>()
        .add_systems(Update, index_rig_sockets);
    // Some plugins (avian3d's diagnostics registration among them) defer resource setup to
    // `Plugin::finish()`, which `App::run()` calls automatically but manual `app.update()`
    // driving does not — must be called explicitly before the first update (mirrors
    // arena_editor's own `preview_play.rs` harness exactly).
    app.finish();
    app.cleanup();
    app
}

/// Insert a skill directly into `SkillLibrary` (bypassing disk — these are in-memory fixtures,
/// same as every other `SkillLibrary` test in this crate).
fn insert_skill(app: &mut App, id: &str, rules: Skill, timeline: CastTimeline) {
    app.world_mut().resource_mut::<SkillLibrary>().skills.insert(
        id.to_string(),
        SkillEntry {
            rules,
            timeline,
            rules_path: PathBuf::new(),
            timeline_path: PathBuf::new(),
            dirty_rules: false,
            dirty_timeline: false,
            disk_hash: (0, 0),
        },
    );
}

fn open_skill(app: &mut App, id: &str) {
    app.world_mut().resource_mut::<SkillLibrary>().open = Some(id.to_string());
}

fn play(app: &mut App, ticks: usize) {
    app.world_mut().write_message(GameStartedEvent);
    for _ in 0..ticks {
        app.update();
    }
}

// ---------------------------------------------------------------------------
// Test 1: cast a template skill on the stage → damage on the dummy.
// ---------------------------------------------------------------------------

#[test]
fn play_resolves_damage_on_the_dummy() {
    let mut app = test_app();
    let (rules, timeline) = SkillArchetype::Projectile.build("bolt");
    insert_skill(&mut app, "bolt", rules, timeline);
    open_skill(&mut app, "bolt");
    // A couple of settling frames before Play: the stage must exist (`ensure_stage`) and the
    // registry must be synced (`sync_sim_registries`) before the cast lands.
    app.update();
    app.update();

    play(&mut app, 90);

    let rec = app.world().resource::<EventRecorder>();
    assert!(!rec.cast_began.is_empty(), "the bolt should have begun casting");
    assert!(
        !rec.damage_resolved.is_empty(),
        "the projectile should have resolved damage on the dummy"
    );
    assert!(
        rec.damage_resolved.iter().map(|d| d.total_damage).sum::<f64>() > 0.0,
        "total damage dealt should be positive"
    );
}

/// The stage is PERSISTENT: the caster exists without any Play, and there is exactly one (the
/// projectile template is not a chaining skill, so exactly one dummy too).
#[test]
fn stage_is_persistent_before_any_play() {
    let mut app = test_app();
    app.update();
    app.update();
    let casters = app
        .world_mut()
        .query_filtered::<Entity, With<PreviewCaster>>()
        .iter(app.world())
        .count();
    let dummies = app
        .world_mut()
        .query_filtered::<Entity, With<PreviewDummy>>()
        .iter(app.world())
        .count();
    assert_eq!(casters, 1, "one persistent stage caster, with no skill open yet");
    assert_eq!(dummies, 1, "one default dummy, with no skill open yet");
}

// ---------------------------------------------------------------------------
// Test 2: a GroundPoint skill → its zone window spawns above the aim marker (cast_point
// preserved) — the historic "blizzard blocker" a ground-targeted cast must not regress.
// ---------------------------------------------------------------------------

#[test]
fn groundpoint_zone_spawns_above_the_aim_marker_and_hits_the_dummy() {
    let mut app = test_app();
    let (rules, timeline) = SkillArchetype::Zone.build("storm");
    assert!(
        matches!(timeline.acquisition, Acquisition::GroundPoint { .. }),
        "the zone archetype must be GroundPoint-acquired for this test to exercise anything"
    );
    insert_skill(&mut app, "storm", rules, timeline);
    open_skill(&mut app, "storm");
    app.update();
    app.update();

    play(&mut app, 60);

    let rec = app.world().resource::<EventRecorder>();
    let hit = rec
        .hit_confirmed
        .iter()
        .find(|h| h.window_id == "zone")
        .expect("the zone window should hit the dummy standing at the aim marker");
    let marker = SPAWN_MARKERS[1];
    assert!(
        hit.position.distance(marker) < 0.5,
        "the zone's hit position {:?} should be AT the ground/aim marker {:?} (cast_point \
         preserved, not collapsed to a direction)",
        hit.position,
        marker
    );
}

// ---------------------------------------------------------------------------
// Test 3 (the flagship): a fireball-pair fixture (bolt triggers explosion on world impact) —
// seek/step past impact → the explosion's OWN `DamageResolved` appears IN THE PREVIEW SIM. This
// is the one test that specifically exercises the stage's flat-floor `HitboxWorldHit` reporter
// (CRITICAL ADAPTATION #2): without it, an `OnImpact`-triggered skill can never fire with no
// game host providing world-hit detection.
// ---------------------------------------------------------------------------

/// A GroundPoint bolt that free-falls (`MotionDirection::Down`) from 3 units above the stage's
/// aim marker — which is exactly where the default (non-chaining) dummy stands (see
/// `stage::ground_marker`'s doc comment) — straight into the floor. `strikes: false` (a carrier
/// volume) so it can never end via `HitEntity` on the way down through the dummy's hurtbox; the
/// ONLY way it can end is `HitWorld` (via the stage's `report_ground_hits`) or `Fuse`. Its rules
/// carry an `OnImpact` lifecycle condition triggering "fireball_explosion" — evaluated by
/// `end_hitboxes` only for a `HitWorld` ending.
fn fireball_timeline() -> CastTimeline {
    CastTimeline {
        skill_id: "fireball".to_string(),
        phase_durations: PhaseDurations { windup: 0.05, active: 0.1, recovery: 0.1 },
        collision_windows: vec![CollisionWindow {
            id: "bolt".to_string(),
            spawn: WindowSpawn::Scheduled { phase: WindowPhase::Active, offset: 0.0 },
            anchor: WindowAnchor::CastPoint,
            anchor_offset: Vec3::new(0.0, 3.0, 0.0),
            strikes: false,
            active_duration: 2.0,
            shape: CollisionShape::Sphere { radius: 0.4 },
            motion: VolumeMotion::Linear { speed: 20.0 },
            motion_direction: MotionDirection::Down,
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::FirstOnly,
            rehit_interval: None,
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
        cues: Default::default(),
    }
}

fn fireball_rules() -> Skill {
    Skill {
        id: "fireball".to_string(),
        name: "Fireball".to_string(),
        delivery: Delivery::Projectile,
        damage: DamageConfig {
            base_damages: vec![BaseDamage::new(stat_core::DamageType::Fire, 20.0, 20.0)],
            weapon_effectiveness: 0.0,
            damage_effectiveness: 1.0,
            ..DamageConfig::default()
        },
        conditions: vec![SkillCondition {
            trigger_skill: "fireball_explosion".to_string(),
            additional: true,
            condition: loot_core::types::TriggerCondition::OnImpact,
        }],
        ..Default::default()
    }
}

/// The explosion's own timeline: a `CastPoint`-anchored blast at the trigger's payload position
/// (the impact point `end_hitboxes` resolved) — never player-cast (`execute_skill_timeline`
/// bypasses `Acquisition` entirely), wide enough to reach the dummy standing at the aim marker
/// (the impact lands directly beneath it, see the module's fireball geometry doc above).
fn fireball_explosion_timeline() -> CastTimeline {
    CastTimeline {
        skill_id: "fireball_explosion".to_string(),
        phase_durations: PhaseDurations { windup: 0.0, active: 1.0, recovery: 0.0 },
        collision_windows: vec![CollisionWindow {
            id: "blast".to_string(),
            spawn: WindowSpawn::Scheduled { phase: WindowPhase::Active, offset: 0.0 },
            anchor: WindowAnchor::CastPoint,
            anchor_offset: Vec3::ZERO,
            strikes: true,
            active_duration: 0.3,
            shape: CollisionShape::Sphere { radius: 2.0 },
            motion: VolumeMotion::Static,
            motion_direction: Default::default(),
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::OncePerTarget,
            rehit_interval: None,
            emitter: None,
        }],
        acquisition: Acquisition::default(),
        vfx_cues: Default::default(),
        chain_radius: 6.0,
        chargeable: false,
        max_hold: 1.0,
        cues: Default::default(),
    }
}

fn fireball_explosion_rules() -> Skill {
    Skill {
        id: "fireball_explosion".to_string(),
        name: "Fireball Explosion".to_string(),
        delivery: Delivery::Instant,
        damage: DamageConfig {
            base_damages: vec![BaseDamage::new(stat_core::DamageType::Fire, 15.0, 15.0)],
            weapon_effectiveness: 0.0,
            damage_effectiveness: 1.0,
            ..DamageConfig::default()
        },
        ..Default::default()
    }
}

#[test]
fn fireball_pair_composes_the_sub_cast_in_preview() {
    let mut app = test_app();
    insert_skill(&mut app, "fireball", fireball_rules(), fireball_timeline());
    insert_skill(
        &mut app,
        "fireball_explosion",
        fireball_explosion_rules(),
        fireball_explosion_timeline(),
    );
    open_skill(&mut app, "fireball");
    app.update();
    app.update();

    // The bolt free-falls from 3 units up at 20 u/s (~0.15s to the floor) plus the 0.05+0.1s
    // windup/active before it even spawns — 60 ticks (1s) is ample for it to land, trigger, and
    // for the explosion's own 1s active window to resolve its hit.
    play(&mut app, 60);

    let rec = app.world().resource::<EventRecorder>();
    let bolt_end = rec
        .hitbox_ended
        .iter()
        .find(|e| e.window_id == "bolt")
        .expect("the bolt should have ended");
    assert_eq!(
        bolt_end.reason,
        obelisk_bevy::events::EndReason::HitWorld,
        "the bolt must end via the stage's flat-floor world-hit reporter, not a direct hit \
         (strikes: false) or a fuse timeout"
    );

    let ids: Vec<&str> = rec.damage_resolved.iter().map(|d| d.skill_id.as_str()).collect();
    assert!(
        ids.contains(&"fireball_explosion"),
        "the OnImpact-triggered explosion must resolve damage IN THE PREVIEW SIM — got {ids:?}"
    );
    assert_eq!(
        ids.iter().filter(|i| **i == "fireball_explosion").count(),
        1,
        "exactly one explosion resolve, got {ids:?}"
    );
}

/// Determinism: the same seed produces the same total damage across the whole fireball-pair
/// composition (bolt fall + world-impact trigger + explosion resolve).
#[test]
fn fireball_pair_is_deterministic() {
    let total = || {
        let mut app = test_app();
        insert_skill(&mut app, "fireball", fireball_rules(), fireball_timeline());
        insert_skill(
            &mut app,
            "fireball_explosion",
            fireball_explosion_rules(),
            fireball_explosion_timeline(),
        );
        open_skill(&mut app, "fireball");
        app.update();
        app.update();
        play(&mut app, 60);
        app.world()
            .resource::<EventRecorder>()
            .damage_resolved
            .iter()
            .map(|d| d.total_damage)
            .sum::<f64>()
    };
    let (a, b) = (total(), total());
    assert!(a > 0.0);
    assert_eq!(a, b, "identical fixtures must resolve identical total damage");
}

// ---------------------------------------------------------------------------
// Cue-driven cosmetics: a fired cue whose bound effect resolves against VfxLibrary spawns a
// PreviewCosmetic (proves the EffectLibrary-then-VfxLibrary resolution order actually renders
// something end-to-end, not just that it fails to panic when neither library has the name).
// ---------------------------------------------------------------------------

#[test]
fn cue_bound_to_a_vfx_preset_spawns_a_preview_cosmetic() {
    let mut app = test_app();
    app.world_mut()
        .resource_mut::<VfxLibrary>()
        .effects
        .insert("Test Muzzle".to_string(), bevy_vfx::VfxSystem::default());

    let (mut rules, mut timeline) = SkillArchetype::Projectile.build("zap");
    rules.id = "zap".to_string();
    timeline.skill_id = "zap".to_string();
    timeline.cues.insert(
        "on_cast".to_string(),
        obelisk_bevy::assets::CueBinding {
            effect: Some("Test Muzzle".to_string()),
            attach: obelisk_bevy::assets::CueAttach::World,
            anim: None,
            params: Vec::new(),
        },
    );
    insert_skill(&mut app, "zap", rules, timeline);
    open_skill(&mut app, "zap");
    app.update();
    app.update();

    play(&mut app, 10);

    let cosmetics = app
        .world_mut()
        .query_filtered::<Entity, With<PreviewCosmetic>>()
        .iter(app.world())
        .count();
    assert!(cosmetics > 0, "the on_cast cue should have spawned at least one PreviewCosmetic");
}

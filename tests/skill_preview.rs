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

use bevy_editor_game::{AnimationLibrary, GameEntity, GameResetEvent, GameStartedEvent, GameState};
use bevy_vfx::VfxLibrary;

use obelisk_bevy::assets::{
    AcqFallback, Acquisition, CastTimeline, CollisionShape, CollisionWindow, HitFilter, HitMode,
    MotionDirection, PaintMode, PaintSpec, PhaseDurations, SurfaceRequirement, VolumeMotion,
    WindowAnchor, WindowPhase, WindowSpawn,
};
use obelisk_bevy::core::spawn_rng::SpawnRng;
use obelisk_bevy::surfaces::{
    StandingState, SurfacePatch, SurfaceRegistry, SurfaceSeq, SurfaceType, SurfaceVisuals,
};
use obelisk_bevy::testkit::{EventRecorder, EventRecorderPlugin};
use stat_core::{BaseDamage, DamageConfig, Delivery, Skill, SkillCondition};

use bevy_modal_editor::editor::EditorMode;
use bevy_modal_editor::effects::{data::EffectMarker, EffectLibrary};
use bevy_modal_editor::skill::library::{SkillEntry, SkillLibrary};
use bevy_modal_editor::skill::preview::{
    stage::{
        ground_marker, PreviewCaster, PreviewDummy, PreviewSimPlugin, PreviewControllerPlugin,
        PreviewStageFloor, PreviewStageReset, SPAWN_MARKERS,
    },
    rig::PreviewRigPlugin,
    cosmetics::{PreviewCosmetic, PreviewCosmeticsPlugin},
    sockets::{index_rig_sockets, RigSockets},
    surfaces::{attach_patch_visuals, StagedPaint, StagedPaints, SurfacePatchVisual},
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
        .init_state::<EditorMode>()
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

/// Drive `EditorMode` into `Skill` and let the transition apply (Finding 1, Task 10 review): the
/// preview stage now spawns on `OnEnter(EditorMode::Skill)` and its systems are gated
/// `run_if(in_state(EditorMode::Skill))` (mirroring `MeshModelPlugin`'s pre-existing `Blockout`
/// precedent — see `src/modeling/mod.rs`), so every test that exercises the stage must explicitly
/// enter Skill mode first. `test_app()` defaults to `View`, same as the real editor.
fn enter_skill_mode(app: &mut App) {
    app.world_mut().resource_mut::<NextState<EditorMode>>().set(EditorMode::Skill);
    app.update();
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
    enter_skill_mode(&mut app);
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
    enter_skill_mode(&mut app);
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
    enter_skill_mode(&mut app);
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
            paints: None,
        }],
        acquisition: Acquisition::GroundPoint {
            range: 20.0,
            fallback: AcqFallback::Then(Box::new(Acquisition::SelfPoint)),
            on_surface: None,
        },
        vfx_cues: Default::default(),
        chain_radius: 6.0,
        chargeable: false,
        max_hold: 1.0,
        cues: Default::default(),
        charge_cues: Vec::new(),
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
            paints: None,
        }],
        acquisition: Acquisition::default(),
        vfx_cues: Default::default(),
        chain_radius: 6.0,
        chargeable: false,
        max_hold: 1.0,
        cues: Default::default(),
        charge_cues: Vec::new(),
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
    enter_skill_mode(&mut app);
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
        enter_skill_mode(&mut app);
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
    enter_skill_mode(&mut app);
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
            duration: None,
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

// ---------------------------------------------------------------------------
// Finding 1 (Task 10 review): the stage is scoped to `EditorMode::Skill` — it must not exist (or
// tick) in any other mode, mirroring `MeshModelPlugin`'s `OnEnter`/`OnExit(EditorMode::Blockout)`
// precedent (`src/modeling/mod.rs`). A general-purpose editor session that never enters Skill
// mode must never pay for (or collide with) the preview stage.
// ---------------------------------------------------------------------------

/// (caster count, dummy count, floor-entity count) — `PreviewStageFloor` is on both the collider
/// entity and (when windowed) its visual sibling, but this headless harness has no
/// `StandardMaterial` assets, so exactly one floor entity is expected here.
fn stage_entity_counts(app: &mut App) -> (usize, usize, usize) {
    let casters = app.world_mut().query_filtered::<Entity, With<PreviewCaster>>().iter(app.world()).count();
    let dummies = app.world_mut().query_filtered::<Entity, With<PreviewDummy>>().iter(app.world()).count();
    let floors = app.world_mut().query_filtered::<Entity, With<PreviewStageFloor>>().iter(app.world()).count();
    (casters, dummies, floors)
}

#[test]
fn stage_only_exists_while_in_skill_mode() {
    let mut app = test_app();
    // Default mode is View (same as the real editor) — the stage must not exist yet.
    app.update();
    app.update();
    assert_eq!(
        stage_entity_counts(&mut app),
        (0, 0, 0),
        "no stage entities should exist before Skill mode is ever entered"
    );

    enter_skill_mode(&mut app);
    app.update();
    assert_eq!(
        stage_entity_counts(&mut app),
        (1, 1, 1),
        "entering Skill mode should spawn the caster, the default dummy, and the floor"
    );

    app.world_mut().resource_mut::<NextState<EditorMode>>().set(EditorMode::View);
    app.update();
    assert_eq!(
        stage_entity_counts(&mut app),
        (0, 0, 0),
        "leaving Skill mode should despawn the whole stage"
    );
}

/// Empirical proof that the obelisk `FixedUpdate` sim itself doesn't tick outside Skill mode
/// (not just that the stage entities are absent) — the same fixture as
/// `play_resolves_damage_on_the_dummy`, but never entering Skill mode at all.
#[test]
fn sim_does_not_advance_outside_skill_mode() {
    let mut app = test_app();
    // Deliberately stay in the default View mode.
    let (rules, timeline) = SkillArchetype::Projectile.build("inert");
    insert_skill(&mut app, "inert", rules, timeline);
    open_skill(&mut app, "inert");
    app.update();
    app.update();

    play(&mut app, 90);

    let rec = app.world().resource::<EventRecorder>();
    assert!(rec.cast_began.is_empty(), "no cast should begin outside Skill mode (no caster even exists)");
    assert!(rec.damage_resolved.is_empty(), "the obelisk sim must not resolve damage outside Skill mode");
}

// ---------------------------------------------------------------------------
// Finding 2 (Task 10 review): preview cosmetics must NOT be `GameEntity`-tagged — the generic
// `ResetCommand` (`editor::game`) despawns every `GameEntity` synchronously, BEFORE firing
// `GameResetEvent`, which would hard-despawn a cosmetic mid-flight (live `VfxSystem`, grace 0)
// and bypass the two-render-frame grace ladder `bevy_vfx` needs (this exact panic class has
// recurred 3x in the sim's history). `reset_stage_on_reset` (this crate's own `GameResetEvent`
// handler) instead expires cosmetics in place, letting `reap_preview_cosmetics` retire them
// safely over the next couple of render frames.
// ---------------------------------------------------------------------------

#[test]
fn preview_cosmetics_are_not_game_entity_tagged() {
    let mut app = test_app();
    enter_skill_mode(&mut app);
    app.world_mut()
        .resource_mut::<VfxLibrary>()
        .effects
        .insert("Test Muzzle Reset".to_string(), bevy_vfx::VfxSystem::default());

    let (mut rules, mut timeline) = SkillArchetype::Projectile.build("zap_reset");
    rules.id = "zap_reset".to_string();
    timeline.skill_id = "zap_reset".to_string();
    timeline.cues.insert(
        "on_cast".to_string(),
        obelisk_bevy::assets::CueBinding {
            effect: Some("Test Muzzle Reset".to_string()),
            attach: obelisk_bevy::assets::CueAttach::World,
            anim: None,
            params: Vec::new(),
            duration: None,
        },
    );
    insert_skill(&mut app, "zap_reset", rules, timeline);
    open_skill(&mut app, "zap_reset");
    app.update();
    app.update();

    play(&mut app, 10);

    let cosmetics: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<PreviewCosmetic>>()
        .iter(app.world())
        .collect();
    assert!(!cosmetics.is_empty(), "the on_cast cue should have spawned a cosmetic");
    for e in &cosmetics {
        assert!(
            app.world().get::<GameEntity>(*e).is_none(),
            "a preview cosmetic must NOT be GameEntity-tagged — the generic Reset's synchronous \
             despawn pass would bypass the grace ladder and reintroduce the bevy_vfx dead-entity \
             panic class (Finding 2, Task 10 review)"
        );
        assert!(
            app.world().get::<bevy_vfx::VfxSystem>(*e).is_some(),
            "sanity: this cosmetic should carry a live VfxSystem (grace 0) — exactly the state \
             that used to be hazardous to hard-despawn"
        );
    }
}

#[test]
fn game_reset_event_expires_cosmetics_via_the_ladder_not_a_hard_despawn() {
    let mut app = test_app();
    enter_skill_mode(&mut app);
    app.world_mut()
        .resource_mut::<VfxLibrary>()
        .effects
        .insert("Test Muzzle Reset 2".to_string(), bevy_vfx::VfxSystem::default());

    let (mut rules, mut timeline) = SkillArchetype::Projectile.build("zap_reset2");
    rules.id = "zap_reset2".to_string();
    timeline.skill_id = "zap_reset2".to_string();
    timeline.cues.insert(
        "on_cast".to_string(),
        obelisk_bevy::assets::CueBinding {
            effect: Some("Test Muzzle Reset 2".to_string()),
            attach: obelisk_bevy::assets::CueAttach::World,
            anim: None,
            params: Vec::new(),
            duration: None,
        },
    );
    insert_skill(&mut app, "zap_reset2", rules, timeline);
    open_skill(&mut app, "zap_reset2");
    app.update();
    app.update();

    play(&mut app, 10);

    let cosmetic = app
        .world_mut()
        .query_filtered::<Entity, With<PreviewCosmetic>>()
        .iter(app.world())
        .next()
        .expect("the on_cast cue should have spawned a cosmetic");
    assert!(
        app.world().get::<bevy_vfx::VfxSystem>(cosmetic).is_some(),
        "sanity: the cosmetic should still carry its live VfxSystem before reset"
    );

    // Mirrors `ResetCommand::apply`'s tail call — the private `ResetCommand` type itself lives in
    // the host editor crate's `editor::game` module, out of this preview-only harness's reach,
    // but `reset_stage_on_reset` is exactly what it drives.
    app.world_mut().write_message(GameResetEvent);
    // Two frames: `reset_stage_on_reset` and `reap_preview_cosmetics` are both plain `Update`
    // systems with no ordering constraint between them, so the expiry may or may not be visible
    // to reap on the very same frame it's set — two updates guarantee reap has run at least once
    // AFTER the expiry, regardless of intra-frame ordering.
    app.update();
    app.update();
    assert!(
        app.world().get_entity(cosmetic).is_ok(),
        "the cosmetic must still exist just after reset — expired in place, not hard-despawned \
         (a hard despawn here is exactly the panic this test guards against)"
    );
    assert!(
        app.world().get::<bevy_vfx::VfxSystem>(cosmetic).is_none(),
        "the reap ladder's grace-0 step should have removed the VfxSystem driver by now"
    );

    // Two more render frames complete the grace ladder (grace 1 -> 2 -> despawn).
    app.update();
    app.update();
    assert!(
        app.world().get_entity(cosmetic).is_err(),
        "the cosmetic should be despawned once the two-render-frame grace ladder completes"
    );
}

// ---------------------------------------------------------------------------
// Finding 3 (Task 10 review): cue effect-name resolution order (`EffectLibrary` first, then
// `VfxLibrary`) and unresolvable-name safety.
// ---------------------------------------------------------------------------

#[test]
fn cue_name_in_both_libraries_resolves_to_the_effect_library_entry() {
    let mut app = test_app();
    enter_skill_mode(&mut app);
    const DUP_NAME: &str = "Duplicate Preset Name";
    app.world_mut()
        .resource_mut::<EffectLibrary>()
        .effects
        .insert(DUP_NAME.to_string(), EffectMarker::default());
    app.world_mut()
        .resource_mut::<VfxLibrary>()
        .effects
        .insert(DUP_NAME.to_string(), bevy_vfx::VfxSystem::default());

    let (mut rules, mut timeline) = SkillArchetype::Projectile.build("dupname");
    rules.id = "dupname".to_string();
    timeline.skill_id = "dupname".to_string();
    timeline.cues.insert(
        "on_cast".to_string(),
        obelisk_bevy::assets::CueBinding {
            effect: Some(DUP_NAME.to_string()),
            attach: obelisk_bevy::assets::CueAttach::World,
            anim: None,
            params: Vec::new(),
            duration: None,
        },
    );
    insert_skill(&mut app, "dupname", rules, timeline);
    open_skill(&mut app, "dupname");
    app.update();
    app.update();

    play(&mut app, 10);

    let cosmetic = app
        .world_mut()
        .query_filtered::<Entity, With<PreviewCosmetic>>()
        .iter(app.world())
        .next()
        .expect("the on_cast cue should have spawned a cosmetic");
    assert!(
        app.world().get::<EffectMarker>(cosmetic).is_some(),
        "a name present in both libraries must resolve to the EffectLibrary entry (canonical, \
         EffectLibrary-first order)"
    );
    assert!(
        app.world().get::<bevy_vfx::VfxSystem>(cosmetic).is_none(),
        "must not ALSO carry the VfxLibrary system once EffectLibrary already resolved the name"
    );
}

#[test]
fn cue_name_in_neither_library_warns_and_spawns_nothing_no_panic() {
    let mut app = test_app();
    enter_skill_mode(&mut app);

    let (mut rules, mut timeline) = SkillArchetype::Projectile.build("ghostcue");
    rules.id = "ghostcue".to_string();
    timeline.skill_id = "ghostcue".to_string();
    // Two cues sharing the SAME unresolvable name — also exercises the once-per-name `warn!`
    // dedup (`spawn_cue_effect`'s `warned: &mut HashSet<String>` — a double-warn isn't directly
    // observable from outside the module, but a repeated lookup against the same missing name
    // must still not panic or spawn anything on either cue).
    timeline.cues.insert(
        "on_cast".to_string(),
        obelisk_bevy::assets::CueBinding {
            effect: Some("Nonexistent Preset".to_string()),
            attach: obelisk_bevy::assets::CueAttach::World,
            anim: None,
            params: Vec::new(),
            duration: None,
        },
    );
    timeline.cues.insert(
        "on_hit".to_string(),
        obelisk_bevy::assets::CueBinding {
            effect: Some("Nonexistent Preset".to_string()),
            attach: obelisk_bevy::assets::CueAttach::World,
            anim: None,
            params: Vec::new(),
            duration: None,
        },
    );
    insert_skill(&mut app, "ghostcue", rules, timeline);
    open_skill(&mut app, "ghostcue");
    app.update();
    app.update();

    // No panic across a full cast (including a resolved hit, which fires the on_hit cue too) —
    // the crux of this test.
    play(&mut app, 90);

    // `spawn_cue_effect` always spawns the bookkeeping entity (`PreviewCosmetic` +
    // `CosmeticLifetime` — needed unconditionally so `CueAttach::Follow`/beam-arc rendering has
    // somewhere to attach regardless of resolution), but an unresolvable name must never attach
    // a driver to it: neither an `EffectLibrary` marker nor a `VfxLibrary` system. This is
    // finding 3's own "spawns nothing / a placeholder" framing — a placeholder with no driver is
    // exactly as inert (renders nothing) as spawning no entity at all.
    let cosmetics: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<PreviewCosmetic>>()
        .iter(app.world())
        .collect();
    for e in cosmetics {
        assert!(
            app.world().get::<EffectMarker>(e).is_none(),
            "an unresolvable name must never resolve an EffectLibrary marker"
        );
        assert!(
            app.world().get::<bevy_vfx::VfxSystem>(e).is_none(),
            "an unresolvable name must never resolve a VfxLibrary system"
        );
    }
}

// ---------------------------------------------------------------------------
// Regression: an authored-but-UNBOUND vfx_cues slot must still fire its cue. obelisk's cue
// systems gate firing on `CastTimeline::vfx_cues[slot]`; a slot with no `cues` binding is a
// legitimate authored shape — obelisk-arena's firebolt authors `on_end_bolt` in `vfx_cues` with
// NO binding, purely as the trail-teardown trigger (the client despawns the Follow cosmetic when
// the end cue arrives). `sync_sim_registries` used to REPLACE the authored `vfx_cues` with the
// identity map over `cues` keys, silently dropping such slots — the preview's bolt then sailed
// through the dummy and the floor ("the firebolt does not collide") because its teardown cue
// never fired even though the sim resolved the hit.
// ---------------------------------------------------------------------------

#[test]
fn unbound_vfx_cue_slots_still_fire() {
    let mut app = test_app();
    enter_skill_mode(&mut app);
    let (rules, mut timeline) = SkillArchetype::Projectile.build("bolt");
    // The firebolt pattern: a teardown slot present in vfx_cues with NO cues binding.
    timeline
        .vfx_cues
        .insert("on_end_bolt".to_string(), "on_end_bolt".to_string());
    insert_skill(&mut app, "bolt", rules, timeline);
    open_skill(&mut app, "bolt");
    app.update();
    app.update();

    play(&mut app, 90);

    let rec = app.world().resource::<EventRecorder>();
    assert!(
        !rec.hitbox_ended.is_empty(),
        "the projectile must end (HitEntity on the dummy)"
    );
    assert!(
        rec.cues
            .iter()
            .any(|c| c.kind == obelisk_bevy::events::CueKind::OnEnd),
        "an authored-but-unbound on_end vfx_cues slot must still fire its cue (the cosmetic \
         teardown trigger) — sync_sim_registries must PRESERVE authored vfx_cues entries, not \
         clobber them with the identity-over-`cues`-keys map. fired: {:?}",
        rec.cues
            .iter()
            .map(|c| (c.cue_id.clone(), format!("{:?}", c.kind)))
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Regression (handle stability): the egui Skill panel takes `ResMut<SkillLibrary>` while drawing,
// so the library reads as changed EVERY frame a skill is open — not just on real edits.
// `sync_sim_registries` used to re-`add` every timeline unconditionally on any change, minting a
// NEW asset + handle per frame: per-frame asset/GC churn, asset-event spam, and a standing race
// for anything holding a timeline handle across ticks. Handles must stay STABLE under per-frame
// library dirtying (content updates go through the existing handle in place), and an in-flight
// cast must keep working throughout.
// ---------------------------------------------------------------------------

#[test]
fn per_frame_library_writes_keep_timeline_handles_stable() {
    use obelisk_bevy::assets::CastTimelineHandles;

    let mut app = test_app();
    enter_skill_mode(&mut app);
    let (rules, timeline) = SkillArchetype::Projectile.build("bolt");
    insert_skill(&mut app, "bolt", rules, timeline);
    open_skill(&mut app, "bolt");
    app.update();
    app.update();

    let handle_before = app
        .world()
        .resource::<CastTimelineHandles>()
        .0
        .get("bolt")
        .map(|h| h.id())
        .expect("timeline registered after the settle frames");

    app.world_mut().write_message(GameStartedEvent);
    for _ in 0..90 {
        // The panel's per-frame `ResMut<SkillLibrary>` deref (no real edit).
        app.world_mut().resource_mut::<SkillLibrary>().set_changed();
        app.update();
    }

    let handle_after = app
        .world()
        .resource::<CastTimelineHandles>()
        .0
        .get("bolt")
        .map(|h| h.id())
        .expect("timeline still registered");
    assert_eq!(
        handle_before, handle_after,
        "per-frame library dirtying must NOT mint a new timeline asset/handle each frame — \
         unchanged content syncs in place through the existing handle"
    );

    let rec = app.world().resource::<EventRecorder>();
    assert!(
        !rec.hit_window_opened.is_empty() && !rec.damage_resolved.is_empty(),
        "the in-flight cast still opens its window and lands damage under per-frame writes"
    );
}

// ---------------------------------------------------------------------------
// Surfaces (Task 2): the preview stage runs the obelisk surfaces sim (paint/decay/standing) and a
// stage reset returns it to bare ground, re-zeroing every stream the sim draws from so scrub's
// "same seed -> identical" holds for painted content too.
// ---------------------------------------------------------------------------

/// A minimal in-memory frost surface type (no disk): tiny `merge_radius` so consecutive trail
/// splats (0.5 m apart) are NOT deduped, generous `max_patches` so nothing is evicted mid-run.
fn frost_surface() -> SurfaceType {
    SurfaceType {
        id: "frost".to_string(),
        lifetime: 180.0,
        merge_radius: 0.1,
        max_patches: 64,
        patch_radius: 0.45,
        standing: None,
        on_skill_contact: Vec::new(),
        visuals: None,
    }
}

/// Overwrite the `SurfaceRegistry` with a single-type map (in-memory — no `config/surfaces` disk
/// load). Works whether or not `ObeliskSurfacesPlugin` has inserted its empty default yet, so it
/// is safe to call at both the RED (plugin absent) and GREEN stages.
fn insert_surface(app: &mut App, st: SurfaceType) {
    let mut map = std::collections::HashMap::new();
    map.insert(st.id.clone(), st);
    app.world_mut().insert_resource(SurfaceRegistry(map));
}

/// A projectile that paints a frost `Trail` as it flies — the `Projectile` archetype (which
/// already aims at the dummy and spawns from the caster) with its window's motion slowed to
/// 8 u/s and a `PaintSpec` bolted on. Painting is a window PROPERTY, so the archetype's whole
/// cast/aim/spawn machinery is reused unchanged.
fn frost_trail_skill() -> (Skill, CastTimeline) {
    let (rules, mut timeline) = SkillArchetype::Projectile.build("frost_trail");
    let w = &mut timeline.collision_windows[0];
    w.motion = VolumeMotion::Linear { speed: 8.0 };
    w.paints = Some(PaintSpec {
        surface: "frost".to_string(),
        radius: 0.45,
        mode: PaintMode::Trail { step: 0.5 },
        lifetime: None,
    });
    (rules, timeline)
}

fn surface_patch_count(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<Entity, With<SurfacePatch>>()
        .iter(app.world())
        .count()
}

/// The sorted, quantized (mm) XZ positions of every live surface patch.
fn surface_patch_positions(app: &mut App) -> Vec<(i64, i64, i64)> {
    let mut v: Vec<(i64, i64, i64)> = app
        .world_mut()
        .query_filtered::<&Transform, With<SurfacePatch>>()
        .iter(app.world())
        .map(|t| {
            (
                (t.translation.x * 1000.0) as i64,
                (t.translation.y * 1000.0) as i64,
                (t.translation.z * 1000.0) as i64,
            )
        })
        .collect();
    v.sort();
    v
}

/// The painting window paints a frost trail as it flies (>= 3 patches over ~1 s), and a stage
/// reset returns the stage to bare ground (0 patches) — the plugin is composed AND the reset
/// despawns painted content. Without `ObeliskSurfacesPlugin` in the sim this fails with 0 patches
/// (the trail painter never runs); without the reset's patch despawn it fails with the patches
/// still standing.
#[test]
fn painting_skill_produces_patches_and_reset_clears_them() {
    use bevy::ecs::system::RunSystemOnce;

    let mut app = test_app();
    enter_skill_mode(&mut app);
    insert_surface(&mut app, frost_surface());
    let (rules, timeline) = frost_trail_skill();
    insert_skill(&mut app, "frost_trail", rules, timeline);
    open_skill(&mut app, "frost_trail");
    app.update();
    app.update();

    play(&mut app, 60);
    assert!(
        surface_patch_count(&mut app) >= 3,
        "the frost trail should have painted at least 3 patches over ~1 s of travel, got {}",
        surface_patch_count(&mut app)
    );

    app.world_mut()
        .run_system_once(|mut reset: PreviewStageReset| reset.reset_stage())
        .expect("stage reset runs");
    assert_eq!(
        surface_patch_count(&mut app),
        0,
        "a stage reset must despawn every painted patch (bare ground)"
    );
}

/// Determinism: two identical Play runs on the same stage paint the trail at IDENTICAL quantized
/// positions. `play` funnels through `start_preview` -> `reset_stage`, so the second run's reset
/// clears the first run's patches and re-zeroes the surface streams before re-casting. Guards
/// against any future non-determinism (e.g. jitter) leaking into painted-patch positions.
#[test]
fn surface_scrub_restart_is_deterministic() {
    let mut app = test_app();
    enter_skill_mode(&mut app);
    insert_surface(&mut app, frost_surface());
    let (rules, timeline) = frost_trail_skill();
    insert_skill(&mut app, "frost_trail", rules, timeline);
    open_skill(&mut app, "frost_trail");
    app.update();
    app.update();

    play(&mut app, 60);
    let first = surface_patch_positions(&mut app);
    play(&mut app, 60);
    let second = surface_patch_positions(&mut app);

    assert!(first.len() >= 3, "the trail should paint patches to compare, got {}", first.len());
    assert_eq!(
        first, second,
        "same charge + seed -> IDENTICAL painted-patch positions across a scrub restart"
    );
}

/// The reset re-zeroes every stream the surfaces sim draws from — pinned DIRECTLY (each assertion
/// falsifies exactly one reset). `SurfaceSeq` (the deterministic patch ordinal) returns to 0;
/// `StandingState` (the per-victim rehit clocks + the previous-tick inside-set — a combatant
/// stands in the trail's first splat, which is painted at the caster's own spawn) returns to
/// default; `SpawnRng` (the emitter-jitter stream — the PRE-SURFACES determinism gap this task
/// closes) is reseeded to `seed_combat_rng(0)`'s derived `0x5EED_5EED` stream, so a scrub restart
/// draws identical emitter jitter every time.
#[test]
fn stage_reset_rezeroes_surface_and_spawn_streams() {
    use bevy::ecs::system::RunSystemOnce;
    use rand::{Rng, SeedableRng};

    let mut app = test_app();
    enter_skill_mode(&mut app);
    insert_surface(&mut app, frost_surface());
    let (rules, timeline) = frost_trail_skill();
    insert_skill(&mut app, "frost_trail", rules, timeline);
    open_skill(&mut app, "frost_trail");
    app.update();
    app.update();

    play(&mut app, 60);

    // Pre-reset: the streams are "dirty" from the run.
    assert!(app.world().resource::<SurfaceSeq>().0 > 0, "paints advanced SurfaceSeq");
    assert!(
        !app.world().resource::<StandingState>().inside_prev.is_empty(),
        "a combatant stands in a painted patch, so StandingState.inside_prev is populated"
    );
    // Nothing draws SpawnRng without an emitter, so perturb it by hand to make the reseed
    // observable (the whole point of the fix — an emitter run would perturb it for real).
    app.world_mut().resource_mut::<SpawnRng>().0.r#gen::<u64>();

    app.world_mut()
        .run_system_once(|mut reset: PreviewStageReset| reset.reset_stage())
        .expect("stage reset runs");

    assert_eq!(app.world().resource::<SurfaceSeq>().0, 0, "reset re-zeroes SurfaceSeq");
    let standing = app.world().resource::<StandingState>();
    assert!(
        standing.inside_prev.is_empty() && standing.next_due.is_empty(),
        "reset restores StandingState to default"
    );
    let got = app.world_mut().resource_mut::<SpawnRng>().0.r#gen::<u64>();
    let want = rand_chacha::ChaCha8Rng::seed_from_u64(0x5EED_5EED).r#gen::<u64>();
    assert_eq!(
        got, want,
        "reset reseeds SpawnRng to seed_combat_rng(0)'s derived 0x5EED_5EED stream (game parity)"
    );
}

// ---------------------------------------------------------------------------
// Surfaces (Task 3): every live patch renders a tinted decal — the preview attaches a
// `SurfacePatchVisual`-marked child (carrying a `ForwardDecal` in the real windowed editor) to
// each painted patch, plus an optional looping vfx child when the surface authors one.
// ---------------------------------------------------------------------------

/// SHIPPED ASSERTION: the `SurfacePatchVisual` marker child, NOT `ForwardDecal`. `ForwardDecal`'s
/// on-add hook reads the private `ForwardDecalMesh` resource that only `PbrPlugin`'s
/// `ForwardDecalPlugin` inserts; this `MinimalPlugins` harness has neither it nor the
/// `Assets<ForwardDecalMaterial<StandardMaterial>>` store, so `attach_patch_visuals` render-infra-
/// gates the decal component (it would panic here) and only the render-independent
/// `SurfacePatchVisual` marker child ships headlessly. The system is registered DIRECTLY here (not
/// via `PreviewSurfacesPlugin`, whose guarded `MaterialPlugin` add WOULD create the material store
/// and re-arm the panic path) — see `skill::preview::surfaces` for the full rationale.
#[test]
fn patches_get_decal_visual_children() {
    let mut app = test_app();
    enter_skill_mode(&mut app);
    insert_surface(&mut app, frost_surface());
    let (rules, timeline) = frost_trail_skill();
    insert_skill(&mut app, "frost_trail", rules, timeline);
    open_skill(&mut app, "frost_trail");
    app.add_systems(Update, attach_patch_visuals.run_if(in_state(EditorMode::Skill)));
    app.update();
    app.update();

    play(&mut app, 60);

    let patches = surface_patch_count(&mut app);
    assert!(
        patches > 0,
        "the frost trail should have painted patches to carry visuals, got {patches}"
    );

    // Every painted patch spawns exactly one `SurfacePatchVisual` decal child…
    let visuals: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<SurfacePatchVisual>>()
        .iter(app.world())
        .collect();
    assert_eq!(
        visuals.len(),
        patches,
        "each painted patch should spawn exactly one SurfacePatchVisual decal child \
         (got {} visuals for {} patches)",
        visuals.len(),
        patches
    );
    // …and each is a CHILD of the patch it decorates.
    for v in visuals {
        let parent = app
            .world()
            .get::<ChildOf>(v)
            .expect("a SurfacePatchVisual must be a child of its patch")
            .parent();
        assert!(
            app.world().get::<SurfacePatch>(parent).is_some(),
            "a SurfacePatchVisual's parent must be the SurfacePatch it decorates"
        );
    }
}

/// The optional looping vfx child: a surface whose `[visuals].vfx` names a registered `VfxLibrary`
/// preset gets a second child carrying a live `VfxSystem` (resolved through cosmetics' shared
/// `resolve_vfx_effect`, parented with NO lifetime — it loops for the patch's life and dies with
/// it). `VfxSystem` is headless-constructible (the cue-cosmetics tests spawn it the same way), so
/// this path is asserted directly on the `MinimalPlugins` harness.
#[test]
fn authored_surface_vfx_spawns_a_looping_vfx_child() {
    let mut app = test_app();
    enter_skill_mode(&mut app);
    app.world_mut()
        .resource_mut::<VfxLibrary>()
        .effects
        .insert("Frost Embers".to_string(), bevy_vfx::VfxSystem::default());
    let mut st = frost_surface();
    st.visuals = Some(SurfaceVisuals {
        decal: Some("textures/decal_splat.png".to_string()),
        color: Some([0.3, 0.6, 1.0, 0.7]),
        vfx: Some("Frost Embers".to_string()),
    });
    insert_surface(&mut app, st);
    let (rules, timeline) = frost_trail_skill();
    insert_skill(&mut app, "frost_trail", rules, timeline);
    open_skill(&mut app, "frost_trail");
    app.add_systems(Update, attach_patch_visuals.run_if(in_state(EditorMode::Skill)));
    app.update();
    app.update();

    play(&mut app, 60);

    let patches = surface_patch_count(&mut app);
    assert!(patches > 0, "the frost trail should have painted patches, got {patches}");

    // Count VfxSystem entities parented to a SurfacePatch (the surface's looping vfx child) —
    // filtering by parent so a future cue-authored cosmetic VfxSystem can never confuse this.
    let vfx_entities: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<bevy_vfx::VfxSystem>>()
        .iter(app.world())
        .collect();
    let surface_vfx = vfx_entities
        .iter()
        .filter(|&&fx| {
            app.world()
                .get::<ChildOf>(fx)
                .is_some_and(|c| app.world().get::<SurfacePatch>(c.parent()).is_some())
        })
        .count();
    assert!(
        surface_vfx > 0,
        "an authored surface [visuals].vfx should spawn a looping VfxSystem child parented to a \
         patch (found {surface_vfx} of {} VfxSystem entities under {patches} patches)",
        vfx_entities.len()
    );
}

// ---------------------------------------------------------------------------
// Surfaces (Task 5, the flagship): the stage paint tool. `StagedPaints` is session state the
// designer pre-paints via the palette; every `reset_stage` (Play, editor Reset, scrub restart)
// re-applies it AFTER the Task-2 clear, so a surface-GATED cast is testable in-editor and the
// scrubber stays honest (staged ground survives replay-from-t=0).
// ---------------------------------------------------------------------------

/// A `GroundPoint` cast GATED on a frost surface (spec §5.1): its `on_surface` requirement means
/// the aimed point must land on a frost patch, else the sim rejects (`Fizzle`). One trivial Static
/// `CastPoint` window so the timeline is valid — the test asserts on the GATE, not the window.
/// `snap: true, consume: true`: an accepted cast recenters on the matched patch AND spends it.
fn spire_probe_timeline() -> CastTimeline {
    CastTimeline {
        skill_id: "spire_probe".to_string(),
        phase_durations: PhaseDurations { windup: 0.05, active: 0.1, recovery: 0.05 },
        collision_windows: vec![CollisionWindow {
            id: "probe".to_string(),
            spawn: WindowSpawn::Scheduled { phase: WindowPhase::Active, offset: 0.0 },
            anchor: WindowAnchor::CastPoint,
            anchor_offset: Vec3::ZERO,
            strikes: true,
            active_duration: 0.2,
            shape: CollisionShape::Sphere { radius: 1.0 },
            motion: VolumeMotion::Static,
            motion_direction: Default::default(),
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::OncePerTarget,
            rehit_interval: None,
            emitter: None,
            paints: None,
        }],
        acquisition: Acquisition::GroundPoint {
            range: 60.0,
            fallback: AcqFallback::Fizzle,
            on_surface: Some(SurfaceRequirement {
                surface: "frost".to_string(),
                snap: true,
                consume: true,
            }),
        },
        vfx_cues: Default::default(),
        chain_radius: 6.0,
        chargeable: false,
        max_hold: 1.0,
        cues: Default::default(),
        charge_cues: Vec::new(),
    }
}

fn spire_probe_rules() -> Skill {
    Skill {
        id: "spire_probe".to_string(),
        name: "Spire Probe".to_string(),
        delivery: Delivery::Instant,
        damage: DamageConfig {
            base_damages: vec![BaseDamage::new(stat_core::DamageType::Fire, 5.0, 5.0)],
            weapon_effectiveness: 0.0,
            damage_effectiveness: 1.0,
            ..DamageConfig::default()
        },
        ..Default::default()
    }
}

/// THE FLAGSHIP: a surface-gated cast that is impossible to test without staged ground. WITHOUT a
/// staged frost patch the `on_surface` gate rejects the cast; WITH one staged at the stage's
/// ground-aim marker (the SAME point `resolve_stage_acquisition` resolves a `GroundPoint` to —
/// `ground_marker`), Play re-applies it through `reset_stage` and the cast BEGINS and CONSUMES it.
/// A re-reset re-stages the consumed patch — staged state is durable across replays.
#[test]
fn staged_frost_makes_a_gated_cast_succeed() {
    use bevy::ecs::system::RunSystemOnce;

    let mut app = test_app();
    enter_skill_mode(&mut app);
    insert_surface(&mut app, frost_surface());
    insert_skill(&mut app, "spire_probe", spire_probe_rules(), spire_probe_timeline());
    open_skill(&mut app, "spire_probe");
    app.update();
    app.update();

    // WITHOUT staging: the GroundPoint cast is gated on a frost patch that isn't there. The stage
    // resolves the aim to the ground marker (in range), so the sim-side `on_surface` check is what
    // rejects — a paid Fizzle.
    play(&mut app, 20);
    {
        let rec = app.world().resource::<EventRecorder>();
        assert!(
            rec.cast_rejected.iter().any(|r| r.skill_id == "spire_probe"),
            "with no frost staged, the on_surface gate must REJECT the gated cast — rejected: {:?}",
            rec.cast_rejected
                .iter()
                .map(|r| (r.skill_id.clone(), format!("{:?}", r.reason)))
                .collect::<Vec<_>>()
        );
        assert!(
            !rec.cast_began.iter().any(|c| c.skill_id == "spire_probe"),
            "the gated cast must NOT begin without the required frost surface"
        );
    }
    assert_eq!(surface_patch_count(&mut app), 0, "no patch exists without staging");

    // Clear the recorder so the WITH-staging assertions read only this phase's events.
    *app.world_mut().resource_mut::<EventRecorder>() = EventRecorder::default();

    // WITH a frost paint staged AT the stage's ground-aim marker (`ground_marker` — exactly where a
    // `GroundPoint` stage cast aims): Play funnels through `reset_stage`, which re-applies the
    // staged patch BEFORE the cast pends, so the gate now matches.
    app.world_mut().resource_mut::<StagedPaints>().0.push(StagedPaint {
        surface: "frost".to_string(),
        position: ground_marker(),
    });
    play(&mut app, 20);
    {
        let rec = app.world().resource::<EventRecorder>();
        assert!(
            rec.cast_began.iter().any(|c| c.skill_id == "spire_probe"),
            "a staged frost patch under the aim marker must let the gated cast BEGIN — began: {:?}, \
             rejected: {:?}",
            rec.cast_began.iter().map(|c| c.skill_id.clone()).collect::<Vec<_>>(),
            rec.cast_rejected
                .iter()
                .map(|r| (r.skill_id.clone(), format!("{:?}", r.reason)))
                .collect::<Vec<_>>()
        );
    }
    // `consume: true`: the accepted cast spent the staged patch (back to bare ground).
    assert_eq!(
        surface_patch_count(&mut app),
        0,
        "the gated cast (consume: true) must have consumed the staged frost patch"
    );

    // Durable: a re-reset re-applies the staged patch — staged state survives being consumed, so
    // the next replay starts from the same ground.
    app.world_mut()
        .run_system_once(|mut reset: PreviewStageReset| reset.reset_stage())
        .expect("stage reset runs");
    assert_eq!(
        surface_patch_count(&mut app),
        1,
        "a re-reset must re-stage the consumed frost patch (staged state is durable across replays)"
    );
}

/// Scrub restart re-sims from t=0 through `reset_stage`; a staged paint must be re-applied on EVERY
/// reset, exactly once — the reset clears first, so no duplication accrues across restarts.
#[test]
fn staged_paints_survive_scrub_restart() {
    use bevy::ecs::system::RunSystemOnce;

    let mut app = test_app();
    enter_skill_mode(&mut app);
    insert_surface(&mut app, frost_surface());
    // Settle so the caster (the re-applied paint's owner) exists before the first reset.
    app.update();
    app.update();

    app.world_mut().resource_mut::<StagedPaints>().0.push(StagedPaint {
        surface: "frost".to_string(),
        position: ground_marker(),
    });

    app.world_mut()
        .run_system_once(|mut reset: PreviewStageReset| reset.reset_stage())
        .expect("stage reset runs");
    assert_eq!(
        surface_patch_count(&mut app),
        1,
        "the first reset re-applies exactly one staged patch"
    );

    app.world_mut()
        .run_system_once(|mut reset: PreviewStageReset| reset.reset_stage())
        .expect("stage reset runs");
    assert_eq!(
        surface_patch_count(&mut app),
        1,
        "a second reset clears THEN re-applies — still exactly one patch, not two (re-applied \
         after the clear, never duplicated)"
    );
}

//! Task 11 headless integration tests for the sim-backed synchronous scrub — ported/adapted
//! from `arena_editor`'s `tests/scrub_preview.rs` (obelisk-arena @ `f6472e4`), plus a NEW
//! flagship exercising the strip's dynamic trailing extent (v2's own contribution: v1's strip
//! span was fully resolvable from authored data; v2's schema deleted authored window chaining,
//! so a rules-triggered sub-cast is invisible to the base span and must be discovered live —
//! see `crate::skill::preview::scrub`'s module doc comment).
//!
//! Verified on a from-scratch headless app (`MinimalPlugins` + the preview's own plugins, same
//! harness shape as `tests/skill_preview.rs`), driving `ScrubSim.target` directly and stepping —
//! headless, no window, no mouse.

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
use obelisk_bevy::prelude::Hitbox;
use obelisk_bevy::testkit::{EventRecorder, EventRecorderPlugin};
use stat_core::{BaseDamage, DamageConfig, Delivery, Skill, SkillCondition};

use bevy_modal_editor::editor::EditorMode;
use bevy_modal_editor::skill::library::{SkillEntry, SkillLibrary};
use bevy_modal_editor::skill::panel::strip;
use bevy_modal_editor::skill::preview::{
    cosmetics::PreviewCosmeticsPlugin,
    rig::PreviewRigPlugin,
    scrub::{MarkerKind, PreviewScrubPlugin, ScrubMarkers, ScrubMode, ScrubSim},
    sockets::{index_rig_sockets, RigSockets},
    stage::{PreviewControllerPlugin, PreviewSimPlugin},
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
        .init_resource::<bevy_modal_editor::effects::EffectLibrary>()
        .init_resource::<VfxLibrary>()
        .init_resource::<AnimationLibrary>()
        .init_asset::<AnimationGraph>()
        .add_plugins(PreviewSimPlugin)
        .add_plugins(PreviewControllerPlugin)
        .add_plugins(PreviewRigPlugin)
        .add_plugins(PreviewCosmeticsPlugin)
        // The scrub machinery under test — wired exactly as `SkillPreviewPlugin` wires it.
        .add_plugins(PreviewScrubPlugin)
        .init_resource::<RigSockets>()
        .add_systems(Update, index_rig_sockets);
    app.finish();
    app.cleanup();
    app
}

/// Drive `EditorMode` into `Skill` and let the transition apply — the stage (and `drive_scrub`,
/// Task 11) only run there (mirrors `tests/skill_preview.rs`'s `enter_skill_mode`).
fn enter_skill_mode(app: &mut App) {
    app.world_mut().resource_mut::<NextState<EditorMode>>().set(EditorMode::Skill);
    app.update();
}

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

fn step(app: &mut App, n: usize) {
    for _ in 0..n {
        app.update();
    }
}

fn set_target(app: &mut App, t: f32) {
    app.world_mut().resource_mut::<ScrubSim>().target = Some(t);
}

// ---------------------------------------------------------------------------
// Ported determinism suite (arena_editor's `scrub_preview.rs`), on `SkillArchetype::Projectile`:
// windup 0.2s, "bolt" opens at 0.2s, crosses the 8 m duel gap at 25 u/s (~0.32s flight) — the
// true hit lands ~0.52s.
// ---------------------------------------------------------------------------

#[test]
fn seek_freezes_the_sim_at_the_target_before_the_hit() {
    let mut app = test_app();
    enter_skill_mode(&mut app);
    let (rules, timeline) = SkillArchetype::Projectile.build("bolt");
    insert_skill(&mut app, "bolt", rules, timeline);
    open_skill(&mut app, "bolt");
    step(&mut app, 3); // stage spawns + registry sync

    // Seek to mid-flight: after the window opens (0.2) but before the true hit (~0.52).
    set_target(&mut app, 0.35);
    step(&mut app, 10); // the seek itself completes synchronously within one `drive_scrub` call

    let scrub = app.world().resource::<ScrubSim>();
    assert_eq!(scrub.mode, ScrubMode::Frozen, "seek must land in Frozen");
    assert!(
        (scrub.clock - 0.35).abs() < 0.1,
        "frozen near the target: {}",
        scrub.clock
    );
    // The bolt hitbox EXISTS, frozen mid-flight, and no damage has resolved yet.
    let hitboxes = app.world_mut().query::<&Hitbox>().iter(app.world()).count();
    assert_eq!(hitboxes, 1, "bolt frozen mid-flight");
    let rec = app.world().resource::<EventRecorder>();
    assert!(
        rec.damage_resolved.is_empty(),
        "no damage before the true hit moment"
    );

    // FROZEN means frozen: many frames later the clock hasn't moved.
    let clock_before = app.world().resource::<ScrubSim>().clock;
    step(&mut app, 30);
    let clock_after = app.world().resource::<ScrubSim>().clock;
    assert_eq!(clock_before, clock_after, "the sim is paused at the instant");
}

#[test]
fn seeking_past_the_hit_resolves_real_damage_and_backward_replays() {
    let mut app = test_app();
    enter_skill_mode(&mut app);
    let (rules, timeline) = SkillArchetype::Projectile.build("bolt");
    insert_skill(&mut app, "bolt", rules, timeline);
    open_skill(&mut app, "bolt");
    step(&mut app, 3);

    // Past the whole flight: the hit resolves for real.
    set_target(&mut app, 0.9);
    step(&mut app, 10);
    let first = {
        let rec = app.world().resource::<EventRecorder>();
        let dmg: Vec<f64> = rec.damage_resolved.iter().map(|d| d.total_damage).collect();
        assert!(!dmg.is_empty(), "seeking past the hit resolves REAL damage");
        dmg
    };

    // Backward drag: restart + reseek — deterministic, so the same damage resolves again.
    set_target(&mut app, 0.8);
    step(&mut app, 10);
    let rec = app.world().resource::<EventRecorder>();
    let total = rec.damage_resolved.len();
    assert_eq!(
        total,
        first.len() * 2,
        "the replayed run resolves the same number of hits"
    );
    let second: Vec<f64> = rec
        .damage_resolved
        .iter()
        .skip(first.len())
        .map(|d| d.total_damage)
        .collect();
    assert_eq!(first, second, "same seed, identical replay");
}

#[test]
fn replay_runs_to_the_end_and_freezes() {
    let mut app = test_app();
    enter_skill_mode(&mut app);
    let (rules, timeline) = SkillArchetype::Projectile.build("bolt");
    let base = strip::base_span(&timeline);
    insert_skill(&mut app, "bolt", rules, timeline);
    open_skill(&mut app, "bolt");
    step(&mut app, 3);

    app.world_mut().resource_mut::<ScrubSim>().replay_requested = true;
    // base span ~2.2s -> ~132 fixed ticks at 1x; run enough frames (each `app.update()` advances
    // ~1 fixed tick under `TimeUpdateStrategy::ManualDuration`).
    step(&mut app, 220);

    let scrub = app.world().resource::<ScrubSim>();
    assert_eq!(scrub.mode, ScrubMode::Frozen, "replay freezes at the end");
    assert!(
        scrub.clock >= base - 0.05,
        "ran the whole base span ({base}): {}",
        scrub.clock
    );
    let rec = app.world().resource::<EventRecorder>();
    assert!(!rec.damage_resolved.is_empty(), "the replayed cast hit for real");
}

// ---------------------------------------------------------------------------
// The flagship: a fireball-pair (bolt free-falls into the world, triggers an explosion via
// OnImpact) — seeking past the impact must show the strip's DYNAMIC END extend past the base
// span (the base span has zero visibility into the triggered "fireball_explosion" sub-cast —
// it's a wholly separate skill/timeline) AND the explosion's own "blast" window spawned —
// visible IN THE SCRUB, not merely resolved-and-gone.
//
// Numbers are chosen so the gap is comfortable, not tick-perfect-fragile: the explosion's OWN
// `windup` (0.5s, ticked on the `TriggeredExec`'s independent virtual clock, starting the
// instant the bolt hits the world) delays "blast" well past `fireball`'s own base span (0.6s),
// regardless of the bolt window's authored `active_duration` margin over its ~0.15s real fall.
// ---------------------------------------------------------------------------

fn fireball_timeline() -> CastTimeline {
    CastTimeline {
        skill_id: "fireball".to_string(),
        phase_durations: PhaseDurations { windup: 0.1, active: 0.1, recovery: 0.1 },
        collision_windows: vec![CollisionWindow {
            id: "bolt".to_string(),
            spawn: WindowSpawn::Scheduled { phase: WindowPhase::Active, offset: 0.0 },
            anchor: WindowAnchor::CastPoint,
            anchor_offset: Vec3::new(0.0, 3.0, 0.0),
            strikes: false,
            active_duration: 0.5,
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

fn fireball_explosion_timeline() -> CastTimeline {
    CastTimeline {
        skill_id: "fireball_explosion".to_string(),
        // The 0.5s windup is what gives the flagship its comfortable margin — see this section's
        // doc comment.
        phase_durations: PhaseDurations { windup: 0.5, active: 0.3, recovery: 0.0 },
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
fn seek_past_impact_shows_the_explosion() {
    let mut app = test_app();
    enter_skill_mode(&mut app);
    let base = strip::base_span(&fireball_timeline());
    insert_skill(&mut app, "fireball", fireball_rules(), fireball_timeline());
    insert_skill(
        &mut app,
        "fireball_explosion",
        fireball_explosion_rules(),
        fireball_explosion_timeline(),
    );
    open_skill(&mut app, "fireball");
    step(&mut app, 3);

    // Past the base span (0.6s: windup .1 + "bolt"'s own .1+.5). The bolt spawns at .1s and
    // falls (GROUND_Y + the 3-unit offset) at 20 u/s (~0.18s), hits ~0.28s, and the OnImpact
    // trigger fires ~1-2 ticks later (~0.3s); the explosion's own 0.5s windup then delays
    // "blast" to ~0.8s. 1.0s lands comfortably inside "blast"'s own active window (~0.8-1.1s).
    set_target(&mut app, 1.0);
    step(&mut app, 10);

    let scrub = app.world().resource::<ScrubSim>();
    assert_eq!(scrub.mode, ScrubMode::Frozen, "seek must land in Frozen");
    assert!(
        scrub.dynamic_end > base + 0.05,
        "the strip must extend past the base span ({base}) while the explosion's blast hitbox \
         still lives: dynamic_end={}",
        scrub.dynamic_end
    );

    // The explosion resolved damage IN THE PREVIEW SIM.
    let rec = app.world().resource::<EventRecorder>();
    let ids: Vec<&str> = rec.damage_resolved.iter().map(|d| d.skill_id.as_str()).collect();
    assert!(
        ids.contains(&"fireball_explosion"),
        "the OnImpact-triggered explosion must resolve damage during the scrub — got {ids:?}"
    );

    // The explosion's OWN "blast" window is still alive at the frozen instant — the sub-cast is
    // VISIBLE in the scrub, not just resolved-and-gone.
    let blast_alive = app
        .world_mut()
        .query::<&Hitbox>()
        .iter(app.world())
        .any(|h| h.window_id == "blast");
    assert!(
        blast_alive,
        "the explosion's blast hitbox should still be alive, frozen, at the scrubbed instant"
    );

    // Event markers recorded the cascade: the bolt's own window opening/ending, and the
    // explosion's "blast" window opening. NOTE: obelisk-bevy does not fire `TriggerFired` for
    // this specific lifecycle path (`end_hitboxes`'s OnImpact/OnExpire evaluation calls
    // `execute_skill_timeline` directly) — only for on-hit-confirmed and effect-condition
    // triggers (see `event_markers_record_hit_and_trigger_moments` below, which exercises the
    // Trigger marker via a path that DOES fire it). So this flagship checks WindowOpened/Ended,
    // not Trigger.
    let markers = app.world().resource::<ScrubMarkers>();
    assert!(
        markers.0.iter().any(|m| m.kind == MarkerKind::WindowOpened && m.label == "bolt"),
        "a WindowOpened marker should record the bolt window: {:?}",
        markers.0
    );
    assert!(
        markers
            .0
            .iter()
            .any(|m| m.kind == MarkerKind::WindowOpened && m.label == "blast"),
        "a WindowOpened marker should record the blast window: {:?}",
        markers.0
    );
    assert!(
        markers.0.iter().any(|m| m.kind == MarkerKind::Ended && m.label == "bolt"),
        "an Ended marker should record the bolt window's HitWorld end: {:?}",
        markers.0
    );
}

/// The Trigger marker (`MarkerKind::Trigger`), exercised via a path that DOES fire
/// `TriggerFired`. obelisk-bevy's `partition_conditions` (`combat/system.rs`) routes a
/// `SkillCondition` to one of two disjoint paths depending SOLELY on whether `trigger_skill`
/// has a REGISTERED `CastTimeline` handle: a spatial `execute_skill_timeline` sub-cast (the
/// flagship above, and every lifecycle OnImpact/OnExpire trigger) never fires `TriggerFired`;
/// an INLINE stat_core-resolved secondary packet DOES. This editor's `sync_sim_registries`
/// always 1:1-syncs `SkillRegistry` (rules) and `CastTimelineHandles` (timelines) together from
/// `SkillLibrary`, so an ordinary authored skill can never reach the inline path — this test
/// deliberately registers "spark"'s rules directly into `SkillRegistry`, bypassing
/// `SkillLibrary`/`CastTimelineHandles` entirely, to reach it.
#[test]
fn event_markers_record_hit_and_trigger_moments() {
    let mut app = test_app();
    enter_skill_mode(&mut app);
    let (mut rules, timeline) = SkillArchetype::Projectile.build("bolt");
    rules.conditions.push(SkillCondition {
        trigger_skill: "spark".to_string(),
        additional: true,
        condition: loot_core::types::TriggerCondition::Always,
    });
    insert_skill(&mut app, "bolt", rules, timeline);
    open_skill(&mut app, "bolt");
    step(&mut app, 3);

    // "spark" is known to `SkillRegistry` (so stat_core can resolve it as an inline secondary
    // packet) but deliberately has NO `CastTimelineHandles` entry — see this test's doc comment.
    // Inserted AFTER `SkillLibrary` has settled so no later `sync_sim_registries` resync (which
    // rebuilds `SkillRegistry` purely from `SkillLibrary`) wipes it back out.
    let (spark_rules, _) = SkillArchetype::Strike.build("spark");
    app.world_mut()
        .resource_mut::<obelisk_bevy::prelude::SkillRegistry>()
        .0
        .insert("spark".to_string(), spark_rules);

    // Past the true hit (~0.52s).
    set_target(&mut app, 0.9);
    step(&mut app, 10);

    let markers = app.world().resource::<ScrubMarkers>();
    assert!(
        markers.0.iter().any(|m| m.kind == MarkerKind::Hit),
        "a Hit marker should record the confirmed hit: {:?}",
        markers.0
    );
    assert!(
        markers.0.iter().any(|m| m.kind == MarkerKind::Trigger && m.label == "spark"),
        "a Trigger marker should record the on-hit 'Always' condition firing 'spark': {:?}",
        markers.0
    );
}

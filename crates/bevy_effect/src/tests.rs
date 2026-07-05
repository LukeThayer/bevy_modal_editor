//! Characterization tests for the effect runtime (`runtime.rs` + `data.rs`).
//!
//! These pin the observable behavior of `advance_effects` / `advance_tweens`
//! as captured in Task 2, *before* the extraction of the runtime out of
//! `bevy_modal_editor` into this crate — they are the extraction's safety net
//! and moved here with the code. Assertions are unchanged from the pre-move
//! suite; only import paths differ. If a future change makes one of these
//! fail, that's a signal the runtime's contract shifted, not necessarily a
//! bug.
//!
//! Lives inline (rather than under a `tests/` dir) because `advance_effects`
//! and `advance_tweens` are `pub(crate)` — the suite drives the runtime
//! directly without going through the full `EffectPlugin`. The editor's
//! `tests/effect_marker_scene_fixture.rs` (which pins the reflected scene
//! type path) stays editor-side, where `build_editor_scene` lives.
//!
//! The `spawn_effect_recursion_stops_at_depth_cap` test at the bottom is NOT
//! part of the characterization contract — it covers the depth guard added
//! during the extraction.

use super::*;
use crate::runtime::{advance_effects, advance_tweens, MAX_SPAWN_EFFECT_DEPTH};
use bevy::prelude::*;
use bevy::app::TaskPoolPlugin;
use bevy::asset::{AssetApp, AssetPlugin};
use bevy::time::{TimePlugin, TimeUpdateStrategy};
use bevy_vfx::{VfxLibrary, VfxSystem};
use std::time::Duration;

/// Fixed per-frame delta used by all tests (10 fps keeps the arithmetic
/// readable and comfortably clear of f32 rounding noise near trigger
/// thresholds).
const DT: f32 = 0.1;

/// Build a minimal headless App wired with just the effect runtime's own
/// systems (not the full `EffectPlugin` — see module docs) and manual time
/// stepping.
fn test_app() -> App {
    let mut app = App::new();
    app.add_plugins((TaskPoolPlugin::default(), TimePlugin, AssetPlugin::default()))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(DT)))
        .init_resource::<VfxLibrary>()
        .init_resource::<EffectLibrary>()
        .add_systems(Update, (advance_effects, advance_tweens).chain());

    // bevy_time's very first tick always reports a zero delta while it
    // establishes its baseline `Instant` (see `Real::update_with_instant`);
    // burn that frame here so every test can reason about elapsed time as
    // `frame_index * DT` from the first *real* `tick()`.
    app.update();
    app
}

/// Spawn an entity carrying an `EffectMarker` + a `Playing` `EffectPlayback`,
/// skipping `rebuild_effect_playback` (not added to `test_app`) since we
/// attach the playback state directly.
fn spawn_effect(app: &mut App, steps: Vec<EffectStep>) -> Entity {
    app.world_mut()
        .spawn((
            EffectMarker { steps },
            EffectPlayback {
                state: PlaybackState::Playing,
                ..default()
            },
            Transform::default(),
            GlobalTransform::default(),
        ))
        .id()
}

fn playback<'w>(app: &'w App, entity: Entity) -> &'w EffectPlayback {
    app.world()
        .get::<EffectPlayback>(entity)
        .expect("entity should still have an EffectPlayback")
}

fn tick(app: &mut App, frames: u32) {
    for _ in 0..frames {
        app.update();
    }
}

fn spawn_primitive(tag: &str) -> EffectAction {
    EffectAction::SpawnPrimitive {
        tag: tag.into(),
        shape: PrimitiveShape::Cube,
        offset: Vec3::ZERO,
        material: None,
        rigid_body: None,
    }
}

// ---------------------------------------------------------------------------
// Trigger coverage
// ---------------------------------------------------------------------------

#[test]
fn at_time_fires_once_at_t() {
    let mut app = test_app();
    let entity = spawn_effect(
        &mut app,
        vec![EffectStep {
            name: "boom".into(),
            trigger: EffectTrigger::AtTime(0.25),
            actions: vec![spawn_primitive("core")],
        }],
    );

    tick(&mut app, 2); // elapsed = 0.2, not yet
    assert!(!playback(&app, entity).fired_steps.contains(&0));
    assert!(playback(&app, entity).spawned.is_empty());

    tick(&mut app, 1); // elapsed = 0.3 >= 0.25, fires
    assert!(playback(&app, entity).fired_steps.contains(&0));
    assert_eq!(playback(&app, entity).spawned.len(), 1);
    let spawned = *playback(&app, entity).spawned.get("core").unwrap();

    tick(&mut app, 5); // stays fired — no re-spawn, no duplicate entity
    assert_eq!(playback(&app, entity).spawned.len(), 1);
    assert_eq!(playback(&app, entity).spawned.get("core").copied(), Some(spawned));
}

#[test]
fn after_rule_chains_off_named_rule_with_delay() {
    let mut app = test_app();
    let entity = spawn_effect(
        &mut app,
        vec![
            EffectStep {
                name: "start".into(),
                trigger: EffectTrigger::AtTime(0.0),
                actions: vec![EffectAction::EmitEvent("noop".into())],
            },
            EffectStep {
                name: "chained".into(),
                trigger: EffectTrigger::AfterRule {
                    source_rule: "start".into(),
                    delay: 0.3,
                },
                actions: vec![spawn_primitive("child")],
            },
        ],
    );

    tick(&mut app, 1); // elapsed = 0.1: "start" fires (0.1 >= 0.0)
    assert!(playback(&app, entity).rule_fire_times.contains_key("start"));
    assert!(playback(&app, entity).spawned.is_empty());

    tick(&mut app, 2); // elapsed = 0.3: need fire_time(~0.1) + 0.3 = ~0.4, not yet
    assert!(playback(&app, entity).spawned.is_empty());

    tick(&mut app, 1); // elapsed = 0.4: chained fires
    assert_eq!(playback(&app, entity).spawned.len(), 1);
}

#[test]
fn repeating_interval_stops_at_max_count() {
    let mut app = test_app();
    let entity = spawn_effect(
        &mut app,
        vec![EffectStep {
            name: "tick".into(),
            trigger: EffectTrigger::RepeatingInterval {
                interval: 0.2,
                max_count: Some(3),
            },
            actions: vec![EffectAction::EmitEvent("tick".into())],
        }],
    );

    tick(&mut app, 20); // plenty of frames for the interval to exhaust max_count
    assert_eq!(playback(&app, entity).repeat_counts.get("tick").copied(), Some(3));
}

#[test]
fn on_spawn_fires_immediately_then_never_again() {
    let mut app = test_app();
    let entity = spawn_effect(
        &mut app,
        vec![EffectStep {
            name: "spawn".into(),
            trigger: EffectTrigger::OnSpawn,
            actions: vec![spawn_primitive("core")],
        }],
    );

    // Today's `OnSpawn` heuristic is `elapsed < dt * 2.0`, checked against the
    // *current frame's* elapsed/dt — so it fires on the first frame with a
    // nonzero delta (the second real `app.update()`, since the very first is
    // always dt=0), not on frame 0 itself.
    tick(&mut app, 1);
    assert!(playback(&app, entity).fired_steps.contains(&0));
    assert_eq!(playback(&app, entity).spawned.len(), 1);

    tick(&mut app, 10);
    assert_eq!(playback(&app, entity).spawned.len(), 1, "OnSpawn must not re-fire");
}

#[test]
fn after_idle_timeout_re_arms_itself_once_seeded_by_a_prior_rule() {
    // `AfterIdleTimeout` requires `last_fire_time > 0.0` before it can ever
    // fire — it cannot be the *only* rule in an effect. "seed" provides that
    // baseline. Once armed, firing resets `last_fire_time`, so it behaves as
    // a repeating "nothing else has happened for `timeout` seconds" pulse
    // rather than a one-shot idle detector — characterizing that here.
    let mut app = test_app();
    let entity = spawn_effect(
        &mut app,
        vec![
            EffectStep {
                name: "seed".into(),
                trigger: EffectTrigger::AtTime(0.0),
                actions: vec![EffectAction::EmitEvent("seed".into())],
            },
            EffectStep {
                name: "idle".into(),
                trigger: EffectTrigger::AfterIdleTimeout { timeout: 0.3 },
                actions: vec![EffectAction::EmitEvent("idle".into())],
            },
        ],
    );

    tick(&mut app, 4); // elapsed = 0.4: seed fired at ~0.1, 0.4 - 0.1 >= 0.3 -> first idle fire
    assert_eq!(playback(&app, entity).repeat_counts.get("idle").copied(), Some(1));

    tick(&mut app, 3); // elapsed = 0.7: 0.7 - 0.4 >= 0.3 -> fires again
    assert_eq!(playback(&app, entity).repeat_counts.get("idle").copied(), Some(2));
}

#[test]
fn on_collision_fires_when_collision_tag_is_present() {
    // `detect_effect_collisions` (which populates `collision_tags` from
    // Avian3D `Collisions`) isn't wired into `test_app` — it needs a running
    // `PhysicsPlugins`, which is out of scope for this runtime suite. We
    // characterize `advance_effects`' consumption of the field directly by
    // poking it, the same way the collision-detection system does.
    let mut app = test_app();
    let entity = spawn_effect(
        &mut app,
        vec![EffectStep {
            name: "hit".into(),
            trigger: EffectTrigger::OnCollision { tag: "proj".into() },
            actions: vec![EffectAction::EmitEvent("hit".into())],
        }],
    );

    tick(&mut app, 3);
    assert!(!playback(&app, entity).fired_steps.contains(&0));

    app.world_mut()
        .get_mut::<EffectPlayback>(entity)
        .unwrap()
        .collision_tags
        .insert("proj".into());
    tick(&mut app, 1);
    assert!(playback(&app, entity).fired_steps.contains(&0));
}

#[test]
fn on_effect_event_never_fires_today_pending_events_cleared_same_frame() {
    // Characterizes a real quirk, not a hypothetical: `advance_effects`
    // extends `pending_events` with this frame's `EmitEvent` output *after*
    // the per-step loop, then — still within the very same call — its
    // trailing cleanup loop clears `pending_events` again before the next
    // frame's per-step loop ever runs. So an `OnEffectEvent` trigger driven
    // by a sibling step's `EmitEvent` can never observe the event, in the
    // same frame or any later one. This looks like an ordering bug, but
    // per the task brief we characterize TODAY's behavior rather than fix
    // it — Task 3 should decide deliberately whether to preserve or repair
    // this when it moves the runtime.
    let mut app = test_app();
    let entity = spawn_effect(
        &mut app,
        vec![
            EffectStep {
                name: "trigger".into(),
                trigger: EffectTrigger::AtTime(0.1),
                actions: vec![EffectAction::EmitEvent("boom".into())],
            },
            EffectStep {
                name: "reactor".into(),
                trigger: EffectTrigger::OnEffectEvent("boom".into()),
                actions: vec![spawn_primitive("debris")],
            },
        ],
    );

    tick(&mut app, 20); // generous margin; if this starts failing, the quirk was fixed
    assert!(
        playback(&app, entity).fired_steps.contains(&0),
        "the emitting step should still fire on its own AtTime trigger"
    );
    assert!(
        playback(&app, entity).spawned.is_empty(),
        "OnEffectEvent should never observe an EmitEvent under today's same-frame-clear ordering"
    );
}

// ---------------------------------------------------------------------------
// Action coverage
// ---------------------------------------------------------------------------

#[test]
fn spawn_particle_resolves_preset_by_name_from_vfx_library() {
    let mut app = test_app();
    let sentinel = VfxSystem {
        duration: 7.5,
        looping: false,
        ..default()
    };
    app.world_mut()
        .resource_mut::<VfxLibrary>()
        .effects
        .insert("Fire".into(), sentinel.clone());

    let entity = spawn_effect(
        &mut app,
        vec![EffectStep {
            name: "spark".into(),
            trigger: EffectTrigger::OnSpawn,
            actions: vec![EffectAction::SpawnParticle {
                tag: "fx".into(),
                preset: "Fire".into(),
                at: SpawnLocation::Offset(Vec3::ZERO),
            }],
        }],
    );

    tick(&mut app, 1);
    let child = *playback(&app, entity).spawned.get("fx").expect("fx should have spawned");
    let system = app
        .world()
        .get::<VfxSystem>(child)
        .expect("spawned entity should carry the resolved VfxSystem");
    assert_eq!(*system, sentinel);
}

#[test]
fn spawn_particle_unknown_preset_falls_back_to_vfx_system_default() {
    // Today's semantics: `vfx_library.effects.get(preset).cloned().unwrap_or_default()`
    // silently falls back to `VfxSystem::default()` for an unrecognized preset
    // name rather than erroring, warning loudly, or skipping the spawn.
    // Characterized here, not fixed.
    let mut app = test_app();

    let entity = spawn_effect(
        &mut app,
        vec![EffectStep {
            name: "spark".into(),
            trigger: EffectTrigger::OnSpawn,
            actions: vec![EffectAction::SpawnParticle {
                tag: "fx".into(),
                preset: "DoesNotExist".into(),
                at: SpawnLocation::Offset(Vec3::ZERO),
            }],
        }],
    );

    tick(&mut app, 1);
    let child = *playback(&app, entity)
        .spawned
        .get("fx")
        .expect("fx should still spawn even with an unknown preset name");
    let system = app
        .world()
        .get::<VfxSystem>(child)
        .expect("spawned entity should carry a VfxSystem");
    assert_eq!(*system, VfxSystem::default());
}

#[test]
fn spawn_primitive_creates_a_tagged_effect_child() {
    let mut app = test_app();
    let entity = spawn_effect(
        &mut app,
        vec![EffectStep {
            name: "spawn".into(),
            trigger: EffectTrigger::OnSpawn,
            actions: vec![spawn_primitive("core")],
        }],
    );

    tick(&mut app, 1);
    let child = *playback(&app, entity).spawned.get("core").expect("core should have spawned");

    let world = app.world();
    assert!(world.get::<Mesh3d>(child).is_some());
    assert!(world.get::<MeshMaterial3d<StandardMaterial>>(child).is_some());
    let effect_child = world.get::<EffectChild>(child).expect("spawned entity should be tagged as an EffectChild");
    assert_eq!(effect_child.tag, "core");
    assert_eq!(effect_child.effect_entity, entity);
}

#[test]
fn despawn_removes_the_tagged_entity() {
    let mut app = test_app();
    let entity = spawn_effect(
        &mut app,
        vec![
            EffectStep {
                name: "spawn".into(),
                trigger: EffectTrigger::OnSpawn,
                actions: vec![spawn_primitive("core")],
            },
            EffectStep {
                name: "remove".into(),
                trigger: EffectTrigger::AtTime(0.25),
                actions: vec![EffectAction::Despawn { tag: "core".into() }],
            },
        ],
    );

    tick(&mut app, 1);
    let child = *playback(&app, entity).spawned.get("core").expect("core should have spawned");
    assert!(app.world().get_entity(child).is_ok());

    tick(&mut app, 3); // elapsed = 0.4: despawn step fires
    assert!(playback(&app, entity).spawned.get("core").is_none());
    assert!(app.world().get_entity(child).is_err(), "despawned entity should no longer exist");
}

#[test]
fn nested_spawn_effect_one_level_runs_its_own_steps() {
    // The child effect gets its own `OnSpawn` step that spawns a primitive
    // tagged "inner" as a child of *itself*, not of the parent — verifying
    // one full level of nesting is live, not just cloned data.
    let mut app = test_app();
    app.world_mut().resource_mut::<EffectLibrary>().effects.insert(
        "ChildFx".into(),
        EffectMarker {
            steps: vec![EffectStep {
                name: "inner-spawn".into(),
                trigger: EffectTrigger::OnSpawn,
                actions: vec![spawn_primitive("inner")],
            }],
        },
    );

    let root = spawn_effect(
        &mut app,
        vec![EffectStep {
            name: "spawn-child-fx".into(),
            trigger: EffectTrigger::OnSpawn,
            actions: vec![EffectAction::SpawnEffect {
                tag: "childfx".into(),
                preset: "ChildFx".into(),
                at: SpawnLocation::Offset(Vec3::ZERO),
                inherit_velocity: false,
            }],
        }],
    );

    tick(&mut app, 1); // root's OnSpawn fires: spawns the child effect entity
    let child_fx_entity = *playback(&app, root)
        .spawned
        .get("childfx")
        .expect("childfx should have spawned");
    assert!(app.world().get::<EffectMarker>(child_fx_entity).is_some());
    assert_eq!(
        app.world().get::<EffectPlayback>(child_fx_entity).unwrap().state,
        PlaybackState::Playing
    );
    // The child effect's own OnSpawn step hasn't had a chance to run yet.
    assert!(playback(&app, child_fx_entity).spawned.is_empty());

    tick(&mut app, 1); // child effect's own OnSpawn fires now
    let inner = *playback(&app, child_fx_entity)
        .spawned
        .get("inner")
        .expect("nested effect's own OnSpawn step should have fired");
    let inner_child = app.world().get::<EffectChild>(inner).unwrap();
    assert_eq!(inner_child.effect_entity, child_fx_entity, "inner should be tagged as a child of the nested effect, not the root");
}

#[test]
fn tween_value_scale_interpolates_linearly_then_completes() {
    let mut app = test_app();
    let entity = spawn_effect(
        &mut app,
        vec![
            EffectStep {
                name: "spawn".into(),
                trigger: EffectTrigger::OnSpawn,
                actions: vec![spawn_primitive("core")],
            },
            EffectStep {
                name: "grow".into(),
                trigger: EffectTrigger::AfterRule {
                    source_rule: "spawn".into(),
                    delay: 0.0,
                },
                actions: vec![EffectAction::TweenValue {
                    target_tag: "core".into(),
                    property: TweenProperty::Scale,
                    from: 1.0,
                    to: 2.0,
                    duration: 0.4,
                    easing: EasingType::Linear,
                }],
            },
        ],
    );

    tick(&mut app, 1); // OnSpawn fires: spawns "core"
    let child = *playback(&app, entity).spawned.get("core").unwrap();

    tick(&mut app, 1); // AfterRule (delay 0.0) fires: tween starts (start_time ~0.2)
    assert_eq!(playback(&app, entity).active_tweens.len(), 1);

    tick(&mut app, 2); // ~halfway through the 0.4s tween
    let scale = app.world().get::<Transform>(child).unwrap().scale;
    assert!(scale.x > 1.0 && scale.x < 2.0, "scale should be partway interpolated, got {scale:?}");

    tick(&mut app, 10); // well past the tween's duration
    assert_eq!(playback(&app, entity).active_tweens.len(), 0, "completed tween should be removed");
    let final_scale = app.world().get::<Transform>(child).unwrap().scale;
    assert_eq!(final_scale, Vec3::splat(2.0));
}

// ---------------------------------------------------------------------------
// Extraction-added coverage (NOT characterization)
// ---------------------------------------------------------------------------

#[test]
fn spawn_effect_recursion_stops_at_depth_cap() {
    // NOT part of the Task-2 characterization contract: the depth guard was
    // added when the runtime moved into `bevy_effect`. A preset that spawns
    // itself used to grow unboundedly (one level per frame); now the chain is
    // cut after `MAX_SPAWN_EFFECT_DEPTH` hops, dropping the spawn with a
    // `warn!`.
    let mut app = test_app();
    app.world_mut().resource_mut::<EffectLibrary>().effects.insert(
        "Ouroboros".into(),
        EffectMarker {
            steps: vec![EffectStep {
                name: "spawn-self".into(),
                trigger: EffectTrigger::OnSpawn,
                actions: vec![EffectAction::SpawnEffect {
                    tag: "self".into(),
                    preset: "Ouroboros".into(),
                    at: SpawnLocation::Offset(Vec3::ZERO),
                    inherit_velocity: false,
                }],
            }],
        },
    );

    let marker = app
        .world()
        .resource::<EffectLibrary>()
        .effects
        .get("Ouroboros")
        .unwrap()
        .clone();
    spawn_effect(&mut app, marker.steps);

    let count_effects = |app: &mut App| {
        let mut q = app.world_mut().query::<&EffectMarker>();
        q.iter(app.world()).count()
    };

    // Each nesting level needs a frame for its own OnSpawn to fire; give the
    // chain far more frames than the cap so unbounded growth would be obvious.
    tick(&mut app, 30);
    let expected = 1 + MAX_SPAWN_EFFECT_DEPTH as usize; // root (depth 0) + capped chain
    assert_eq!(
        count_effects(&mut app),
        expected,
        "self-spawning preset must stop after {MAX_SPAWN_EFFECT_DEPTH} hops"
    );

    // And it must stay stopped — the deepest playback keeps its OnSpawn from
    // re-firing, so no further entities appear.
    tick(&mut app, 10);
    assert_eq!(count_effects(&mut app), expected);
}

//! Task 12 headless integration tests for the window-proxy lifecycle: selecting a window
//! materializes an ephemeral viewport-gizmo proxy entity at its resolved stage position;
//! deselecting (or selecting a different/stale skill) despawns it. Gizmo DRAWING and the mouse-
//! driven radius drag are NOT headless-testable (real rendering / camera+cursor input — see
//! `crate::skill::proxies`' own module doc comment); the proxy entity's LIFECYCLE and the
//! position-resolution math (unit-tested directly in `proxies.rs` itself) are what this file
//! covers, on a from-scratch headless app — same harness shape as `tests/skill_scrub.rs`.

#![cfg(feature = "obelisk")]

use std::path::PathBuf;
use std::time::Duration;

use bevy::prelude::*;

use bevy_editor_game::{AnimationLibrary, GameResetEvent, GameStartedEvent, GameState};
use bevy_vfx::VfxLibrary;

use obelisk_bevy::assets::CastTimeline;
use stat_core::Skill;

use bevy_modal_editor::editor::EditorMode;
use bevy_modal_editor::skill::library::{SkillEntry, SkillLibrary};
use bevy_modal_editor::skill::preview::{
    cosmetics::PreviewCosmeticsPlugin,
    rig::PreviewRigPlugin,
    stage::{PreviewCaster, PreviewControllerPlugin, PreviewDummy, PreviewSimPlugin, SPAWN_MARKERS},
};
use bevy_modal_editor::skill::proxies::{sync_window_proxy, SkillSelection, WindowProxy};
use bevy_modal_editor::skill::templates::SkillArchetype;

fn test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::asset::AssetPlugin { file_path: ".".into(), ..default() })
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::scene::ScenePlugin)
        .add_plugins(bevy::state::app::StatesPlugin)
        .init_state::<GameState>()
        .init_state::<EditorMode>()
        .add_message::<GameStartedEvent>()
        .add_message::<GameResetEvent>()
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(1.0 / 60.0)))
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
        // The system under test — wired exactly as `SkillProxyPlugin` wires it (mode-gated,
        // same as production). `SkillSelection` itself is `init_resource`d directly rather than
        // through `SkillProxyPlugin` so this harness doesn't also need `bevy_egui` registered for
        // `drag_proxy_radius`'s `EguiContexts` param — mirrors `tests/skill_scrub.rs`'s own
        // practice of hand-composing only the systems a test actually needs.
        .init_resource::<SkillSelection>()
        .add_systems(Update, sync_window_proxy.run_if(in_state(EditorMode::Skill)));
    app.finish();
    app.cleanup();
    app
}

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

fn select(app: &mut App, id: &str, window: usize) {
    let mut sel = app.world_mut().resource_mut::<SkillSelection>();
    sel.for_id = Some(id.to_string());
    sel.window = Some(window);
}

fn proxy_transforms(app: &mut App) -> Vec<Transform> {
    app.world_mut().query::<(&Transform, &WindowProxy)>().iter(app.world()).map(|(t, _)| *t).collect()
}

fn caster_pos(app: &mut App) -> Vec3 {
    app.world_mut()
        .query_filtered::<&Transform, With<PreviewCaster>>()
        .iter(app.world())
        .next()
        .expect("stage caster must exist")
        .translation
}

fn dummy_pos(app: &mut App) -> Vec3 {
    app.world_mut()
        .query_filtered::<&Transform, With<PreviewDummy>>()
        .iter(app.world())
        .next()
        .expect("stage dummy must exist")
        .translation
}

#[test]
fn no_proxy_exists_before_any_selection() {
    let mut app = test_app();
    enter_skill_mode(&mut app);
    let (rules, timeline) = SkillArchetype::Strike.build("strike");
    insert_skill(&mut app, "strike", rules, timeline);
    open_skill(&mut app, "strike");
    step(&mut app, 3); // stage settles

    assert!(proxy_transforms(&mut app).is_empty(), "no window is selected yet");
}

#[test]
fn selecting_a_window_materializes_a_proxy_at_the_expected_position() {
    let mut app = test_app();
    enter_skill_mode(&mut app);
    // Strike template: one Caster-anchored window, offset (0, 1, 1.5) — see
    // `templates::strike_template`.
    let (rules, timeline) = SkillArchetype::Strike.build("strike");
    insert_skill(&mut app, "strike", rules, timeline);
    open_skill(&mut app, "strike");
    step(&mut app, 3);

    let caster = caster_pos(&mut app);
    select(&mut app, "strike", 0);
    step(&mut app, 1);

    let transforms = proxy_transforms(&mut app);
    assert_eq!(transforms.len(), 1, "exactly one proxy must materialize");
    let expected = caster + Vec3::new(0.0, 1.0, 1.5);
    assert!(
        transforms[0].translation.distance(expected) < 1e-4,
        "got {:?}, expected {:?}",
        transforms[0].translation,
        expected
    );
}

#[test]
fn deselecting_despawns_the_proxy() {
    let mut app = test_app();
    enter_skill_mode(&mut app);
    let (rules, timeline) = SkillArchetype::Strike.build("strike");
    insert_skill(&mut app, "strike", rules, timeline);
    open_skill(&mut app, "strike");
    step(&mut app, 3);

    select(&mut app, "strike", 0);
    step(&mut app, 1);
    assert_eq!(proxy_transforms(&mut app).len(), 1, "sanity: proxy exists once selected");

    app.world_mut().resource_mut::<SkillSelection>().window = None;
    step(&mut app, 1);

    assert!(proxy_transforms(&mut app).is_empty(), "deselecting must despawn the proxy");
}

#[test]
fn selecting_a_different_skill_without_updating_for_id_is_ignored() {
    // A selection whose `for_id` doesn't match `SkillLibrary.open` must resolve to nothing — the
    // same "stale state from a different skill" guard `SkillSaveState`/`ChipSwitchPrompt` use.
    let mut app = test_app();
    enter_skill_mode(&mut app);
    let (rules, timeline) = SkillArchetype::Strike.build("strike");
    insert_skill(&mut app, "strike", rules, timeline);
    open_skill(&mut app, "strike");
    step(&mut app, 3);

    {
        let mut sel = app.world_mut().resource_mut::<SkillSelection>();
        sel.for_id = Some("some_other_skill".to_string());
        sel.window = Some(0);
    }
    step(&mut app, 1);

    assert!(proxy_transforms(&mut app).is_empty(), "a selection for a different skill must be ignored");
}

#[test]
fn cast_point_window_resolves_through_the_real_stage_dummy() {
    // A `GroundPoint`-acquisition window (`WindowAnchor::CastPoint`) exercises the reused
    // `stage::resolve_stage_acquisition` path end-to-end (real ECS caster/dummy, not just the
    // pure fn's own unit tests in `proxies.rs`) — resolves to the stage's fixed ground marker,
    // `SPAWN_MARKERS[1]`, which is also where the (non-chaining) dummy stands by construction.
    let mut app = test_app();
    enter_skill_mode(&mut app);
    let (rules, timeline) = SkillArchetype::Zone.build("zone");
    insert_skill(&mut app, "zone", rules, timeline);
    open_skill(&mut app, "zone");
    step(&mut app, 3);

    let dummy = dummy_pos(&mut app);
    assert!(
        dummy.distance(SPAWN_MARKERS[1]) < 1e-4,
        "sanity: the default dummy stands at the ground marker"
    );

    select(&mut app, "zone", 0);
    step(&mut app, 1);

    let transforms = proxy_transforms(&mut app);
    assert_eq!(transforms.len(), 1);
    assert!(
        transforms[0].translation.distance(SPAWN_MARKERS[1]) < 1e-4,
        "got {:?}",
        transforms[0].translation
    );
}

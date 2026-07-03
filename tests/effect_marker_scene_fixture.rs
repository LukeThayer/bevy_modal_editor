//! Scene-serialization fixture test for `EffectMarker`.
//!
//! `tests/fixtures/effect_marker_scene.ron` was captured from TODAY's code by
//! serializing a world containing an `EffectMarker` entity through the
//! editor's own scene machinery (`build_editor_scene` + `DynamicScene::
//! serialize`) — the same path `SaveSceneCommand` uses for real scene files.
//! It pins the **full reflected type path**
//! (`bevy_modal_editor::effects::data::EffectMarker`) that existing saved
//! scenes contain on disk.
//!
//! Task 3 (extraction of the effect runtime into `crates/bevy_effect`) must
//! keep this fixture deserializing: if the move changes the type path without
//! a compatibility shim, every previously saved scene with effects breaks,
//! and so does this test.

use bevy::prelude::*;
use bevy::scene::serde::SceneDeserializer;
use bevy::scene::DynamicScene;
use serde::de::DeserializeSeed;

use bevy_modal_editor::effects::{EffectAction, EffectMarker, EffectTrigger};
use bevy_modal_editor::scene::build_editor_scene;
use bevy_modal_editor::SceneEntity;

const FIXTURE: &str = include_str!("fixtures/effect_marker_scene.ron");
const EFFECT_MARKER_TYPE_PATH: &str = "bevy_modal_editor::effects::data::EffectMarker";

/// The raw fixture text must keep referring to the full type path that saved
/// scenes on disk contain. This is the load-bearing string for Task 3.
#[test]
fn fixture_pins_the_full_effect_marker_type_path() {
    assert!(
        FIXTURE.contains(EFFECT_MARKER_TYPE_PATH),
        "fixture must reference `{EFFECT_MARKER_TYPE_PATH}` — if this changed, \
         saved scenes from before the change can no longer be deserialized"
    );
    // And the type itself must still reflect under that path.
    assert_eq!(EffectMarker::type_path(), EFFECT_MARKER_TYPE_PATH);
}

#[test]
fn fixture_deserializes_into_a_world_with_the_expected_effect_marker() {
    // App::new() with default bevy features auto-registers all derived
    // Reflect types (reflect_auto_register), including EffectMarker.
    let mut app = App::new();

    let scene = {
        let type_registry = app.world().resource::<AppTypeRegistry>().clone();
        let registry = type_registry.read();
        let scene_deserializer = SceneDeserializer {
            type_registry: &registry,
        };
        let mut ron_deserializer =
            ron::de::Deserializer::from_str(FIXTURE).expect("fixture should be valid RON");
        let scene: DynamicScene = scene_deserializer
            .deserialize(&mut ron_deserializer)
            .expect("fixture should deserialize against the current type registry");
        scene
    };

    let mut entity_map = bevy::ecs::entity::EntityHashMap::default();
    scene
        .write_to_world(app.world_mut(), &mut entity_map)
        .expect("deserialized scene should write into a world");

    let mut query = app.world_mut().query::<&EffectMarker>();
    let markers: Vec<&EffectMarker> = query.iter(app.world()).collect();
    assert_eq!(markers.len(), 1, "exactly one EffectMarker entity expected");
    let marker = markers[0];

    // Spot-check the step structure survived the round trip intact.
    assert_eq!(marker.steps.len(), 4);
    assert_eq!(marker.steps[0].name, "spawn on start");
    assert!(matches!(marker.steps[0].trigger, EffectTrigger::OnSpawn));
    assert!(matches!(
        &marker.steps[1].trigger,
        EffectTrigger::AfterRule { source_rule, delay }
            if source_rule == "spawn on start" && *delay == 0.25
    ));
    assert!(matches!(
        marker.steps[2].trigger,
        EffectTrigger::RepeatingInterval { interval, max_count: Some(3) } if interval == 0.2
    ));
    assert!(matches!(
        marker.steps[3].trigger,
        EffectTrigger::AfterIdleTimeout { timeout } if timeout == 1.0
    ));
    assert!(matches!(
        &marker.steps[2].actions[0],
        EffectAction::SpawnParticle { tag, preset, .. } if tag == "spark" && preset == "Fire"
    ));
    assert!(matches!(
        &marker.steps[3].actions[0],
        EffectAction::Despawn { tag } if tag == "core"
    ));
}

/// Full round trip through the editor's own save path: world -> DynamicScene
/// (via `build_editor_scene`, the allow-list `SaveSceneCommand` uses) -> RON
/// -> the serialized output must carry the same full type path the fixture
/// pins. Guards against the *writer* side silently changing the path too.
#[test]
fn build_editor_scene_serializes_effect_marker_under_the_pinned_type_path() {
    let mut app = App::new();

    let entity = app
        .world_mut()
        .spawn((SceneEntity, EffectMarker::default()))
        .id();

    let scene = build_editor_scene(app.world(), std::iter::once(entity));
    let type_registry = app.world().resource::<AppTypeRegistry>().clone();
    let registry = type_registry.read();
    let serialized = scene.serialize(&registry).expect("scene should serialize");

    assert!(
        serialized.contains(EFFECT_MARKER_TYPE_PATH),
        "editor scene serialization must emit `{EFFECT_MARKER_TYPE_PATH}`, got:\n{serialized}"
    );
}

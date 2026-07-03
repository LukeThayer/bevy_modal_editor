//! Editor shell around `bevy_effect`'s GLTF-source support.
//!
//! The [`GltfSource`] component and its materializing systems (scene load on
//! add/change, cleanup on removal) moved into `crates/bevy_effect` because the
//! effect runtime's `SpawnGltf` action depends on them. They are re-exported
//! here under their old paths; the reflected type path is pinned inside
//! `bevy_effect` via `#[type_path]`, so saved scenes are unaffected.
//!
//! The editor-only piece — spawning a `SceneEntity`-tracked GLTF object via
//! [`SpawnGltfEvent`] — stays here.

use avian3d::prelude::*;
use bevy::prelude::*;

pub use bevy_effect::{GltfLoaded, GltfSource};

use super::SceneEntity;

/// Event to spawn a GLTF object in the scene
#[derive(Message)]
pub struct SpawnGltfEvent {
    /// Path to the GLTF/GLB file (relative to assets folder)
    pub path: String,
    /// Position to spawn at
    pub position: Vec3,
    /// Rotation to spawn with
    pub rotation: Quat,
}

pub struct GltfSourcePlugin;

impl Plugin for GltfSourcePlugin {
    fn build(&self, app: &mut App) {
        // The materializing systems live in bevy_effect now; the effect
        // plugin may already have installed them (both sides guard).
        if !app.is_plugin_added::<bevy_effect::GltfSourcePlugin>() {
            app.add_plugins(bevy_effect::GltfSourcePlugin);
        }

        app.add_message::<SpawnGltfEvent>()
            .add_systems(Update, handle_spawn_gltf);
    }
}

/// Handle spawning GLTF objects
fn handle_spawn_gltf(
    mut commands: Commands,
    mut events: MessageReader<SpawnGltfEvent>,
) {
    for event in events.read() {
        // Extract filename for the entity name
        let name = event.path
            .rsplit('/')
            .next()
            .unwrap_or(&event.path)
            .trim_end_matches(".gltf")
            .trim_end_matches(".glb")
            .to_string();

        commands.spawn((
            SceneEntity,
            Name::new(name),
            GltfSource {
                path: event.path.clone(),
                scene_index: 0,
            },
            Transform::from_translation(event.position).with_rotation(event.rotation),
            RigidBody::Static,
        ));

        info!("Spawned GLTF object: {}", event.path);
    }
}

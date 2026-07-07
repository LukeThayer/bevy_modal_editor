//! Serializable scene marker components — the shared vocabulary between the editor's scene
//! serialization and any GAME that loads editor-authored `.scn.ron` scenes (e.g. obelisk-arena's
//! level loader). Moved here from `bevy_modal_editor::scene`; every type's `type_path` is PINNED
//! to its original module path because `DynamicScene` RON stores full type paths — existing
//! saved scenes must keep deserializing, and consumers register these types under those paths.
//!
//! The light markers' `Default` values are inlined copies of the editor's
//! `constants::light_colors` values (this crate must not depend on editor internals); the editor
//! remains the value authority and these defaults only seed newly-inserted markers.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub use bevy_effect::primitive::PrimitiveShape;

/// Marker component for entities that are part of the editable scene.
#[derive(Component, Default, Reflect)]
#[reflect(Component)]
#[type_path = "bevy_modal_editor::scene"]
pub struct SceneEntity;

/// Component to track what primitive shape an entity is.
#[derive(Component, Serialize, Deserialize, Clone, Reflect)]
#[reflect(Component)]
#[type_path = "bevy_modal_editor::scene::primitives"]
pub struct PrimitiveMarker {
    pub shape: PrimitiveShape,
}

/// Marker component for group entities (containers for nesting).
#[derive(Component, Serialize, Deserialize, Clone, Default, Reflect)]
#[reflect(Component)]
#[type_path = "bevy_modal_editor::scene::primitives"]
pub struct GroupMarker;

/// Marker component for locked entities (prevents editing).
#[derive(Component, Serialize, Deserialize, Clone, Default, Reflect)]
#[reflect(Component)]
#[type_path = "bevy_modal_editor::scene::primitives"]
pub struct Locked;

/// Marker component for point lights.
#[derive(Component, Serialize, Deserialize, Clone, Reflect)]
#[reflect(Component, Default)]
#[type_path = "bevy_modal_editor::scene::primitives"]
pub struct SceneLightMarker {
    pub color: Color,
    pub intensity: f32,
    pub range: f32,
    pub shadows_enabled: bool,
    #[serde(default)]
    pub radius: f32,
}

impl Default for SceneLightMarker {
    fn default() -> Self {
        Self {
            color: Color::srgb(1.0, 0.95, 0.8),
            intensity: 80000.0,
            range: 30.0,
            shadows_enabled: true,
            radius: 0.0,
        }
    }
}

/// Marker component for directional lights (sun).
#[derive(Component, Serialize, Deserialize, Clone, Reflect)]
#[reflect(Component)]
#[type_path = "bevy_modal_editor::scene::primitives"]
pub struct DirectionalLightMarker {
    pub color: Color,
    pub illuminance: f32,
    pub shadows_enabled: bool,
}

impl Default for DirectionalLightMarker {
    fn default() -> Self {
        Self {
            color: Color::srgb(1.0, 0.98, 0.9),
            illuminance: 10000.0,
            shadows_enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::reflect::TypePath;

    /// The pins ARE the contract: existing `.scn.ron` files reference these exact paths.
    #[test]
    fn type_paths_are_pinned_to_original_editor_modules() {
        assert_eq!(
            SceneEntity::type_path(),
            "bevy_modal_editor::scene::SceneEntity"
        );
        assert_eq!(
            PrimitiveMarker::type_path(),
            "bevy_modal_editor::scene::primitives::PrimitiveMarker"
        );
        assert_eq!(
            SceneLightMarker::type_path(),
            "bevy_modal_editor::scene::primitives::SceneLightMarker"
        );
        assert_eq!(
            DirectionalLightMarker::type_path(),
            "bevy_modal_editor::scene::primitives::DirectionalLightMarker"
        );
        assert_eq!(
            GroupMarker::type_path(),
            "bevy_modal_editor::scene::primitives::GroupMarker"
        );
        assert_eq!(
            Locked::type_path(),
            "bevy_modal_editor::scene::primitives::Locked"
        );
    }
}

//! # Bevy Modal Editor
//!
//! A modal level editor plugin for Bevy games with Avian3D physics support.
//!
//! ## Quick Start
//!
//! Add the editor to your Bevy app:
//!
//! ```no_run
//! use bevy::prelude::*;
//! use bevy_modal_editor::EditorPlugin;
//!
//! fn main() {
//!     App::new()
//!         .add_plugins(DefaultPlugins)
//!         .add_plugins(EditorPlugin::default())
//!         .run();
//! }
//! ```
//!
//! ## Making Entities Editable
//!
//! Mark your entities with `SceneEntity` to make them visible and selectable in the editor:
//!
//! ```ignore
//! commands.spawn((
//!     Name::new("My Object"),
//!     SceneEntity,
//!     // ... other components
//! ));
//! ```
//!
//! ## Editor Modes
//!
//! The editor uses vim-like modal editing:
//!
//! - **View mode**: Camera navigation (WASD + mouse)
//! - **Edit mode** (`E` or `V`): Transform objects (Q=translate, W=rotate, E=scale)
//! - **Insert mode** (`I`): Add new primitives
//! - **Object Inspector** (`O`): Edit component properties
//! - **Hierarchy** (`H`): View scene hierarchy
//!
//! Press `?` for the full help menu.

pub mod asset_libraries;
pub mod commands;
pub mod constants;
pub mod editor;
pub mod effects;
pub mod gizmos;
pub mod materials;
pub mod modeling;
pub mod navigation;
pub mod vfx;
pub mod prefabs;
pub mod scene;
pub mod selection;
#[cfg(feature = "obelisk")]
pub mod skill;
pub mod ui;
pub mod utils;

/// Host hookup for the Skill mode preview caster rig: insert this resource (after
/// `register_gltf_library(..)`) and the preview stage replaces its capsule stand-in with the
/// named scene — lighting up the bone picker (socket index), the anim preview, and
/// bone-anchored cue/charge preview.
#[cfg(feature = "obelisk")]
pub use skill::preview::rig::PreviewCasterRig;

// Re-export the game API crate
pub use bevy_editor_game;

// Re-export the main plugin and configuration
pub use editor::{EditorPlugin, EditorPluginConfig, GamePlugin, recommended_image_plugin};

// Re-export commonly used types
pub use scene::{
    DirectionalLightMarker, GroupMarker, Locked, PrimitiveMarker,
    PrimitiveShape, SceneEntity, SceneLightMarker,
};

// Re-export selection types
pub use selection::Selected;

// Re-export editor state types
pub use editor::{AxisConstraint, EditorMode, EditorState, TransformOperation};

// Re-export from bevy_editor_game
pub use bevy_editor_game::{
    CustomEntityEntry, CustomEntityRegistry, CustomEntityType, RegisterCustomEntityExt,
    InspectorWidgetFn, GizmoDrawFn, RegenerateFn,
    GameCamera, GameEntity, GameState, PauseEvent, PlayEvent, ResetEvent,
    GameStartedEvent, GameResumedEvent, GamePausedEvent, GameResetEvent,
    SceneComponentRegistry, RegisterSceneComponentExt,
    ValidationMessage, ValidationRegistry, ValidationRule, ValidationSeverity,
    RegisterValidationExt,
    AssetRef, AssetType,
};

// Re-export material system
pub use bevy_editor_game::{
    BaseMaterialProps, MaterialDefinition, MaterialLibrary, MaterialRef,
};

// Re-export asset library types
pub use bevy_editor_game::{
    AnimationLibrary, GltfLibraryConfig, MeshLibrary, MeshRef, RegisterGltfLibraryExt,
    SceneLibrary,
};
pub use asset_libraries::AssetLibraryState;
pub use materials::RegisterMaterialTypeExt;

// Re-export scene loading
pub use editor::{SceneLoadingProgress, SceneLoadingState};

// Re-export camera types
pub use editor::EditorCamera;

// Re-export serialization events
pub use scene::{LoadSceneEvent, SaveSceneEvent};

// Re-export command/history types
pub use commands::{
    DeleteSelectedEvent, DuplicateSelectedEvent, RedoEvent, SnapshotHistory, TakeSnapshotCommand,
    UndoEvent,
};

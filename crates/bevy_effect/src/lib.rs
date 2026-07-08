//! Effect sequencer runtime for Bevy, extracted from `bevy_modal_editor`.
//!
//! An effect is a list of **steps**, each with a **trigger** (time, collision,
//! rule chaining, …) and one or more **actions** (spawn primitives / particles /
//! GLTF models / decals / child effects, apply physics, tween values, …).
//!
//! This crate owns:
//! - the data model ([`EffectMarker`] and friends, see [`data`]),
//! - the `.fx.ron` serde format and a directory loader
//!   ([`load_effects_from_dir`]),
//! - the playback runtime ([`EffectPlugin`]) including the [`GltfSource`]
//!   materializing systems used by `EffectAction::SpawnGltf`.
//!
//! Editors/games layer their own persistence (auto-save) and UI on top.
//!
//! # Reflect type paths
//!
//! Scene-persisted types keep their **original** `bevy_modal_editor` reflect
//! type paths (via `#[type_path]`) so scenes saved before the extraction keep
//! loading. Do not remove those attributes.

pub mod data;
pub mod gltf;
pub mod loader;
pub mod presets;
pub mod primitive;
pub mod runtime;

#[cfg(test)]
mod tests;

pub use data::*;
pub use gltf::{GltfLoaded, GltfSource, GltfSourcePlugin};
pub use loader::load_effects_from_dir;
pub use primitive::PrimitiveShape;
pub use runtime::{cleanup_effect, max_spawned_particle_lifetime, stop_effect, EffectLibrary, EffectPlugin};

//! Editor shell around the extracted `bevy_effect` runtime crate.
//!
//! The data model, playback runtime, `.fx.ron` format + loader, and default
//! presets all live in `crates/bevy_effect` now. This module:
//! - re-exports everything under the old `crate::effects::*` paths (the
//!   reflected type paths themselves are pinned inside `bevy_effect` via
//!   `#[type_path]`, so saved scenes are unaffected by the move),
//! - seeds the [`EffectLibrary`] with the built-in presets plus everything in
//!   `assets/effects/`,
//! - auto-saves library changes back to `assets/effects/*.fx.ron` (the crate
//!   deliberately owns only the *loading* side).

pub use bevy_effect::data;
pub use bevy_effect::presets;
pub use bevy_effect::*;

use std::collections::HashMap;
use std::path::Path;

use bevy::prelude::*;

const EFFECTS_DIR: &str = "assets/effects";

/// Editor effect plugin: the `bevy_effect` runtime plus preset
/// initialization and `.fx.ron` auto-save.
///
/// (Shadows the glob-re-exported `bevy_effect::EffectPlugin`, which it adds.)
pub struct EffectPlugin;

impl Plugin for EffectPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy_effect::EffectPlugin)
            .add_systems(PreStartup, init_effect_library)
            .add_systems(Update, auto_save_effect_presets);
    }
}

// ---------------------------------------------------------------------------
// Library initialization
// ---------------------------------------------------------------------------

fn init_effect_library(mut library: ResMut<EffectLibrary>) {
    for (name, marker) in presets::default_presets() {
        library.effects.entry(name.to_string()).or_insert(marker);
    }
    load_effects_from_dir(&mut library, Path::new(EFFECTS_DIR));
}

// ---------------------------------------------------------------------------
// Disk persistence (save side; loading lives in bevy_effect::loader)
// ---------------------------------------------------------------------------

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

fn save_preset_to_disk(name: &str, marker: &EffectMarker) {
    let dir = Path::new(EFFECTS_DIR);
    if let Err(e) = std::fs::create_dir_all(dir) {
        warn!("Failed to create effects directory: {}", e);
        return;
    }

    let filename = sanitize_filename(name);
    let path = dir.join(format!("{}.fx.ron", filename));

    let pretty = ron::ser::PrettyConfig::default();
    match ron::ser::to_string_pretty(marker, pretty) {
        Ok(ron_str) => {
            if let Err(e) = std::fs::write(&path, &ron_str) {
                warn!("Failed to write effect preset '{}': {}", name, e);
            }
        }
        Err(e) => {
            warn!("Failed to serialize effect preset '{}': {}", name, e);
        }
    }
}

fn auto_save_effect_presets(
    library: Res<EffectLibrary>,
    mut prev_state: Local<HashMap<String, String>>,
) {
    if !library.is_changed() {
        return;
    }

    for (name, marker) in &library.effects {
        let ron_str = ron::to_string(marker).unwrap_or_default();
        let changed = match prev_state.get(name) {
            Some(prev) => prev != &ron_str,
            None => true,
        };
        if changed {
            save_preset_to_disk(name, marker);
            prev_state.insert(name.clone(), ron_str);
        }
    }
}

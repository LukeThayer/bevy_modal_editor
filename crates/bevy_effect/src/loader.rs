//! `.fx.ron` effect preset loading.
//!
//! This crate owns the on-disk format: one file per preset, named
//! `<preset name>.fx.ron`, containing a serde-RON [`EffectMarker`].
//! Saving (and auto-saving) is left to the host application.

use std::path::Path;

use bevy::prelude::*;

use crate::data::EffectMarker;
use crate::runtime::EffectLibrary;

/// Load every `*.fx.ron` preset in `dir` into `library`, keyed by file stem.
///
/// Missing/unreadable directories are silently skipped; unparseable files are
/// logged with `warn!` and skipped. Existing entries with the same name are
/// overwritten.
pub fn load_effects_from_dir(library: &mut EffectLibrary, dir: &Path) {
    if !dir.is_dir() {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let fname = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if !fname.ends_with(".fx.ron") {
            continue;
        }

        let name = fname.trim_end_matches(".fx.ron").to_string();
        if name.is_empty() {
            continue;
        }

        let Ok(contents) = std::fs::read_to_string(&path) else {
            warn!("Failed to read effect preset file: {:?}", path);
            continue;
        };

        match ron::from_str::<EffectMarker>(&contents) {
            Ok(marker) => {
                library.effects.insert(name.clone(), marker);
                info!("Loaded effect preset '{}' from disk", name);
            }
            Err(e) => {
                warn!("Failed to parse effect preset '{:?}': {}", path, e);
            }
        }
    }
}

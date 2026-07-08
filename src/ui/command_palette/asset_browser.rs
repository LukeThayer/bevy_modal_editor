//! Asset browser palette — fuzzy search palette that recursively scans the
//! `assets/` directory.

use std::path::Path;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiTextureHandle};

use crate::editor::{EditorMode, InsertObjectType, InsertState, StartInsertEvent};
use crate::scene::{LoadSceneEvent, SaveSceneEvent, SpawnSplatEvent};
use crate::ui::fuzzy_palette::{
    draw_fuzzy_palette, fuzzy_filter, PaletteConfig, PaletteItem, PaletteResult, PaletteState,
};
use crate::ui::gltf_preview::GltfPreviewState;
use crate::ui::theme::colors;

use super::CommandPaletteState;

// ── Types re-exported for external consumers ─────────────────────────

/// Which texture slot is being picked for
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextureSlot {
    BaseColor,
    NormalMap,
    MetallicRoughness,
    Emissive,
    Occlusion,
    DepthMap,
    ParticleTexture,
    DecalBaseColor,
    DecalNormalMap,
    DecalEmissive,
    EffectDecalTexture,
}

/// Result of a texture pick operation, consumed by the material editor
#[derive(Resource, Default)]
pub struct TexturePickResult(pub Option<TexturePickData>);

/// Data for a completed texture pick
pub struct TexturePickData {
    pub slot: TextureSlot,
    pub entity: Option<Entity>,
    pub path: String,
}

/// Result of a GLTF pick operation (e.g. from effect editor)
#[derive(Resource, Default)]
pub struct GltfPickResult(pub Option<GltfPickData>);

/// Data for a completed GLTF pick
pub struct GltfPickData {
    pub entity: Option<Entity>,
    pub path: String,
}

// ── Browse operation ─────────────────────────────────────────────────

/// What kind of file operation the asset browser is performing.
#[derive(Clone)]
pub(crate) enum BrowseOperation {
    LoadScene,
    SaveScene,
    InsertGltf,
    InsertScene,
    InsertSplat,
    PickTexture { slot: TextureSlot, entity: Option<Entity> },
    PickGltf { entity: Option<Entity> },
}

// ── Asset file item ──────────────────────────────────────────────────

/// A single file entry found in the `assets/` directory.
pub(crate) struct AssetFileItem {
    /// Path relative to `assets/` (e.g. "textures/brick.png")
    pub relative_path: String,
    /// Filename only (e.g. "brick.png")
    pub(crate) filename: String,
    /// Parent directory relative to `assets/`, or empty for root files
    pub(crate) directory: String,
    /// True only for the virtual "Save as" item
    pub is_save_as: bool,
}

impl PaletteItem for AssetFileItem {
    fn label(&self) -> &str {
        &self.filename
    }

    fn category(&self) -> Option<&str> {
        if self.directory.is_empty() {
            None
        } else {
            Some(&self.directory)
        }
    }

    fn keywords(&self) -> &[String] {
        &[]
    }
}

// ── Scanning ─────────────────────────────────────────────────────────

/// The directory the ASSET SERVER loads from — set once by `EditorPlugin` from the app's
/// `AssetPlugin::file_path`. A host game that points `AssetPlugin` somewhere other than the
/// CWD-relative `assets/` (e.g. obelisk-arena's editor shell, which runs from
/// `crates/arena_editor/` but loads assets from the workspace root) would otherwise get EMPTY
/// asset-server-backed pickers (textures/gltf/splats): the files the server can load aren't
/// under the CWD. Fs-backed browse operations (scene save/load/insert, which really do
/// read/write CWD-relative `assets/`) deliberately keep using [`scan_assets`].
static ASSET_SCAN_ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Record the asset server's root for [`scan_asset_server_root`]. First call wins (one asset
/// root per process — every `App` in a test binary shares it, which is fine: they all configure
/// the same root). Relative paths mean CWD-relative, matching how Bevy's `FileAssetReader`
/// resolves a relative `file_path` under `cargo run`.
pub fn set_asset_scan_root(root: std::path::PathBuf) {
    let _ = ASSET_SCAN_ROOT.set(root);
}

/// The asset server's root as recorded by [`set_asset_scan_root`] (fallback: the CWD-relative
/// `assets/`). Public: the vfx/effect preset auto-savers write their libraries HERE (so an edit
/// saved in a host game's editor shell lands where that game's asset loading actually reads),
/// not into a possibly-different CWD `assets/`.
pub fn asset_scan_root() -> &'static Path {
    ASSET_SCAN_ROOT
        .get()
        .map(|p| p.as_path())
        .unwrap_or_else(|| Path::new("assets"))
}

/// Recursively scan the CWD-relative `assets/` for files matching the given extensions — for
/// FS-backed browse operations (scene save/load/insert read and write `assets/…` relative to the
/// CWD). Asset-server-backed pickers use [`scan_asset_server_root`] instead.
pub(crate) fn scan_assets(extensions: &[&str]) -> Vec<AssetFileItem> {
    scan_root(Path::new("assets"), extensions)
}

/// Recursively scan the ASSET SERVER's root (see [`set_asset_scan_root`]) — for pickers whose
/// selection is loaded through the asset server (textures, gltf, splats): the relative paths
/// this returns are exactly what `AssetServer::load` resolves.
pub(crate) fn scan_asset_server_root(extensions: &[&str]) -> Vec<AssetFileItem> {
    scan_root(asset_scan_root(), extensions)
}

fn scan_root(root: &Path, extensions: &[&str]) -> Vec<AssetFileItem> {
    if !root.is_dir() {
        return Vec::new();
    }
    let mut items = Vec::new();
    scan_dir_recursive(root, root, extensions, &mut items);
    items.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    items
}

fn scan_dir_recursive(
    base: &Path,
    dir: &Path,
    extensions: &[&str],
    out: &mut Vec<AssetFileItem>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir_recursive(base, &path, extensions, out);
        } else {
            let fname = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            let fname_lower = fname.to_ascii_lowercase();
            if extensions.iter().any(|ext| fname_lower.ends_with(ext)) {
                let relative = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                let filename = fname.to_string();
                let directory = path
                    .parent()
                    .and_then(|p| p.strip_prefix(base).ok())
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();

                out.push(AssetFileItem {
                    relative_path: relative,
                    filename,
                    directory,
                    is_save_as: false,
                });
            }
        }
    }
}

// ── Preview cleanup ──────────────────────────────────────────────────

/// Remove the preview texture from egui and clear state fields.
fn cleanup_preview(state: &mut CommandPaletteState, contexts: &mut EguiContexts) {
    if let Some(ref handle) = state.preview_handle.take() {
        contexts.remove_image(handle);
    }
    state.preview_texture_id = None;
    state.preview_path = None;
}

// ── Draw ─────────────────────────────────────────────────────────────

/// Draw the asset browser palette.
pub(super) fn draw_asset_browser(
    contexts: &mut EguiContexts,
    state: &mut ResMut<CommandPaletteState>,
    load_events: &mut MessageWriter<LoadSceneEvent>,
    save_events: &mut MessageWriter<SaveSceneEvent>,
    insert_events: &mut MessageWriter<StartInsertEvent>,
    insert_state: &mut ResMut<InsertState>,
    next_mode: &mut ResMut<NextState<EditorMode>>,
    texture_pick: &mut ResMut<TexturePickResult>,
    gltf_pick: &mut ResMut<GltfPickResult>,
    asset_server: &Res<AssetServer>,
    gltf_preview_state: &mut ResMut<GltfPreviewState>,
    splat_events: &mut MessageWriter<SpawnSplatEvent>,
) -> Result {
    // Clone the egui context so we can also use contexts.add_image/remove_image later.
    let ctx = contexts.ctx_mut()?.clone();

    let operation = match &state.browse_operation {
        Some(op) => op.clone(),
        None => {
            state.open = false;
            return Ok(());
        }
    };

    let (title, title_color, subtitle, hint, action_label) = match &operation {
        BrowseOperation::LoadScene => (
            "LOAD SCENE",
            colors::ACCENT_BLUE,
            "Open a scene file",
            "Type to search scenes...",
            "load",
        ),
        BrowseOperation::SaveScene => (
            "SAVE SCENE",
            colors::ACCENT_GREEN,
            "Save to file",
            "Type filename or search...",
            "save",
        ),
        BrowseOperation::InsertGltf => (
            "INSERT GLTF",
            colors::ACCENT_ORANGE,
            "Pick a model to insert",
            "Type to search models...",
            "insert",
        ),
        BrowseOperation::InsertScene => (
            "INSERT SCENE",
            colors::ACCENT_ORANGE,
            "Pick a scene to insert",
            "Type to search scenes...",
            "insert",
        ),
        BrowseOperation::InsertSplat => (
            "INSERT SPLAT",
            colors::ACCENT_ORANGE,
            "Pick a gaussian splat to insert",
            "Type to search splats...",
            "insert",
        ),
        BrowseOperation::PickTexture { .. } => (
            "PICK TEXTURE",
            colors::ACCENT_PURPLE,
            "Select a texture image",
            "Type to search textures...",
            "select",
        ),
        BrowseOperation::PickGltf { .. } => (
            "PICK MODEL",
            colors::ACCENT_ORANGE,
            "Select a GLTF/GLB model",
            "Type to search models...",
            "select",
        ),
    };

    // Bridge CommandPaletteState to PaletteState
    let mut palette_state = PaletteState::from_bridge(
        std::mem::take(&mut state.query),
        state.selected_index,
        state.just_opened,
    );

    // ── Texture preview tracking (PickTexture mode only) ────────────
    let is_pick_texture = matches!(&operation, BrowseOperation::PickTexture { .. });
    let preview_panel: Option<Box<dyn FnOnce(&mut egui::Ui)>> = if is_pick_texture {
        // Resolve the currently highlighted item's path
        let filtered = fuzzy_filter(&state.asset_items, &palette_state.query);
        let highlighted_path = filtered
            .get(palette_state.selected_index)
            .map(|fi| fi.item.relative_path.clone());

        // Update preview if highlighted item changed
        if highlighted_path != state.preview_path {
            // Remove old egui texture
            if let Some(ref old_handle) = state.preview_handle.take() {
                contexts.remove_image(old_handle);
                state.preview_texture_id = None;
            }

            if let Some(ref path) = highlighted_path {
                let handle: Handle<Image> = asset_server.load(path.clone());
                let tex_id = contexts.add_image(EguiTextureHandle::Strong(handle.clone()));
                state.preview_handle = Some(handle);
                state.preview_texture_id = Some(tex_id);
            }

            state.preview_path = highlighted_path;
        }

        // Build preview closure if we have a texture to show
        if let (Some(tex_id), Some(path)) = (state.preview_texture_id, &state.preview_path) {
            let tex_id = tex_id;
            let filename = Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            Some(Box::new(move |ui: &mut egui::Ui| {
                ui.label(
                    egui::RichText::new("Preview")
                        .small()
                        .strong()
                        .color(colors::TEXT_SECONDARY),
                );
                ui.add_space(4.0);
                let size = ui.available_width().min(220.0);
                ui.image(egui::load::SizedTexture::new(tex_id, [size, size]));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(&filename)
                        .color(colors::TEXT_PRIMARY)
                        .strong(),
                );
            }))
        } else {
            None
        }
    } else if matches!(&operation, BrowseOperation::InsertGltf | BrowseOperation::PickGltf { .. }) {
        // Resolve the currently highlighted item's path for GLTF preview
        let filtered = fuzzy_filter(&state.asset_items, &palette_state.query);
        let highlighted_path = filtered
            .get(palette_state.selected_index)
            .map(|fi| fi.item.relative_path.clone());

        // Drive the GLTF preview state with the highlighted path
        gltf_preview_state.current_path = highlighted_path;

        // Build preview closure if we have a render texture
        if let Some(tex_id) = gltf_preview_state.texture.egui_texture_id {
            let filename = gltf_preview_state
                .current_path
                .as_ref()
                .and_then(|p| {
                    Path::new(p)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                })
                .unwrap_or_default();
            let is_framed = gltf_preview_state.current_path.is_some();
            Some(Box::new(move |ui: &mut egui::Ui| {
                ui.label(
                    egui::RichText::new("Preview")
                        .small()
                        .strong()
                        .color(colors::TEXT_SECONDARY),
                );
                ui.add_space(4.0);
                let size = ui.available_width().min(220.0);
                ui.image(egui::load::SizedTexture::new(tex_id, [size, size]));
                ui.add_space(4.0);
                if is_framed {
                    ui.label(
                        egui::RichText::new(&filename)
                            .color(colors::TEXT_PRIMARY)
                            .strong(),
                    );
                }
            }))
        } else {
            None
        }
    } else {
        None
    };

    let config = PaletteConfig {
        title,
        title_color,
        subtitle,
        hint_text: hint,
        action_label,
        size: [450.0, 350.0],
        show_categories: true,
        preview_panel,
        ..Default::default()
    };

    let result = draw_fuzzy_palette(&ctx, &mut palette_state, &state.asset_items, config);

    // Sync state back
    state.query = palette_state.query;
    state.selected_index = palette_state.selected_index;
    state.just_opened = palette_state.just_opened;

    match result {
        PaletteResult::Selected(index) => {
            let relative_path = state.asset_items[index].relative_path.clone();
            let is_save_as = state.asset_items[index].is_save_as;
            let query = state.query.trim().to_string();

            match &operation {
                BrowseOperation::LoadScene => {
                    let full_path = format!("assets/{}", relative_path);
                    load_events.write(LoadSceneEvent { path: full_path });
                }
                BrowseOperation::SaveScene => {
                    let path = if is_save_as {
                        let name = if query.is_empty() {
                            "scene".to_string()
                        } else if query.ends_with(".scn.ron") {
                            query[..query.len() - 8].to_string()
                        } else if query.ends_with(".ron") {
                            query[..query.len() - 4].to_string()
                        } else {
                            query
                        };
                        format!("assets/scenes/{}.scn.ron", name)
                    } else {
                        format!("assets/{}", relative_path)
                    };
                    save_events.write(SaveSceneEvent { path });
                }
                BrowseOperation::InsertGltf => {
                    insert_state.gltf_path = Some(relative_path);
                    insert_events.write(StartInsertEvent {
                        object_type: InsertObjectType::Gltf,
                    });
                    next_mode.set(EditorMode::Insert);
                }
                BrowseOperation::InsertScene => {
                    let full_path = format!("assets/{}", relative_path);
                    insert_state.scene_path = Some(full_path);
                    insert_events.write(StartInsertEvent {
                        object_type: InsertObjectType::Scene,
                    });
                    next_mode.set(EditorMode::Insert);
                }
                BrowseOperation::InsertSplat => {
                    splat_events.write(SpawnSplatEvent {
                        path: relative_path,
                        position: Vec3::ZERO,
                        rotation: Quat::IDENTITY,
                    });
                }
                BrowseOperation::PickTexture { slot, entity } => {
                    texture_pick.0 = Some(TexturePickData {
                        slot: *slot,
                        entity: *entity,
                        path: relative_path,
                    });
                }
                BrowseOperation::PickGltf { entity } => {
                    gltf_pick.0 = Some(GltfPickData {
                        entity: *entity,
                        path: relative_path,
                    });
                }
            }

            cleanup_preview(state, contexts);
            gltf_preview_state.current_path = None;
            state.open = false;
            state.browse_operation = None;
        }
        PaletteResult::Closed => {
            cleanup_preview(state, contexts);
            gltf_preview_state.current_path = None;
            state.open = false;
            state.browse_operation = None;
        }
        PaletteResult::Open => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `scan_root` lists matching files RELATIVE to whatever root it is given — the load-bearing
    /// property behind `scan_asset_server_root`: a host game that points `AssetPlugin::file_path`
    /// away from the CWD (obelisk-arena's editor shell) must still get its textures listed, as
    /// asset-server-relative paths.
    #[test]
    fn scan_root_lists_relative_to_the_given_root() {
        let dir = std::env::temp_dir().join(format!("asset_scan_test_{}", std::process::id()));
        let particles = dir.join("textures/particles");
        std::fs::create_dir_all(&particles).unwrap();
        std::fs::write(particles.join("spark.png"), b"x").unwrap();
        std::fs::write(particles.join("readme.txt"), b"x").unwrap();

        let items = scan_root(&dir, &["png"]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].relative_path, "textures/particles/spark.png");

        // A root that doesn't exist lists nothing (never panics).
        assert!(scan_root(Path::new("/nonexistent-root-xyz"), &["png"]).is_empty());
    }
}

//! Skill palette — browse/open skills, create new ones from an archetype template,
//! or rescan content roots. Mirrors `particle_preset.rs`'s browse/apply/new shape;
//! only compiled with `--features obelisk` (see `crate::skill`).

use std::path::PathBuf;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::editor::{EditorMode, EditorState};
use crate::effects::EffectLibrary;
use crate::skill::preview::stage::{ground_marker, PreviewCaster};
use crate::skill::preview::surfaces::{StagedPaint, StagedPaints};
use crate::skill::{insert_new_skill, scan_and_merge_root, unique_id, SkillArchetype, SkillLibrary};
use crate::ui::fuzzy_palette::{draw_fuzzy_palette, PaletteConfig, PaletteItem, PaletteResult, PaletteState};
use crate::ui::theme::colors;
use bevy_vfx::VfxLibrary;
use obelisk_bevy::surfaces::{PaintSurface, SurfacePatch, SurfaceRegistry};

use super::{CommandPaletteState, PaletteMode};

/// What selecting a palette row does.
enum SkillRow {
    NewSkill(SkillArchetype),
    Rescan,
    Existing(String),
    /// Stage a pre-paint of this surface type at the stage's ground-aim marker (Task 5): pushes a
    /// [`StagedPaint`] (re-applied on every reset) AND paints it live now for instant feedback.
    StagePaint(String),
    /// Clear all staged pre-paints and despawn every live patch — back to bare ground (Task 5).
    StageClearPaints,
}

struct SkillItem {
    label: String,
    row: SkillRow,
    /// `false` for "New Skill" rows when `SkillLibrary::roots` is empty — there's
    /// nowhere to write the new skill's files. Mirrors the panel's own empty-state
    /// hint (`skill::mod` — "Call RegisterObeliskContentExt::register_obelisk_content
    /// (root)..."); `insert_new_skill` refuses a fabricated write root, so the row
    /// must refuse to be selected in the first place rather than silently no-op.
    enabled: bool,
    suffix: Option<&'static str>,
}

impl PaletteItem for SkillItem {
    fn label(&self) -> &str {
        &self.label
    }

    fn always_visible(&self) -> bool {
        !matches!(self.row, SkillRow::Existing(_))
    }

    fn accent_color(&self) -> Option<egui::Color32> {
        match self.row {
            SkillRow::NewSkill(_) => Some(colors::ACCENT_GREEN),
            SkillRow::Rescan => Some(colors::ACCENT_ORANGE),
            SkillRow::Existing(_) => None,
            SkillRow::StagePaint(_) => Some(colors::ACCENT_CYAN),
            SkillRow::StageClearPaints => Some(colors::STATUS_ERROR),
        }
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn suffix(&self) -> Option<&str> {
        self.suffix
    }
}

/// Turn free-typed palette query text into an id-safe slug: lowercase,
/// non-alphanumeric runs collapsed to a single `_`, trimmed of leading/trailing `_`.
fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut last_was_sep = true; // suppress a leading separator
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_sep = false;
        } else if !last_was_sep {
            out.push('_');
            last_was_sep = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

/// Draw the skill palette using the shared fuzzy palette widget. Called only from
/// `draw_skill_preset_palette_system` below (own system, not threaded through the
/// main `draw_command_palette` dispatch, so no non-`obelisk` code needs to know
/// `SkillLibrary` exists).
fn draw_skill_preset_palette(
    ctx: &egui::Context,
    state: &mut ResMut<CommandPaletteState>,
    library: &Res<SkillLibrary>,
    registry: &Res<SurfaceRegistry>,
    commands: &mut Commands,
) -> Result {
    let mut palette_state = PaletteState::from_bridge(
        std::mem::take(&mut state.query),
        state.selected_index,
        state.just_opened,
    );

    // No content root registered ⇒ there's nowhere to write a new skill's files.
    // Disable "New Skill" rows rather than let them fall through to a fabricated
    // write root — mirrors the panel's own empty-state hint.
    let has_root = library.default_root().is_some();
    let no_root_hint = "no content root — call register_obelisk_content()";

    let mut items: Vec<SkillItem> = SkillArchetype::ALL
        .into_iter()
        .map(|archetype| SkillItem {
            label: format!("+ New Skill ({})", archetype.label()),
            row: SkillRow::NewSkill(archetype),
            enabled: has_root,
            suffix: if has_root { None } else { Some(no_root_hint) },
        })
        .collect();
    items.push(SkillItem {
        label: "Rescan content roots".to_string(),
        row: SkillRow::Rescan,
        enabled: true,
        suffix: None,
    });

    // Surfaces (Task 5): a "stage: paint <id> patch" row per registered surface type + a
    // clear-all row — present only when surface content is loaded (mirrors the New Skill rows'
    // registry-gated visibility: an empty registry yields no Stage rows at all). A staged patch
    // is re-applied on every reset AND lands at the ground-aim marker (`ground_marker`) — exactly
    // where a `GroundPoint`-gated cast aims — so a surface-gated skill (frost_spire) is castable
    // in-preview and survives scrub/Play/Reset.
    let mut surface_ids: Vec<String> = registry.0.keys().cloned().collect();
    surface_ids.sort();
    if !surface_ids.is_empty() {
        items.extend(surface_ids.iter().map(|id| SkillItem {
            label: format!("Stage: paint {id} patch"),
            row: SkillRow::StagePaint(id.clone()),
            enabled: true,
            suffix: None,
        }));
        items.push(SkillItem {
            label: "Stage: clear staged paints".to_string(),
            row: SkillRow::StageClearPaints,
            enabled: true,
            suffix: None,
        });
    }

    let mut ids: Vec<String> = library.skills.keys().cloned().collect();
    ids.sort();
    items.extend(ids.into_iter().map(|id| SkillItem {
        label: id.clone(),
        row: SkillRow::Existing(id),
        enabled: true,
        suffix: None,
    }));

    let config = PaletteConfig {
        title: "SKILLS",
        title_color: colors::ACCENT_PURPLE,
        subtitle: "Skill library",
        hint_text: "Type to search skills, or name a new one...",
        action_label: "open",
        size: [340.0, 380.0],
        show_categories: false,
        ..Default::default()
    };

    let result = draw_fuzzy_palette(ctx, &mut palette_state, &items, config);

    state.query = palette_state.query;
    state.selected_index = palette_state.selected_index;
    state.just_opened = palette_state.just_opened;

    match result {
        PaletteResult::Selected(index) => {
            match &items[index].row {
                SkillRow::NewSkill(archetype) => {
                    let archetype = *archetype;
                    let query = state.query.trim().to_string();
                    commands.queue(move |world: &mut World| {
                        let mut library = world.resource_mut::<SkillLibrary>();
                        let base = if query.is_empty() {
                            slugify(archetype.label())
                        } else {
                            slugify(&query)
                        };
                        let base = if base.is_empty() { "skill".to_string() } else { base };
                        let id = unique_id(&base, &library);
                        let (mut rules, mut timeline) = archetype.build(&id);
                        rules.id = id.clone();
                        timeline.skill_id = id.clone();
                        // `default_root()` is `None` when no content root is
                        // registered — `insert_new_skill` refuses to fabricate a
                        // fallback in that case. The palette row is disabled
                        // whenever `has_root` is false (see `draw_skill_preset_palette`),
                        // so reaching here with `None` shouldn't happen; still, don't
                        // silently no-op — `library.open` only advances on success.
                        let write_root = library.default_root().map(PathBuf::from);
                        if let Some(new_id) =
                            insert_new_skill(&mut library, rules, timeline, write_root.as_deref())
                        {
                            library.open = Some(new_id);
                        }
                    });
                }
                SkillRow::Rescan => {
                    commands.queue(|world: &mut World| {
                        let roots = world.resource::<SkillLibrary>().roots.clone();
                        world.resource_scope(|world, mut skill_library: Mut<SkillLibrary>| {
                            world.resource_scope(|world, mut effect_library: Mut<EffectLibrary>| {
                                world.resource_scope(|world, mut vfx_library: Mut<VfxLibrary>| {
                                    world.resource_scope(
                                        |_world,
                                         mut surface_registry: Mut<
                                            obelisk_bevy::surfaces::SurfaceRegistry,
                                        >| {
                                            for root in &roots {
                                                scan_and_merge_root(
                                                    root,
                                                    &mut skill_library,
                                                    &mut effect_library,
                                                    &mut vfx_library,
                                                    &mut surface_registry,
                                                );
                                            }
                                        },
                                    );
                                });
                            });
                        });
                    });
                }
                SkillRow::Existing(id) => {
                    let id = id.clone();
                    commands.queue(move |world: &mut World| {
                        world.resource_mut::<SkillLibrary>().open = Some(id);
                    });
                }
                SkillRow::StagePaint(surface) => {
                    let surface = surface.clone();
                    commands.queue(move |world: &mut World| {
                        // Stage at EXACTLY the stage's ground-aim marker (`ground_marker` — the
                        // point `resolve_stage_acquisition` resolves a `GroundPoint` to), so the
                        // staged patch is where a gated cast will aim. Push it into the durable
                        // `StagedPaints` (re-applied on every reset) AND paint it live now for
                        // instant feedback — the reset re-apply clears-then-repaints, so the live
                        // patch and the staged entry converge on one patch, never a duplicate.
                        let position = ground_marker();
                        world.resource_mut::<StagedPaints>().0.push(StagedPaint {
                            surface: surface.clone(),
                            position,
                        });
                        let caster = {
                            let mut q = world.query_filtered::<Entity, With<PreviewCaster>>();
                            q.iter(world).next()
                        };
                        if let Some(caster) = caster {
                            world.trigger(PaintSurface { surface, position, owner: caster });
                        }
                    });
                }
                SkillRow::StageClearPaints => {
                    commands.queue(|world: &mut World| {
                        world.resource_mut::<StagedPaints>().0.clear();
                        let live: Vec<Entity> = {
                            let mut q = world.query_filtered::<Entity, With<SurfacePatch>>();
                            q.iter(world).collect()
                        };
                        for e in live {
                            world.despawn(e);
                        }
                    });
                }
            }
            state.open = false;
        }
        PaletteResult::Closed => {
            state.open = false;
        }
        PaletteResult::Open => {}
    }

    Ok(())
}

/// System wrapper: only renders while the palette is open in `SkillPreset` mode AND
/// the editor is actually in `EditorMode::Skill` (mirrors `MaterialPreset`/
/// `ParticlePreset`'s auto-close-on-mode-change behavior). Registered by
/// `CommandPalettePlugin` only under `--features obelisk`, alongside (not inside)
/// `draw_command_palette` — see that fn's `PaletteMode::SkillPreset` no-op arm.
pub(super) fn draw_skill_preset_palette_system(
    mut contexts: EguiContexts,
    mut state: ResMut<CommandPaletteState>,
    editor_state: Res<EditorState>,
    editor_mode: Res<State<EditorMode>>,
    library: Res<SkillLibrary>,
    registry: Res<SurfaceRegistry>,
    mut commands: Commands,
) -> Result {
    if !editor_state.ui_enabled {
        return Ok(());
    }
    if !state.open || state.mode != PaletteMode::SkillPreset {
        return Ok(());
    }
    if *editor_mode.get() != EditorMode::Skill {
        state.open = false;
        return Ok(());
    }

    let ctx = contexts.ctx_mut()?;
    draw_skill_preset_palette(ctx, &mut state, &library, &registry, &mut commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_collapses_and_lowercases() {
        assert_eq!(slugify("  Ice Spike!! "), "ice_spike");
        assert_eq!(slugify("Fire Bolt 2"), "fire_bolt_2");
        assert_eq!(slugify("---"), "");
    }
}

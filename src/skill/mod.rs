//! Skill mode — obelisk skill authoring (K key, right panel).
//!
//! This module only compiles with `--features obelisk`. `EditorMode::Skill`
//! itself always exists (see `editor::state::EditorMode`), but without this
//! feature nothing ever transitions into it: the K-key handler in
//! `editor::input` is `#[cfg(feature = "obelisk")]`, and this module (which
//! owns the panel and every skill system) is not compiled in at all.
//!
//! The panel (drawn by `draw_skill_panel` below, registered via
//! `ui::skill_editor::SkillEditorPlugin` per the panel-plugin convention — see that
//! module) shows the currently open skill's id, or an empty-state hint when no
//! content root has been registered yet. `library` (Task 5) owns `SkillLibrary` +
//! content-root scanning; `templates` owns the archetype starter templates.
//!
//! `SkillModePlugin` here owns non-UI systems: the `SkillLibrary`/content-root
//! machinery and the probe; it is registered from `EditorPlugin::build`.

pub mod cue_slots;
pub mod edits;
pub mod library;
pub mod panel;
pub mod readouts;
pub mod save;
pub mod templates;
pub mod validation;

pub use cue_slots::{cue_slots, CueSlot};
pub use library::{
    delete_skill, duplicate_skill, insert_new_skill, rename_skill, scan_and_merge_root,
    scan_content_root, skills_referencing, unique_id, PendingContentRoots, RegisterObeliskContentExt,
    SkillEntry, SkillLibrary,
};
pub use save::{reload_skill, save_skill, save_skill_overwrite, SaveError, SaveTarget};
pub use templates::SkillArchetype;
pub use validation::{validate_skill, Problem, ValidationReport};

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::render::view::window::screenshot::{save_to_disk, Screenshot};
use bevy_egui::egui;

use bevy_editor_game::{
    AnimationLibrary, RegisterValidationExt, ValidationMessage, ValidationRule, ValidationSeverity,
};
use bevy_vfx::VfxLibrary;

use crate::editor::{EditorMode, EditorState, PanelSide, PinnedWindows};
use crate::effects::EffectLibrary;
use crate::ui::theme::{colors, draw_pin_button, panel as panel_theme, panel_frame, section_header};

pub struct SkillModePlugin;

impl Plugin for SkillModePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SkillLibrary>()
            .init_resource::<PendingContentRoots>()
            .init_resource::<SkillPanelScrollHint>()
            .init_resource::<SkillSaveState>()
            .add_systems(Startup, library::scan_registered_content_roots)
            .add_systems(Update, skill_probe)
            .register_validation(ValidationRule { name: "skill_rules", validate: skill_validation_rule });
    }
}

/// `bevy_editor_game::ValidationRegistry` rule (Task 8): scans EVERY skill in `SkillLibrary`
/// (not just the one open in the panel — see `crate::skill::validation`'s per-entry
/// `validate_skill`, which the panel itself runs each frame for just the open skill) so
/// problems surface pre-Play even for skills nobody currently has open. Re-run cadence is the
/// registry's own (`ui::validation::run_validation` polls every 60 frames) — this fn is cheap
/// at editor content-library scale (a handful to a few dozen skills), so no extra
/// change-detection gate is layered on top of that existing cadence.
fn skill_validation_rule(world: &mut World) -> Vec<ValidationMessage> {
    let Some(library) = world.get_resource::<SkillLibrary>() else {
        return Vec::new();
    };
    let Some(effects) = world.get_resource::<EffectLibrary>() else {
        return Vec::new();
    };
    let Some(vfx) = world.get_resource::<VfxLibrary>() else {
        return Vec::new();
    };
    let anim = world.get_resource::<AnimationLibrary>();

    let mut messages = Vec::new();
    for (id, entry) in &library.skills {
        let report = validation::validate_skill(entry, library, effects, vfx, anim);
        for problem in &report.problems {
            messages.push(ValidationMessage {
                severity: if problem.blocking { ValidationSeverity::Error } else { ValidationSeverity::Warning },
                message: format!("skill '{id}': {}", problem.message),
                entity: None,
            });
        }
    }
    messages
}

/// Panel-local state for Task 8's stale-disk prompt — NOT a modal: when `save_skill` returns
/// `SaveError::StaleDisk`, the header renders an inline "Reload from disk" / "Overwrite" choice
/// in place of the Save button until one is picked (or the user edits again / switches skills,
/// which clears it — see `draw_skill_panel`). Also holds the last save error's message (any
/// variant, not just `StaleDisk`) so a transient `Io` failure is visible too, not just silently
/// swallowed.
#[derive(Resource, Default)]
pub struct SkillSaveState {
    /// The skill id this prompt/error applies to — `draw_skill_panel` ignores stale state
    /// belonging to a different (or no) skill than the one currently open.
    pub for_id: Option<String>,
    /// `Some(which)` while the stale-disk prompt is showing for the currently open skill.
    pub stale_prompt: Option<SaveTarget>,
    /// The last save attempt's error message, if any (cleared on a successful save or when the
    /// open skill changes).
    pub last_error: Option<String>,
}

/// Debug-only scroll-position override for the Skill panel's `ScrollArea`, consumed by
/// `draw_skill_panel` (forces `ScrollArea::vertical_scroll_offset` when `> 0.0`; `0.0`,
/// the default, leaves scrolling entirely to the user as normal). Exists solely so
/// `skill_probe` can script the panel to a specific scroll position before a screenshot
/// (a long region — e.g. Behavior's window cards — otherwise renders below the visible
/// fold with no mouse to scroll it in a headless run). Real interactive use never
/// touches this resource, so it's always `0.0` outside `--skill-probe`.
#[derive(Resource, Default)]
pub struct SkillPanelScrollHint(pub f32);

/// Draw the Skill panel (exclusive world access).
///
/// Copies the `ai_editor.rs` template shape: right-side window, pinnable,
/// visible when in Skill mode or pinned while another right-side mode is
/// active (in which case it's displaced to the left, same as every other
/// panel). Registered by `ui::skill_editor::SkillEditorPlugin` (this fn
/// stays here, alongside the future `SkillLibrary`, per the panel-plugin
/// convention: UI-layer plugins register draw systems; this module owns the
/// data/logic).
pub(crate) fn draw_skill_panel(world: &mut World) {
    if !world.resource::<EditorState>().ui_enabled {
        return;
    }

    let current_mode = *world.resource::<State<EditorMode>>().get();
    let is_pinned = world.resource::<PinnedWindows>().0.contains(&EditorMode::Skill);
    if current_mode != EditorMode::Skill && !is_pinned {
        return;
    }

    // Get egui context
    let ctx = {
        let Some(mut egui_ctx) = world
            .query::<&mut bevy_egui::EguiContext>()
            .iter_mut(world)
            .next()
        else {
            return;
        };
        egui_ctx.get_mut().clone()
    };

    // Calculate panel position (right side)
    let panel_height = panel_theme::available_height(&ctx);

    // If pinned and the active mode also uses the right side, move to the left
    let displaced = is_pinned
        && current_mode != EditorMode::Skill
        && current_mode.panel_side() == Some(PanelSide::Right);
    let (anchor_align, anchor_offset) = if displaced {
        (egui::Align2::LEFT_TOP, [panel_theme::WINDOW_PADDING, panel_theme::WINDOW_PADDING])
    } else {
        (egui::Align2::RIGHT_TOP, [-panel_theme::WINDOW_PADDING, panel_theme::WINDOW_PADDING])
    };

    let mut pin_toggled = false;
    let scroll_hint = world.resource::<SkillPanelScrollHint>().0;

    // Snapshot Task 8's save-prompt state BEFORE `library` takes its borrow below (both are
    // needed inside the closure; `SkillSaveState` mutations happen only after the window closure
    // ends, same deferred-write pattern the library write-back and `pin_toggled` already use).
    // A prompt/error only applies to the id it was raised for — if the open skill changed since
    // (e.g. the user pressed F and opened something else), treat it as cleared.
    let save_state = world.resource::<SkillSaveState>();
    let open_skill_at_start = world.resource::<SkillLibrary>().open.clone();
    let (stale_prompt, last_error) = if save_state.for_id == open_skill_at_start {
        (save_state.stale_prompt, save_state.last_error.clone())
    } else {
        (None, None)
    };

    let library = world.resource::<SkillLibrary>();
    let open_skill = library.open.clone();
    let has_content_roots = !library.roots.is_empty();
    let skill_count = library.skills.len();
    // Clone the open skill's entry out of the library (same idiom `ui::effect_editor.rs` uses
    // for `EffectMarker`: clone out, edit the clone against `&SkillLibrary` for read-only
    // pickers/readouts, write the clone back once the window closure — and with it every
    // borrow of `library` — has ended). Regions mutate `entry` directly and flip the relevant
    // `dirty_*` flag; the header below (still inside this closure) turns a Save click into an
    // actual disk write on the same clone, once regions are done editing it for the frame.
    let mut editing_entry = open_skill.as_ref().and_then(|id| library.skills.get(id).cloned());
    let effect_library = world.resource::<EffectLibrary>();
    let vfx_library = world.resource::<VfxLibrary>();
    let anim_library = world.get_resource::<AnimationLibrary>();
    let report = editing_entry
        .as_ref()
        .map(|entry| validation::validate_skill(entry, library, effect_library, vfx_library, anim_library))
        .unwrap_or_default();

    let mut save_clicked = false;
    let mut reload_clicked = false;
    let mut overwrite_clicked = false;
    let mut jump_to_effect_mode = false;

    egui::Window::new("Skill")
        .id(egui::Id::new("skill_editor_panel"))
        .frame(panel_frame(&ctx.style()))
        .anchor(anchor_align, anchor_offset)
        .default_width(panel_theme::DEFAULT_WIDTH)
        .min_width(panel_theme::MIN_WIDTH)
        .min_height(panel_height)
        .max_height(panel_height)
        .resizable(true)
        .collapsible(false)
        .title_bar(false)
        .show(&ctx, |ui| {
            // Title with pin button
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Skill")
                        .strong()
                        .color(colors::ACCENT_PURPLE),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    pin_toggled = draw_pin_button(ui, is_pinned);
                });
            });
            ui.separator();

            let mut scroll_area = egui::ScrollArea::vertical().auto_shrink([false; 2]);
            if scroll_hint > 0.0 {
                scroll_area = scroll_area.vertical_scroll_offset(scroll_hint);
            }
            scroll_area
                .show(ui, |ui| {
                    if !has_content_roots {
                        ui.label(
                            egui::RichText::new("No content roots registered")
                                .color(colors::TEXT_SECONDARY),
                        );
                        ui.label(
                            egui::RichText::new(
                                "Call RegisterObeliskContentExt::register_obelisk_content(root) \
                                 to point Skill mode at a content root.",
                            )
                            .color(colors::TEXT_SECONDARY)
                            .small(),
                        );
                        return;
                    }

                    match (&open_skill, &mut editing_entry) {
                        (Some(id), Some(entry)) => {
                            ui.label(egui::RichText::new(id).strong().color(colors::ACCENT_PURPLE));
                            draw_save_header(
                                ui,
                                entry,
                                &report,
                                stale_prompt,
                                &mut save_clicked,
                                &mut reload_clicked,
                                &mut overwrite_clicked,
                            );
                            if let Some(err) = &last_error {
                                ui.label(egui::RichText::new(err).small().color(colors::STATUS_ERROR));
                            }
                            ui.separator();

                            panel::rules::draw_rules_region(ui, entry, library, &report);

                            ui.separator();
                            panel::behavior::draw_behavior_region(ui, entry, &report);

                            ui.separator();
                            section_header(ui, "Presentation", true, |ui| {
                                panel::presentation::draw_presentation_region(
                                    ui,
                                    entry,
                                    effect_library,
                                    vfx_library,
                                    anim_library,
                                    &report,
                                    &mut jump_to_effect_mode,
                                );
                            });
                        }
                        (None, _) | (_, None) => {
                            ui.label(
                                egui::RichText::new(format!("{skill_count} skill(s) loaded"))
                                    .color(colors::TEXT_SECONDARY),
                            );
                            ui.label(
                                egui::RichText::new("Press F to open or create a skill")
                                    .color(colors::TEXT_SECONDARY)
                                    .small(),
                            );
                        }
                    }
                });
        });

    // `library` (and `report`, which borrowed nothing but was built from it) go out of scope
    // here — the immutable `SkillLibrary` borrow ends at its last use inside the closure above,
    // so `world` can be mutably borrowed again below. Apply any Save/Reload/Overwrite click to
    // the local clone FIRST (so the write-back below carries the just-updated disk_hash/dirty
    // flags), then write it back. `None` (no click this frame) must NOT call any of the three —
    // each mutates `entry` (dirty flags, disk_hash) as a side effect, so an unconditional call
    // here would silently auto-save every frame regardless of the button.
    let action_result: Option<Result<(), SaveError>> = editing_entry.as_mut().and_then(|entry| {
        if reload_clicked {
            Some(save::reload_skill(entry))
        } else if overwrite_clicked {
            Some(save::save_skill_overwrite(entry))
        } else if save_clicked {
            Some(save::save_skill(entry))
        } else {
            None
        }
    });

    if let (Some(id), Some(entry)) = (open_skill.clone(), editing_entry) {
        world.resource_mut::<SkillLibrary>().skills.insert(id, entry);
    }

    if let Some(result) = action_result {
        let mut save_state = world.resource_mut::<SkillSaveState>();
        save_state.for_id = open_skill;
        match result {
            Ok(()) => {
                save_state.stale_prompt = None;
                save_state.last_error = None;
            }
            Err(SaveError::StaleDisk { which }) => {
                save_state.stale_prompt = Some(which);
                save_state.last_error = Some(format!("{which} file changed on disk since it was loaded"));
            }
            Err(e) => {
                save_state.last_error = Some(e.to_string());
            }
        }
    }

    if pin_toggled {
        let mut pinned = world.resource_mut::<PinnedWindows>();
        if !pinned.0.remove(&EditorMode::Skill) {
            pinned.0.insert(EditorMode::Skill);
        }
    }

    // Presentation region's "\u{2192} Effect mode" button (Task 9, kept minimal per the brief):
    // pin the Skill panel FIRST (unconditional insert, not the pin button's toggle — the whole
    // point is the round trip works even if the panel wasn't already pinned) so it survives the
    // mode switch, then switch modes. The Effect panel itself finds nothing selected and shows
    // its own empty state; the user picks the preset by name there themselves (see the module
    // doc comment on `panel::presentation` for why this stays this minimal in v1).
    if jump_to_effect_mode {
        world.resource_mut::<PinnedWindows>().0.insert(EditorMode::Skill);
        world.resource_mut::<NextState<EditorMode>>().set(EditorMode::Effect);
    }
}

/// The panel header's Save row (Task 8): dirty badges, then EITHER the Save button (normal
/// case) OR — while `stale_prompt` is `Some` — an inline "Reload from disk" / "Overwrite" choice
/// in its place (not a modal: same row, same frame, nothing else in the panel is blocked). Only
/// sets the `*_clicked` out-params; `draw_skill_panel` performs the actual save/reload/overwrite
/// after the window closure ends (see its doc comment on why — `library`'s borrow is still live
/// here).
fn draw_save_header(
    ui: &mut egui::Ui,
    entry: &SkillEntry,
    report: &ValidationReport,
    stale_prompt: Option<SaveTarget>,
    save_clicked: &mut bool,
    reload_clicked: &mut bool,
    overwrite_clicked: &mut bool,
) {
    ui.horizontal(|ui| {
        if entry.dirty_rules {
            ui.label(egui::RichText::new("rules*").small().color(colors::ACCENT_ORANGE));
        }
        if entry.dirty_timeline {
            ui.label(egui::RichText::new("timeline*").small().color(colors::ACCENT_ORANGE));
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(which) = stale_prompt {
                if ui
                    .button("Overwrite")
                    .on_hover_text(
                        "Force-write despite the on-disk change: rules are re-patched onto the newest \
                         on-disk file (other on-disk edits to rules survive), timeline is fully rewritten \
                         from the editor's copy",
                    )
                    .clicked()
                {
                    *overwrite_clicked = true;
                }
                if ui
                    .button("Reload")
                    .on_hover_text("Discard in-editor edits and reload both files from disk")
                    .clicked()
                {
                    *reload_clicked = true;
                }
                ui.label(
                    egui::RichText::new(format!("{which} changed on disk"))
                        .small()
                        .color(colors::STATUS_WARNING),
                );
            } else {
                let dirty = entry.dirty_rules || entry.dirty_timeline;
                let blocking = report.has_blocking();
                let button = ui.add_enabled(dirty && !blocking, egui::Button::new("Save"));
                if blocking {
                    let tooltip = report.blocking_messages().collect::<Vec<_>>().join("\n");
                    button.on_hover_text(format!("Blocked by validation:\n{tooltip}"));
                } else if !dirty {
                    button.on_hover_text("No changes to save");
                } else if button.clicked() {
                    *save_clicked = true;
                }
            }
        });
    });
}

/// Frame-scripted probe for headless verification of the Skill panel and palette.
///
/// Launch the binary with `--skill-probe` and this system will, over a few
/// hundred frames: enter Skill mode; at frame 150, insert a TEMPLATE-created projectile skill
/// ("probe_fireball") plus a strike-template skill it triggers on impact
/// ("probe_fireball_explosion") directly into `SkillLibrary` (no disk — a fake, never-scanned
/// content root just satisfies the panel's `has_content_roots` gate) and open the fireball, so
/// frame 200's screenshot (`/tmp/skill_mode_probe.png`) exercises the real Rules region (Task 6:
/// readouts, tier-1 fields, the trigger card) instead of the empty state; then (Task 8) at frame
/// 210 pushes a deliberately-dangling `trigger_skill` onto `probe_fireball`'s conditions and
/// screenshots at 213 (`/tmp/skill_mode_probe_validation.png`) — the real `validate_skill` now
/// runs every frame, so this exercises the trigger card's inline blocking message AND the
/// header's disabled Save button end-to-end; then (Task 7) scrolls the panel down via
/// `SkillPanelScrollHint` and screenshots the Behavior region — Acquisition card +
/// `probe_fireball`'s one window card — to `/tmp/skill_mode_probe_behavior.png`; then (Task 9)
/// scrolls further to the panel's true bottom and screenshots the Presentation region — cue
/// rows for `on_cast` (bound to the Projectile template's starter "Skill Muzzle" effect),
/// `on_window_bolt`, `on_end_bolt`, and `on_hit` (bound to "Skill Impact"), each showing its
/// slot's correct legality affordances — to `/tmp/skill_mode_probe_presentation.png`; then opens
/// the skill palette (F in Skill mode — here driven directly via `CommandPaletteState`) and
/// screenshots it to `/tmp/skill_palette_probe.png` before exiting. It's a permanent debug
/// harness — every later Skill-panel task reuses it to confirm the panel/palette render
/// correctly without a human driving the UI. No-op (and effectively free — one `env::args()`
/// scan per frame) unless the flag is present.
fn skill_probe(
    mut next_mode: ResMut<NextState<EditorMode>>,
    mut frame: Local<u32>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut palette: ResMut<crate::ui::CommandPaletteState>,
    mut skill_library: ResMut<SkillLibrary>,
    mut scroll_hint: ResMut<SkillPanelScrollHint>,
) {
    if !std::env::args().any(|arg| arg == "--skill-probe") {
        return;
    }

    *frame += 1;
    match *frame {
        90 => next_mode.set(EditorMode::Skill),
        150 => {
            use stat_core::{SkillCondition, TriggerCondition};

            let root = std::path::PathBuf::from("/tmp/skill_probe_fake_root");
            skill_library.roots.push(root.clone());

            let (mut fireball_rules, fireball_timeline) =
                SkillArchetype::Projectile.build("probe_fireball");
            fireball_rules.conditions.push(SkillCondition {
                trigger_skill: "probe_fireball_explosion".to_string(),
                additional: true,
                condition: TriggerCondition::OnImpact,
            });
            let fireball_id = fireball_rules.id.clone();
            skill_library.skills.insert(
                fireball_id.clone(),
                SkillEntry {
                    rules: fireball_rules,
                    timeline: fireball_timeline,
                    rules_path: library::rules_path_for(&root, &fireball_id),
                    timeline_path: library::timeline_path_for(&root, &fireball_id),
                    dirty_rules: true,
                    dirty_timeline: true,
                    disk_hash: (0, 0),
                },
            );

            let (explosion_rules, explosion_timeline) =
                SkillArchetype::Strike.build("probe_fireball_explosion");
            let explosion_id = explosion_rules.id.clone();
            skill_library.skills.insert(
                explosion_id.clone(),
                SkillEntry {
                    rules: explosion_rules,
                    timeline: explosion_timeline,
                    rules_path: library::rules_path_for(&root, &explosion_id),
                    timeline_path: library::timeline_path_for(&root, &explosion_id),
                    dirty_rules: true,
                    dirty_timeline: true,
                    disk_hash: (0, 0),
                },
            );

            skill_library.open = Some(fireball_id);
        }
        200 => {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk("/tmp/skill_mode_probe.png"));
        }
        // Task 8: mutate `probe_fireball`'s rules with a deliberately-dangling trigger (a
        // `trigger_skill` naming no skill anywhere in the library) so the next screenshot
        // exercises `validate_skill`'s dangling-reference rule end-to-end: the trigger card
        // should show its blocking message inline (Rules region, `for_condition`) and the
        // header's Save button should render disabled (still scrolled to the top — Rules is
        // the panel's first region, no `SkillPanelScrollHint` needed for this one).
        210 => {
            use stat_core::{SkillCondition, TriggerCondition};
            if let Some(fireball) = skill_library.skills.get_mut("probe_fireball") {
                fireball.rules.conditions.push(SkillCondition {
                    trigger_skill: "probe_nonexistent_target".to_string(),
                    additional: true,
                    condition: TriggerCondition::Always,
                });
            }
        }
        213 => {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk("/tmp/skill_mode_probe_validation.png"));
        }
        // Task 7: scroll the panel down (via `SkillPanelScrollHint` — no mouse to drag a
        // real scrollbar in a headless run) so the Behavior region's Acquisition card and
        // `probe_fireball`'s one window card ("bolt") are both in frame for the next
        // screenshot; Rules' readouts/tier-1/triggers scroll off the top, already covered
        // by the frame-200/213 shots above.
        215 => scroll_hint.0 = 820.0,
        230 => {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk("/tmp/skill_mode_probe_behavior.png"));
        }
        // Task 9: Presentation is the panel's last region, below every window card — an
        // intentionally oversized offset clamps `ScrollArea` to the true bottom regardless of
        // exact content height. `probe_fireball` (`SkillArchetype::Projectile`) already carries
        // an `on_cast -> "Skill Muzzle"` cue binding from `templates.rs::cast_and_hit_cues` (set
        // when the skill was inserted at frame 150) plus `on_hit -> "Skill Impact"` — nothing
        // else needs to mutate it for this screenshot to exercise a bound row (Effect picker
        // showing "Skill Muzzle", no Attach/Anim on `on_hit`) alongside the unbound
        // `on_window_bolt`/`on_end_bolt` rows (Attach picker legal on the former, neither on the
        // latter) — every legality affordance from the normative cue table in one shot.
        232 => scroll_hint.0 = 1500.0,
        234 => {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk("/tmp/skill_mode_probe_presentation.png"));
        }
        235 => scroll_hint.0 = 0.0,
        250 => palette.open_skill_preset(),
        310 => {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk("/tmp/skill_palette_probe.png"));
        }
        400 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_mode_panel_side_is_right() {
        assert_eq!(EditorMode::Skill.panel_side(), Some(PanelSide::Right));
    }
}

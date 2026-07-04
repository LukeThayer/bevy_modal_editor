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
pub mod preview;
pub mod proxies;
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
pub use preview::SkillPreviewPlugin;
pub use proxies::{SkillProxyPlugin, SkillSelection};
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
            .init_resource::<ChipSwitchPrompt>()
            .add_systems(Startup, library::scan_registered_content_roots)
            .add_systems(Update, skill_probe)
            .register_validation(ValidationRule { name: "skill_rules", validate: skill_validation_rule })
            // Task 10: the deterministic preview stage — engine/logic (obelisk sim composition +
            // stage lifecycle + cue-driven cosmetics), NOT UI, so it lives on this plugin (the
            // panel itself stays on `ui::skill_editor::SkillEditorPlugin`, per the panel-plugin
            // convention noted in the module doc comment above).
            .add_plugins(SkillPreviewPlugin)
            // Task 12: the selected window's ephemeral viewport gizmo proxy — same
            // engine/logic-not-UI reasoning as `SkillPreviewPlugin` above.
            .add_plugins(SkillProxyPlugin);
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

/// Task 12: pending "switch to a causality-chip's target while the currently open skill is
/// unsaved" confirmation — same INLINE-not-modal spirit as `SkillSaveState`'s stale-disk prompt
/// (Task 8): rendered in the chips row's place until confirmed or cancelled, never a popup.
/// `for_id` is ignored (treated as no prompt) whenever it doesn't match the currently open skill
/// — same "stale state from a different skill is discarded" convention `SkillSaveState` uses —
/// so switching skills through some OTHER path (the palette, say) while a prompt was pending
/// can't leave a confusing leftover prompt behind.
#[derive(Resource, Default)]
pub struct ChipSwitchPrompt {
    pub for_id: Option<String>,
    pub pending_target: Option<String>,
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

    // Task 12: same "ignore stale state from a different skill" snapshot pattern as the save
    // state above, for the causality-chip switch prompt and the viewport-proxy window selection.
    // Both are plain locals mutated directly inside the closure below (cheap `Option` types, no
    // borrow-checker conflict with `library`'s immutable borrow) and written back to their
    // resources once the closure — and every borrow of `library` — has ended.
    let chip_prompt_state = world.resource::<ChipSwitchPrompt>();
    let mut chip_prompt = if chip_prompt_state.for_id == open_skill_at_start {
        chip_prompt_state.pending_target.clone()
    } else {
        None
    };
    let selection_state = world.resource::<proxies::SkillSelection>();
    let mut selected_window = if selection_state.for_id == open_skill_at_start {
        selection_state.window
    } else {
        None
    };
    let mut switch_to_skill: Option<String> = None;

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

    // The scrub strip (Task 11): read-only snapshots of the scrub session + its event-marker log
    // + the live-Play mirror, alongside every other resource this closure reads. `Option`
    // because a minimal test app may draw this panel without `PreviewScrubPlugin` registered.
    let scrub = world.get_resource::<preview::ScrubSim>();
    let scrub_markers = world.get_resource::<preview::ScrubMarkers>();
    let playhead = world.get_resource::<preview::Playhead>();

    let mut save_clicked = false;
    let mut reload_clicked = false;
    let mut overwrite_clicked = false;
    let mut jump_to_effect_mode = false;
    // Strip out-params (module doc comment on `panel::strip::draw_scrub_strip`): applied to the
    // real `ScrubSim` after the window closure ends.
    let mut scrub_new_target: Option<f32> = None;
    let mut scrub_replay_clicked = false;
    let mut scrub_new_charge: Option<u8> = None;

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

                            // Task 12: the causality-chips row — compact navigation, distinct
                            // from the Rules region's full trigger CARDS (see
                            // `panel::chips`'s module doc comment). A Trigger chip click either
                            // switches immediately (nothing unsaved to lose) or stages the inline
                            // dirty-check prompt right below, mirroring `draw_save_header`'s own
                            // "inline row, never a modal" convention.
                            if let Some(target) = panel::chips::draw_chips_row(ui, entry, library) {
                                if entry.dirty_rules || entry.dirty_timeline {
                                    chip_prompt = Some(target);
                                } else {
                                    switch_to_skill = Some(target);
                                    chip_prompt = None;
                                }
                            }
                            if let Some(target) = chip_prompt.clone() {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Unsaved changes \u{2014} switch to '{target}' anyway?"
                                        ))
                                        .small()
                                        .color(colors::STATUS_WARNING),
                                    );
                                    if ui.button("Switch anyway").clicked() {
                                        switch_to_skill = Some(target.clone());
                                        chip_prompt = None;
                                    }
                                    if ui.button("Cancel").clicked() {
                                        chip_prompt = None;
                                    }
                                });
                            }

                            ui.separator();

                            // The scrub strip (Task 11): above the regions — a persistent
                            // structure-and-time surface, same spirit as v1's bottom-dock
                            // timeline. Absent (no-op) if the scrub plugin isn't registered.
                            if let (Some(scrub), Some(markers), Some(playhead)) =
                                (scrub, scrub_markers, playhead)
                            {
                                panel::strip::draw_scrub_strip(
                                    ui,
                                    entry,
                                    scrub,
                                    markers,
                                    playhead,
                                    &mut scrub_new_target,
                                    &mut scrub_replay_clicked,
                                    &mut scrub_new_charge,
                                );
                                ui.separator();
                            }

                            panel::rules::draw_rules_region(ui, entry, library, &report);

                            ui.separator();
                            panel::behavior::draw_behavior_region(ui, entry, &report, &mut selected_window);

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

    // Task 12 review (Fix 1): write-back-then-switch, extracted to `apply_skill_switch` (see its
    // doc comment) so the ordering invariant it embodies is regression-tested instead of resting
    // solely on these two operations' order inside this closure. `switch_to_skill` is only ever
    // set inside the `(Some, Some)` match arm above, so it's always `None` here whenever
    // `editing_entry` is — nothing is lost by only applying it alongside the write-back.
    if let (Some(id), Some(entry)) = (open_skill.clone(), editing_entry) {
        let mut library = world.resource_mut::<SkillLibrary>();
        apply_skill_switch(&mut library, entry, &id, switch_to_skill);
    }

    // Task 12: write back the chip-switch prompt + viewport-proxy window selection for the skill
    // that was actually rendered THIS frame (`open_skill`, captured before any switch above).
    // Written unconditionally (even when unchanged, or `open_skill` is `None`) — a since-switched
    // `for_id` is exactly what the read-side snapshot (above `chip_prompt`/`selected_window`)
    // already treats as stale, so no extra "did it actually change" branch is needed here.
    {
        let mut prompt = world.resource_mut::<ChipSwitchPrompt>();
        prompt.for_id = open_skill.clone();
        prompt.pending_target = chip_prompt;
    }
    {
        let mut selection = world.resource_mut::<proxies::SkillSelection>();
        selection.for_id = open_skill.clone();
        selection.window = selected_window;
    }

    let mut save_succeeded = false;
    if let Some(result) = action_result {
        let mut save_state = world.resource_mut::<SkillSaveState>();
        save_state.for_id = open_skill;
        match result {
            Ok(()) => {
                save_state.stale_prompt = None;
                save_state.last_error = None;
                save_succeeded = true;
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
    // Task 12 review (Fix 3): a pending chip-switch "unsaved changes — switch anyway?" prompt is
    // stale the instant a Save/Reload/Overwrite succeeds — whichever of the three, `entry`'s dirty
    // flags are now clear, so the prompt's premise ("you'd be switching away from something
    // unsaved") no longer holds. The `ChipSwitchPrompt` write-back above already ran this frame
    // with whatever `chip_prompt` the closure computed (unaware of the save result, which is only
    // known after the closure ends) — this deliberately runs AFTER and resets it to empty.
    if save_succeeded {
        *world.resource_mut::<ChipSwitchPrompt>() = ChipSwitchPrompt::default();
    }

    if pin_toggled {
        let mut pinned = world.resource_mut::<PinnedWindows>();
        if !pinned.0.remove(&EditorMode::Skill) {
            pinned.0.insert(EditorMode::Skill);
        }
    }

    // Apply the strip's out-params (Task 11) to the real `ScrubSim`, now that the window
    // closure — and with it every read-only borrow of it above — has ended.
    if (scrub_new_target.is_some() || scrub_replay_clicked || scrub_new_charge.is_some())
        && let Some(mut scrub) = world.get_resource_mut::<preview::ScrubSim>()
    {
        if let Some(t) = scrub_new_target {
            scrub.target = Some(t);
        }
        if scrub_replay_clicked {
            scrub.replay_requested = true;
        }
        if let Some(c) = scrub_new_charge {
            scrub.charge = c;
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

/// Task 12 review (Fix 1 + Fix 2): apply a departing skill's write-back and (optionally) switch
/// `library.open` to a new target — extracted out of `draw_skill_panel`'s closing block into its
/// own fn purely so the ordering invariant it embodies is unit-testable (see the `tests` module
/// below) rather than resting only on statement order inside a 350-line, egui-bound function.
///
/// **Fix 1 — write-back regardless of switch.** `editing` (the departing skill's just-edited
/// clone, dirty flags and all) is ALWAYS written back into `library.skills` under `editing_id`,
/// whether or not `switch_to` is also `Some`. This is what the property "switching skills via a
/// chip preserves the departing DIRTY skill's in-memory edits" rests on: if a future change ever
/// made the write-back conditional on `switch_to.is_none()` (e.g. reasoning "why persist an entry
/// we're navigating away from?" — a plausible-sounding but wrong optimization, since
/// `library.skills` is the ONLY place `editing`'s edits live until Save), the departing skill's
/// edits would silently vanish the instant the user switched away via a chip. See
/// `switching_away_preserves_the_departing_dirty_entry` below.
///
/// **Fix 2 — no phantom-open on a dangling target.** `switch_to` only actually changes
/// `library.open` when the target id resolves to a real entry in `library.skills`. A Trigger chip
/// renders for a dangling `trigger_skill` reference by design (see `panel::chips`'s module doc
/// comment) — clicking one used to set `library.open` to that nonexistent id regardless, which
/// rendered the panel's generic "nothing open" empty state (reads as "the skill vanished", not
/// "that target doesn't exist yet"). Staying on the current skill is the minimum fix; a future
/// revision could surface a transient hint instead. See
/// `switching_to_a_dangling_target_leaves_open_unchanged` below.
fn apply_skill_switch(
    library: &mut SkillLibrary,
    editing: SkillEntry,
    editing_id: &str,
    switch_to: Option<String>,
) {
    library.skills.insert(editing_id.to_string(), editing);
    if let Some(target) = switch_to
        && library.skills.contains_key(&target)
    {
        library.open = Some(target);
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
/// content root just satisfies the panel's `has_content_roots` gate), PLUS a second, isolated
/// "probe_scrub_demo"/"probe_scrub_demo_explosion" pair tuned so the triggered explosion
/// resolves PAST its own base span (`probe_fireball`'s stock template numbers don't — see that
/// insertion's own comment), and open the fireball, so frame 200's screenshot
/// (`/tmp/skill_mode_probe.png`) exercises the real Rules region (Task 6: readouts, tier-1
/// fields, the trigger card) instead of the empty state; then (Task 11) drags to mid-flight
/// (`/tmp/skill_mode_probe_scrub.png`) and then — Task 11 review — opens "probe_scrub_demo" and
/// drags to the strip's FAR EDGE through the widget's own `strip::strip_click_to_target` math,
/// revealing "probe_scrub_demo_explosion"'s triggered blast past the base timeline
/// (`/tmp/skill_mode_probe_scrub_trailing.png`), then reopens "probe_fireball"; then (Task 8) at
/// frame 210 pushes a deliberately-dangling `trigger_skill` onto `probe_fireball`'s conditions
/// and screenshots at 213
/// (`/tmp/skill_mode_probe_validation.png`) — the real `validate_skill` now runs every frame, so
/// this exercises the trigger card's inline blocking message AND the header's disabled Save
/// button end-to-end; then (Task 7) scrolls the panel down via
/// `SkillPanelScrollHint` and screenshots the Behavior region — Acquisition card +
/// `probe_fireball`'s one window card — to `/tmp/skill_mode_probe_behavior.png`; then (Task 9)
/// scrolls further to the panel's true bottom and screenshots the Presentation region — cue
/// rows for `on_cast` (bound to the Projectile template's starter "Skill Muzzle" effect),
/// `on_window_bolt`, `on_end_bolt`, and `on_hit` (bound to "Skill Impact"), each showing its
/// slot's correct legality affordances — to `/tmp/skill_mode_probe_presentation.png` (Task 9
/// review, Finding 1: frame 215 also seeds a throwaway, probe-only `VfxLibrary` preset and
/// binds it onto the previously-empty `on_window_bolt` row with a charge param, so this
/// screenshot additionally exercises the new charge-param `ComboBox` picker — deliberately a
/// FRESH name, not `on_cast`/`on_hit`'s real starter effects, so the probe never permanently
/// alters what an ordinary new skill's starter cues look like: `VfxLibrary` auto-saves ANY
/// resource mutation straight to `assets/vfx/*.vfx.ron` — see `vfx::auto_save_vfx_presets` —
/// so colliding with a real starter effect name here would leak a permanent, misleading
/// "(effect; also vfx)" ambiguity label into every future skill built from any archetype
/// template. The probe-only preset's own auto-saved file is deleted after verification each
/// time this harness is exercised, same discipline as the fake content root at frame 150);
/// then opens the skill palette (F in Skill mode — here driven directly via
/// `CommandPaletteState`) and screenshots it to `/tmp/skill_palette_probe.png`; then (Task 10
/// review) exits Skill mode to show the stage torn down
/// (`/tmp/skill_mode_probe_view_no_stage.png`); then (Task 12) re-enters Skill mode, drops the
/// frame-210 dangling trigger so only the one real causality edge remains, selects
/// `probe_fireball`'s "bolt" window directly via `SkillSelection`, and screenshots
/// (`/tmp/skill_mode_probe_chips_gizmo.png`) the chips row (`probe_fireball \u{2192}
/// probe_fireball_explosion`) alongside the selected window's sphere-outline gizmo at the caster —
/// before exiting.
/// It's a permanent debug harness — every later Skill-panel task reuses it to confirm the
/// panel/palette render correctly without a human driving the UI. No-op (and effectively free —
/// one `env::args()` scan per frame) unless the flag is present.
#[allow(clippy::too_many_arguments)] // one Bevy system param per resource touched; same
                                      // rationale as `panel::presentation::draw_cue_row`.
fn skill_probe(
    mut next_mode: ResMut<NextState<EditorMode>>,
    mut frame: Local<u32>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut palette: ResMut<crate::ui::CommandPaletteState>,
    mut skill_library: ResMut<SkillLibrary>,
    mut vfx_library: ResMut<VfxLibrary>,
    mut scroll_hint: ResMut<SkillPanelScrollHint>,
    mut game_started: MessageWriter<bevy_editor_game::GameStartedEvent>,
    mut next_game_state: ResMut<NextState<bevy_editor_game::GameState>>,
    mut editor_state: ResMut<EditorState>,
    mut scrub: ResMut<preview::ScrubSim>,
    mut selection: ResMut<proxies::SkillSelection>,
) {
    if !std::env::args().any(|arg| arg == "--skill-probe") {
        return;
    }

    *frame += 1;
    match *frame {
        90 => next_mode.set(EditorMode::Skill),
        150 => {
            use obelisk_bevy::assets::{
                AcqFallback, Acquisition, CastTimeline, CollisionShape, CollisionWindow, HitFilter,
                HitMode, MotionDirection, PhaseDurations, VolumeMotion, WindowAnchor, WindowPhase,
                WindowSpawn,
            };
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

            // Task 11 review: a SEPARATE, probe-only pair for the far-edge scrub demo below —
            // never mutating `probe_fireball` itself, so every OTHER screenshot in this probe
            // stays byte-for-byte unaffected (same discipline as the throwaway VfxLibrary preset
            // seeded at frame 215). TWO independent reasons `probe_fireball` itself can't serve
            // this demo, both confirmed empirically while building it:
            // 1. Its stock Projectile-template numbers (base ~2.2s, from the "bolt" window's
            //    generous 2.0s `active_duration`) are too generous — the triggered explosion
            //    would fully resolve INSIDE that base regardless of timing, so scrubbing to the
            //    far edge would show no discovered band.
            // 2. More fundamentally: `probe_fireball`'s "bolt" flies STRAIGHT AT the dummy
            //    (`Acquisition::Aim`, `WindowAnchor::Caster`) and ends via `EndReason::HitEntity`
            //    when it strikes — obelisk-bevy's lifecycle-trigger evaluation
            //    (`timeline::advance::end_hitboxes`) maps ONLY `HitWorld → OnImpact` and
            //    `Fuse → OnExpire`; `HitEntity` maps to no lifecycle condition at all (that hit
            //    already ran the separate hit-phase evaluation instead — see `end_hitboxes`'s own
            //    doc comment). So `probe_fireball`'s `OnImpact` condition can never fire by
            //    hitting the dummy, REGARDLESS of retuned durations — this appears to be a
            //    pre-existing latent no-op in the probe's own fixture, worth a reviewer's eyes
            //    separately from this task.
            // This bespoke pair instead mirrors `tests/skill_scrub.rs`'s OWN proven
            // `fireball_timeline`/`fireball_explosion_timeline` fixture verbatim (values already
            // exercised green by `far_edge_widget_click_reveals_the_triggered_explosion` and
            // `seek_past_impact_shows_the_explosion`): a `GroundPoint`-acquired window that FALLS
            // (`MotionDirection::Down`) from 3 units above the cast point and genuinely ends via
            // `HitWorld`, reliably firing `OnImpact` — base ~0.6s; the explosion's own 0.5s
            // windup (ticked on its `TriggeredExec`'s independent virtual clock, from the ~0.25s
            // impact) delays "blast" to ~0.75-1.05s, comfortably past base.
            let demo_timeline = CastTimeline {
                skill_id: "probe_scrub_demo".to_string(),
                phase_durations: PhaseDurations { windup: 0.1, active: 0.1, recovery: 0.1 },
                collision_windows: vec![CollisionWindow {
                    id: "bolt".to_string(),
                    spawn: WindowSpawn::Scheduled { phase: WindowPhase::Active, offset: 0.0 },
                    anchor: WindowAnchor::CastPoint,
                    anchor_offset: Vec3::new(0.0, 3.0, 0.0),
                    strikes: false,
                    active_duration: 0.5,
                    shape: CollisionShape::Sphere { radius: 0.4 },
                    motion: VolumeMotion::Linear { speed: 20.0 },
                    motion_direction: MotionDirection::Down,
                    hit_filter: HitFilter::Enemies,
                    hit_mode: HitMode::FirstOnly,
                    rehit_interval: None,
                    emitter: None,
                }],
                acquisition: Acquisition::GroundPoint {
                    range: 20.0,
                    fallback: AcqFallback::Then(Box::new(Acquisition::SelfPoint)),
                },
                vfx_cues: Default::default(),
                chain_radius: 6.0,
                chargeable: false,
                max_hold: 1.0,
                cues: Default::default(),
            };
            // Rules borrowed from the Projectile template (id/name/delivery/starter damage
            // shape) — only its TIMELINE is discarded in favor of the bespoke one above.
            let (mut demo_rules, _) = SkillArchetype::Projectile.build("probe_scrub_demo");
            demo_rules.name = "Probe Scrub Demo".to_string();
            demo_rules.conditions.push(SkillCondition {
                trigger_skill: "probe_scrub_demo_explosion".to_string(),
                additional: true,
                condition: TriggerCondition::OnImpact,
            });
            let demo_id = demo_rules.id.clone();
            skill_library.skills.insert(
                demo_id.clone(),
                SkillEntry {
                    rules: demo_rules,
                    timeline: demo_timeline,
                    rules_path: library::rules_path_for(&root, &demo_id),
                    timeline_path: library::timeline_path_for(&root, &demo_id),
                    dirty_rules: true,
                    dirty_timeline: true,
                    disk_hash: (0, 0),
                },
            );

            let demo_explosion_timeline = CastTimeline {
                skill_id: "probe_scrub_demo_explosion".to_string(),
                phase_durations: PhaseDurations { windup: 0.5, active: 0.3, recovery: 0.0 },
                collision_windows: vec![CollisionWindow {
                    id: "blast".to_string(),
                    spawn: WindowSpawn::Scheduled { phase: WindowPhase::Active, offset: 0.0 },
                    anchor: WindowAnchor::CastPoint,
                    anchor_offset: Vec3::ZERO,
                    strikes: true,
                    active_duration: 0.3,
                    shape: CollisionShape::Sphere { radius: 2.0 },
                    motion: VolumeMotion::Static,
                    motion_direction: Default::default(),
                    hit_filter: HitFilter::Enemies,
                    hit_mode: HitMode::OncePerTarget,
                    rehit_interval: None,
                    emitter: None,
                }],
                acquisition: Acquisition::default(),
                vfx_cues: Default::default(),
                chain_radius: 6.0,
                chargeable: false,
                max_hold: 1.0,
                cues: Default::default(),
            };
            // Rules borrowed from the Strike template, same reasoning as `demo_rules` above.
            let (mut demo_explosion_rules, _) =
                SkillArchetype::Strike.build("probe_scrub_demo_explosion");
            demo_explosion_rules.name = "Probe Scrub Demo Explosion".to_string();
            let demo_explosion_id = demo_explosion_rules.id.clone();
            skill_library.skills.insert(
                demo_explosion_id.clone(),
                SkillEntry {
                    rules: demo_explosion_rules,
                    timeline: demo_explosion_timeline,
                    rules_path: library::rules_path_for(&root, &demo_explosion_id),
                    timeline_path: library::timeline_path_for(&root, &demo_explosion_id),
                    dirty_rules: true,
                    dirty_timeline: true,
                    disk_hash: (0, 0),
                },
            );
        }
        200 => {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk("/tmp/skill_mode_probe.png"));
        }
        // Task 11 (sim-backed synchronous scrub): `probe_fireball` is a Projectile-archetype
        // skill (windup 0.2s, then its "bolt" window opens and crosses the 8 m duel gap at
        // 25 u/s, ~0.32s flight — landing the true hit around 0.52s). Seeking to 0.35s lands
        // comfortably MID-FLIGHT: after the window opens, well before the hit. `drive_scrub`'s
        // synchronous seek loop completes entirely within its own next invocation (a single
        // `Update` system call), so the freeze is fully settled by the very next frame.
        202 => {
            scrub.target = Some(0.35);
        }
        204 => {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk("/tmp/skill_mode_probe_scrub.png"));
        }
        // Task 11 review fix: open the dedicated "probe_scrub_demo" pair (seeded at frame 150 —
        // see its own comment for why it's separate from `probe_fireball`) and drag toward the
        // FAR EDGE through the strip WIDGET's own `strip_click_to_target` click math
        // (`rel_x = 1.0`), not a direct `ScrubSim.target` poke — proving the strip's headline
        // feature (scrub PAST the base timeline to reveal the `OnImpact`-triggered
        // "probe_scrub_demo_explosion") is reachable through the real widget, not just the
        // engine mechanism. Resetting `ScrubSim` first forces `drive_scrub` to restart on the
        // newly-opened skill rather than continuing whatever the frame-202 mid-flight demo left
        // frozen on `probe_fireball`.
        205 => {
            *scrub = preview::ScrubSim::default();
            skill_library.open = Some("probe_scrub_demo".to_string());
            let base = skill_library
                .skills
                .get("probe_scrub_demo")
                .map(|e| panel::strip::base_span(&e.timeline))
                .unwrap_or(0.0001);
            scrub.target = Some(panel::strip::strip_click_to_target(1.0, base, scrub.dynamic_end));
        }
        208 => {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk("/tmp/skill_mode_probe_scrub_trailing.png"));
        }
        // Hand the sim back to Idle AND reopen `probe_fireball` before the rest of the probe
        // continues, so the later screenshots (validation/behavior/presentation/Play) show the
        // stage exactly as they did before this task — undisturbed by this demo's frozen
        // mid-cast instant or its stand-in skill.
        209 => {
            *scrub = preview::ScrubSim::default();
            skill_library.open = Some("probe_fireball".to_string());
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
        //
        // Task 9 review (Finding 1): also seed a throwaway `VfxLibrary` preset under a
        // probe-only name (never a real starter/content name — see the fn doc comment for why)
        // with two params, and bind it onto the previously-empty `on_window_bolt` row with a
        // charge param naming one of them. Exercises `discoverable_params`/the new `ComboBox`
        // picker in the frame-234 screenshot below without touching `on_cast`/`on_hit`'s
        // pre-existing "Skill Muzzle"/"Skill Impact" starter bindings. Harmless to the
        // frame-230 Behavior screenshot (that region never reads cues/effects).
        215 => {
            scroll_hint.0 = 820.0;
            const PROBE_VFX_PRESET: &str = "Probe Charge Demo Vfx";
            vfx_library.effects.entry(PROBE_VFX_PRESET.to_string()).or_insert_with(|| bevy_vfx::VfxSystem {
                params: vec![
                    bevy_vfx::VfxParam { name: "scale".to_string(), value: bevy_vfx::VfxParamValue::Float(1.0) },
                    bevy_vfx::VfxParam { name: "intensity".to_string(), value: bevy_vfx::VfxParamValue::Float(1.0) },
                ],
                ..Default::default()
            });
            if let Some(fireball) = skill_library.skills.get_mut("probe_fireball") {
                use obelisk_bevy::assets::{CueAttach, CueBinding, CueParam, ParamSource};
                fireball.timeline.cues.insert(
                    "on_window_bolt".to_string(),
                    CueBinding {
                        effect: Some(PROBE_VFX_PRESET.to_string()),
                        attach: CueAttach::World,
                        anim: None,
                        params: vec![CueParam { param: "scale".to_string(), source: ParamSource::Charge }],
                    },
                );
            }
        }
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
        // showing "Skill Muzzle", no Attach/Anim on `on_hit`) alongside `on_end_bolt` (neither
        // legal) — every legality affordance from the normative cue table in one shot. Task 9
        // review (Finding 1): frame 215 above additionally binds `on_window_bolt` (previously
        // "(none)") to a fresh probe-only vfx preset with a charge param, so this row now ALSO
        // shows a Charge Params `ComboBox` (selected: "scale") instead of free text, alongside
        // its pre-existing Attach picker.
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
        // Task 10: close the palette (it would otherwise cover the viewport) and trigger Play —
        // directly, rather than via `PlayEvent`/`GamePlugin` (this binary never adds
        // `GamePlugin`, matching arena_editor's own headless-preview convention of driving the
        // stage straight off `GameStartedEvent`) — so the preview stage's `start_preview` casts
        // `probe_fireball` (open since frame 150) on the persistent caster+dummy duel.
        320 => {
            palette.open = false;
            next_game_state.set(bevy_editor_game::GameState::Playing);
            game_started.write(bevy_editor_game::GameStartedEvent);
        }
        // Hide the UI (mirrors what the real `PlayCommand` does — `editor::game.rs` — on an
        // ordinary Play; this probe bypasses `PlayCommand` itself, see frame 320's own comment,
        // so it has to flip the same flag by hand) so the Skill panel — which occupies the right
        // half of the window and would otherwise obscure the dummy standing at +X — stops
        // drawing, WITHOUT leaving Skill mode. Task 10 review, Finding 1: the stage is now
        // scoped to `EditorMode::Skill` (spawned `OnEnter`, torn down `OnExit`), so switching to
        // View here — as this probe used to — would tear the whole duel down mid-cast instead of
        // just hiding a panel. Staying in Skill mode keeps the stage (and the sim ticking it)
        // exactly as live as an ordinary Play would.
        325 => editor_state.ui_enabled = false,
        // Two viewport screenshots straddling the cast's flight (windup 0.2s + ~0.3s to cross
        // the 8 m duel at the projectile template's 25 u/s — real-time frames, not fixed sim
        // ticks, so these are generous estimates, not an exact tick count). Both taken while
        // still in Skill mode (see frame 325) — the stage must still be rendering/ticking here.
        335 => {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk("/tmp/skill_mode_probe_play_early.png"));
        }
        365 => {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk("/tmp/skill_mode_probe_play.png"));
        }
        // Task 10 review, Finding 1 (new): NOW leave Skill mode, well after the cast screenshots
        // above — demonstrating the other half of the mode-gate: the stage must be ABSENT (no
        // caster/dummy/floor, no cast in flight) once outside Skill mode, instead of the old
        // "persistent regardless of mode" behavior.
        375 => next_mode.set(EditorMode::View),
        385 => {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk("/tmp/skill_mode_probe_view_no_stage.png"));
        }
        // Task 12: re-enter Skill mode (re-spawns a fresh stage — `probe_fireball` is still
        // `library.open` from frame 209 onward, never changed since) so the chips row + window
        // proxy gizmo can be demonstrated against a live caster. Also restore the UI (frame 325
        // hid it for the bare-viewport Play screenshots and never turned it back on) — without
        // this the Skill panel (and its chips row) never draws at all, only the gizmo would show.
        390 => {
            next_mode.set(EditorMode::Skill);
            editor_state.ui_enabled = true;
        }
        // Drop the frame-210 dangling trigger so this screenshot shows exactly the ONE real
        // causality edge (`probe_fireball \u{2192} probe_fireball_explosion`) the brief's own
        // example names — nothing later in this probe reads `probe_fireball`'s conditions again,
        // so this is safe to do here, right before the shot, rather than earlier. Then select
        // window index 0 ("bolt", the Projectile template's sole window: `Caster` anchor, zero
        // offset, `Sphere{radius:0.4}` — see `templates::projectile_template`) directly via
        // `SkillSelection`, exactly the way frame 202 pokes `ScrubSim.target` directly rather
        // than dragging a widget: this probe drives STATE, not simulated clicks.
        391 => {
            if let Some(fireball) = skill_library.skills.get_mut("probe_fireball") {
                fireball.rules.conditions.retain(|c| c.trigger_skill != "probe_nonexistent_target");
            }
            selection.for_id = Some("probe_fireball".to_string());
            selection.window = Some(0);
            // A real finding from THIS probe pass: `SkillPanelScrollHint` only FORCES an offset
            // when `> 0.0` (frame 235 set it to exactly `0.0`, i.e. "stop forcing" — but egui's
            // `ScrollArea` remembers its own last-forced position across frames regardless, so
            // without this it would still show frame 234's forced-to-bottom Presentation view,
            // scrolling the chips row — near the panel's TOP — out of frame entirely).
            scroll_hint.0 = 1.0;
        }
        394 => {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk("/tmp/skill_mode_probe_chips_gizmo.png"));
        }
        410 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::templates::strike_template;
    use std::path::PathBuf;

    #[test]
    fn skill_mode_panel_side_is_right() {
        assert_eq!(EditorMode::Skill.panel_side(), Some(PanelSide::Right));
    }

    fn entry(id: &str) -> SkillEntry {
        let (rules, timeline) = strike_template(id);
        SkillEntry {
            rules,
            timeline,
            rules_path: PathBuf::new(),
            timeline_path: PathBuf::new(),
            dirty_rules: false,
            dirty_timeline: false,
            disk_hash: (0, 0),
        }
    }

    // --- Task 12 review, Fix 1: the write-back-then-switch invariant ---

    /// The property under test: "switching skills via a chip preserves the departing DIRTY
    /// skill's in-memory edits." Library starts with a CLEAN "A" (currently open) and a clean
    /// "B". A clone of "A" is mutated dirty (mirrors what `draw_skill_panel`'s region closures do
    /// to `editing_entry` over the course of a frame) and handed to `apply_skill_switch` alongside
    /// a switch to "B". Both must hold: the switch applies, AND "A"'s slot in `library.skills`
    /// carries the dirty edits forward rather than the stale clean snapshot it started this test
    /// with — i.e. the switch must never come at the cost of discarding the departing skill's
    /// edits.
    #[test]
    fn switching_away_preserves_the_departing_dirty_entry() {
        let mut library = SkillLibrary::default();
        library.skills.insert("A".to_string(), entry("A"));
        library.skills.insert("B".to_string(), entry("B"));
        library.open = Some("A".to_string());

        let mut dirty_a = library.skills["A"].clone();
        dirty_a.rules.mana_cost = 999.0;
        dirty_a.dirty_rules = true;

        apply_skill_switch(&mut library, dirty_a, "A", Some("B".to_string()));

        assert_eq!(library.open, Some("B".to_string()), "the switch must apply");
        let a = &library.skills["A"];
        assert_eq!(a.rules.mana_cost, 999.0, "A's edit must survive the switch away from it");
        assert!(a.dirty_rules, "A's dirty flag must survive the switch away from it");
    }

    /// A regression that skips (or conditions away) the write-back whenever a switch is also
    /// requested — see `apply_skill_switch`'s own doc comment for exactly this scenario — would
    /// fail the assertion above: "A" would keep reading back as its pre-edit clean snapshot
    /// (`mana_cost` at the template default, `dirty_rules` false) instead of carrying the dirty
    /// edit through. This test pins that failure signature down explicitly so it's obvious what
    /// broke if this ever regresses, without relying on hand-editing `apply_skill_switch` to
    /// prove it (which was done manually once while authoring this fix: temporarily reordering it
    /// to switch-then-write-back-conditionally reproduced exactly this failure, confirming the
    /// test's bite).
    #[test]
    fn departing_entry_is_never_left_on_its_stale_clean_snapshot() {
        let mut library = SkillLibrary::default();
        let clean_a = entry("A");
        let clean_mana = clean_a.rules.mana_cost;
        library.skills.insert("A".to_string(), clean_a);
        library.skills.insert("B".to_string(), entry("B"));
        library.open = Some("A".to_string());

        let mut dirty_a = library.skills["A"].clone();
        dirty_a.rules.mana_cost = clean_mana + 1234.0;
        dirty_a.dirty_rules = true;
        dirty_a.dirty_timeline = true;

        apply_skill_switch(&mut library, dirty_a, "A", Some("B".to_string()));

        let a = &library.skills["A"];
        assert_ne!(a.rules.mana_cost, clean_mana, "must not silently revert to the stale clean copy");
        assert!(a.dirty_rules && a.dirty_timeline);
    }

    #[test]
    fn no_switch_requested_still_writes_back() {
        let mut library = SkillLibrary::default();
        library.skills.insert("A".to_string(), entry("A"));
        library.open = Some("A".to_string());

        let mut dirty_a = library.skills["A"].clone();
        dirty_a.rules.mana_cost = 42.0;
        dirty_a.dirty_rules = true;

        apply_skill_switch(&mut library, dirty_a, "A", None);

        assert_eq!(library.open, Some("A".to_string()), "no switch requested — open must not move");
        assert_eq!(library.skills["A"].rules.mana_cost, 42.0);
    }

    // --- Task 12 review, Fix 2: no phantom-open on a dangling chip target ---

    #[test]
    fn switching_to_a_dangling_target_leaves_open_unchanged() {
        let mut library = SkillLibrary::default();
        library.skills.insert("A".to_string(), entry("A"));
        library.open = Some("A".to_string());

        let mut dirty_a = library.skills["A"].clone();
        dirty_a.dirty_rules = true;

        apply_skill_switch(&mut library, dirty_a, "A", Some("ghost".to_string()));

        assert_eq!(library.open, Some("A".to_string()), "must not navigate to a nonexistent skill");
        assert!(
            library.skills["A"].dirty_rules,
            "write-back must still happen even when the switch itself is rejected"
        );
    }
}

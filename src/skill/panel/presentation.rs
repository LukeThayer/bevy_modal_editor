//! Presentation region (Task 9): one row per `CueSlot` (`crate::skill::cue_slots`) binding an
//! editor Effect/vfx preset (+ attach mode, + editor-only anim clip, + charge-param rows) into
//! `entry.timeline.cues`. Legality per slot (which pickers a row even shows) is decided entirely
//! by `CueSlot::attach_legal`/`anim_legal` — see that module's doc comment for the normative
//! table this mirrors.
//!
//! Same idioms as `panel::behavior`/`panel::rules`: one light card per row, every mutation ORs
//! into a local `changed` flipped once into `entry.dirty_timeline` at the end, and any
//! `report.for_cue(slot_id)` problem (Task 8: unknown Effect preset / unknown anim clip) renders
//! inline on its row exactly like `for_condition`/`for_window` do on their cards.
//!
//! **Effect picker.** One combined list: every `EffectLibrary` key (as-is) plus every
//! `VfxLibrary` key not already present in `EffectLibrary` (suffixed `" (vfx)"` for display
//! only — the stored `CueBinding.effect` string is always the bare name, matching how
//! `crate::skill::validation::validate_skill` resolves a cue's effect against BOTH libraries).
//! Picking "(none)" (or leaving both `effect`/`anim` unset with no params) removes the row's
//! `CueBinding` from `entry.timeline.cues` entirely — an empty binding is not authored data.
//!
//! **Jump-to-Effect-mode.** A row with a bound Effect preset gets a "\u{2192} Effect mode"
//! button. Per the brief (v1, kept minimal): clicking it only requests a mode switch + pins the
//! Skill panel (so the round trip back works) — it does NOT select/spawn the preset entity or
//! pre-filter the Effect palette; the user finds the preset by name in Effect mode themselves.
//! Since this module has no `World`/`NextState` access (it's a plain `&mut egui::Ui` region,
//! same as every other region here), the click is surfaced as an out-param
//! (`jump_to_effect_mode: &mut bool`) that `crate::skill::draw_skill_panel` acts on AFTER the
//! panel's window closure ends — the exact deferred-write pattern that fn already uses for its
//! own pin button (`pin_toggled`) and Save/Reload/Overwrite clicks.

use bevy_egui::egui;

use obelisk_bevy::assets::{CueAttach, CueBinding, CueParam, ParamSource};

use bevy_editor_game::AnimationLibrary;
use bevy_vfx::VfxLibrary;

use crate::effects::EffectLibrary;
use crate::skill::cue_slots::{cue_slots, CueSlot};
use crate::skill::library::SkillEntry;
use crate::skill::validation::ValidationReport;
use crate::ui::theme::{colors, grid_label, GRID_SPACING};

/// Draw the whole Presentation region for `entry` (the currently open skill). `jump_to_effect_mode`
/// is set `true` when any row's "\u{2192} Effect mode" button was clicked this frame — see the
/// module doc comment for why this is an out-param rather than a direct mode switch.
pub fn draw_presentation_region(
    ui: &mut egui::Ui,
    entry: &mut SkillEntry,
    effects: &EffectLibrary,
    vfx: &VfxLibrary,
    anims: Option<&AnimationLibrary>,
    report: &ValidationReport,
    jump_to_effect_mode: &mut bool,
) {
    let mut changed = false;
    let effect_options = combined_effect_options(effects, vfx);
    let anim_options = anim_clip_options(anims);

    let slots = cue_slots(&entry.timeline);
    for slot in &slots {
        changed |= draw_cue_row(ui, slot, entry, &effect_options, &anim_options, report, jump_to_effect_mode);
        ui.add_space(4.0);
    }

    if changed {
        entry.dirty_timeline = true;
    }
}

/// `(display label, stored name)`. Every `EffectLibrary` key first (no suffix — the common
/// case, and what every archetype template's starter cues already reference), then every
/// `VfxLibrary` key NOT also in `EffectLibrary`, suffixed `" (vfx)"` for display only.
fn combined_effect_options(effects: &EffectLibrary, vfx: &VfxLibrary) -> Vec<(String, String)> {
    let mut names: Vec<&String> = effects.effects.keys().collect();
    names.sort();
    let mut options: Vec<(String, String)> = names.into_iter().map(|n| (n.clone(), n.clone())).collect();

    let mut vfx_only: Vec<&String> = vfx.effects.keys().filter(|n| !effects.effects.contains_key(*n)).collect();
    vfx_only.sort();
    options.extend(vfx_only.into_iter().map(|n| (format!("{n} (vfx)"), n.clone())));

    options
}

/// `AnimationLibrary` clip keys (already `"file::name"`, see `asset_libraries::index_gltf`),
/// sorted. `None` (no `AnimationLibrary` resource present) yields an empty list — same
/// "unchecked, not clean" contract `validate_skill` uses for the anim-clip rule.
fn anim_clip_options(anims: Option<&AnimationLibrary>) -> Vec<String> {
    let mut names: Vec<String> = anims.map(|a| a.clips.keys().cloned().collect()).unwrap_or_default();
    names.sort();
    names
}

/// Draw one cue-slot row. Returns `true` if the row mutated `entry.timeline.cues`.
#[allow(clippy::too_many_arguments)]
fn draw_cue_row(
    ui: &mut egui::Ui,
    slot: &CueSlot,
    entry: &mut SkillEntry,
    effect_options: &[(String, String)],
    anim_options: &[String],
    report: &ValidationReport,
    jump_to_effect_mode: &mut bool,
) -> bool {
    let mut changed = false;

    let frame = egui::Frame::new()
        .fill(colors::BG_MEDIUM)
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::same(6));

    let resp = frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&slot.label).strong().color(colors::TEXT_PRIMARY));

            let bound_effect = entry.timeline.cues.get(&slot.slot_id).and_then(|b| b.effect.clone());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(effect_name) = &bound_effect
                    && !effect_name.is_empty()
                    && ui
                        .button(egui::RichText::new("\u{2192} Effect mode").color(colors::ACCENT_ORANGE))
                        .on_hover_text(format!(
                            "Switch to Effect mode to edit '{effect_name}' — pins the Skill panel \
                             first so it's still here when you come back."
                        ))
                        .clicked()
                {
                    *jump_to_effect_mode = true;
                }
            });
        });

        for problem in report.for_cue(&slot.slot_id) {
            let color = if problem.blocking { colors::STATUS_ERROR } else { colors::STATUS_WARNING };
            ui.label(egui::RichText::new(&problem.message).small().color(color));
        }

        ui.add_space(2.0);

        // Effect picker — always shown (every slot in the vocabulary can bind an effect).
        let mut effect_value = entry.timeline.cues.get(&slot.slot_id).and_then(|b| b.effect.clone());
        if effect_picker(ui, &slot.slot_id, &mut effect_value, effect_options) {
            changed = true;
            set_or_clear_field(entry, &slot.slot_id, |b| b.effect = effect_value.clone());
        }

        // Attach/anim pickers render whenever the SLOT is legal for them, independent of
        // whether a binding currently exists (unlike charge params below) — the row needs to
        // show the legality affordance itself (e.g. "this slot accepts Follow") even before an
        // effect is chosen. No write happens unless the user actually picks a non-default value
        // (`attach_picker`/`anim_picker` only report `changed` on a real click), so merely
        // rendering an unbound row's pickers never fabricates a `CueBinding`.
        if slot.attach_legal {
            let mut attach = entry.timeline.cues.get(&slot.slot_id).map(|b| b.attach).unwrap_or_default();
            if attach_picker(ui, &slot.slot_id, &mut attach) {
                changed = true;
                set_or_clear_field(entry, &slot.slot_id, |b| b.attach = attach);
            }
        }

        if slot.anim_legal {
            let mut anim_value = entry.timeline.cues.get(&slot.slot_id).and_then(|b| b.anim.clone());
            if anim_picker(ui, &slot.slot_id, &mut anim_value, anim_options) {
                changed = true;
                set_or_clear_field(entry, &slot.slot_id, |b| b.anim = anim_value.clone());
            }
        }

        if entry.timeline.cues.contains_key(&slot.slot_id) {
            changed |= draw_charge_params(ui, &slot.slot_id, entry);
        }

        prune_if_empty(entry, &slot.slot_id);
    });

    let attach_slot_marker = if slot.attach_legal { colors::ACCENT_CYAN } else { colors::TEXT_MUTED };
    let card_rect = resp.response.rect;
    let stripe = egui::Rect::from_min_max(card_rect.left_top(), egui::pos2(card_rect.left() + 3.0, card_rect.bottom()));
    ui.painter().rect_filled(stripe, egui::CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 }, attach_slot_marker);

    changed
}

/// Ensure `slot_id` has a `CueBinding` in `entry.timeline.cues` (inserting a default one if
/// absent), apply `set`, then prune it back out if the result is empty — see `prune_if_empty`.
/// Every picker callback in `draw_cue_row` routes through this so "pick '(none)' on the only
/// populated field" and "removing the last charge param" both correctly delete the binding
/// rather than leaving an inert empty one behind.
fn set_or_clear_field(entry: &mut SkillEntry, slot_id: &str, set: impl FnOnce(&mut CueBinding)) {
    let binding = entry.timeline.cues.entry(slot_id.to_string()).or_default();
    set(binding);
}

/// A `CueBinding` with `effect: None`, `anim: None`, no params, and `attach` still at its
/// `World` default is not authored data — remove it from the map entirely rather than persist
/// an inert entry (keeps `.cast.ron` clean and matches "removes the binding entirely when
/// everything None/empty" from the brief). `attach` is checked too: an author who picks `Follow`
/// on an otherwise-empty `on_window_*`/`emit_*` row (attach with no effect yet — legal, if
/// unusual) must not have that choice silently discarded.
fn prune_if_empty(entry: &mut SkillEntry, slot_id: &str) {
    if let Some(binding) = entry.timeline.cues.get(slot_id)
        && binding.effect.is_none()
        && binding.anim.is_none()
        && binding.params.is_empty()
        && binding.attach == CueAttach::default()
    {
        entry.timeline.cues.remove(slot_id);
    }
}

fn effect_picker(
    ui: &mut egui::Ui,
    id_salt: &str,
    value: &mut Option<String>,
    options: &[(String, String)],
) -> bool {
    let mut changed = false;
    egui::Grid::new(("cue_effect_grid", id_salt.to_string()))
        .num_columns(2)
        .spacing(GRID_SPACING)
        .show(ui, |ui| {
            grid_label(ui, "Effect");
            let current_display = value
                .as_deref()
                .and_then(|name| options.iter().find(|(_, stored)| stored == name))
                .map(|(display, _)| display.as_str())
                .or(value.as_deref())
                .unwrap_or("(none)");
            egui::ComboBox::from_id_salt(("cue_effect_picker", id_salt.to_string()))
                .selected_text(current_display)
                .width(160.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(value.is_none(), "(none)").clicked() && value.is_some() {
                        *value = None;
                        changed = true;
                    }
                    for (display, stored) in options {
                        let selected = value.as_deref() == Some(stored.as_str());
                        if ui.selectable_label(selected, display).clicked() && !selected {
                            *value = Some(stored.clone());
                            changed = true;
                        }
                    }
                });
            ui.end_row();
        });
    changed
}

fn attach_picker(ui: &mut egui::Ui, id_salt: &str, attach: &mut CueAttach) -> bool {
    let mut changed = false;
    egui::Grid::new(("cue_attach_grid", id_salt.to_string()))
        .num_columns(2)
        .spacing(GRID_SPACING)
        .show(ui, |ui| {
            grid_label(ui, "Attach");
            let label = match attach {
                CueAttach::World => "World",
                CueAttach::Follow => "Follow",
            };
            egui::ComboBox::from_id_salt(("cue_attach_picker", id_salt.to_string()))
                .selected_text(label)
                .width(100.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(*attach == CueAttach::World, "World").clicked()
                        && *attach != CueAttach::World
                    {
                        *attach = CueAttach::World;
                        changed = true;
                    }
                    if ui.selectable_label(*attach == CueAttach::Follow, "Follow").clicked()
                        && *attach != CueAttach::Follow
                    {
                        *attach = CueAttach::Follow;
                        changed = true;
                    }
                });
            ui.end_row();
        });
    if *attach == CueAttach::Follow {
        ui.label(
            egui::RichText::new(
                "Follow: the host flies a proxy along this cue's motion data; the window's end \
                 event (not this binding) terminates it — no matching On End binding needed.",
            )
            .small()
            .color(colors::TEXT_MUTED),
        );
    }
    changed
}

fn anim_picker(ui: &mut egui::Ui, id_salt: &str, value: &mut Option<String>, options: &[String]) -> bool {
    let mut changed = false;
    egui::Grid::new(("cue_anim_grid", id_salt.to_string()))
        .num_columns(2)
        .spacing(GRID_SPACING)
        .show(ui, |ui| {
            grid_label(ui, "Anim (editor-only)");
            let current = value.as_deref().unwrap_or("(none)");
            egui::ComboBox::from_id_salt(("cue_anim_picker", id_salt.to_string()))
                .selected_text(current)
                .width(160.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(value.is_none(), "(none)").clicked() && value.is_some() {
                        *value = None;
                        changed = true;
                    }
                    for name in options {
                        let selected = value.as_deref() == Some(name.as_str());
                        if ui.selectable_label(selected, name).clicked() && !selected {
                            *value = Some(name.clone());
                            changed = true;
                        }
                    }
                });
            ui.end_row();
        });
    ui.label(
        egui::RichText::new(
            "Preview-only (D7): the networked game host does not consume this — it's here so the \
             Skill mode preview can play a cast animation, nothing more.",
        )
        .small()
        .color(colors::TEXT_MUTED),
    );
    changed
}

/// Charge-param rows (`ParamSource::Charge` is the only source v1 supports — the source half of
/// each row is a fixed label, not a picker). Shown whenever the slot has a binding at all
/// (regardless of `attach_legal`/`anim_legal` — a params-only cue, e.g. driving an effect's own
/// exposed scale param off charge with no `attach`/`anim` authored, is legal on every slot per
/// `CueBinding`'s schema).
fn draw_charge_params(ui: &mut egui::Ui, slot_id: &str, entry: &mut SkillEntry) -> bool {
    let mut changed = false;
    let mut remove_index = None;

    ui.label(egui::RichText::new("Charge Params").small().color(colors::TEXT_SECONDARY));

    let Some(binding) = entry.timeline.cues.get_mut(slot_id) else {
        return false;
    };

    for (i, param) in binding.params.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            changed |= ui.add(egui::TextEdit::singleline(&mut param.param).desired_width(120.0)).changed();
            ui.label(egui::RichText::new("\u{2190} Charge").small().color(colors::TEXT_MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(egui::RichText::new("\u{00d7}").color(colors::STATUS_ERROR)).frame(false))
                    .on_hover_text("Remove param binding")
                    .clicked()
                {
                    remove_index = Some(i);
                }
            });
        });
    }

    if let Some(i) = remove_index {
        binding.params.remove(i);
        changed = true;
    }

    if ui.button(egui::RichText::new("+ charge param").color(colors::ACCENT_GREEN)).clicked() {
        binding.params.push(CueParam { param: String::new(), source: ParamSource::Charge });
        changed = true;
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combined_effect_options_dedupes_and_suffixes_vfx_only() {
        let mut effects = EffectLibrary::default();
        effects.effects.insert("Explosion".to_string(), Default::default());
        let mut vfx = VfxLibrary::default();
        vfx.effects.insert("Explosion".to_string(), Default::default()); // shadowed by EffectLibrary
        vfx.effects.insert("Spark".to_string(), Default::default());

        let options = combined_effect_options(&effects, &vfx);
        assert_eq!(
            options,
            vec![("Explosion".to_string(), "Explosion".to_string()), ("Spark (vfx)".to_string(), "Spark".to_string())]
        );
    }

    #[test]
    fn anim_clip_options_empty_when_no_library() {
        assert!(anim_clip_options(None).is_empty());
    }
}

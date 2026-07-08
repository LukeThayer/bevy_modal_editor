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
//! A name present in BOTH libraries collapses to ONE pickable entry (the binding can only ever
//! store the bare name, with no library tag riding along) labeled `" (effect; also vfx)"` to
//! flag the ambiguity — see `combined_effect_options`'s doc comment for the full rationale.
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
//!
//! **Charge-param rows (Task 9 review, Finding 1).** A row's charge-param name is looked up
//! against the bound Effect's discoverable Vfx params (`discoverable_params`) — v1 scope: ONLY
//! a name that resolves DIRECTLY as a `VfxLibrary` preset (`vfx.effects.get(name)`) with a
//! non-empty `VfxSystem.params` counts. An `EffectLibrary` preset that internally spawns a vfx
//! (via one or more `EffectAction::SpawnParticle` steps, each naming its own preset) is too deep
//! to resolve for v1 — there's no single vfx system to read params from. When the discoverable
//! set is non-empty the row renders a `ComboBox` over those names (plus the current value even
//! when it's off-list, so switching effects or hand-edited `.cast.ron` data never silently loses
//! an existing param name); otherwise it falls back to the free-text `TextEdit` it always used.
//! In BOTH modes, a non-empty param name that isn't in the discoverable set (including "no
//! discoverable params at all") gets an inline warning label, styled like every other
//! `report.for_cue` problem here (small, `colors::STATUS_WARNING`). This warning IS the brief's
//! validation-warning deliverable — deliberately a presentation-only check, not a new
//! `validation.rs` rule (a param-name typo has no effect on `has_blocking`/Save).

use bevy_egui::egui;

use obelisk_bevy::assets::{CueAttach, CueBinding, CueParam, ParamSource};

use bevy_editor_game::AnimationLibrary;
use bevy_vfx::VfxLibrary;

use crate::effects::EffectLibrary;
use crate::skill::cue_slots::{cue_slots, CueSlot};
use crate::skill::library::SkillEntry;
use crate::skill::preview::cosmetics::resolve_cue_duration;
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
        changed |= draw_cue_row(ui, slot, entry, &effect_options, &anim_options, vfx, report, jump_to_effect_mode);
        ui.add_space(4.0);
    }

    if changed {
        entry.dirty_timeline = true;
    }
}

/// `(display label, stored name)`. Every `EffectLibrary` key first (no suffix, UNLESS it
/// collides with a `VfxLibrary` name — see below), then every `VfxLibrary` key NOT also in
/// `EffectLibrary`, suffixed `" (vfx)"` for display only.
///
/// **Collision handling is schema-inherent — not something this fn (or any picker) can actually
/// fix.** `CueBinding.effect` is a bare `String`: it names an effect by string identity alone,
/// with no library tag riding along. So when the SAME name exists in both libraries, there is
/// only one possible stored value for it (e.g. `"Fire"`), and authoring it can only ever mean
/// "resolve 'Fire' by name" — never "resolve `EffectLibrary`'s Fire, specifically, as opposed to
/// `VfxLibrary`'s." Listing both libraries' entries as separate picker rows would write the
/// IDENTICAL stored string from either one — a false choice, not a real one — so this fn keeps
/// deduping down to one pickable entry per colliding name. Which preset the RUNTIME actually
/// resolves to is decided by library-lookup order — defined by Task 10's preview and the game
/// client, not by anything here (`validate_skill` itself accepts a cue name that resolves
/// against EITHER library, so a collision isn't even flagged as invalid data). Authors should
/// avoid cross-library name collisions entirely; short of that, this fn at least flags the
/// ambiguity: a colliding `EffectLibrary` entry's label is suffixed `" (effect; also vfx)"` so
/// its picker row visibly differs from an unambiguous one, even though the stored value (and
/// therefore which preset ultimately wins at runtime) is unchanged.
fn combined_effect_options(effects: &EffectLibrary, vfx: &VfxLibrary) -> Vec<(String, String)> {
    let mut names: Vec<&String> = effects.effects.keys().collect();
    names.sort();
    let mut options: Vec<(String, String)> = names
        .into_iter()
        .map(|n| {
            let label = if vfx.effects.contains_key(n) { format!("{n} (effect; also vfx)") } else { n.clone() };
            (label, n.clone())
        })
        .collect();

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
    vfx: &VfxLibrary,
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

        // Duration control — only meaningful once an effect is bound (it tunes how long THAT
        // effect plays). Rendered after attach/anim so the row reads effect → where → how long.
        if entry
            .timeline
            .cues
            .get(&slot.slot_id)
            .is_some_and(|b| b.effect.is_some())
        {
            let mut duration_value =
                entry.timeline.cues.get(&slot.slot_id).and_then(|b| b.duration);
            let bound_effect =
                entry.timeline.cues.get(&slot.slot_id).and_then(|b| b.effect.clone());
            if duration_picker(ui, &slot.slot_id, &mut duration_value, bound_effect.as_deref(), vfx)
            {
                changed = true;
                set_or_clear_field(entry, &slot.slot_id, |b| b.duration = duration_value);
            }
        }

        if entry.timeline.cues.contains_key(&slot.slot_id) {
            changed |= draw_charge_params(ui, &slot.slot_id, entry, vfx);
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
        && binding.duration.is_none()
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

/// The Duration row: how long the bound effect PLAYS (emits) before the host stops emission and
/// lets live particles drain out on their authored lifetimes. `None` = auto — the effect preset's
/// own Duration (the VFX editor field), else the host default — with the resolved value shown in
/// the auto button so the designer always sees what they'll get. Explicit ⇄ auto round-trips
/// without losing the resolved number (switching to explicit seeds the drag value from it).
fn duration_picker(
    ui: &mut egui::Ui,
    id_salt: &str,
    duration: &mut Option<f32>,
    effect_name: Option<&str>,
    vfx: &VfxLibrary,
) -> bool {
    let mut changed = false;
    let auto_value =
        resolve_cue_duration(None, effect_name.and_then(|n| vfx.effects.get(n)));
    egui::Grid::new(("cue_duration_grid", id_salt.to_string()))
        .num_columns(2)
        .spacing(GRID_SPACING)
        .show(ui, |ui| {
            grid_label(ui, "Duration");
            ui.horizontal(|ui| {
                match duration {
                    Some(value) => {
                        let mut v = *value;
                        let resp = ui.add(
                            egui::DragValue::new(&mut v)
                                .range(0.0..=60.0)
                                .speed(0.05)
                                .suffix(" s")
                                .min_decimals(1),
                        );
                        if resp.changed() && (v - *value).abs() > f32::EPSILON {
                            *duration = Some(v);
                            changed = true;
                        }
                        if ui
                            .small_button("auto")
                            .on_hover_text(format!(
                                "Clear the authored duration — falls back to {auto_value:.1}s \
                                 (the effect preset's own Duration, else the default)."
                            ))
                            .clicked()
                        {
                            *duration = None;
                            changed = true;
                        }
                    }
                    None => {
                        if ui
                            .button(format!("auto ({auto_value:.1} s)"))
                            .on_hover_text(
                                "Playing on auto: the effect preset's own Duration (set it in \
                                 the VFX editor), else the host default. Click to author an \
                                 explicit duration for THIS cue. Particles always finish their \
                                 own lifetimes after the duration ends — this tunes how long \
                                 the effect EMITS.",
                            )
                            .clicked()
                        {
                            *duration = Some(auto_value);
                            changed = true;
                        }
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

/// The row's bound effect's discoverable Vfx param names — v1 scope: ONLY a name that resolves
/// DIRECTLY as a `VfxLibrary` preset (`vfx.effects.get(name)`) counts, and only if that preset's
/// `VfxSystem.params` is non-empty. An `EffectLibrary` preset that internally spawns a vfx (one
/// or more `EffectAction::SpawnParticle` steps, each naming its own vfx preset) is too deep to
/// resolve for v1 — there's no single vfx system to read params from, so this fn treats such a
/// name the same as any other unrecognized one: empty. Also empty when `effect_name` is `None`
/// or names nothing in `VfxLibrary`. Pure — see the `tests` module for coverage.
fn discoverable_params(effect_name: Option<&str>, vfx: &VfxLibrary) -> Vec<String> {
    effect_name
        .and_then(|name| vfx.effects.get(name))
        .map(|system| system.params.iter().map(|p| p.name.clone()).collect())
        .unwrap_or_default()
}

/// Render one charge-param row's name field: a `ComboBox` over `discoverable` when non-empty,
/// else the free-text `TextEdit` this row always used. The `ComboBox` always keeps `value`
/// selectable even when it's off-list (not present in `discoverable`) — switching to an effect
/// with a different param set, or a hand-authored `.cast.ron` naming a param from before the
/// bound effect's current param list, must never silently blank out or replace existing data.
fn charge_param_name_field(
    ui: &mut egui::Ui,
    id_salt: &str,
    index: usize,
    value: &mut String,
    discoverable: &[String],
) -> bool {
    if discoverable.is_empty() {
        return ui.add(egui::TextEdit::singleline(value).desired_width(120.0)).changed();
    }

    let mut changed = false;
    let current = if value.is_empty() { "(none)" } else { value.as_str() };
    egui::ComboBox::from_id_salt(("charge_param_picker", id_salt.to_string(), index))
        .selected_text(current)
        .width(120.0)
        .show_ui(ui, |ui| {
            if !value.is_empty() && !discoverable.contains(value) {
                // Already selected (it's the row's current value) and clicking it back onto
                // itself is a no-op, so the response is intentionally discarded.
                let _ = ui.selectable_label(true, format!("{value} (current)"));
            }
            for name in discoverable {
                let selected = value.as_str() == name.as_str();
                if ui.selectable_label(selected, name).clicked() && !selected {
                    *value = name.clone();
                    changed = true;
                }
            }
        });
    changed
}

/// Charge-param rows (`ParamSource::Charge` is the only source v1 supports — the source half of
/// each row is a fixed label, not a picker). Shown whenever the slot has a binding at all
/// (regardless of `attach_legal`/`anim_legal` — a params-only cue, e.g. driving an effect's own
/// exposed scale param off charge with no `attach`/`anim` authored, is legal on every slot per
/// `CueBinding`'s schema).
///
/// **Param-name discovery (Task 9 review, Finding 1).** `discoverable_params` resolves the
/// row's bound effect name directly against `VfxLibrary`; `charge_param_name_field` renders a
/// `ComboBox` over the result when non-empty, else falls back to free text. Either way, a
/// non-empty param name that isn't in the discoverable set — including when the set is empty
/// because the effect isn't a direct `VfxLibrary` preset, or is unbound — gets an inline warning
/// label, the same small/colored idiom `report.for_cue` problems use above `draw_cue_row`'s
/// pickers. This warning IS the brief's validation-warning deliverable: deliberately a
/// presentation-only check local to this module, not a new `validation.rs` rule — a param-name
/// typo does not gate the Save button.
fn draw_charge_params(ui: &mut egui::Ui, slot_id: &str, entry: &mut SkillEntry, vfx: &VfxLibrary) -> bool {
    let mut changed = false;
    let mut remove_index = None;

    ui.label(egui::RichText::new("Charge Params").small().color(colors::TEXT_SECONDARY));

    let Some(binding) = entry.timeline.cues.get(slot_id) else {
        return false;
    };
    let effect_name = binding.effect.clone();
    let discoverable = discoverable_params(effect_name.as_deref(), vfx);

    let Some(binding) = entry.timeline.cues.get_mut(slot_id) else {
        return false;
    };

    for (i, param) in binding.params.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            changed |= charge_param_name_field(ui, slot_id, i, &mut param.param, &discoverable);
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

        if !param.param.is_empty() && !discoverable.contains(&param.param) {
            let message = if discoverable.is_empty() {
                "no discoverable params — verify against the effect".to_string()
            } else {
                format!("'{}' is not a known param of '{}'", param.param, effect_name.as_deref().unwrap_or("?"))
            };
            ui.label(egui::RichText::new(message).small().color(colors::STATUS_WARNING));
        }
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
    fn combined_effect_options_flags_collision_and_suffixes_vfx_only() {
        let mut effects = EffectLibrary::default();
        effects.effects.insert("Explosion".to_string(), Default::default());
        let mut vfx = VfxLibrary::default();
        vfx.effects.insert("Explosion".to_string(), Default::default()); // collides with EffectLibrary
        vfx.effects.insert("Spark".to_string(), Default::default());

        let options = combined_effect_options(&effects, &vfx);
        // Deduped to one entry per colliding name (never two rows storing the identical value),
        // but the collision is flagged in the label rather than silently hidden (Finding 2).
        assert_eq!(
            options,
            vec![
                ("Explosion (effect; also vfx)".to_string(), "Explosion".to_string()),
                ("Spark (vfx)".to_string(), "Spark".to_string())
            ]
        );
    }

    #[test]
    fn anim_clip_options_empty_when_no_library() {
        assert!(anim_clip_options(None).is_empty());
    }

    #[test]
    fn discoverable_params_only_resolves_direct_vfx_presets() {
        use bevy_vfx::{VfxParam, VfxParamValue, VfxSystem};

        let mut vfx = VfxLibrary::default();
        vfx.effects.insert(
            "Fire".to_string(),
            VfxSystem {
                params: vec![
                    VfxParam { name: "scale".to_string(), value: VfxParamValue::Float(1.0) },
                    VfxParam { name: "intensity".to_string(), value: VfxParamValue::Float(1.0) },
                ],
                ..Default::default()
            },
        );

        assert_eq!(
            discoverable_params(Some("Fire"), &vfx),
            vec!["scale".to_string(), "intensity".to_string()]
        );
        // Not a direct VfxLibrary preset (unknown, or an EffectLibrary-only name) -> empty.
        assert!(discoverable_params(Some("Skill Muzzle"), &vfx).is_empty());
        // No effect bound at all -> empty.
        assert!(discoverable_params(None, &vfx).is_empty());
    }

    #[test]
    fn discoverable_params_empty_for_vfx_preset_with_no_params() {
        let mut vfx = VfxLibrary::default();
        vfx.effects.insert("Bare".to_string(), Default::default()); // VfxSystem::default() -> params: []
        assert!(discoverable_params(Some("Bare"), &vfx).is_empty());
    }
}

//! Behavior region (Task 7): phases + charge, the acquisition card, window cards with
//! emitter sub-cards.
//!
//! `draw_behavior_region` is the region's entry point, called from
//! `crate::skill::draw_skill_panel` (below the Rules region — see that fn) for the
//! currently open skill. Layout, top to bottom:
//! 1. **Phases & Charge** — windup/active/recovery drags, `chargeable`/`max_hold`,
//!    `chain_radius` (editable only when `rules.damage.can_chain` — otherwise a
//!    read-only note, since it's dead data without a chainable beam skill, per
//!    `obelisk_bevy::assets::CastTimeline::chain_radius`'s own doc comment).
//! 2. **Acquisition** — one card: a plain-language variant dropdown (`ACQUISITION_CATALOG`,
//!    same hand-built-catalog idiom as `panel::rules`'s `CONDITION_CATALOG`), the fallible
//!    variants' `range`/`filter` fields, and a ONE-level-deep `AcqFallback::Then` editor
//!    (`draw_fallback`) — a chain going deeper renders read-only, matching the templates
//!    (no archetype template nests past one level).
//! 3. **Windows (`N`)** — one card per `CollisionWindow`: id (uniqueness-guarded rename),
//!    spawn kind (Scheduled phase+offset, or a Template note), shape, motion (+ a Down
//!    override toggle), anchor + offset, strikes/hit filter/hit mode/rehit/fuse, and an
//!    emitter sub-card. A "+ window" menu offers the four `edits::WindowArchetype` shapes.
//!
//! Every mutation flips `entry.dirty_timeline` (checked once at the end of
//! `draw_behavior_region`, OR-ing every helper's returned `bool` — the same idiom
//! `panel::rules::draw_rules_region` uses for `dirty_rules`).
//!
//! **Live validation.** Every frame, `obelisk_bevy::assets::validate_timeline` runs
//! against the CURRENT (already-edited-this-frame) `entry.timeline` copy; every one of
//! its `Err` messages names the offending window by id in single quotes (`"window
//! '{id}' ..."` — see that fn's doc comment), so `draw_windows` matches the message
//! against each card's id and renders it inline there — no separate "which window is
//! this about" logic needed. `validate_timeline` stops at its first failure, so at most
//! one live message shows per frame; Task 8's real `ValidationReport` (`report.for_window`,
//! added alongside this task — see `crate::skill::validation`) is the multi-problem
//! channel once that lands for real (it's still the always-empty stub today).
//!
//! **Guard-refusal messages.** `edits.rs`'s pure fns return `Result<(), String>` for
//! operations that can be refused (rename collision, remove blocked by an emitter
//! reference, spawn-kind flip blocked, etc). Since this module is a stack of plain
//! functions with no persistent UI-only state of its own, a refusal's message is stashed in
//! egui's per-context transient memory keyed by the window's id (`guard_error`/
//! `set_guard_error`/`clear_guard_error`) and rendered on that window's card until a
//! later successful edit on the same card clears it — mirrors how every other transient
//! "sticky until dismissed" UI state in this codebase is threaded (`ui.ctx().data_mut`,
//! see e.g. `src/ui/vfx_editor.rs`'s `AxisLinkState`).

use bevy_egui::egui;

use obelisk_bevy::assets::{
    AcqFallback, Acquisition, CastTimeline, CollisionShape, CollisionWindow, HitFilter, HitMode,
    MotionDirection, VolumeMotion, WindowAnchor, WindowPhase, WindowSpawn,
};

use crate::skill::edits::{self, WindowArchetype};
use crate::skill::library::SkillEntry;
use crate::skill::validation::ValidationReport;
use crate::ui::theme::{colors, grid_label, section_header, GRID_SPACING};

/// Draw the whole Behavior region for `entry` (the currently open skill). See the module
/// doc comment for the layout and the dirty/validation/guard-error conventions every
/// helper below follows.
///
/// `selected_window` is Task 12's viewport-proxy selection signal (`crate::skill::proxies::
/// SkillSelection::window`, threaded here as a plain `&mut Option<usize>` — the caller,
/// `crate::skill::draw_skill_panel`, snapshots the resource before this closure runs and writes
/// the (possibly-changed) value back after it ends, the same deferred-write pattern every other
/// cross-region signal in this panel uses). A window card's select toggle writes it; removing the
/// selected window (or a lower-indexed one, shifting indices down) here keeps it valid — see
/// `draw_windows`.
pub fn draw_behavior_region(
    ui: &mut egui::Ui,
    entry: &mut SkillEntry,
    report: &ValidationReport,
    selected_window: &mut Option<usize>,
) {
    let mut changed = false;

    section_header(ui, "Phases & Charge", true, |ui| {
        changed |= draw_phases_and_charge(ui, entry);
    });

    section_header(ui, "Acquisition", true, |ui| {
        changed |= draw_acquisition_card(ui, &mut entry.timeline.acquisition);
    });

    // Live-validate the timeline AFTER this frame's Phases/Acquisition edits (both can
    // affect window validity: chargeable fields don't, but acquisition reachability
    // does) and BEFORE the window cards render, so a just-typed acquisition change's
    // consequence shows up on the right card the same frame.
    let live_error = obelisk_bevy::assets::validate_timeline(&entry.timeline).err();

    let window_count = entry.timeline.collision_windows.len();
    section_header(ui, &format!("Windows ({window_count})"), true, |ui| {
        changed |= draw_windows(ui, &mut entry.timeline, live_error.as_deref(), report, selected_window);
    });

    if changed {
        entry.dirty_timeline = true;
    }
}

// ---------------------------------------------------------------------------
// Phases & Charge
// ---------------------------------------------------------------------------

fn draw_phases_and_charge(ui: &mut egui::Ui, entry: &mut SkillEntry) -> bool {
    let mut changed = false;
    let can_chain = entry.rules.damage.can_chain;

    egui::Grid::new("behavior_phases_grid")
        .num_columns(2)
        .spacing(GRID_SPACING)
        .show(ui, |ui| {
            grid_label(ui, "Windup");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut entry.timeline.phase_durations.windup)
                        .range(0.0..=30.0)
                        .speed(0.01)
                        .suffix(" s"),
                )
                .changed();
            ui.end_row();

            grid_label(ui, "Active");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut entry.timeline.phase_durations.active)
                        .range(0.0..=30.0)
                        .speed(0.01)
                        .suffix(" s"),
                )
                .changed();
            ui.end_row();

            grid_label(ui, "Recovery");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut entry.timeline.phase_durations.recovery)
                        .range(0.0..=30.0)
                        .speed(0.01)
                        .suffix(" s"),
                )
                .changed();
            ui.end_row();

            grid_label(ui, "Chargeable");
            changed |= ui.checkbox(&mut entry.timeline.chargeable, "").changed();
            ui.end_row();

            grid_label(ui, "Max Hold");
            ui.add_enabled_ui(entry.timeline.chargeable, |ui| {
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut entry.timeline.max_hold)
                            .range(0.1..=30.0)
                            .speed(0.01)
                            .suffix(" s"),
                    )
                    .changed();
            });
            ui.end_row();

            grid_label(ui, "Chain Radius");
            if can_chain {
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut entry.timeline.chain_radius)
                            .range(0.0..=100.0)
                            .speed(0.1)
                            .suffix(" m"),
                    )
                    .changed();
            } else {
                ui.label(
                    egui::RichText::new(format!(
                        "{:.1} m (inert — Rules \u{2192} Costs & Damage \u{2192} Can Chain is off)",
                        entry.timeline.chain_radius
                    ))
                    .color(colors::TEXT_MUTED)
                    .small(),
                );
            }
            ui.end_row();
        });

    changed
}

// ---------------------------------------------------------------------------
// Acquisition card
// ---------------------------------------------------------------------------

struct AcquisitionCatalogEntry {
    label: &'static str,
    make: fn() -> Acquisition,
}

/// Plain-language acquisition variant labels (brief: "Free aim" / "Hitscan target" /
/// "Ground point" / "Self") — same hand-built-catalog idiom as `panel::rules`'s
/// `CONDITION_CATALOG`.
const ACQUISITION_CATALOG: &[AcquisitionCatalogEntry] = &[
    AcquisitionCatalogEntry { label: "Free aim", make: || Acquisition::Aim },
    AcquisitionCatalogEntry { label: "Self", make: || Acquisition::SelfPoint },
    AcquisitionCatalogEntry {
        label: "Hitscan target",
        make: || Acquisition::HitscanEntity {
            range: 20.0,
            filter: HitFilter::Enemies,
            fallback: AcqFallback::Fizzle,
        },
    },
    AcquisitionCatalogEntry {
        label: "Ground point",
        make: || Acquisition::GroundPoint { range: 20.0, fallback: AcqFallback::Fizzle },
    },
];

fn acquisition_label(acq: &Acquisition) -> &'static str {
    match acq {
        Acquisition::Aim => "Free aim",
        Acquisition::SelfPoint => "Self",
        Acquisition::HitscanEntity { .. } => "Hitscan target",
        Acquisition::GroundPoint { .. } => "Ground point",
    }
}

fn acquisition_picker(ui: &mut egui::Ui, id_salt: impl std::hash::Hash, acq: &mut Acquisition) -> bool {
    let mut changed = false;
    let current = acquisition_label(acq);
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(current)
        .width(150.0)
        .show_ui(ui, |ui| {
            for entry in ACQUISITION_CATALOG {
                if ui.selectable_label(current == entry.label, entry.label).clicked() && current != entry.label {
                    *acq = (entry.make)();
                    changed = true;
                }
            }
        });
    changed
}

fn draw_acquisition_card(ui: &mut egui::Ui, acq: &mut Acquisition) -> bool {
    let mut changed = false;

    let frame = egui::Frame::new()
        .fill(colors::BG_MEDIUM)
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::same(6));

    frame.show(ui, |ui| {
        egui::Grid::new("acquisition_grid")
            .num_columns(2)
            .spacing(GRID_SPACING)
            .show(ui, |ui| {
                grid_label(ui, "Mode");
                changed |= acquisition_picker(ui, "acq_variant", acq);
                ui.end_row();

                match acq {
                    Acquisition::Aim | Acquisition::SelfPoint => {}
                    Acquisition::HitscanEntity { range, filter, .. } => {
                        grid_label(ui, "Range");
                        changed |= ui
                            .add(egui::DragValue::new(range).range(0.5..=200.0).speed(0.1).suffix(" m"))
                            .changed();
                        ui.end_row();
                        grid_label(ui, "Filter");
                        changed |= hit_filter_picker(ui, "acq_hitscan_filter", filter);
                        ui.end_row();
                    }
                    Acquisition::GroundPoint { range, .. } => {
                        grid_label(ui, "Range");
                        changed |= ui
                            .add(egui::DragValue::new(range).range(0.5..=200.0).speed(0.1).suffix(" m"))
                            .changed();
                        ui.end_row();
                    }
                }
            });

        match acq {
            Acquisition::Aim | Acquisition::SelfPoint => {}
            Acquisition::HitscanEntity { fallback, .. } | Acquisition::GroundPoint { fallback, .. } => {
                ui.add_space(2.0);
                changed |= draw_fallback(ui, fallback, 0);
            }
        }
    });

    changed
}

/// One level of `AcqFallback::Then` nesting is editable in the UI; a chain going deeper
/// than that renders as a read-only note (matches the templates — `zone_template`'s
/// `GroundPoint { fallback: Then(SelfPoint) }` is exactly one level; no archetype
/// template goes deeper).
fn draw_fallback(ui: &mut egui::Ui, fallback: &mut AcqFallback, depth: u8) -> bool {
    let mut changed = false;

    if depth >= 2 {
        ui.label(
            egui::RichText::new("\u{2026} deeper fallback chain (edit the .cast.ron directly)")
                .small()
                .color(colors::TEXT_MUTED),
        );
        return false;
    }

    egui::Grid::new(("acq_fallback_grid", depth))
        .num_columns(2)
        .spacing(GRID_SPACING)
        .show(ui, |ui| {
            grid_label(ui, "On Miss");
            let is_fizzle = matches!(fallback, AcqFallback::Fizzle);
            egui::ComboBox::from_id_salt(("acq_fallback_kind", depth))
                .selected_text(if is_fizzle { "Fizzle (reject cast)" } else { "Then\u{2026}" })
                .width(150.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(is_fizzle, "Fizzle (reject cast)").clicked() && !is_fizzle {
                        *fallback = AcqFallback::Fizzle;
                        changed = true;
                    }
                    if ui.selectable_label(!is_fizzle, "Then\u{2026}").clicked() && is_fizzle {
                        *fallback = AcqFallback::Then(Box::new(Acquisition::SelfPoint));
                        changed = true;
                    }
                });
            ui.end_row();
        });

    if let AcqFallback::Then(inner) = fallback {
        ui.indent(("acq_fallback_indent", depth), |ui| {
            egui::Grid::new(("acq_fallback_inner_grid", depth))
                .num_columns(2)
                .spacing(GRID_SPACING)
                .show(ui, |ui| {
                    grid_label(ui, "Then");
                    changed |= acquisition_picker(ui, ("acq_fallback_inner_variant", depth), inner);
                    ui.end_row();

                    match inner.as_mut() {
                        Acquisition::Aim | Acquisition::SelfPoint => {}
                        Acquisition::HitscanEntity { range, filter, .. } => {
                            grid_label(ui, "Range");
                            changed |= ui
                                .add(egui::DragValue::new(range).range(0.5..=200.0).speed(0.1).suffix(" m"))
                                .changed();
                            ui.end_row();
                            grid_label(ui, "Filter");
                            changed |= hit_filter_picker(ui, ("acq_fallback_inner_filter", depth), filter);
                            ui.end_row();
                        }
                        Acquisition::GroundPoint { range, .. } => {
                            grid_label(ui, "Range");
                            changed |= ui
                                .add(egui::DragValue::new(range).range(0.5..=200.0).speed(0.1).suffix(" m"))
                                .changed();
                            ui.end_row();
                        }
                    }
                });

            match inner.as_mut() {
                Acquisition::Aim | Acquisition::SelfPoint => {}
                Acquisition::HitscanEntity { fallback: inner_fb, .. }
                | Acquisition::GroundPoint { fallback: inner_fb, .. } => {
                    changed |= draw_fallback(ui, inner_fb, depth + 1);
                }
            }
        });
    }

    changed
}

// ---------------------------------------------------------------------------
// Shared enum pickers (obelisk's cast enums don't derive `loot_core::EnumVariants` — hand
// -built pickers, same fallback idiom `panel::rules::damage_type_dropdown` establishes for
// stat_core enums that DO have it and this one can't reach for anyway).
// ---------------------------------------------------------------------------

fn hit_filter_picker(ui: &mut egui::Ui, id_salt: impl std::hash::Hash, filter: &mut HitFilter) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(format!("{filter:?}"))
        .width(90.0)
        .show_ui(ui, |ui| {
            for f in [HitFilter::Caster, HitFilter::Allies, HitFilter::Enemies, HitFilter::All] {
                if ui.selectable_value(filter, f, format!("{f:?}")).clicked() {
                    changed = true;
                }
            }
        });
    changed
}

fn hit_mode_label(mode: &HitMode) -> &'static str {
    match mode {
        HitMode::OncePerTarget => "Once per target",
        HitMode::FirstOnly => "First hit only",
        HitMode::EveryTick => "Every tick",
    }
}

fn hit_mode_picker(ui: &mut egui::Ui, id_salt: impl std::hash::Hash, mode: &mut HitMode) -> bool {
    let mut changed = false;
    let current = hit_mode_label(mode);
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(current)
        .width(120.0)
        .show_ui(ui, |ui| {
            for variant in [HitMode::OncePerTarget, HitMode::FirstOnly, HitMode::EveryTick] {
                let label = hit_mode_label(&variant);
                if ui.selectable_label(current == label, label).clicked() && current != label {
                    *mode = variant;
                    changed = true;
                }
            }
        });
    changed
}

fn shape_label(shape: &CollisionShape) -> &'static str {
    match shape {
        CollisionShape::Sphere { .. } => "Sphere",
        CollisionShape::Capsule { .. } => "Capsule",
        CollisionShape::Cone { .. } => "Cone",
    }
}

/// `(plain-language label, variant constructor)` — the shape of every "variant picker with
/// sensible per-variant defaults" table in this module (mirrors `ACQUISITION_CATALOG`'s
/// `AcquisitionCatalogEntry` shape, just inline since these two are used in exactly one
/// place each).
type VariantOption<T> = (&'static str, fn() -> T);

fn shape_picker(ui: &mut egui::Ui, id_salt: impl std::hash::Hash, shape: &mut CollisionShape) -> bool {
    let mut changed = false;
    let current = shape_label(shape);
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(current)
        .width(90.0)
        .show_ui(ui, |ui| {
            let options: [VariantOption<CollisionShape>; 3] = [
                ("Sphere", || CollisionShape::Sphere { radius: 0.5 }),
                ("Capsule", || CollisionShape::Capsule { radius: 0.4, height: 1.0 }),
                ("Cone", || CollisionShape::Cone { angle: 90.0, range: 5.0 }),
            ];
            for (label, make) in options {
                if ui.selectable_label(current == label, label).clicked() && current != label {
                    *shape = make();
                    changed = true;
                }
            }
        });
    changed
}

fn motion_label(motion: &VolumeMotion) -> &'static str {
    match motion {
        VolumeMotion::Static => "Static",
        VolumeMotion::Linear { .. } => "Linear",
        VolumeMotion::Ballistic { .. } => "Ballistic",
        VolumeMotion::Beam => "Beam",
    }
}

fn motion_picker(ui: &mut egui::Ui, id_salt: impl std::hash::Hash, motion: &mut VolumeMotion) -> bool {
    let mut changed = false;
    let current = motion_label(motion);
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(current)
        .width(90.0)
        .show_ui(ui, |ui| {
            let options: [VariantOption<VolumeMotion>; 4] = [
                ("Static", || VolumeMotion::Static),
                ("Linear", || VolumeMotion::Linear { speed: 20.0 }),
                ("Ballistic", || VolumeMotion::Ballistic { speed: 20.0, gravity: 20.0 }),
                ("Beam", || VolumeMotion::Beam),
            ];
            for (label, make) in options {
                if ui.selectable_label(current == label, label).clicked() && current != label {
                    *motion = make();
                    changed = true;
                }
            }
        });
    changed
}

// ---------------------------------------------------------------------------
// Guard-error transient memory (see module doc comment)
// ---------------------------------------------------------------------------

fn guard_error_id(window_id: &str) -> egui::Id {
    egui::Id::new(("skill_behavior_guard_error", window_id))
}

fn set_guard_error(ui: &egui::Ui, window_id: &str, msg: String) {
    ui.ctx().data_mut(|d| d.insert_temp(guard_error_id(window_id), msg));
}

fn clear_guard_error(ui: &egui::Ui, window_id: &str) {
    ui.ctx().data_mut(|d| d.remove::<String>(guard_error_id(window_id)));
}

fn guard_error(ui: &egui::Ui, window_id: &str) -> Option<String> {
    ui.ctx().data_mut(|d| d.get_temp::<String>(guard_error_id(window_id)))
}

// ---------------------------------------------------------------------------
// Window cards
// ---------------------------------------------------------------------------

struct WindowCardOutcome {
    changed: bool,
    remove_requested: bool,
}

fn draw_windows(
    ui: &mut egui::Ui,
    tl: &mut CastTimeline,
    live_error: Option<&str>,
    report: &ValidationReport,
    selected_window: &mut Option<usize>,
) -> bool {
    let mut changed = false;
    let template_ids: Vec<String> = tl
        .collision_windows
        .iter()
        .filter(|w| w.spawn == WindowSpawn::Template)
        .map(|w| w.id.clone())
        .collect();

    let mut remove_index = None;
    let count = tl.collision_windows.len();
    for i in 0..count {
        let id = tl.collision_windows[i].id.clone();
        let card_live_error = live_error.filter(|msg| msg.contains(&format!("'{id}'")));
        let outcome = window_card(ui, i, tl, &template_ids, card_live_error, report, selected_window);
        changed |= outcome.changed;
        if outcome.remove_requested {
            remove_index = Some(i);
        }
    }

    if let Some(i) = remove_index {
        let id = tl.collision_windows[i].id.clone();
        match edits::remove_window(tl, i) {
            Ok(()) => {
                changed = true;
                // Keep the viewport-proxy selection (Task 12) valid across a removal: the
                // removed index itself deselects; anything ABOVE it shifts down by one (indices
                // past `i` all moved back a slot); anything below is unaffected.
                *selected_window = match *selected_window {
                    Some(s) if s == i => None,
                    Some(s) if s > i => Some(s - 1),
                    other => other,
                };
            }
            Err(e) => set_guard_error(ui, &id, e),
        }
    }

    ui.add_space(4.0);
    ui.menu_button(egui::RichText::new("+ window").color(colors::ACCENT_GREEN), |ui| {
        for archetype in WindowArchetype::ALL {
            if ui.button(archetype.label()).clicked() {
                edits::add_window_from_archetype(tl, archetype);
                changed = true;
                ui.close();
            }
        }
    });

    changed
}

fn window_id_editor(ui: &mut egui::Ui, idx: usize, tl: &mut CastTimeline) -> bool {
    let mut changed = false;
    let current_id = tl.collision_windows[idx].id.clone();
    let buf_key = egui::Id::new(("skill_window_id_buf", idx));

    let mut buf = ui
        .ctx()
        .data_mut(|d| d.get_temp::<String>(buf_key))
        .unwrap_or_else(|| current_id.clone());
    let resp = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(90.0));

    if resp.lost_focus() && buf != current_id {
        match edits::rename_window(tl, idx, &buf) {
            Ok(()) => {
                changed = true;
                clear_guard_error(ui, &current_id);
            }
            Err(e) => set_guard_error(ui, &current_id, e),
        }
    }

    let now_id = tl.collision_windows[idx].id.clone();
    let persisted = if resp.has_focus() { buf } else { now_id };
    ui.ctx().data_mut(|d| d.insert_temp(buf_key, persisted));

    changed
}

fn draw_spawn_kind(ui: &mut egui::Ui, idx: usize, tl: &mut CastTimeline) -> bool {
    let mut changed = false;
    let window_id = tl.collision_windows[idx].id.clone();
    let is_template = tl.collision_windows[idx].spawn == WindowSpawn::Template;

    egui::Grid::new(("window_spawn_grid", idx))
        .num_columns(2)
        .spacing(GRID_SPACING)
        .show(ui, |ui| {
            grid_label(ui, "Spawn");
            let current = if is_template { "Template" } else { "Scheduled" };
            egui::ComboBox::from_id_salt(("window_spawn_kind", idx))
                .selected_text(current)
                .width(100.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(!is_template, "Scheduled").clicked() && is_template {
                        match edits::set_spawn_kind(tl, idx, false) {
                            Ok(()) => {
                                changed = true;
                                clear_guard_error(ui, &window_id);
                            }
                            Err(e) => set_guard_error(ui, &window_id, e),
                        }
                    }
                    if ui.selectable_label(is_template, "Template").clicked() && !is_template {
                        match edits::set_spawn_kind(tl, idx, true) {
                            Ok(()) => {
                                changed = true;
                                clear_guard_error(ui, &window_id);
                            }
                            Err(e) => set_guard_error(ui, &window_id, e),
                        }
                    }
                });
            ui.end_row();
        });

    match &mut tl.collision_windows[idx].spawn {
        WindowSpawn::Scheduled { phase, offset } => {
            egui::Grid::new(("window_spawn_fields", idx))
                .num_columns(2)
                .spacing(GRID_SPACING)
                .show(ui, |ui| {
                    grid_label(ui, "Phase");
                    egui::ComboBox::from_id_salt(("window_phase", idx))
                        .selected_text(format!("{phase:?}"))
                        .width(90.0)
                        .show_ui(ui, |ui| {
                            for p in [WindowPhase::Windup, WindowPhase::Active, WindowPhase::Recovery] {
                                if ui.selectable_value(phase, p, format!("{p:?}")).clicked() {
                                    changed = true;
                                }
                            }
                        });
                    ui.end_row();

                    grid_label(ui, "Offset");
                    changed |= ui
                        .add(egui::DragValue::new(offset).range(0.0..=30.0).speed(0.01).suffix(" s"))
                        .changed();
                    ui.end_row();
                });
        }
        WindowSpawn::Template => {
            ui.label(
                egui::RichText::new("Template — spawned only by an emitter (see below)")
                    .small()
                    .color(colors::TEXT_MUTED),
            );
        }
    }

    changed
}

fn draw_shape(ui: &mut egui::Ui, idx: usize, shape: &mut CollisionShape) -> bool {
    let mut changed = false;
    egui::Grid::new(("window_shape_grid", idx))
        .num_columns(2)
        .spacing(GRID_SPACING)
        .show(ui, |ui| {
            grid_label(ui, "Shape");
            changed |= shape_picker(ui, ("window_shape_kind", idx), shape);
            ui.end_row();

            match shape {
                CollisionShape::Sphere { radius } => {
                    grid_label(ui, "Radius");
                    changed |= ui
                        .add(egui::DragValue::new(radius).range(0.05..=50.0).speed(0.02).suffix(" m"))
                        .changed();
                    ui.end_row();
                }
                CollisionShape::Capsule { radius, height } => {
                    grid_label(ui, "Radius");
                    changed |= ui
                        .add(egui::DragValue::new(radius).range(0.05..=50.0).speed(0.02).suffix(" m"))
                        .changed();
                    ui.end_row();
                    grid_label(ui, "Height");
                    changed |= ui
                        .add(egui::DragValue::new(height).range(0.05..=50.0).speed(0.02).suffix(" m"))
                        .changed();
                    ui.end_row();
                }
                CollisionShape::Cone { angle, range } => {
                    grid_label(ui, "Angle");
                    changed |= ui
                        .add(egui::DragValue::new(angle).range(1.0..=360.0).speed(0.5).suffix("\u{00b0}"))
                        .changed();
                    ui.end_row();
                    grid_label(ui, "Range");
                    changed |= ui
                        .add(egui::DragValue::new(range).range(0.1..=100.0).speed(0.05).suffix(" m"))
                        .changed();
                    ui.end_row();
                }
            }
        });
    changed
}

fn draw_motion(ui: &mut egui::Ui, idx: usize, w: &mut CollisionWindow) -> bool {
    let mut changed = false;
    egui::Grid::new(("window_motion_grid", idx))
        .num_columns(2)
        .spacing(GRID_SPACING)
        .show(ui, |ui| {
            grid_label(ui, "Motion");
            changed |= motion_picker(ui, ("window_motion_kind", idx), &mut w.motion);
            ui.end_row();

            match &mut w.motion {
                VolumeMotion::Static => {
                    ui.label(
                        egui::RichText::new("anchored where it spawns").small().color(colors::TEXT_MUTED),
                    );
                    ui.end_row();
                }
                VolumeMotion::Linear { speed } => {
                    grid_label(ui, "Speed");
                    changed |= ui
                        .add(egui::DragValue::new(speed).range(0.1..=200.0).speed(0.2).suffix(" m/s"))
                        .changed();
                    ui.end_row();
                }
                VolumeMotion::Ballistic { speed, gravity } => {
                    grid_label(ui, "Speed");
                    changed |= ui
                        .add(egui::DragValue::new(speed).range(0.1..=200.0).speed(0.2).suffix(" m/s"))
                        .changed();
                    ui.end_row();
                    grid_label(ui, "Gravity");
                    changed |= ui
                        .add(egui::DragValue::new(gravity).range(0.0..=200.0).speed(0.2).suffix(" m/s\u{00b2}"))
                        .changed();
                    ui.end_row();
                }
                VolumeMotion::Beam => {
                    ui.label(
                        egui::RichText::new("strikes the designated target instantly")
                            .small()
                            .color(colors::TEXT_MUTED),
                    );
                    ui.end_row();
                }
            }

            grid_label(ui, "Direction");
            let mut force_down = w.motion_direction == MotionDirection::Down;
            if ui.checkbox(&mut force_down, "Force straight down").changed() {
                w.motion_direction = if force_down { MotionDirection::Down } else { MotionDirection::Inherit };
                changed = true;
            }
            ui.end_row();
        });
    changed
}

fn draw_anchor(ui: &mut egui::Ui, idx: usize, w: &mut CollisionWindow) -> bool {
    let mut changed = false;
    egui::Grid::new(("window_anchor_grid", idx))
        .num_columns(2)
        .spacing(GRID_SPACING)
        .show(ui, |ui| {
            grid_label(ui, "Anchor");
            ui.horizontal(|ui| {
                if ui.selectable_value(&mut w.anchor, WindowAnchor::Caster, "Caster").clicked() {
                    changed = true;
                }
                if ui.selectable_value(&mut w.anchor, WindowAnchor::CastPoint, "Cast Point").clicked() {
                    changed = true;
                }
            });
            ui.end_row();

            grid_label(ui, "Offset");
            ui.horizontal(|ui| {
                changed |= ui.add(egui::DragValue::new(&mut w.anchor_offset.x).speed(0.05).prefix("x ")).changed();
                changed |= ui.add(egui::DragValue::new(&mut w.anchor_offset.y).speed(0.05).prefix("y ")).changed();
                changed |= ui.add(egui::DragValue::new(&mut w.anchor_offset.z).speed(0.05).prefix("z ")).changed();
            });
            ui.end_row();
        });

    if w.anchor == WindowAnchor::CastPoint {
        ui.label(
            egui::RichText::new(
                "Cast Point requires an acquisition that can produce a point (Ground Point or \
                 Self, directly or via a fallback) — see the Acquisition card above.",
            )
            .small()
            .color(colors::TEXT_MUTED),
        );
    }

    changed
}

fn draw_hits(ui: &mut egui::Ui, idx: usize, w: &mut CollisionWindow) -> bool {
    let mut changed = false;
    egui::Grid::new(("window_hits_grid", idx))
        .num_columns(2)
        .spacing(GRID_SPACING)
        .show(ui, |ui| {
            grid_label(ui, "Strikes");
            let strikes_resp = ui.checkbox(&mut w.strikes, "");
            changed |= strikes_resp.changed();
            strikes_resp.on_hover_text(
                "On: a normal hitbox — can produce hits. Off: a carrier volume — it flies, \
                 ends, and fires cues/events, but overlap detection skips it entirely; it can \
                 never confirm a hit (e.g. a beam's visual trail riding alongside the real \
                 strike window).",
            );
            ui.end_row();

            grid_label(ui, "Hit Filter");
            changed |= hit_filter_picker(ui, ("window_hit_filter", idx), &mut w.hit_filter);
            ui.end_row();

            grid_label(ui, "Hit Mode");
            changed |= hit_mode_picker(ui, ("window_hit_mode", idx), &mut w.hit_mode);
            ui.end_row();

            grid_label(ui, "Rehit Interval");
            let mut rehit = w.rehit_interval.unwrap_or(0.0);
            let rehit_resp = ui
                .add(egui::DragValue::new(&mut rehit).range(0.0..=10.0).speed(0.02).suffix(" s (0 = off)"));
            if rehit_resp.changed() {
                w.rehit_interval = (rehit > 0.0).then_some(rehit);
                changed = true;
            }
            ui.end_row();

            grid_label(ui, "Fuse");
            changed |= ui
                .add(egui::DragValue::new(&mut w.active_duration).range(0.01..=60.0).speed(0.01).suffix(" s"))
                .changed();
            ui.end_row();
        });
    changed
}

fn draw_emitter_subcard(ui: &mut egui::Ui, idx: usize, tl: &mut CastTimeline, template_ids: &[String]) -> bool {
    let mut changed = false;
    let window_id = tl.collision_windows[idx].id.clone();
    let has_emitter = tl.collision_windows[idx].emitter.is_some();

    ui.add_space(4.0);

    if has_emitter {
        let frame = egui::Frame::new()
            .fill(colors::BG_DARKEST)
            .corner_radius(egui::CornerRadius::same(4))
            .inner_margin(egui::Margin::same(6));

        // Deferred removal (module doc comment's "deferred-write pattern"): the × below only
        // flags `remove_clicked` this frame — the actual `edits::remove_emitter` call (which
        // clears `emitter`) happens AFTER `frame.show` returns, not before. The
        // `if !remove_clicked` guard below skips the field-editing grid (and its
        // `.as_mut().unwrap()`) entirely on the same frame the × was clicked, so nothing ever
        // unwraps the field this same call — mirrors how the "+ emitter" path below never
        // reads a freshly-`Some` field until the frame after it's added.
        let mut remove_clicked = false;
        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Emitter").strong().color(colors::ACCENT_CYAN));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(egui::RichText::new("\u{00d7}").color(colors::STATUS_ERROR)).frame(false))
                        .on_hover_text("Remove emitter")
                        .clicked()
                    {
                        remove_clicked = true;
                    }
                });
            });

            if remove_clicked {
                return;
            }

            let emitter = tl.collision_windows[idx].emitter.as_mut().unwrap();
            egui::Grid::new(("emitter_grid", idx))
                .num_columns(2)
                .spacing(GRID_SPACING)
                .show(ui, |ui| {
                    grid_label(ui, "Rate");
                    changed |= ui
                        .add(egui::DragValue::new(&mut emitter.rate).range(0.1..=100.0).speed(0.1).suffix("/s"))
                        .changed();
                    ui.end_row();

                    grid_label(ui, "Jitter");
                    changed |= ui
                        .add(egui::DragValue::new(&mut emitter.jitter).range(0.0..=20.0).speed(0.05).suffix(" m"))
                        .changed();
                    ui.end_row();

                    grid_label(ui, "Target");
                    let current_target = emitter.window.clone();
                    let display = if current_target.is_empty() { "(none)" } else { current_target.as_str() };
                    let mut new_target = None;
                    egui::ComboBox::from_id_salt(("emitter_target", idx))
                        .selected_text(display)
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            for id in template_ids {
                                if ui.selectable_label(&current_target == id, id.as_str()).clicked()
                                    && &current_target != id
                                {
                                    new_target = Some(id.clone());
                                }
                            }
                        });
                    if let Some(id) = new_target {
                        emitter.window = id;
                        changed = true;
                    }
                    ui.end_row();
                });
        });

        if remove_clicked {
            match edits::remove_emitter(tl, idx) {
                Ok(()) => {
                    changed = true;
                    clear_guard_error(ui, &window_id);
                }
                Err(e) => set_guard_error(ui, &window_id, e),
            }
        }
    } else {
        ui.horizontal(|ui| {
            if !template_ids.is_empty() {
                egui::ComboBox::from_id_salt(("emitter_add_existing", idx))
                    .selected_text("+ emitter (existing Template)")
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        for id in template_ids {
                            if ui.button(id.as_str()).clicked() {
                                match edits::add_emitter(tl, idx, id) {
                                    Ok(()) => {
                                        changed = true;
                                        clear_guard_error(ui, &window_id);
                                    }
                                    Err(e) => set_guard_error(ui, &window_id, e),
                                }
                            }
                        }
                    });
            }
            if ui
                .button(egui::RichText::new("+ emitter (new shard)").color(colors::ACCENT_GREEN))
                .clicked()
            {
                edits::add_emitter_with_new_template(tl, idx);
                changed = true;
                clear_guard_error(ui, &window_id);
            }
        });
    }

    changed
}

#[allow(clippy::too_many_arguments)] // one param per thing the card reads/writes; same
                                      // rationale as `panel::rules::trigger_card`.
fn window_card(
    ui: &mut egui::Ui,
    idx: usize,
    tl: &mut CastTimeline,
    template_ids: &[String],
    live_error: Option<&str>,
    report: &ValidationReport,
    selected_window: &mut Option<usize>,
) -> WindowCardOutcome {
    let mut changed = false;
    let mut remove_requested = false;

    let window_id = tl.collision_windows[idx].id.clone();
    let is_selected = *selected_window == Some(idx);

    let frame = egui::Frame::new()
        .fill(if is_selected { colors::BG_HIGHLIGHT } else { colors::BG_MEDIUM })
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::same(6));

    let resp = frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            changed |= window_id_editor(ui, idx, tl);
            if tl.collision_windows[idx].spawn == WindowSpawn::Template {
                ui.label(egui::RichText::new("TEMPLATE").small().color(colors::ACCENT_ORANGE));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(egui::RichText::new("\u{00d7}").color(colors::STATUS_ERROR)).frame(false))
                    .on_hover_text("Remove window")
                    .clicked()
                {
                    remove_requested = true;
                }
                // Task 12: select this window for the viewport gizmo proxy
                // (`crate::skill::proxies`) — shows its shape (sphere/capsule/cone), resolved to
                // its stage anchor position, as a draggable-radius gizmo in the viewport.
                let (icon, color) =
                    if is_selected { ("\u{25c9}", colors::ACCENT_CYAN) } else { ("\u{25cb}", colors::TEXT_MUTED) };
                if ui
                    .add(egui::Button::new(egui::RichText::new(icon).color(color)).frame(false))
                    .on_hover_text(if is_selected {
                        "Selected \u{2014} shown as a gizmo in the viewport. Click to deselect."
                    } else {
                        "Select \u{2014} show this window's shape as a gizmo in the viewport"
                    })
                    .clicked()
                {
                    *selected_window = if is_selected { None } else { Some(idx) };
                }
            });
        });

        if let Some(msg) = guard_error(ui, &window_id) {
            ui.label(egui::RichText::new(msg).small().color(colors::STATUS_ERROR));
        }
        if let Some(msg) = live_error {
            ui.label(egui::RichText::new(msg).small().color(colors::STATUS_ERROR));
        }
        for problem in report.for_window(&window_id) {
            let color = if problem.blocking { colors::STATUS_ERROR } else { colors::STATUS_WARNING };
            ui.label(egui::RichText::new(&problem.message).small().color(color));
        }

        ui.add_space(2.0);
        changed |= draw_spawn_kind(ui, idx, tl);

        ui.add_space(2.0);
        changed |= draw_shape(ui, idx, &mut tl.collision_windows[idx].shape);

        ui.add_space(2.0);
        changed |= draw_motion(ui, idx, &mut tl.collision_windows[idx]);

        ui.add_space(2.0);
        changed |= draw_anchor(ui, idx, &mut tl.collision_windows[idx]);

        ui.add_space(2.0);
        changed |= draw_hits(ui, idx, &mut tl.collision_windows[idx]);

        changed |= draw_emitter_subcard(ui, idx, tl, template_ids);
    });

    // Accent stripe over the card's left edge, matching `panel::rules::trigger_card`'s
    // style — orange for a Template window (matches the "TEMPLATE" badge above), cyan for
    // an ordinary scheduled one.
    let is_template = tl.collision_windows[idx].spawn == WindowSpawn::Template;
    let card_rect = resp.response.rect;
    let stripe = egui::Rect::from_min_max(card_rect.left_top(), egui::pos2(card_rect.left() + 3.0, card_rect.bottom()));
    ui.painter().rect_filled(
        stripe,
        egui::CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 },
        if is_template { colors::ACCENT_ORANGE } else { colors::ACCENT_CYAN },
    );
    ui.add_space(4.0);

    WindowCardOutcome { changed, remove_requested }
}

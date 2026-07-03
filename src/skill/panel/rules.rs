//! Rules region (Task 6): task-first tiers, trigger cards, live readouts.
//!
//! `draw_rules_region` is the whole region's entry point, called from
//! `crate::skill::draw_skill_panel` for the currently open skill. Layout, top to bottom:
//! 1. **Readouts** — live-computed summary (`crate::skill::readouts`): per-hit damage, max
//!    strikes, full-chain range, dmg/mana.
//! 2. **Costs & Damage** (tier 1) — mana/cooldown, damage lines, crit, effect applications.
//! 3. **Triggers** — one card per `SkillCondition` in `rules.conditions`.
//! 4. **Advanced** (collapsed by default) — read-only summary of the v1 plumbing this region
//!    doesn't edit yet (damage conversions, use conditions, auras, conditional bonuses).
//!
//! Every mutation flips `entry.dirty_rules` (checked once at the end of `draw_rules_region`
//! rather than at every call site — every helper below returns `bool` "did anything change"
//! instead of touching `entry.dirty_rules` directly, so the flag can't be forgotten at a new
//! call site).
//!
//! The condition dropdown's variant table (`CONDITION_CATALOG`) and its per-variant payload
//! editor (`condition_payload_fields`) are adapted from the reference implementation in the
//! vendored obelisk checkout — `obelisk_editor/src/editors/global_conditional.rs`'s
//! `CONDITION_VARIANTS`/`edit_condition_fields` — re-typed against this crate's own widget
//! idioms (`grid_label`/`DragValue`/`ComboBox`, matching `src/ui/effect_editor.rs`) since
//! `obelisk_editor` is a sibling app in the obelisk repo, not a dependency here. `loot_core`'s
//! `TriggerCondition` has no `EnumVariants` impl (its variants carry payloads, which don't fit
//! that trait's `&'static [Self]` shape) — this hand-built catalog is the established pattern
//! for picking one anyway (see also `obelisk-arena`'s `trigger_ui.rs::trigger_prototypes`,
//! same idea).

use bevy_egui::egui;

use loot_core::types::EnumVariants;
use stat_core::skill::{ApplicationTarget, EffectApplication};
use stat_core::{BaseDamage, ConditionPhase, DamageType, Skill, SkillCondition, TriggerCondition};

use crate::skill::library::{SkillEntry, SkillLibrary};
use crate::skill::readouts::{entry_has_real_timeline, skill_readout};
use crate::skill::validation::ValidationReport;
use crate::ui::theme::{colors, grid_label, section_header, GRID_SPACING};

/// Draw the whole Rules region for `entry` (the currently open skill, already cloned out of
/// `library` by the caller — see `crate::skill::draw_skill_panel`). `library` is read-only here:
/// it feeds the trigger-target picker (every other skill id, including `entry`'s own — self-
/// triggers are legal, depth-capped elsewhere) and the readouts' one-level-deep triggered-strike
/// lookup. `report` is Task 8's `ValidationReport` (stub today, see `crate::skill::validation`)
/// — trigger cards render any `"condition:{index}"` problems inline.
pub fn draw_rules_region(
    ui: &mut egui::Ui,
    entry: &mut SkillEntry,
    library: &SkillLibrary,
    report: &ValidationReport,
) {
    draw_readouts(ui, entry, library);
    ui.add_space(6.0);

    let mut changed = false;

    section_header(ui, "Costs & Damage", true, |ui| {
        changed |= draw_tier1(ui, &mut entry.rules);
    });

    let trigger_count = entry.rules.conditions.len();
    section_header(ui, &format!("Triggers ({trigger_count})"), true, |ui| {
        changed |= draw_trigger_cards(ui, entry, library, report);
    });

    section_header(ui, "Advanced", false, |ui| {
        draw_advanced(ui, &entry.rules);
    });

    if changed {
        entry.dirty_rules = true;
    }
}

// ---------------------------------------------------------------------------
// Readouts
// ---------------------------------------------------------------------------

fn draw_readouts(ui: &mut egui::Ui, entry: &SkillEntry, library: &SkillLibrary) {
    let readout = skill_readout(&entry.rules, &entry.timeline, |target_id| {
        library.skills.get(target_id).is_some_and(entry_has_real_timeline)
    });

    let frame = egui::Frame::new()
        .fill(colors::BG_MEDIUM)
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::same(8));

    frame.show(ui, |ui| {
        ui.label(
            egui::RichText::new("Readouts")
                .strong()
                .color(colors::ACCENT_PURPLE),
        );

        let (lo, hi) = readout.per_hit;
        ui.label(format!("Per hit: {lo:.0}-{hi:.0}"));

        let s = readout.strikes;
        let approx_marker = if s.approximate() { " \u{2248}" } else { "" };
        let shards_marker = if s.has_unbounded_emitter { " + shards" } else { "" };
        let strikes_resp =
            ui.label(format!("Max strikes: {}{approx_marker}{shards_marker}", s.total()));
        strikes_resp.on_hover_text(format!(
            "{} scheduled + {} chain + {} triggered (triggered is \u{2248}, one level deep — see \
             the readouts module docs)",
            s.scheduled, s.chain, s.triggered
        ));

        let (flo, fhi) = readout.full_chain;
        ui.label(format!("Full range: {flo:.0}-{fhi:.0}{approx_marker}"));

        match readout.per_mana {
            Some((plo, phi)) => {
                ui.label(format!("Dmg/mana: {plo:.2}-{phi:.2}{approx_marker}"));
            }
            None => {
                ui.label(egui::RichText::new("Free (no mana cost)").color(colors::TEXT_MUTED));
            }
        }

        if readout.crit_chance > 0.0 {
            ui.label(format!("Crit chance: {:.1}%", readout.crit_chance));
        }
    });
}

// ---------------------------------------------------------------------------
// Tier 1 — costs, damage, crit, effect applications
// ---------------------------------------------------------------------------

fn draw_tier1(ui: &mut egui::Ui, rules: &mut Skill) -> bool {
    let mut changed = false;

    egui::Grid::new("rules_tier1_costs")
        .num_columns(2)
        .spacing(GRID_SPACING)
        .show(ui, |ui| {
            grid_label(ui, "Mana Cost");
            changed |= ui
                .add(egui::DragValue::new(&mut rules.mana_cost).range(0.0..=10_000.0).speed(0.5))
                .changed();
            ui.end_row();

            grid_label(ui, "Cooldown");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut rules.cooldown)
                        .range(0.0..=600.0)
                        .speed(0.1)
                        .suffix(" s"),
                )
                .changed();
            ui.end_row();
        });

    ui.add_space(6.0);
    ui.label(egui::RichText::new("Damage").strong().color(colors::TEXT_PRIMARY));
    changed |= draw_damage_lines(ui, &mut rules.damage.base_damages);

    ui.add_space(6.0);
    egui::Grid::new("rules_tier1_crit")
        .num_columns(2)
        .spacing(GRID_SPACING)
        .show(ui, |ui| {
            grid_label(ui, "Crit Chance");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut rules.damage.base_crit_chance)
                        .range(0.0..=100.0)
                        .speed(0.5)
                        .suffix("%"),
                )
                .changed();
            ui.end_row();

            grid_label(ui, "Crit Multi Bonus");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut rules.damage.crit_multiplier_bonus)
                        .range(0.0..=10.0)
                        .speed(0.01),
                )
                .changed();
            ui.end_row();

            grid_label(ui, "Guaranteed Crit");
            changed |= ui.checkbox(&mut rules.damage.guaranteed_crit, "").changed();
            ui.end_row();
        });

    ui.add_space(6.0);
    ui.label(
        egui::RichText::new("Effect Applications")
            .strong()
            .color(colors::TEXT_PRIMARY),
    );
    changed |= draw_effect_applications(ui, &mut rules.effect_applications);

    changed
}

fn draw_damage_lines(ui: &mut egui::Ui, base_damages: &mut Vec<BaseDamage>) -> bool {
    let mut changed = false;
    let mut remove_index = None;

    for (i, bd) in base_damages.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            changed |= damage_type_dropdown(ui, ("dmg_line_type", i), &mut bd.damage_type);
            ui.label("min");
            changed |= ui
                .add(egui::DragValue::new(&mut bd.min).range(0.0..=1_000_000.0).speed(0.5))
                .changed();
            ui.label("max");
            changed |= ui
                .add(egui::DragValue::new(&mut bd.max).range(0.0..=1_000_000.0).speed(0.5))
                .changed();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(egui::RichText::new("\u{00d7}").color(colors::STATUS_ERROR)).frame(false))
                    .on_hover_text("Remove damage line")
                    .clicked()
                {
                    remove_index = Some(i);
                }
            });
        });
    }

    if let Some(i) = remove_index {
        base_damages.remove(i);
        changed = true;
    }

    if ui
        .button(egui::RichText::new("+ damage line").color(colors::ACCENT_GREEN))
        .clicked()
    {
        base_damages.push(BaseDamage::new(DamageType::Physical, 0.0, 0.0));
        changed = true;
    }

    changed
}

fn draw_effect_applications(ui: &mut egui::Ui, apps: &mut Vec<EffectApplication>) -> bool {
    let mut changed = false;

    // Guard exactly like arena_editor's `effect_id_list` (crates/arena_editor/src/panel.rs):
    // `effect_registry()` PANICS if uninitialized, so every read goes through
    // `effect_registry_initialized()` first — empty picker (falls back to free text) rather than
    // a crash when no content root has populated the registry yet.
    let effect_ids: Vec<String> = if stat_core::config::effect_registry_initialized() {
        let mut ids: Vec<String> = stat_core::config::effect_registry()
            .all_ids()
            .into_iter()
            .map(str::to_owned)
            .collect();
        ids.sort();
        ids
    } else {
        Vec::new()
    };

    let mut remove_index = None;
    for (i, app) in apps.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if effect_ids.is_empty() {
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut app.effect_id).desired_width(100.0))
                    .changed();
            } else {
                let display = if app.effect_id.is_empty() { "(none)" } else { app.effect_id.as_str() };
                egui::ComboBox::from_id_salt(("effect_app_id", i))
                    .selected_text(display)
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for id in &effect_ids {
                            if ui.selectable_label(app.effect_id == *id, id.as_str()).clicked()
                                && app.effect_id != *id
                            {
                                app.effect_id = id.clone();
                                changed = true;
                            }
                        }
                    });
            }

            egui::ComboBox::from_id_salt(("effect_app_target", i))
                .selected_text(app.target.variant_name())
                .width(70.0)
                .show_ui(ui, |ui| {
                    for v in ApplicationTarget::all_variants() {
                        if ui.selectable_label(app.target == *v, v.variant_name()).clicked()
                            && app.target != *v
                        {
                            app.target = *v;
                            changed = true;
                        }
                    }
                });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(egui::RichText::new("\u{00d7}").color(colors::STATUS_ERROR)).frame(false))
                    .on_hover_text("Remove effect application")
                    .clicked()
                {
                    remove_index = Some(i);
                }
            });
        });
    }

    if let Some(i) = remove_index {
        apps.remove(i);
        changed = true;
    }

    if ui
        .button(egui::RichText::new("+ effect").color(colors::ACCENT_GREEN))
        .clicked()
    {
        let default_id = effect_ids.first().cloned().unwrap_or_default();
        apps.push(EffectApplication::target_direct(default_id));
        changed = true;
    }

    changed
}

fn damage_type_dropdown(ui: &mut egui::Ui, id_salt: impl std::hash::Hash, value: &mut DamageType) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(value.variant_name())
        .width(80.0)
        .show_ui(ui, |ui| {
            for v in DamageType::all_variants() {
                if ui.selectable_label(*value == *v, v.variant_name()).clicked() && *value != *v {
                    *value = *v;
                    changed = true;
                }
            }
        });
    changed
}

// ---------------------------------------------------------------------------
// Trigger cards
// ---------------------------------------------------------------------------

fn draw_trigger_cards(
    ui: &mut egui::Ui,
    entry: &mut SkillEntry,
    library: &SkillLibrary,
    report: &ValidationReport,
) -> bool {
    let mut changed = false;
    let self_id = entry.rules.id.clone();
    let self_has_real_timeline = entry_has_real_timeline(entry);
    let target_ids: Vec<String> = library.skills.keys().cloned().collect();

    let mut remove_index = None;
    let count = entry.rules.conditions.len();
    for i in 0..count {
        let trigger_skill = entry.rules.conditions[i].trigger_skill.clone();
        // Self-references read the ENTRY's own (possibly just-edited) timeline state rather
        // than the library's stale copy — everyone else reads the library, since only `entry`
        // is being live-edited here.
        let target_has_timeline = if trigger_skill == self_id {
            self_has_real_timeline
        } else {
            library.skills.get(&trigger_skill).is_some_and(entry_has_real_timeline)
        };

        let cond = &mut entry.rules.conditions[i];
        if trigger_card(ui, i, cond, &target_ids, target_has_timeline, report, &mut changed) {
            remove_index = Some(i);
        }
    }

    if let Some(i) = remove_index {
        entry.rules.conditions.remove(i);
        changed = true;
    }

    ui.add_space(4.0);
    if ui
        .button(egui::RichText::new("+ trigger").color(colors::ACCENT_GREEN))
        .clicked()
    {
        // "Always → first other skill or empty" (brief): self is legal but shouldn't be the
        // default suggestion for a freshly-added trigger.
        let default_target = target_ids
            .iter()
            .find(|id| **id != self_id)
            .cloned()
            .unwrap_or_default();
        entry.rules.conditions.push(SkillCondition {
            trigger_skill: default_target,
            additional: false,
            condition: TriggerCondition::Always,
        });
        changed = true;
    }

    changed
}

/// Draw one trigger card. Returns `true` if the card's remove button was clicked (the caller
/// removes it from `entry.rules.conditions` — this fn can't do it itself, it only ever sees one
/// element by `&mut`). Any in-card edit ORs into `*changed`.
#[allow(clippy::too_many_arguments)]
fn trigger_card(
    ui: &mut egui::Ui,
    index: usize,
    cond: &mut SkillCondition,
    target_ids: &[String],
    target_has_timeline: bool,
    report: &ValidationReport,
    changed: &mut bool,
) -> bool {
    let mut removed = false;
    let accent = condition_accent(&cond.condition);

    let frame = egui::Frame::new()
        .fill(colors::BG_MEDIUM)
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::same(6));

    let resp = frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            let target_display = if cond.trigger_skill.is_empty() {
                "(none)"
            } else {
                cond.trigger_skill.as_str()
            };
            ui.label(
                egui::RichText::new(format!(
                    "WHEN {} \u{2192} CAST {}",
                    condition_label(&cond.condition),
                    target_display
                ))
                .strong()
                .color(colors::TEXT_PRIMARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(egui::RichText::new("\u{00d7}").color(colors::STATUS_ERROR)).frame(false))
                    .on_hover_text("Remove trigger")
                    .clicked()
                {
                    removed = true;
                }
            });
        });

        for problem in report.for_condition(index) {
            let color = if problem.blocking { colors::STATUS_ERROR } else { colors::STATUS_WARNING };
            ui.label(egui::RichText::new(&problem.message).small().color(color));
        }

        ui.add_space(2.0);

        egui::Grid::new(("trigger_card_grid", index))
            .num_columns(2)
            .spacing(GRID_SPACING)
            .show(ui, |ui| {
                grid_label(ui, "Condition");
                *changed |= condition_picker(ui, ("trigger_cond_type", index), &mut cond.condition);
                ui.end_row();

                grid_label(ui, "Cast");
                *changed |= target_picker(ui, ("trigger_cond_target", index), &mut cond.trigger_skill, target_ids);
                ui.end_row();
            });

        *changed |= condition_payload_fields(ui, index, &mut cond.condition);

        ui.add_space(2.0);
        let additional_resp = ui.horizontal(|ui| {
            // D4: a trigger targeting a skill with its own real timeline executes spatially as
            // a free sub-cast — it can never replace the primary packet, so `additional` is
            // forced true and the checkbox is locked. Packet targets (no timeline) keep the
            // free choice between "replaces primary" (false) and "fires alongside" (true).
            if target_has_timeline && !cond.additional {
                cond.additional = true;
                *changed = true;
            }
            let mut additional = cond.additional;
            ui.add_enabled_ui(!target_has_timeline, |ui| {
                if ui.checkbox(&mut additional, "Additional").changed() {
                    cond.additional = additional;
                    *changed = true;
                }
            });
        });
        additional_resp.response.on_hover_text(if target_has_timeline {
            "D4: this target has its own timeline, so it always executes as an additional free \
             sub-cast (no mana, no cooldown, original-caster attribution) — it can never replace \
             the primary packet, so this is locked on."
        } else {
            "Additional = fires alongside the primary hit. Unchecked = replaces the primary \
             damage packet."
        });
    });

    // Accent stripe over the card's left edge, matching src/ui/effect_editor.rs's card style.
    let card_rect = resp.response.rect;
    let stripe = egui::Rect::from_min_max(
        card_rect.left_top(),
        egui::pos2(card_rect.left() + 3.0, card_rect.bottom()),
    );
    ui.painter().rect_filled(
        stripe,
        egui::CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 },
        accent,
    );
    ui.add_space(4.0);

    removed
}

fn condition_picker(ui: &mut egui::Ui, id_salt: impl std::hash::Hash, cond: &mut TriggerCondition) -> bool {
    let mut changed = false;
    let current_label = condition_label(cond);
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(current_label)
        .width(170.0)
        .show_ui(ui, |ui| {
            let mut last_group = "";
            for entry in CONDITION_CATALOG {
                if entry.group != last_group {
                    if !last_group.is_empty() {
                        ui.separator();
                    }
                    ui.label(egui::RichText::new(entry.group).small().weak());
                    last_group = entry.group;
                }
                if ui.selectable_label(current_label == entry.label, entry.label).clicked()
                    && current_label != entry.label
                {
                    *cond = (entry.make)();
                    changed = true;
                }
            }
        });
    changed
}

fn target_picker(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash,
    target: &mut String,
    target_ids: &[String],
) -> bool {
    let mut changed = false;
    let display = if target.is_empty() { "(none)" } else { target.as_str() };
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(display)
        .width(150.0)
        .show_ui(ui, |ui| {
            for id in target_ids {
                if ui.selectable_label(target == id, id.as_str()).clicked() && target != id {
                    *target = id.clone();
                    changed = true;
                }
            }
        });
    changed
}

/// Per-variant payload fields — same five shapes `obelisk_editor`'s `edit_condition_fields`
/// handles (threshold / effect id / effect id + min stacks / n / damage type); every other
/// variant is a unit case with nothing to edit.
fn condition_payload_fields(ui: &mut egui::Ui, index: usize, cond: &mut TriggerCondition) -> bool {
    let mut changed = false;
    match cond {
        TriggerCondition::PlayerLowLife { threshold }
        | TriggerCondition::TargetLowLife { threshold }
        | TriggerCondition::PlayerLowMana { threshold }
        | TriggerCondition::DamageOverThreshold { threshold }
        | TriggerCondition::OnOverkill { threshold }
        | TriggerCondition::OnLowLifeReached { threshold } => {
            ui.horizontal(|ui| {
                grid_label(ui, "Threshold");
                changed |= ui
                    .add(egui::DragValue::new(threshold).speed(0.01).range(0.0..=10_000.0))
                    .changed();
            });
        }
        TriggerCondition::TargetHasEffect { id }
        | TriggerCondition::SelfHasEffect { id }
        | TriggerCondition::TargetNoEffect { id }
        | TriggerCondition::OnEffectConsumed { id }
        | TriggerCondition::OnEffectChargeUsed { id } => {
            ui.horizontal(|ui| {
                grid_label(ui, "Effect ID");
                changed |= ui.add(egui::TextEdit::singleline(id).desired_width(120.0)).changed();
            });
        }
        TriggerCondition::TargetEffectStacks { id, min_stacks }
        | TriggerCondition::SelfEffectStacks { id, min_stacks } => {
            ui.horizontal(|ui| {
                grid_label(ui, "Effect ID");
                changed |= ui.add(egui::TextEdit::singleline(id).desired_width(90.0)).changed();
                ui.label("stacks \u{2265}");
                changed |= ui.add(egui::DragValue::new(min_stacks).range(1..=999)).changed();
            });
        }
        TriggerCondition::EveryNthHit { n } => {
            ui.horizontal(|ui| {
                grid_label(ui, "Every N Hits");
                changed |= ui.add(egui::DragValue::new(n).range(1..=999)).changed();
            });
        }
        TriggerCondition::DamageTypeDealt { damage_type }
        | TriggerCondition::OnDamageTakenOfType { damage_type } => {
            ui.horizontal(|ui| {
                grid_label(ui, "Damage Type");
                changed |= damage_type_dropdown(ui, ("trigger_cond_dt", index), damage_type);
            });
        }
        _ => {}
    }
    changed
}

// ---------------------------------------------------------------------------
// Condition catalog — see the module doc comment for provenance.
// ---------------------------------------------------------------------------

struct ConditionCatalogEntry {
    group: &'static str,
    label: &'static str,
    make: fn() -> TriggerCondition,
}

const CONDITION_CATALOG: &[ConditionCatalogEntry] = &[
    // Unconditional (folded into the "Pre-Calc" group, matching the obelisk_editor precedent).
    ConditionCatalogEntry { group: "Pre-Calc", label: "Always", make: || TriggerCondition::Always },
    // Pre-Calculation
    ConditionCatalogEntry { group: "Pre-Calc", label: "Player Full Life", make: || TriggerCondition::PlayerFullLife },
    ConditionCatalogEntry { group: "Pre-Calc", label: "Player Low Life", make: || TriggerCondition::PlayerLowLife { threshold: 0.35 } },
    ConditionCatalogEntry { group: "Pre-Calc", label: "Player Full Mana", make: || TriggerCondition::PlayerFullMana },
    ConditionCatalogEntry { group: "Pre-Calc", label: "Player Low Mana", make: || TriggerCondition::PlayerLowMana { threshold: 0.35 } },
    ConditionCatalogEntry { group: "Pre-Calc", label: "Player Has Barrier", make: || TriggerCondition::PlayerHasBarrier },
    ConditionCatalogEntry { group: "Pre-Calc", label: "Player No Barrier", make: || TriggerCondition::PlayerNoBarrier },
    ConditionCatalogEntry { group: "Pre-Calc", label: "Self Has Effect", make: || TriggerCondition::SelfHasEffect { id: String::new() } },
    ConditionCatalogEntry { group: "Pre-Calc", label: "Self Effect Stacks", make: || TriggerCondition::SelfEffectStacks { id: String::new(), min_stacks: 1 } },
    ConditionCatalogEntry { group: "Pre-Calc", label: "Target Full Life", make: || TriggerCondition::TargetFullLife },
    ConditionCatalogEntry { group: "Pre-Calc", label: "Target Low Life", make: || TriggerCondition::TargetLowLife { threshold: 0.35 } },
    ConditionCatalogEntry { group: "Pre-Calc", label: "Target Has Effect", make: || TriggerCondition::TargetHasEffect { id: String::new() } },
    ConditionCatalogEntry { group: "Pre-Calc", label: "Target Effect Stacks", make: || TriggerCondition::TargetEffectStacks { id: String::new(), min_stacks: 1 } },
    ConditionCatalogEntry { group: "Pre-Calc", label: "Target No Effect", make: || TriggerCondition::TargetNoEffect { id: String::new() } },
    ConditionCatalogEntry { group: "Pre-Calc", label: "Target Has Barrier", make: || TriggerCondition::TargetHasBarrier },
    ConditionCatalogEntry { group: "Pre-Calc", label: "Target No Barrier", make: || TriggerCondition::TargetNoBarrier },
    ConditionCatalogEntry { group: "Pre-Calc", label: "Every Nth Hit", make: || TriggerCondition::EveryNthHit { n: 3 } },
    // Post-Calculation
    ConditionCatalogEntry { group: "Post-Calc", label: "On Crit", make: || TriggerCondition::OnCrit },
    ConditionCatalogEntry { group: "Post-Calc", label: "On Non-Crit", make: || TriggerCondition::OnNonCrit },
    ConditionCatalogEntry { group: "Post-Calc", label: "Damage Type Dealt", make: || TriggerCondition::DamageTypeDealt { damage_type: DamageType::Physical } },
    ConditionCatalogEntry { group: "Post-Calc", label: "Damage Over Threshold", make: || TriggerCondition::DamageOverThreshold { threshold: 0.0 } },
    ConditionCatalogEntry { group: "Post-Calc", label: "Multiple Damage Types", make: || TriggerCondition::MultipleDamageTypes },
    // Post-Resolution (attacker)
    ConditionCatalogEntry { group: "Post-Res", label: "On Kill", make: || TriggerCondition::OnKill },
    ConditionCatalogEntry { group: "Post-Res", label: "On Barrier Broken", make: || TriggerCondition::OnBarrierBroken },
    ConditionCatalogEntry { group: "Post-Res", label: "On Overkill", make: || TriggerCondition::OnOverkill { threshold: 0.0 } },
    // Defensive Resolution (defender)
    ConditionCatalogEntry { group: "Defensive", label: "On Damage Taken", make: || TriggerCondition::OnDamageTaken },
    ConditionCatalogEntry { group: "Defensive", label: "On Damage Taken (Type)", make: || TriggerCondition::OnDamageTakenOfType { damage_type: DamageType::Physical } },
    ConditionCatalogEntry { group: "Defensive", label: "On Effect Consumed", make: || TriggerCondition::OnEffectConsumed { id: String::new() } },
    ConditionCatalogEntry { group: "Defensive", label: "On Effect Charge Used", make: || TriggerCondition::OnEffectChargeUsed { id: String::new() } },
    ConditionCatalogEntry { group: "Defensive", label: "On Dodge", make: || TriggerCondition::OnDodge },
    ConditionCatalogEntry { group: "Defensive", label: "On Evasion Cap", make: || TriggerCondition::OnEvasionCap },
    ConditionCatalogEntry { group: "Defensive", label: "On Hit Taken", make: || TriggerCondition::OnHitTaken },
    ConditionCatalogEntry { group: "Defensive", label: "On Barrier Depleted", make: || TriggerCondition::OnBarrierDepleted },
    ConditionCatalogEntry { group: "Defensive", label: "On Low Life Reached", make: || TriggerCondition::OnLowLifeReached { threshold: 0.35 } },
    // Lifecycle (spec D3: embedding spatial layer, never resolve-time — the on_impact/on_expire
    // vocabulary the brief calls out by name).
    ConditionCatalogEntry { group: "Lifecycle", label: "On World Impact", make: || TriggerCondition::OnImpact },
    ConditionCatalogEntry { group: "Lifecycle", label: "On Expire", make: || TriggerCondition::OnExpire },
];

/// Human-readable label for the current condition — kept in sync with `CONDITION_CATALOG` (one
/// arm per variant; a new `TriggerCondition` variant needs an entry in both places).
fn condition_label(cond: &TriggerCondition) -> &'static str {
    match cond {
        TriggerCondition::Always => "Always",
        TriggerCondition::PlayerFullLife => "Player Full Life",
        TriggerCondition::PlayerLowLife { .. } => "Player Low Life",
        TriggerCondition::PlayerFullMana => "Player Full Mana",
        TriggerCondition::PlayerLowMana { .. } => "Player Low Mana",
        TriggerCondition::PlayerHasBarrier => "Player Has Barrier",
        TriggerCondition::PlayerNoBarrier => "Player No Barrier",
        TriggerCondition::SelfHasEffect { .. } => "Self Has Effect",
        TriggerCondition::SelfEffectStacks { .. } => "Self Effect Stacks",
        TriggerCondition::TargetFullLife => "Target Full Life",
        TriggerCondition::TargetLowLife { .. } => "Target Low Life",
        TriggerCondition::TargetHasEffect { .. } => "Target Has Effect",
        TriggerCondition::TargetEffectStacks { .. } => "Target Effect Stacks",
        TriggerCondition::TargetNoEffect { .. } => "Target No Effect",
        TriggerCondition::TargetHasBarrier => "Target Has Barrier",
        TriggerCondition::TargetNoBarrier => "Target No Barrier",
        TriggerCondition::EveryNthHit { .. } => "Every Nth Hit",
        TriggerCondition::OnCrit => "On Crit",
        TriggerCondition::OnNonCrit => "On Non-Crit",
        TriggerCondition::DamageTypeDealt { .. } => "Damage Type Dealt",
        TriggerCondition::DamageOverThreshold { .. } => "Damage Over Threshold",
        TriggerCondition::MultipleDamageTypes => "Multiple Damage Types",
        TriggerCondition::OnKill => "On Kill",
        TriggerCondition::OnBarrierBroken => "On Barrier Broken",
        TriggerCondition::OnOverkill { .. } => "On Overkill",
        TriggerCondition::OnDamageTaken => "On Damage Taken",
        TriggerCondition::OnDamageTakenOfType { .. } => "On Damage Taken (Type)",
        TriggerCondition::OnEffectConsumed { .. } => "On Effect Consumed",
        TriggerCondition::OnEffectChargeUsed { .. } => "On Effect Charge Used",
        TriggerCondition::OnDodge => "On Dodge",
        TriggerCondition::OnEvasionCap => "On Evasion Cap",
        TriggerCondition::OnHitTaken => "On Hit Taken",
        TriggerCondition::OnBarrierDepleted => "On Barrier Depleted",
        TriggerCondition::OnLowLifeReached { .. } => "On Low Life Reached",
        TriggerCondition::OnImpact => "On World Impact",
        TriggerCondition::OnExpire => "On Expire",
    }
}

fn condition_accent(cond: &TriggerCondition) -> egui::Color32 {
    match cond.phase() {
        ConditionPhase::PreCalculation => colors::ACCENT_BLUE,
        ConditionPhase::PostCalculation => colors::ACCENT_GREEN,
        ConditionPhase::PostResolution => colors::ACCENT_PURPLE,
        ConditionPhase::DefensiveResolution => colors::ACCENT_CYAN,
        ConditionPhase::Lifecycle => colors::ACCENT_ORANGE,
    }
}

// ---------------------------------------------------------------------------
// Advanced drawer — read-only v1 plumbing summary
// ---------------------------------------------------------------------------

fn draw_advanced(ui: &mut egui::Ui, rules: &Skill) {
    ui.label(format!(
        "Damage conversions: {}",
        if rules.damage.damage_conversions.has_conversions() { "configured" } else { "none" }
    ));
    ui.label(format!("Use conditions: {}", rules.use_conditions.len()));
    ui.label(format!("Aura triggers (global conditionals): {}", rules.global_conditionals.len()));
    ui.label(format!("Conditional bonuses: {}", rules.conditional_modifiers.len()));
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("Edit the TOML directly for these (v1).")
            .small()
            .color(colors::TEXT_MUTED),
    );
}

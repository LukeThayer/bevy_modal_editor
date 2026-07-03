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

pub mod library;
pub mod panel;
pub mod readouts;
pub mod templates;
pub mod validation;

pub use library::{
    delete_skill, duplicate_skill, insert_new_skill, rename_skill, scan_and_merge_root,
    scan_content_root, skills_referencing, unique_id, PendingContentRoots, RegisterObeliskContentExt,
    SkillEntry, SkillLibrary,
};
pub use templates::SkillArchetype;
pub use validation::{validate_skill_stub, ValidationReport};

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::render::view::window::screenshot::{save_to_disk, Screenshot};
use bevy_egui::egui;

use crate::editor::{EditorMode, EditorState, PanelSide, PinnedWindows};
use crate::ui::theme::{colors, draw_pin_button, panel as panel_theme, panel_frame};

pub struct SkillModePlugin;

impl Plugin for SkillModePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SkillLibrary>()
            .init_resource::<PendingContentRoots>()
            .add_systems(Startup, library::scan_registered_content_roots)
            .add_systems(Update, skill_probe);
    }
}

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

    let library = world.resource::<SkillLibrary>();
    let open_skill = library.open.clone();
    let has_content_roots = !library.roots.is_empty();
    let skill_count = library.skills.len();
    // Clone the open skill's entry out of the library (same idiom `ui::effect_editor.rs` uses
    // for `EffectMarker`: clone out, edit the clone against `&SkillLibrary` for read-only
    // pickers/readouts, write the clone back once the window closure — and with it every
    // borrow of `library` — has ended). Regions mutate `entry` directly and flip the relevant
    // `dirty_*` flag; Task 8 owns turning that into an actual disk save.
    let mut editing_entry = open_skill.as_ref().and_then(|id| library.skills.get(id).cloned());
    let report = open_skill
        .as_ref()
        .map(|id| validate_skill_stub(id, library))
        .unwrap_or_default();

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

            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
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
                            ui.separator();

                            panel::rules::draw_rules_region(ui, entry, library, &report);

                            ui.separator();
                            ui.label(
                                egui::RichText::new("Behavior — lands in Task 7")
                                    .color(colors::TEXT_SECONDARY)
                                    .small(),
                            );
                            ui.label(
                                egui::RichText::new("Presentation — lands in Task 9")
                                    .color(colors::TEXT_SECONDARY)
                                    .small(),
                            );
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
    // so `world` can be mutably borrowed again below.
    if let (Some(id), Some(entry)) = (open_skill, editing_entry) {
        world.resource_mut::<SkillLibrary>().skills.insert(id, entry);
    }

    if pin_toggled {
        let mut pinned = world.resource_mut::<PinnedWindows>();
        if !pinned.0.remove(&EditorMode::Skill) {
            pinned.0.insert(EditorMode::Skill);
        }
    }
}

/// Frame-scripted probe for headless verification of the Skill panel and palette.
///
/// Launch the binary with `--skill-probe` and this system will, over a few
/// hundred frames: enter Skill mode; at frame 150, insert a TEMPLATE-created projectile skill
/// ("probe_fireball") plus a strike-template skill it triggers on impact
/// ("probe_fireball_explosion") directly into `SkillLibrary` (no disk — a fake, never-scanned
/// content root just satisfies the panel's `has_content_roots` gate) and open the fireball, so
/// frame 200's screenshot exercises the real Rules region (Task 6: readouts, tier-1 fields, the
/// trigger card) instead of the empty state; then open the skill palette (F in Skill mode — here
/// driven directly via `CommandPaletteState`) and screenshot it to
/// `/tmp/skill_palette_probe.png` before exiting. It's a permanent debug harness — every later
/// Skill-panel task reuses it to confirm the panel/palette render correctly without a human
/// driving the UI. No-op (and effectively free — one `env::args()` scan per frame) unless the
/// flag is present.
fn skill_probe(
    mut next_mode: ResMut<NextState<EditorMode>>,
    mut frame: Local<u32>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut palette: ResMut<crate::ui::CommandPaletteState>,
    mut skill_library: ResMut<SkillLibrary>,
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
        210 => palette.open_skill_preset(),
        270 => {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk("/tmp/skill_palette_probe.png"));
        }
        380 => {
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

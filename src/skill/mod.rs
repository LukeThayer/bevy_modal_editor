//! Skill mode — obelisk skill authoring (K key, right panel).
//!
//! This module only compiles with `--features obelisk`. `EditorMode::Skill`
//! itself always exists (see `editor::state::EditorMode`), but without this
//! feature nothing ever transitions into it: the K-key handler in
//! `editor::input` is `#[cfg(feature = "obelisk")]`, and this module (which
//! owns the panel and every skill system) is not compiled in at all.
//!
//! Currently this is a structural skeleton: `SkillModePlugin` draws an empty
//! placeholder panel. Task 5 replaces the panel body with `SkillLibrary`.

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::render::view::window::screenshot::{save_to_disk, Screenshot};
use bevy_egui::{egui, EguiPrimaryContextPass};

use crate::editor::{EditorMode, EditorState, PanelSide, PinnedWindows};
use crate::ui::theme::{colors, draw_pin_button, panel, panel_frame};

pub struct SkillModePlugin;

impl Plugin for SkillModePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(EguiPrimaryContextPass, draw_skill_panel)
            .add_systems(Update, skill_probe);
    }
}

/// Draw the Skill panel (exclusive world access).
///
/// Copies the `ai_editor.rs` template shape: right-side window, pinnable,
/// visible when in Skill mode or pinned while another right-side mode is
/// active (in which case it's displaced to the left, same as every other
/// panel).
fn draw_skill_panel(world: &mut World) {
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
    let panel_height = panel::available_height(&ctx);

    // If pinned and the active mode also uses the right side, move to the left
    let displaced = is_pinned
        && current_mode != EditorMode::Skill
        && current_mode.panel_side() == Some(PanelSide::Right);
    let (anchor_align, anchor_offset) = if displaced {
        (egui::Align2::LEFT_TOP, [panel::WINDOW_PADDING, panel::WINDOW_PADDING])
    } else {
        (egui::Align2::RIGHT_TOP, [-panel::WINDOW_PADDING, panel::WINDOW_PADDING])
    };

    let mut pin_toggled = false;

    egui::Window::new("Skill")
        .id(egui::Id::new("skill_editor_panel"))
        .frame(panel_frame(&ctx.style()))
        .anchor(anchor_align, anchor_offset)
        .default_width(panel::DEFAULT_WIDTH)
        .min_width(panel::MIN_WIDTH)
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
                    ui.label(
                        egui::RichText::new("SkillLibrary lands in Task 5")
                            .color(colors::TEXT_SECONDARY),
                    );
                });
        });

    if pin_toggled {
        let mut pinned = world.resource_mut::<PinnedWindows>();
        if !pinned.0.remove(&EditorMode::Skill) {
            pinned.0.insert(EditorMode::Skill);
        }
    }
}

/// Frame-scripted probe for headless verification of the Skill panel.
///
/// Launch the binary with `--skill-probe` and this system will, over a few
/// hundred frames: enter Skill mode, take a screenshot to
/// `/tmp/skill_mode_probe.png`, then exit. It's a permanent debug harness —
/// every later Skill-panel task (populating `SkillLibrary`, etc.) reuses it
/// to confirm the panel renders correctly without a human driving the UI.
/// No-op (and effectively free — one `env::args()` scan per frame) unless
/// the flag is present.
fn skill_probe(
    mut next_mode: ResMut<NextState<EditorMode>>,
    mut frame: Local<u32>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    if !std::env::args().any(|arg| arg == "--skill-probe") {
        return;
    }

    *frame += 1;
    match *frame {
        90 => next_mode.set(EditorMode::Skill),
        200 => {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk("/tmp/skill_mode_probe.png"));
        }
        320 => {
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

//! Skill mode panel — UI-layer registration.
//!
//! Follows the panel-plugin convention (`ai_editor`, `effect_editor`,
//! `vfx_editor`): panel-drawing systems live in a private `ui::` submodule
//! registered from `UiPlugin::build`, not in the owning feature's own
//! plugin. The actual draw function lives in `crate::skill` (the future
//! non-UI home for `SkillLibrary`, per Task 5) so this module is just the
//! thin `EguiPrimaryContextPass` registration, matching the sibling panels.
//!
//! Only compiled with `--features obelisk` (see `crate::skill`).

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;

pub struct SkillEditorPlugin;

impl Plugin for SkillEditorPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(EguiPrimaryContextPass, crate::skill::draw_skill_panel);
    }
}

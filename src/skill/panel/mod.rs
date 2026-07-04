//! Panel regions for the Skill mode editor (Task 6+): Rules (Task 6), Behavior (Task 7),
//! Presentation (Task 9, `panel::presentation`). Each region is a free function
//! `draw_*_region(ui, &mut SkillEntry, ...)`
//! called from `crate::skill::draw_skill_panel` once a skill is open — see that fn's doc comment
//! for the clone-out/edit/write-back pattern every region assumes (mutating `entry` directly,
//! flipping the relevant `dirty_*` flag on every change; the caller owns writing the edited
//! clone back into `SkillLibrary`).

pub mod behavior;
pub mod presentation;
pub mod rules;
pub mod strip;

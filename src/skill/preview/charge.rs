//! Charge-tier PREVIEW: the editor-side mirror of the game's charge-hold driver. While the open
//! skill has `charge_cues`, the strip's charge slider (`ScrubSim.charge`) picks the ACTIVE tier
//! and this module keeps that tier's effect looping on the preview caster (bone-anchored via
//! the socket index) and its anim playing on the preview rig — drag the slider and watch the
//! gather cross into the ignite tier, exactly as a held cast would in-game.

use bevy::prelude::*;
use bevy_vfx::{VfxLibrary, VfxSystem};
use obelisk_bevy::assets::{CueAttach, CueParam, ParamSource};

use super::rig::{drive_anim_clip, find_anim_player, PreviewAnimGraph};
use super::sockets::RigSockets;
use super::stage::PreviewCaster;
use super::ScrubSim;
use crate::skill::library::SkillLibrary;

/// Marker + state for the live tier-preview effect entity.
#[derive(Component)]
pub struct ChargeTierPreview {
    skill: String,
    tier: usize,
    /// Last applied quantized charge (32 steps), for live param streaming.
    applied_step: i32,
    /// The anim clip this tier holds on the preview rig (weight is re-zeroed on switch).
    anim: Option<String>,
}

impl ChargeTierPreview {
    /// Whether this tier holds an anim clip (the idle baseline gives way to it).
    pub fn has_anim(&self) -> bool {
        self.anim.is_some()
    }
}

/// Drive the tier preview from the charge slider. Runs every frame in Skill mode; cheap when
/// the open skill has no tiers.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn drive_charge_tier_preview(
    library: Res<SkillLibrary>,
    scrub: Option<Res<ScrubSim>>,
    vfx: Option<Res<VfxLibrary>>,
    casters: Query<Entity, With<PreviewCaster>>,
    sockets: Option<Res<RigSockets>>,
    mut previews: Query<(Entity, &mut ChargeTierPreview, &mut VfxSystem)>,
    anim_graph: Option<Res<PreviewAnimGraph>>,
    children: Query<&Children>,
    mut anim_players: Query<&mut AnimationPlayer>,
    mut commands: Commands,
) {
    let charge_frac = scrub.map(|s| s.charge as f32 / 255.0).unwrap_or(0.0);
    let entry = library.open.as_ref().and_then(|id| library.skills.get(id));
    let tiers = entry
        .map(|e| e.timeline.charge_cues.as_slice())
        .unwrap_or(&[]);
    let skill_id = entry.map(|e| e.rules.id.clone()).unwrap_or_default();
    let target = tiers.iter().rposition(|t| t.threshold <= charge_frac);
    let Ok(caster) = casters.single() else { return };

    // Current preview state (at most one).
    let current = previews.iter_mut().next();

    let clear_anim = |anim: &Option<String>,
                      anim_graph: &Option<Res<PreviewAnimGraph>>,
                      children: &Query<&Children>,
                      anim_players: &mut Query<&mut AnimationPlayer>| {
        if let (Some(clip), Some(graph)) = (anim, anim_graph.as_deref()) {
            if let Some(node) = graph.nodes.get(clip).copied() {
                if let Some(pe) = find_anim_player(caster, children, anim_players) {
                    if let Ok(mut player) = anim_players.get_mut(pe) {
                        drive_anim_clip(&mut player, node, 0.0);
                    }
                }
            }
        }
    };

    match (current, target) {
        (None, None) => {}
        (Some((e, preview, _)), None) => {
            clear_anim(&preview.anim, &anim_graph, &children, &mut anim_players);
            commands.entity(e).despawn();
        }
        (state, Some(t)) => {
            let tier = &tiers[t];
            let needs_spawn = match &state {
                Some((_, preview, _)) => preview.skill != skill_id || preview.tier != t,
                None => true,
            };
            if needs_spawn {
                if let Some((e, preview, _)) = state {
                    clear_anim(&preview.anim, &anim_graph, &children, &mut anim_players);
                    commands.entity(e).despawn();
                }
                let Some(mut system) = tier
                    .cue
                    .effect
                    .as_deref()
                    .and_then(|name| vfx.as_deref().and_then(|lib| lib.effects.get(name)))
                    .cloned()
                else {
                    return;
                };
                apply_tier_params(&mut system, &tier.cue.params, charge_frac);
                let (parent, offset) = match &tier.cue.attach {
                    CueAttach::Bone { socket, offset } => (
                        sockets
                            .as_deref()
                            .and_then(|s| s.by_name.get(socket).copied())
                            .unwrap_or(caster),
                        *offset,
                    ),
                    _ => (caster, Vec3::new(0.0, 1.0, 0.0)),
                };
                commands.spawn((
                    Name::new(format!("charge-tier-preview-{t}")),
                    ChargeTierPreview {
                        skill: skill_id.clone(),
                        tier: t,
                        applied_step: (charge_frac * 32.0) as i32,
                        anim: tier.cue.anim.clone(),
                    },
                    system,
                    Transform::from_translation(offset),
                    Visibility::default(),
                    ChildOf(parent),
                ));
            } else if let Some((_, mut preview, mut system)) = state {
                // Same tier: stream the slider's charge into the loop (quantized).
                let step = (charge_frac * 32.0) as i32;
                if step != preview.applied_step {
                    preview.applied_step = step;
                    apply_tier_params(&mut system, &tier.cue.params, charge_frac);
                }
            }
            // Hold the tier's anim on the preview rig.
            if let (Some(clip), Some(graph)) = (&tier.cue.anim, anim_graph.as_deref()) {
                if let Some(node) = graph.nodes.get(clip).copied() {
                    if let Some(pe) = find_anim_player(caster, &children, &mut anim_players) {
                        if let Ok(mut player) = anim_players.get_mut(pe) {
                            drive_anim_clip(&mut player, node, 1.0);
                        }
                    }
                }
            }
        }
    }
}

/// The editor mirror of the game's charge-param streaming: `scale` maps to a readable absolute
/// size band, other Charge-sourced params get the raw fraction.
fn apply_tier_params(system: &mut VfxSystem, params: &[CueParam], frac: f32) {
    for p in params {
        if matches!(p.source, ParamSource::Charge) {
            let value = if p.param == "scale" { 0.12 + 0.28 * frac } else { frac.max(0.05) };
            super::vfx_bake::apply_modulated_param(system, &p.param, value);
        }
    }
}

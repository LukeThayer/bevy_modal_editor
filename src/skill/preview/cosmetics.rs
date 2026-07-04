//! Preview cosmetics (Task 10 — ported/adapted from `arena_editor::preview_cosmetics`, obelisk-
//! arena `f6472e4`): the `On<CueEvent>` observer that turns the real sim's fired cues into the
//! authored cosmetic reactions. THE grace-ladder invariant (age on sim time, reap on render
//! frames) lives here, carried verbatim — see [`reap_preview_cosmetics`].
//!
//! **v1 → v2 schema adaptations** (the reason this file differs structurally from its source,
//! beyond swapping crate paths):
//! - v1's `EditedSkillFx.lanes: HashMap<CueId, Vec<LaneEvent>>` (multiple named lanes per cue,
//!   each with its own `anim`/`particle`/`beam`/`projectile` sub-shape, socket name + offset,
//!   and per-lane lifetime) is GONE. v2's `obelisk_bevy::assets::CueBinding` is a single, much
//!   leaner shape per cue key: `{ effect: Option<String>, attach: CueAttach, anim: Option<String>,
//!   params: Vec<CueParam> }` — ONE effect name, ONE attach mode (`World` or `Follow`), no
//!   per-binding socket name, no offset, no author-set lifetime. This port renders that leaner
//!   shape faithfully rather than inventing schema the dependency doesn't have:
//!     - no per-binding socket NAME exists to resolve (unlike v1's per-lane socket string), so
//!       `sockets::RigSockets`/`resolve_socket` are wired (indexed, ready for a future schema/
//!       asset that supplies one) but have no call site in this file: EVERY cue cosmetic spawns
//!       UNPARENTED at the cue's own world position — `CueAttach::World`'s own doc comment is
//!       explicit that it "does not track its source afterward," so parenting a `World`-attached
//!       `on_cast`/`on_window_*`/`emit_*` cosmetic to the caster (which DOES move, in a general
//!       game, even though this preview's caster happens to stand still) would be a real
//!       correctness bug, not a harmless simplification — caught and fixed during this task's own
//!       probe pass (see the port report's "concerns" section).
//!     - `CueAttach::Follow` (legal only on `on_window_*`/`emit_*`) is v2's replacement for v1's
//!       dedicated "projectile lane": "the host flies a proxy along the cue's motion data" —
//!       ported as [`PreviewFlight`], computed from the FIRED window's authored `VolumeMotion`
//!       (found by stripping the `on_window_`/`emit_` prefix off the cue id and looking the
//!       window up in the currently-previewing skill's timeline) and the caster's LIVE
//!       `ActiveCast::aim_dir` (the exact direction the real hitbox launched with).
//!     - no per-binding lifetime is authored; every spawned cosmetic gets
//!       [`DEFAULT_COSMETIC_LIFETIME`] (documented adaptation, not a carried invariant).
//! - **Cue effect-name resolution order** (ledger ticket from the Task 9 review): a binding's
//!   `effect` name is resolved against `EffectLibrary` FIRST, then `VfxLibrary` — this is the
//!   canonical order a game client must mirror. A name in neither is skipped with a
//!   once-per-name `warn!`, never a panic (mirrors `CueBinding`'s own doc: an unresolvable name is
//!   inert by construction).
//! - `ParamSource::Charge` is baked from the fired `CueEvent`'s OWN `charge: Option<u8>` (v2,
//!   Task 1's cue-payload work) rather than v1's separate `PreviewCharge` stand-in resource (v1
//!   predates per-cue charge forwarding) — see [`charge_fraction`].
//! - No `WindowPhase::Chained`/beam-arc special-casing: v2 deleted that schema; a beam window's
//!   TWO-anchor open cue (`position_from` carrying the beam's origin) is still rendered as a
//!   sampled arc between the two points — the one piece of v1's beam rendering that's still
//!   schema-relevant (`CueEvent::position_from` is unchanged from v1's beam handling), now driven
//!   by the single resolved `effect` name rather than a dedicated `beam` lane.
//!
//! Every spawned cosmetic is `PreviewCosmetic`-tagged (tests/tracing) — deliberately NOT
//! `GameEntity` (Finding 2, Task 10 review): the generic `ResetCommand` (`editor::game`)
//! synchronously `world.despawn()`s every `GameEntity` BEFORE firing `GameResetEvent`, which would
//! hard-despawn a cosmetic mid-flight (a live `VfxSystem`, grace 0) and bypass the two-render-
//! frame grace ladder below — this exact panic class (a `bevy_vfx`/`bevy_effect` command queued
//! against an already-despawned entity) has recurred 3x in the sim's history. `reset_stage_on_reset`
//! (`stage.rs`'s own `GameResetEvent` handler) instead expires cosmetics IN PLACE, letting
//! [`reap_preview_cosmetics`] retire them through the same safe ladder every other expiry uses.

use std::collections::HashSet;

use bevy::prelude::*;

use bevy_vfx::VfxLibrary;

use obelisk_bevy::assets::{CastTimeline, CueAttach, CueParam, ParamSource, VolumeMotion};
use obelisk_bevy::prelude::{charge_mult, ActiveCast};
use obelisk_bevy::events::{CueEvent, CueKind};

use crate::editor::EditorMode;
use crate::effects::{cleanup_effect, EffectLibrary, EffectPlayback, PlaybackState};
use crate::skill::library::SkillLibrary;

use super::rig::{drive_anim_clip, find_anim_player, PreviewAnimGraph};
use super::stage::{PreviewCastSkill, PreviewCaster};

/// A cosmetic's presentation lifetime — no `CueBinding` in v2 authors one (see the module doc
/// comment), so every spawned cue cosmetic gets this single default.
const DEFAULT_COSMETIC_LIFETIME: f32 = 1.5;

/// Marks a spawned preview cosmetic (particle/effect stand-in) — NOT `GameEntity` (see the module
/// doc comment); a Reset expires it in place via [`CosmeticLifetime`] instead of despawning it
/// directly. Also queried by tests to prove a cue rendered its lanes, and by `stage::despawn_stage`
/// (`OnExit(EditorMode::Skill)`) to expire any straggler left alive when the stage tears down.
#[derive(Component)]
pub struct PreviewCosmetic;

/// Bounds a spawned cosmetic's life: both `bevy_vfx` presets and `bevy_effect` presets default to
/// playing indefinitely, so without this each fired cue would leave a permanently-running effect
/// behind. Ticked by [`age_preview_cosmetics`].
#[derive(Component)]
pub struct CosmeticLifetime {
    pub elapsed: f32,
    pub duration: f32,
    /// Post-expiry countdown (render FRAMES, not sim ticks — see [`reap_preview_cosmetics`]).
    pub grace: u8,
}

/// Flies a preview cosmetic (v2's rendering of `CueAttach::Follow` — see the module doc comment):
/// gravity into velocity, then velocity into position — the same semi-implicit Euler obelisk's
/// `spatial::projectile::move_projectiles` runs on the authoritative hitbox, so the visible proxy
/// traces the arc the sim actually resolves hits along.
#[derive(Component)]
pub struct PreviewFlight {
    pub velocity: Vec3,
    pub gravity: f32,
}

/// Integrate every [`PreviewFlight`] each fixed step. A proxy that reaches the floor plane is
/// pinned there and its [`CosmeticLifetime`] expired (mirrors the sim, which ends a grounded
/// projectile hitbox the same way — see `stage::report_ground_hits`).
pub fn fly_preview_cosmetics(
    time: Res<Time<Fixed>>,
    mut q: Query<(&mut PreviewFlight, &mut Transform, &mut CosmeticLifetime)>,
) {
    let dt = time.delta_secs();
    for (mut flight, mut tf, mut life) in &mut q {
        flight.velocity.y -= flight.gravity * dt;
        let velocity = flight.velocity;
        tf.translation += velocity * dt;
        if tf.translation.y < 0.0 {
            tf.translation.y = 0.0;
            life.elapsed = life.duration;
        }
    }
}

/// Age every [`CosmeticLifetime`] with SIM time (`FixedUpdate`, gated with the sim — advances
/// inside a future synchronous seek (Task 11) exactly as many ticks as the sim ran, freezes with
/// it). Pure mutation: expiry CONSEQUENCES live in [`reap_preview_cosmetics`].
pub fn age_preview_cosmetics(time: Res<Time<Fixed>>, mut q: Query<&mut CosmeticLifetime>) {
    for mut life in &mut q {
        life.elapsed += time.delta_secs();
    }
}

/// Reap expired cosmetics in RENDER frames (plain `Update`, NEVER inside a sim seek): stop the
/// effect, wait two frames, despawn. **Carried verbatim (Global Constraint) — do not "improve"**:
/// the grace MUST count render frames, not sim ticks. `bevy_vfx`'s Update systems queue component
/// commands on live vfx entities, and a same-frame despawn (which a seek's collapsed sim time
/// would cause if this ladder ran on sim ticks instead) makes those commands panic on an
/// already-despawned entity. Two live render frames after the stop signal guarantee no pending
/// upstream command can target the entity when it finally despawns.
///
/// "Stop the effect" is component-kind-dependent (v2 adds a second cosmetic kind v1 never had —
/// `EffectMarker`/`EffectPlayback`, Task 10's own EffectLibrary-resolved cosmetics): a
/// `VfxSystem`-backed cosmetic (VfxLibrary-resolved) is stopped by removing `VfxSystem`, matching
/// v1 exactly; an `EffectMarker`-backed one (EffectLibrary-resolved) is stopped by
/// `bevy_effect::cleanup_effect` — which despawns ITS OWN tracked children immediately (that
/// despawn is `bevy_effect`'s own established stop path, already safe same-frame) and sets it
/// `Stopped` — before the SAME two-frame grace despawns the (by-then childless) root.
pub fn reap_preview_cosmetics(
    mut q: Query<(Entity, &mut CosmeticLifetime, Option<&mut EffectPlayback>)>,
    mut commands: Commands,
) {
    for (e, mut life, playback) in &mut q {
        if life.elapsed < life.duration {
            continue;
        }
        match life.grace {
            0 => {
                commands.entity(e).remove::<bevy_vfx::VfxSystem>();
                if let Some(mut playback) = playback {
                    cleanup_effect(&mut commands, &mut playback);
                }
                life.grace = 1;
            }
            1 => life.grace = 2,
            _ => commands.entity(e).try_despawn(),
        }
    }
}

/// Look up the `VolumeMotion` of the window a fired `on_window_{id}`/`emit_{id}` cue belongs to,
/// by stripping the known prefix back off `cue_id` — v2's `derive_vfx_cues` keys `vfx_cues`
/// (hence every fired `CueEvent::cue_id`) identically to the slot name (see `stage.rs`), so this
/// recovers the window id with no extra bookkeeping.
fn window_motion_for_cue(tl: &CastTimeline, cue_id: &str) -> Option<VolumeMotion> {
    let window_id = cue_id
        .strip_prefix("on_window_")
        .or_else(|| cue_id.strip_prefix("emit_"))?;
    tl.collision_windows
        .iter()
        .find(|w| w.id == window_id)
        .map(|w| w.motion.clone())
}

/// The charge fraction (0..1) `ParamSource::Charge` bindings bake from — the RAW cast-charge byte
/// forwarded on every cue slot (`CueEvent::charge`, Task 1's cue-payload work), normalized like
/// every other `charge: Option<u8>` reader in this schema (`None` = uncharged). Defaults to `1.0`
/// (full strength) rather than `0.0`: an uncharged cast is the COMMON case in this preview (Play
/// casts uncharged until Task 11 threads a scrub charge through — see `stage::start_preview`),
/// and a muzzle/impact burst rendering at zero scale for every ordinary cast would misrepresent
/// the skill far more than rendering it at full strength does (same rationale v1's `PreviewCharge`
/// default carried).
fn charge_fraction(charge: Option<u8>) -> f32 {
    charge.map(|c| c as f32 / 255.0).unwrap_or(1.0)
}

/// Observer: on a fired `CueEvent`, resolve the CURRENTLY PREVIEWING skill's
/// `CastTimeline::cues[ev.cue_id]` binding (if any — see [`PreviewCastSkill`]) and render it.
#[allow(clippy::too_many_arguments)]
pub fn on_preview_cue(
    cue: On<CueEvent>,
    previewing: Res<PreviewCastSkill>,
    library: Res<SkillLibrary>,
    effects: Res<EffectLibrary>,
    vfx: Res<VfxLibrary>,
    graph: Res<PreviewAnimGraph>,
    caster_q: Query<(Entity, &ActiveCast), With<PreviewCaster>>,
    children: Query<&Children>,
    mut players: Query<&mut AnimationPlayer>,
    mut flights: Query<(&mut CosmeticLifetime, &mut PreviewFlight, &mut Transform)>,
    mut commands: Commands,
    mut warned: Local<HashSet<String>>,
) {
    let ev = cue.event();

    // An END cue is the sim saying "the bolt stopped HERE": snap every flying preview cosmetic to
    // the end position and retire it (the preview runs one skill at a time, so no per-cue
    // correlation is needed — carried verbatim from v1).
    if ev.kind == CueKind::OnEnd {
        for (mut life, mut flight, mut tf) in &mut flights {
            life.elapsed = life.duration;
            flight.velocity = Vec3::ZERO;
            flight.gravity = 0.0;
            tf.translation = ev.position;
        }
    }

    let Some(skill_id) = previewing.0.as_ref() else {
        return;
    };
    let Some(entry) = library.skills.get(skill_id) else {
        return;
    };
    let tl = &entry.timeline;
    let Some(binding) = tl.cues.get(&ev.cue_id) else {
        return;
    };

    // No caster is a legal state: a future timeline scrubber (Task 11) fires synthetic cues in
    // edit mode before any duel exists — `Follow` (the only caster-dependent rendering left,
    // now that `World` is never socket-anchored — see below) then just renders with no flight.
    let caster = caster_q.single().ok();

    // Anim rows drive `AnimationLibrary` clips on the stage rig (D7: editor-only presentation —
    // the networked game host never consumes `CueBinding::anim`). `clip` is the FULL
    // `AnimationLibrary` key (`"{gltf}::{clip}"`, matching Task 9's Anim picker exactly — see
    // `rig.rs`'s module doc), driven to full weight (v2 authors no per-binding blend weight,
    // unlike v1's `LaneEvent.anim.weight`). Inert until a host hangs a real rig scene under the
    // caster (no canonical rig asset ships with this editor — see `rig.rs`).
    if let (Some(clip), Some((caster_e, _))) = (&binding.anim, caster)
        && let Some(node) = graph.nodes.get(clip)
        && let Some(player_e) = find_anim_player(caster_e, &children, &players)
        && let Ok(mut player) = players.get_mut(player_e)
    {
        drive_anim_clip(&mut player, *node, 1.0);
    }

    let Some(effect_name) = &binding.effect else {
        return;
    };

    // `CueAttach::World` (`CueAttach`'s own doc comment: "does not track its source afterward")
    // is UNPARENTED at the cue's own position — every kind, always: `on_cast`/`on_hit`/
    // `on_end_*` are always effectively `World` (not attach-legal), and an `on_window_*`/
    // `emit_*` binding that explicitly chose `World` over `Follow` means exactly that: a
    // one-shot burst that does NOT ride along with the caster or the window. `resolve_socket`/
    // `RigSockets` are wired (indexed, ready) but have no call site here today: v2's
    // `CueBinding` carries no per-binding socket NAME (unlike v1), so there is nothing to anchor
    // a `Follow`-style attachment to except the independently-computed world-space proxy below.
    let charge = charge_fraction(ev.charge);
    let spawned = spawn_cue_effect(
        &mut commands,
        &effects,
        &vfx,
        effect_name,
        &binding.params,
        charge,
        ev.position,
        &mut warned,
    );

    if matches!(binding.attach, CueAttach::Follow) {
        let flight = match (window_motion_for_cue(tl, &ev.cue_id), caster) {
            (Some(VolumeMotion::Linear { speed }), Some((_, ac))) => {
                Some((ac.aim_dir * speed * charge_mult(ev.charge), 0.0))
            }
            (Some(VolumeMotion::Ballistic { speed, gravity }), Some((_, ac))) => {
                Some((ac.aim_dir * speed * charge_mult(ev.charge), gravity))
            }
            _ => None,
        };
        if let Some((velocity, gravity)) = flight {
            commands.entity(spawned).insert(PreviewFlight { velocity, gravity });
        }
    }

    // Two-anchor beam arc: bursts sampled along position_from→position (a beam window's open cue
    // carries both anchors) — the same v1 rendering, just off the single resolved `effect` name
    // rather than a dedicated `beam` lane (v2 has none).
    if let Some(from) = ev.position_from {
        const BEAM_SEGMENTS: usize = 6;
        for i in 0..BEAM_SEGMENTS {
            let t = i as f32 / (BEAM_SEGMENTS - 1) as f32;
            spawn_cue_effect(
                &mut commands,
                &effects,
                &vfx,
                effect_name,
                &binding.params,
                charge,
                from.lerp(ev.position, t),
                &mut warned,
            );
        }
    }
}

/// Spawn one cue cosmetic at `translation` (always a world-space root — see the module doc
/// comment and `on_preview_cue`'s own comment on why v2 cosmetics are never socket-parented):
/// **cue effect-name resolution order** (canonical — the game client must mirror it) tries
/// `EffectLibrary` FIRST, then `VfxLibrary`. A name in neither library warns once (never a panic
/// — mirrors `CueBinding`'s own "inert by construction" doc). Returns the spawned entity so
/// callers can attach a [`PreviewFlight`] for `CueAttach::Follow`.
#[allow(clippy::too_many_arguments)]
fn spawn_cue_effect(
    commands: &mut Commands,
    effects: &EffectLibrary,
    vfx: &VfxLibrary,
    name: &str,
    params: &[CueParam],
    charge: f32,
    translation: Vec3,
    warned: &mut HashSet<String>,
) -> Entity {
    let mut base = commands.spawn((
        Transform::from_translation(translation),
        Visibility::default(),
        PreviewCosmetic,
        CosmeticLifetime {
            elapsed: 0.0,
            duration: DEFAULT_COSMETIC_LIFETIME,
            grace: 0,
        },
    ));

    if let Some(marker) = effects.effects.get(name).cloned() {
        base.insert((
            marker,
            EffectPlayback {
                state: PlaybackState::Playing,
                ..default()
            },
        ));
    } else if let Some(mut system) = vfx.effects.get(name).cloned() {
        for p in params {
            match p.source {
                ParamSource::Charge => super::vfx_bake::apply_modulated_param(&mut system, &p.param, charge),
            }
        }
        base.insert(system);
    } else if warned.insert(name.to_string()) {
        warn!(
            "preview cue: effect '{name}' not found in EffectLibrary or VfxLibrary — this cue \
             will render nothing (checked EffectLibrary first, then VfxLibrary — the resolution \
             order a game client must mirror)"
        );
    }

    base.id()
}

/// Wires the preview cosmetics: the cue observer + the age/fly (sim-time, `FixedUpdate`) and reap
/// (render-time, `Update`) systems.
///
/// `age_preview_cosmetics`/`fly_preview_cosmetics` are scoped to `EditorMode::Skill` (Finding 1,
/// Task 10 review) — a cosmetic is only ever created by a cast on the stage, which cannot happen
/// outside Skill mode (the sim that fires `CueEvent`s is gated too — see `stage::add_obelisk_sim`).
/// `on_preview_cue` (the observer) needs no explicit gate: it only fires in reaction to a
/// `CueEvent`, which that same gated sim is the only source of.
///
/// `reap_preview_cosmetics` is deliberately left UN-gated. `stage::despawn_stage`
/// (`OnExit(EditorMode::Skill)`) force-expires (never despawns) any cosmetic still alive when the
/// stage tears down, exactly like a normal `reset_stage` expiry — relying on THIS system to still
/// be running afterward to actually walk the two-render-frame grace ladder and finish the
/// despawn. Gating it too would strand a mid-flight cosmetic forever with a live driver
/// component: either a permanently-playing (and now un-despawnable) piece of VFX clutter outside
/// Skill mode, or — if `despawn_stage` hard-despawned instead of expiring — the exact bevy_vfx
/// dead-entity-command panic Finding 2 just closed off via a different path. The query here is
/// empty (and therefore cheap) whenever no cosmetic exists, which is always true outside a live
/// Skill-mode cast.
pub struct PreviewCosmeticsPlugin;

impl Plugin for PreviewCosmeticsPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_preview_cue)
            .add_systems(
                FixedUpdate,
                (age_preview_cosmetics, fly_preview_cosmetics).run_if(in_state(EditorMode::Skill)),
            )
            .add_systems(Update, reap_preview_cosmetics);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charge_fraction_defaults_full_and_scales_linearly() {
        assert_eq!(charge_fraction(None), 1.0);
        assert_eq!(charge_fraction(Some(0)), 0.0);
        assert!((charge_fraction(Some(255)) - 1.0).abs() < 1e-6);
        assert!((charge_fraction(Some(128)) - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn window_motion_for_cue_strips_known_prefixes() {
        use obelisk_bevy::assets::{
            CollisionShape, CollisionWindow, HitFilter, HitMode, PhaseDurations, WindowSpawn,
        };
        let window = CollisionWindow {
            id: "bolt".into(),
            spawn: WindowSpawn::Scheduled {
                phase: obelisk_bevy::assets::WindowPhase::Active,
                offset: 0.0,
            },
            anchor: Default::default(),
            anchor_offset: Vec3::ZERO,
            strikes: true,
            active_duration: 1.0,
            shape: CollisionShape::Sphere { radius: 0.5 },
            motion: VolumeMotion::Linear { speed: 20.0 },
            motion_direction: Default::default(),
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::FirstOnly,
            rehit_interval: None,
            emitter: None,
        };
        let tl = CastTimeline {
            skill_id: "s".into(),
            phase_durations: PhaseDurations { windup: 0.0, active: 0.0, recovery: 0.0 },
            collision_windows: vec![window],
            acquisition: Default::default(),
            vfx_cues: Default::default(),
            chain_radius: 6.0,
            chargeable: false,
            max_hold: 1.0,
            cues: Default::default(),
        };
        assert!(matches!(
            window_motion_for_cue(&tl, "on_window_bolt"),
            Some(VolumeMotion::Linear { speed }) if speed == 20.0
        ));
        assert!(matches!(
            window_motion_for_cue(&tl, "emit_bolt"),
            Some(VolumeMotion::Linear { speed }) if speed == 20.0
        ));
        assert!(window_motion_for_cue(&tl, "on_hit").is_none());
        assert!(window_motion_for_cue(&tl, "on_window_ghost").is_none());
    }
}

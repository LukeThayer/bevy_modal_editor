//! The deterministic PREVIEW STAGE (Task 10): a persistent caster+dummy duel that runs the real
//! obelisk-bevy simulation, so "Play the real skill" previews byte-for-byte what a game built on
//! this editor's authored content would play. Ported from arena_editor's `preview_controller.rs`
//! / `preview_rig.rs` / `socket.rs` / `preview_cosmetics.rs` and arena_sim's `preview.rs` /
//! `obelisk.rs` / `spawn.rs` / `ballistics.rs` / `tuning.rs` (obelisk-arena @ `f6472e4`) — see each
//! submodule's doc comment for the specific v1 → v2 (schema + host) adaptations.
//!
//! - [`stage`] — the persistent stage lifecycle (spawn/heal/reset) + the sim composition this
//!   editor has no `arena_sim` crate to supply, + the stage-provided `Acquisition` resolution and
//!   flat-floor `HitboxWorldHit` reporter (the two pieces obelisk-bevy structurally cannot supply
//!   itself — see that module's doc comment).
//! - [`sockets`] — rig bone-name index (generic, ported verbatim).
//! - [`rig`] — the generic anim-graph plumbing (no hardcoded rig asset — see that module's doc
//!   comment for why).
//! - [`cosmetics`] — the cue-driven presentation layer (the grace-ladder invariant lives here).
//! - [`vfx_bake`] — the CPU param-baking seam `cosmetics` uses for `ParamSource::Charge`.
//! - [`scrub`] (Task 11) — the sim-backed synchronous scrub (`ScrubSim`/`drive_scrub`), the
//!   strip's dynamic trailing extent, and the event-marker recorder. See that module's doc
//!   comment for the full v1 -> v2 adaptation.

pub mod cosmetics;
pub mod rig;
pub mod scrub;
pub mod sockets;
pub mod stage;
pub mod vfx_bake;

pub use cosmetics::{CosmeticLifetime, PreviewCosmetic, PreviewFlight};
pub use rig::PreviewAnimGraph;
pub use scrub::{
    MarkerKind, PreviewScrubPlugin, ScrubMarker, ScrubMarkers, ScrubMode, ScrubSim,
};
pub use sockets::{resolve_socket, RigSockets};
pub use stage::{
    ballistic_launch_dir, preview_aim, PreviewCastSkill, PreviewCaster, PreviewControllerPlugin,
    PreviewDummy, PreviewSimPlugin, PreviewStageReset, Playhead, StagePost, SPAWN_MARKERS,
};

use bevy::prelude::*;

/// Bundles the whole preview stage: the sim composition + registry sync
/// ([`stage::PreviewSimPlugin`]), the stage lifecycle ([`stage::PreviewControllerPlugin`]), the
/// rig anim-graph plumbing ([`rig::PreviewRigPlugin`]), the rig socket index, the cue-driven
/// cosmetics ([`cosmetics::PreviewCosmeticsPlugin`]), and the sim-backed scrub
/// ([`scrub::PreviewScrubPlugin`], Task 11). Registered by `SkillModePlugin` under
/// `#[cfg(feature = "obelisk")]` — see `crate::skill::SkillModePlugin`.
pub struct SkillPreviewPlugin;

impl Plugin for SkillPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(stage::PreviewSimPlugin)
            .add_plugins(stage::PreviewControllerPlugin)
            .add_plugins(rig::PreviewRigPlugin)
            .add_plugins(cosmetics::PreviewCosmeticsPlugin)
            .add_plugins(scrub::PreviewScrubPlugin)
            .init_resource::<sockets::RigSockets>()
            .add_systems(Update, sockets::index_rig_sockets);
    }
}

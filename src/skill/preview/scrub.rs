//! SIM-BACKED scrubbing (Task 11 — ported from `arena_editor::scrub`, obelisk-arena @
//! `f6472e4`): the scrub head drives the REAL deterministic sim, not a synthetic staging.
//! Dragging to sim-time `t` restarts the cast on the persistent stage (same seed -> identical
//! every time) and runs the fixed-tick sim to `t` SYNCHRONOUSLY — [`drive_scrub`] is an
//! exclusive system that calls `world.run_schedule(FixedUpdate)` for exactly the ticks needed,
//! so every drag frame ends with the sim AT the pointer (no multi-frame catch-up, no
//! virtual-time games, camera untouched). Between drags the sim is FROZEN via a run-condition
//! gate ([`sim_unfrozen`]) composed ADDITIVELY onto the obelisk sets (already gated
//! `run_if(in_state(EditorMode::Skill))` by `stage::add_obelisk_sim` — Task 10): Bevy's
//! `configure_sets` APPENDS conditions for a set already configured elsewhere (confirmed against
//! `bevy_ecs::schedule::node::SystemSets`'s own doc comment — "conditions... may be appended to
//! multiple times... when `configure_sets` is called multiple [times] with the same set"), so
//! both conditions must hold: in Skill mode AND not frozen.
//!
//! Verbs: DRAG the strip (forward continues from the current tick; backward restarts and
//! re-sims the prefix — deterministic, identical every time), (replay) REPLAY (restart, run
//! ambient at 1x, auto-freeze at the strip end), and the charge slider (the cast's charge byte —
//! feeds BOTH the scrub cast and Play, see `stage::start_preview`).
//!
//! **v1 -> v2 adaptation — the strip's DYNAMIC END.** v1's strip span was fully resolvable from
//! authored data alone (`resolved_window_span` walked the authored `Chain`/`Retarget` graph). v2
//! deleted that schema: cross-skill causality is now rules triggers executing a wholly separate
//! skill's timeline as a [`TriggeredExec`] (obelisk-bevy's own `timeline::triggered` module) —
//! the BASE span (`strip::base_span`, this cast's own phases + scheduled windows) has zero
//! visibility into what a triggered sub-cast does or how long it runs. [`ScrubSim::dynamic_end`]
//! is the strip's empirically-discovered trailing extent: [`extend_dynamic_end`] watches for a
//! live `Hitbox`/`TriggeredExec` past the base span and grows the strip to cover it (capped at
//! [`MAX_TRAILING_SECS`] past the base span), so scrubbing a bolt that triggers an explosion on
//! impact shows the explosion resolve AFTER the base timeline's own authored end.
//!
//! **Event markers** ([`ScrubMarkers`]): a lightweight recorder — this preview has no existing
//! event log outside test harnesses (`obelisk_bevy::testkit::EventRecorder`, test-only) — that
//! observes `HitWindowOpened`/`HitConfirmed`/`HitboxEnded`/`TriggerFired` and timestamps each
//! against the scrub clock, for the strip to paint as tick marks. Cleared on every
//! `restart_cast` so it always reflects the CURRENT deterministic run.

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use bevy_editor_game::GameState;

use obelisk_bevy::assets::CastTimeline;
use obelisk_bevy::events::{HitConfirmed, HitWindowOpened, HitboxEnded, TriggerFired};
use obelisk_bevy::prelude::Hitbox;
use obelisk_bevy::timeline::triggered::TriggeredExec;
use obelisk_bevy::ObeliskSet;

use crate::editor::EditorMode;
use crate::skill::library::SkillLibrary;
use crate::skill::panel::strip::base_span;

use super::stage::{stage_cast, PreviewStageReset};

/// Hard cap on how far PAST the base timeline span a seek may run while a Hitbox/TriggeredExec
/// still lives (the strip's trailing sub-cast region — see the module doc comment). A guard
/// against a runaway rules chain, not a tuning knob.
pub const MAX_TRAILING_SECS: f32 = 10.0;

/// Hard cap on fixed ticks one synchronous seek may run — a guard against a runaway span, not a
/// tuning knob. Generous enough to cover the worst case (a long base span PLUS the full
/// `MAX_TRAILING_SECS` trailing region) at 60 Hz.
const MAX_SEEK_TICKS: u32 = 3600;

/// The scrub session's state machine.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
pub enum ScrubMode {
    /// No scrub session: the sim runs free (Play uses this).
    #[default]
    Idle,
    /// Fast-forwarding the sim toward `target`.
    Seeking,
    /// Sim frozen at the reached time; camera stays live.
    Frozen,
    /// Replaying at 1x from the start; freezes at the strip end.
    Replaying,
}

/// The scrub controller resource. The panel writes `target` (strip drag) / requests a replay;
/// `drive_scrub` runs the machine; `clock` is the sim time since the scrubbed cast began.
#[derive(Resource)]
pub struct ScrubSim {
    pub mode: ScrubMode,
    /// Requested sim time (strip drag). `None` until the first grab.
    pub target: Option<f32>,
    /// Sim seconds since the scrub cast started (ticks with the fixed clock while unfrozen).
    pub clock: f32,
    /// Set by the (replay) button: restart and play at 1x.
    pub replay_requested: bool,
    /// The cast's charge byte (the strip slider). 85 ~= 1.0x (tap); 255 = 2.0x (full hold).
    /// Feeds BOTH the scrub cast (`restart_cast`) and Play (`stage::start_preview`).
    pub charge: u8,
    /// The strip's DYNAMIC END (module doc comment): the furthest sim-clock instant at which a
    /// Hitbox/TriggeredExec has been observed alive since the current scrub cast began. Always
    /// >= the base span; reset there on every `restart_cast`; grown by `extend_dynamic_end`.
    pub dynamic_end: f32,
    /// True once a scrub cast has been fired on the stage this session.
    cast_live: bool,
    /// The target the last seek served. Comparing the USER's `target` against this (never
    /// against the clock, which sits one tick past by construction) distinguishes a real new
    /// request from holding still.
    sought: Option<f32>,
    /// Where a replay auto-freezes (the base span, captured at replay start).
    end: f32,
    /// True only INSIDE `drive_scrub`'s synchronous tick loop — unfreezes the obelisk sets for
    /// the manually-run schedule while the ambient fixed loop stays frozen.
    exclusive_running: bool,
}

impl Default for ScrubSim {
    fn default() -> Self {
        Self {
            mode: ScrubMode::Idle,
            target: None,
            clock: 0.0,
            replay_requested: false,
            charge: 85, // ~= 1.0x -- an uncharged tap
            dynamic_end: 0.0,
            cast_live: false,
            sought: None,
            end: 0.0,
            exclusive_running: false,
        }
    }
}

/// Run condition gating the obelisk sim sets (+ the preview cosmetic clocks). The ambient fixed
/// loop runs the sim only when NO scrub session holds it (Idle) or a replay is playing; during
/// Seeking/Frozen all sim advancement happens inside `drive_scrub`'s synchronous loop (which sets
/// `exclusive_running`). `None` (no `PreviewScrubPlugin` registered — e.g. `tests/skill_preview.rs`'s
/// harness) is unconditionally unfrozen: the scrub feature simply isn't present.
pub fn sim_unfrozen(scrub: Option<Res<ScrubSim>>) -> bool {
    scrub.is_none_or(|s| match s.mode {
        ScrubMode::Idle | ScrubMode::Replaying => true,
        ScrubMode::Seeking | ScrubMode::Frozen => s.exclusive_running,
    })
}

/// Tick the scrub clock with the fixed clock while a scrub cast is live; freeze a replay the
/// tick it crosses the base span. Gated like the sim, ordered before the obelisk sets (so `clock`
/// reflects "time as of the tick about to run").
pub fn tick_scrub_clock(time: Res<Time<Fixed>>, mut scrub: ResMut<ScrubSim>) {
    if !scrub.cast_live {
        return;
    }
    scrub.clock += time.delta_secs();
    if scrub.mode == ScrubMode::Replaying && scrub.clock >= scrub.end {
        scrub.mode = ScrubMode::Frozen;
    }
}

/// Grow [`ScrubSim::dynamic_end`] while a Hitbox or TriggeredExec still lives past it (module doc
/// comment). Ordered after every obelisk set so it observes the CURRENT tick's fully-settled
/// entity set (a `TriggeredExec` spawned this tick, or the Hitbox IT spawns, included).
pub fn extend_dynamic_end(
    mut scrub: ResMut<ScrubSim>,
    library: Res<SkillLibrary>,
    hitboxes: Query<(), With<Hitbox>>,
    execs: Query<(), With<TriggeredExec>>,
) {
    if !scrub.cast_live || (hitboxes.is_empty() && execs.is_empty()) {
        return;
    }
    let Some(entry) = library.open.as_ref().and_then(|id| library.skills.get(id)) else {
        return;
    };
    if scrub.clock <= scrub.dynamic_end {
        return;
    }
    let cap = base_span(&entry.timeline) + MAX_TRAILING_SECS;
    scrub.dynamic_end = scrub.clock.min(cap);
}

// ---------------------------------------------------------------------------
// Event markers
// ---------------------------------------------------------------------------

/// Which discrete strip moment a [`ScrubMarker`] records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerKind {
    WindowOpened,
    Hit,
    Ended,
    Trigger,
}

/// One discrete moment observed during the current scrub cast, timestamped against
/// [`ScrubSim::clock`] at the instant it fired.
#[derive(Clone, Debug)]
pub struct ScrubMarker {
    pub time: f32,
    pub kind: MarkerKind,
    /// A short human label (window/skill id) for the strip's hover tooltip.
    pub label: String,
}

/// The strip's event-marker log for the CURRENT scrub cast — cleared on every `restart_cast` so
/// it always reflects the deterministic run in progress, never a stale prior one.
#[derive(Resource, Default)]
pub struct ScrubMarkers(pub Vec<ScrubMarker>);

fn record_window_opened(ev: On<HitWindowOpened>, scrub: Res<ScrubSim>, mut markers: ResMut<ScrubMarkers>) {
    markers.0.push(ScrubMarker {
        time: scrub.clock,
        kind: MarkerKind::WindowOpened,
        label: ev.event().window_id.clone(),
    });
}

fn record_hit(ev: On<HitConfirmed>, scrub: Res<ScrubSim>, mut markers: ResMut<ScrubMarkers>) {
    markers.0.push(ScrubMarker {
        time: scrub.clock,
        kind: MarkerKind::Hit,
        label: ev.event().window_id.clone(),
    });
}

fn record_ended(ev: On<HitboxEnded>, scrub: Res<ScrubSim>, mut markers: ResMut<ScrubMarkers>) {
    markers.0.push(ScrubMarker {
        time: scrub.clock,
        kind: MarkerKind::Ended,
        label: ev.event().window_id.clone(),
    });
}

fn record_trigger(ev: On<TriggerFired>, scrub: Res<ScrubSim>, mut markers: ResMut<ScrubMarkers>) {
    markers.0.push(ScrubMarker {
        time: scrub.clock,
        kind: MarkerKind::Trigger,
        label: ev.event().skill_id.clone(),
    });
}

// ---------------------------------------------------------------------------
// The scrub machine
// ---------------------------------------------------------------------------

/// The currently open skill's timeline, if any (mirrors how `stage::start_preview` resolves
/// "what to cast" from `SkillLibrary`).
fn open_timeline(world: &World) -> Option<CastTimeline> {
    let library = world.resource::<SkillLibrary>();
    let id = library.open.as_ref()?;
    library.skills.get(id).map(|e| e.timeline.clone())
}

/// The scrub state machine — an EXCLUSIVE system: a new target runs the fixed schedule
/// synchronously for exactly the ticks needed (restarting first for backward targets), so the
/// frame ends with the sim AT the pointer and frozen there. Replay runs on the ambient fixed
/// loop at 1x and freezes at the base span. Entering Play (or leaving Skill mode) ends the
/// session.
pub fn drive_scrub(world: &mut World) {
    // Play owns the sim: end any scrub session the moment we leave Editing.
    let game_state = *world.resource::<State<GameState>>().get();
    if game_state != GameState::Editing {
        let mut scrub = world.resource_mut::<ScrubSim>();
        if scrub.mode != ScrubMode::Idle {
            scrub.mode = ScrubMode::Idle;
            scrub.cast_live = false;
        }
        return;
    }

    let Some(tl) = open_timeline(world) else {
        return;
    };
    let base = base_span(&tl).max(0.0001);
    let hard_cap = base + MAX_TRAILING_SECS;

    // Replay request: restart, then let the ambient loop play it at 1x.
    if world.resource::<ScrubSim>().replay_requested {
        restart_cast(world);
        let mut scrub = world.resource_mut::<ScrubSim>();
        scrub.replay_requested = false;
        scrub.mode = ScrubMode::Replaying;
        scrub.end = base;
        scrub.target = None;
        return;
    }

    if world.resource::<ScrubSim>().mode == ScrubMode::Replaying {
        return; // ambient loop is playing; tick_scrub_clock freezes it at the end
    }

    // Seek requests. `target` is clamped to `hard_cap` (base span + the trailing sub-cast
    // allowance), not just `base` -- the whole point of the dynamic end is that a target PAST
    // the base span is a legal request (see the module doc comment).
    let (target, needs_restart) = {
        let scrub = world.resource::<ScrubSim>();
        let Some(raw) = scrub.target else { return };
        let target = raw.clamp(0.0, hard_cap);
        if scrub.sought == Some(target) && scrub.cast_live {
            return; // already there -- hold the frozen instant
        }
        // Backward (or no cast yet): restart and re-sim the prefix. NEVER compare the clock (it
        // sits at tick granularity past the sought time by construction).
        (target, !scrub.cast_live || target < scrub.clock - 1e-4)
    };
    if needs_restart {
        restart_cast(world);
    }
    // Synchronous seek: run the fixed schedule tick by tick until the clock reaches the target.
    // `exclusive_running` unfreezes the obelisk sets for these manual runs only.
    {
        let mut scrub = world.resource_mut::<ScrubSim>();
        scrub.sought = Some(target);
        scrub.mode = ScrubMode::Seeking;
        scrub.exclusive_running = true;
    }
    let mut guard = 0;
    while world.resource::<ScrubSim>().clock < target && guard < MAX_SEEK_TICKS {
        world.run_schedule(FixedUpdate);
        guard += 1;
    }
    let mut scrub = world.resource_mut::<ScrubSim>();
    scrub.exclusive_running = false;
    scrub.mode = ScrubMode::Frozen;
}

/// Restart the deterministic scrub cast: reset the stage (heal, reposition, clear hitboxes +
/// cosmetics, reseed, clear cooldowns), cast the currently open skill with the session's charge,
/// zero the clock, clear the event-marker log, and reset the dynamic end to the fresh base span.
/// Uses `run_system_once` so the queued commands (despawns, the cast) are APPLIED before the
/// caller's synchronous tick loop starts. A no-op if nothing is open.
fn restart_cast(world: &mut World) {
    let Some((skill_id, tl)) = ({
        let library = world.resource::<SkillLibrary>();
        library
            .open
            .as_ref()
            .and_then(|id| library.skills.get(id).map(|e| (id.clone(), e.timeline.clone())))
    }) else {
        return;
    };
    world.resource_mut::<ScrubMarkers>().0.clear();
    let base = base_span(&tl);
    let charge = world.resource::<ScrubSim>().charge;
    world
        .run_system_once(|mut reset: PreviewStageReset| reset.reset_stage())
        .expect("stage reset runs");
    world
        .run_system_once(move |mut reset: PreviewStageReset| {
            stage_cast(&mut reset, &skill_id, &tl, Some(charge));
        })
        .expect("stage cast runs");
    let mut scrub = world.resource_mut::<ScrubSim>();
    scrub.clock = 0.0;
    scrub.cast_live = true;
    scrub.sought = None;
    scrub.dynamic_end = base;
}

/// `OnExit(EditorMode::Skill)`: reset the scrub session and its marker log, mirroring
/// `stage::despawn_stage`'s own teardown -- without this, re-entering Skill mode later would
/// show a stale "frozen at ..." strip state from a session whose stage no longer exists.
fn reset_scrub_on_mode_exit(mut scrub: ResMut<ScrubSim>, mut markers: ResMut<ScrubMarkers>) {
    *scrub = ScrubSim::default();
    markers.0.clear();
}

/// Wires the scrub machinery: `drive_scrub` (Update, exclusive), the fixed-time clock/dynamic-end
/// ticks (gated `sim_unfrozen`), the marker-recording observers, and the ADDITIVE `sim_unfrozen`
/// gate on the obelisk sets (composed onto Task 10's `run_if(in_state(EditorMode::Skill))` --
/// see the module doc comment).
pub struct PreviewScrubPlugin;

impl Plugin for PreviewScrubPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScrubSim>()
            .init_resource::<ScrubMarkers>()
            .add_observer(record_window_opened)
            .add_observer(record_hit)
            .add_observer(record_ended)
            .add_observer(record_trigger)
            .add_systems(Update, drive_scrub.run_if(in_state(EditorMode::Skill)))
            .add_systems(OnExit(EditorMode::Skill), reset_scrub_on_mode_exit)
            .add_systems(
                FixedUpdate,
                (
                    tick_scrub_clock.before(ObeliskSet::Validate),
                    extend_dynamic_end.after(ObeliskSet::TickEffects),
                )
                    .run_if(sim_unfrozen),
            );
        app.configure_sets(
            FixedUpdate,
            (
                ObeliskSet::Validate,
                ObeliskSet::Advance,
                ObeliskSet::Projectiles,
                ObeliskSet::ResolveHits,
                ObeliskSet::TickEffects,
            )
                .run_if(sim_unfrozen),
        );
    }
}

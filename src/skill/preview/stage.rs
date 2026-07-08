//! The deterministic PREVIEW STAGE (Task 10 — ported from arena_editor's `preview_controller.rs`
//! and `sim_config.rs`, and arena_sim's `preview.rs`, `obelisk.rs`, `spawn.rs`, `ballistics.rs`,
//! and `tuning.rs`, obelisk-arena @ `f6472e4`): a PERSISTENT caster+dummy duel that runs the real
//! obelisk-bevy simulation, so "Play the real skill" previews byte-for-byte what the game plays.
//!
//! v1 → v2 adaptations (see the task brief for the full list):
//! - No `arena_sim` crate here (that's obelisk-arena's), so this module also owns what arena_sim
//!   used to: the spawn primitives (`spawn_arena_floor`/`make_arena_combatant`/`SPAWN_MARKERS`),
//!   the ballistic aim solver, and the host-side obelisk-bevy sim composition (mirroring
//!   `ObeliskSimPlugin::build` minus the physics group, which this editor's `EditorPlugin`
//!   already owns) — see [`PreviewSimPlugin`].
//! - Casting is now driven by the AUTHORED `Acquisition` (Task 10, no more `CastTargeting`):
//!   [`resolve_stage_acquisition`] resolves it into a concrete cast verb, walking `AcqFallback`
//!   chains when the primary branch can't be satisfied on the stage's fixed geometry.
//! - `report_ground_hits` (ported from `arena_sim::obelisk`) is the stage's flat-floor
//!   `HitboxWorldHit` reporter — the ONE piece obelisk deliberately doesn't know (the world), and
//!   without it an `OnImpact`-triggered skill (a bolt whose rules trigger an explosion on world
//!   impact) could never fire in a bare editor with no game host.
//! - No `WindowPhase::Chained`/chain-graph staging: v2 deleted that schema; triggered sub-casts,
//!   emitters, and rules-driven chain hops all run IN-SIM (obelisk-bevy owns them — see
//!   `timeline::triggered::advance_triggered_execs` / `timeline::advance::end_hitboxes`). The
//!   stage does nothing special for them beyond composing the sim correctly.
//! - Dummy auto-sync is now RULES-driven (`rules.damage.can_chain`/`chain_count`), not an authored
//!   `EndReaction::Retarget` (deleted): see [`ensure_stage`].
//! - There is no single `EditedSkill` resource — `library.open` names the entry in `SkillLibrary`
//!   currently open in the panel; every stage system resolves it fresh each time (`None` is a
//!   legal, idle state: the stage still exists, just with nothing to cast).

use bevy::prelude::*;

use avian3d::prelude::*;
use stat_core::StatBlock;

use obelisk_bevy::assets::{AcqFallback, Acquisition, CastTimeline, VolumeMotion};
use obelisk_bevy::prelude::{
    ActiveCast, CastSkillExt, CastTimelineHandles, CombatRng, Cooldowns, Faction, Hitbox,
    ObeliskCommandsExt, ObeliskConfigExt, SkillPhase, SkillRegistry,
};
use obelisk_bevy::spatial::filter::passes_filter;

use bevy_editor_game::{GameCamera, GameResetEvent, GameStartedEvent, GameState};

use crate::editor::{EditorCamera, EditorMode};
use crate::skill::library::{SkillEntry, SkillLibrary};

use super::cosmetics::{CosmeticLifetime, PreviewCosmetic};

// ---------------------------------------------------------------------------
// Tuning constants (ported from arena_sim::tuning — byte-identical values; the preview stage
// has no reason to differ from the arena's own geometry, and keeping the numbers matched means
// any future re-comparison against obelisk-arena's own preview stays meaningful).
// ---------------------------------------------------------------------------

/// World Y of a grounded combatant's ORIGIN: a `capsule(0.35, 0.48)` body (half-height 0.59)
/// resting on the static floor (top face at world Y = 0) settles with its origin at ≈0.59.
pub const GROUND_Y: f32 = 0.59;
/// Magnitude of the stage's avian `Gravity` (m/s²) — matches arena_sim's arcade-snappy value.
pub const GRAVITY: f32 = 20.0;
pub const PLAYER_CAPSULE_RADIUS: f32 = 0.35;
pub const PLAYER_CAPSULE_LENGTH: f32 = 0.48;

/// The two fixed stage spawn markers: caster at marker 0, first dummy at marker 1 — facing each
/// other across the +Z... actually the +X axis (8 world units apart), matching arena_sim's own
/// duel layout.
pub const SPAWN_MARKERS: [Vec3; 2] = [
    Vec3::new(-4.0, GROUND_Y, 0.0),
    Vec3::new(4.0, GROUND_Y, 0.0),
];

// ---------------------------------------------------------------------------
// Spawn primitives (ported from arena_sim::spawn — this editor has no arena_sim dependency, so
// the bare combatant/floor recipe lives here now).
// ---------------------------------------------------------------------------

/// Marks the preview's player-controlled caster (Player faction, marker 0).
#[derive(Component)]
pub struct PreviewCaster;

/// Marks the preview's enemy target dummy/dummies (Enemy faction).
#[derive(Component)]
pub struct PreviewDummy;

/// Marks the persistent stage floor — both the physics collider entity and (when windowed) its
/// visual mesh entity. Queried by [`despawn_stage`] (`OnExit(EditorMode::Skill)`, Finding 1, Task
/// 10 review) and by tests asserting the stage doesn't leak outside a Skill-mode session.
#[derive(Component)]
pub struct PreviewStageFloor;

/// Marks a persistent stage combatant's home position (heal/reposition target on reset).
#[derive(Component)]
pub struct StagePost(pub Vec3);

/// Spawn the STATIC stage floor collider: a cuboid spanning ±20 world units horizontally, 1 m
/// thick, positioned so its TOP face sits at world Y = 0 (a `capsule(0.35, 0.48)` body then rests
/// with its origin at [`GROUND_Y`] and its feet at world 0).
pub fn spawn_arena_floor(commands: &mut Commands) {
    const FLOOR_SIZE: f32 = 40.0;
    const FLOOR_THICKNESS: f32 = 1.0;
    commands.spawn((
        Name::new("PreviewStageFloor"),
        PreviewStageFloor,
        RigidBody::Static,
        Collider::cuboid(FLOOR_SIZE, FLOOR_THICKNESS, FLOOR_SIZE),
        Position(Vec3::new(0.0, -FLOOR_THICKNESS / 2.0, 0.0)),
        Rotation::default(),
    ));
}

/// Build one bare stage combatant: a full obelisk combatant + `Faction` + a server-authoritative
/// Dynamic avian body + a CHILD `Hurtbox` sensor capsule. NO networking, NO `grant_skill` — the
/// stage grants/casts explicitly (see [`stage_cast`]).
pub fn make_arena_combatant(
    commands: &mut Commands,
    obelisk_id: &str,
    faction: Faction,
    spawn: Vec3,
) -> Entity {
    let player = commands
        .spawn_empty()
        .make_combatant(StatBlock::with_id(obelisk_id))
        .insert((
            faction,
            Position(spawn),
            Rotation::default(),
            LinearVelocity::default(),
            AngularVelocity::default(),
            RigidBody::Dynamic,
            Collider::capsule(PLAYER_CAPSULE_RADIUS, PLAYER_CAPSULE_LENGTH),
            LockedAxes::default()
                .lock_rotation_x()
                .lock_rotation_y()
                .lock_rotation_z(),
            Friction::new(0.0).with_combine_rule(CoefficientCombine::Min),
        ))
        .id();
    commands.spawn((
        Name::new("Hurtbox"),
        obelisk_bevy::prelude::Hurtbox { owner: player },
        Collider::capsule(PLAYER_CAPSULE_RADIUS, PLAYER_CAPSULE_LENGTH),
        Sensor,
        Transform::default(),
        ChildOf(player),
    ));
    player
}

// ---------------------------------------------------------------------------
// Ballistic aim (ported from arena_sim::ballistics — pure math, no ECS).
// ---------------------------------------------------------------------------

/// The launch direction (unit vector) that lands a ballistic projectile fired at `speed` under
/// `gravity` from `from` onto `to` — the LOW-arc solution. Falls back to the straight line for
/// non-ballistic inputs (`gravity <= 0`) or a (near-)vertical shot, or a 45° lob when out of range.
pub fn ballistic_launch_dir(from: Vec3, to: Vec3, speed: f32, gravity: f32) -> Vec3 {
    let delta = to - from;
    let flat = Vec3::new(delta.x, 0.0, delta.z);
    let d = flat.length();
    if gravity <= 0.0 || d < 1e-4 || speed <= 0.0 {
        return delta.normalize_or(Vec3::X);
    }
    let h = delta.y;
    let s2 = speed * speed;
    let disc = s2 * s2 - gravity * (gravity * d * d + 2.0 * h * s2);
    if disc < 0.0 {
        return (flat / d + Vec3::Y).normalize();
    }
    let tan_theta = (s2 - disc.sqrt()) / (gravity * d);
    (flat / d + Vec3::Y * tan_theta).normalize()
}

/// The preview's cast direction from `from` toward `to`: straight for non-ballistic skills, the
/// low-arc ballistic solution (first `Ballistic` window's speed/gravity) for arcing ones — the
/// aim a free-looking player compensating for gravity would take.
pub fn preview_aim(tl: &CastTimeline, from: Vec3, to: Vec3) -> Vec3 {
    let ballistic = tl.collision_windows.iter().find_map(|w| match w.motion {
        VolumeMotion::Ballistic { speed, gravity } => Some((speed, gravity)),
        _ => None,
    });
    match ballistic {
        Some((speed, gravity)) => ballistic_launch_dir(from, to, speed, gravity),
        None => (to - from).normalize_or(Vec3::X),
    }
}

#[cfg(test)]
mod ballistics_tests {
    use super::*;

    #[test]
    fn lands_on_the_stage_duel_geometry() {
        let (from, to) = (SPAWN_MARKERS[0], SPAWN_MARKERS[1]);
        let dir = ballistic_launch_dir(from, to, 20.0, 9.8);
        assert!((dir.length() - 1.0).abs() < 1e-5);
        assert!(dir.y > 0.0, "a same-height shot lofts slightly upward");
    }

    #[test]
    fn zero_gravity_is_the_straight_line() {
        let dir = ballistic_launch_dir(Vec3::ZERO, Vec3::new(3.0, 4.0, 0.0), 20.0, 0.0);
        assert!((dir - Vec3::new(0.6, 0.8, 0.0)).length() < 1e-5);
    }
}

// ---------------------------------------------------------------------------
// Stage-provided flat-floor world-hit reporter (ported from arena_sim::obelisk::
// report_ground_hits) — obelisk deliberately knows nothing about the world; the HOST detects a
// projectile crossing the floor plane and fires `HitboxWorldHit`, which `end_hitboxes` turns into
// an `EndReason::HitWorld` ending (firing the end cue and any `OnImpact` rules trigger, e.g. a
// bolt's ground explosion). Without this, an `OnImpact`-triggered skill could never fire in a
// bare editor with no game host — see the task brief's CRITICAL ADAPTATION #2.
// ---------------------------------------------------------------------------

#[allow(clippy::type_complexity)]
fn report_ground_hits(
    q: Query<
        (Entity, &Transform),
        (
            With<Hitbox>,
            With<obelisk_bevy::spatial::projectile::Projectile>,
            Without<obelisk_bevy::timeline::advance::WorldHit>,
        ),
    >,
    mut commands: Commands,
) {
    for (e, tf) in &q {
        if tf.translation.y < 0.0 {
            let position = Vec3::new(tf.translation.x, 0.0, tf.translation.z);
            commands.trigger(obelisk_bevy::events::HitboxWorldHit { hitbox: e, position });
        }
    }
}

/// Force the avian `SpatialQueryPipeline` to reflect the current collider set right before
/// obelisk reads it. Ported defensively from `arena_sim::obelisk` — that crate needed this
/// because its host scheduled avian's step in the SAME `FixedUpdate` schedule as obelisk's own
/// sets under a lightyear plugin that reordered things unexpectedly. THIS editor's base
/// `EditorPlugin` instead adds `PhysicsPlugins::default()`, which schedules avian's step in
/// `FixedPostUpdate` — a schedule that runs AFTER `FixedUpdate` within the same fixed tick, so
/// obelisk's reads here see the END of the PREVIOUS tick's physics step (a harmless one-tick
/// lag for a preview stage whose combatants don't move under player input). Kept anyway: cheap,
/// and it means a future host that changes avian's schedule can't silently break obelisk's
/// spatial reads without this safety net picking the slack back up.
fn refresh_spatial_pipeline(mut spatial: SpatialQuery) {
    spatial.update_pipeline();
}

fn refresh_spatial_pipeline_pre_detect(mut spatial: SpatialQuery) {
    spatial.update_pipeline();
}

// ---------------------------------------------------------------------------
// Sim composition (ported from arena_sim::obelisk::add_obelisk_sim / preview::
// ArenaSimPreviewPlugin + arena_editor::sim_config::PreviewSimConfigPlugin) — composes
// obelisk-bevy's sim sub-plugins under this editor's OWN physics group (the base `EditorPlugin`
// already adds `PhysicsPlugins`; `ObeliskSpatialPlugin`, which would add a second one, is
// deliberately never added here — same invariant arena_sim's own doc comment calls out).
// ---------------------------------------------------------------------------

/// The editor-preview physics + obelisk host. Guards against re-adding `PhysicsPlugins` (the
/// base `EditorPlugin` already adds one; a from-scratch headless test app that doesn't include
/// it gets one here, scheduled on `FixedUpdate` matching obelisk-bevy's own sim schedule).
pub struct PreviewSimPlugin;

impl Plugin for PreviewSimPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<PhysicsSchedulePlugin>() {
            app.add_plugins(PhysicsPlugins::new(FixedUpdate));
        }
        // Only set Gravity if nothing else has (a host embedding this editor may already run a
        // real game with its own gravity tuning).
        if !app.world().contains_resource::<Gravity>() {
            app.insert_resource(Gravity(Vec3::new(0.0, -GRAVITY, 0.0)));
        }

        // Global obelisk config: numeric constants (idempotent) + an EMPTY status-effect
        // registry (idempotent) — the archetype templates' `DamageConfig`s don't reference any
        // status effect by id, so there is nothing to load from disk here; a game embedding this
        // editor with real status-effect content can call `add_obelisk_effects` itself before
        // this plugin builds (both init calls are guarded, so an earlier real call wins).
        app.add_obelisk_config_constants_default();
        stat_core::config::ensure_effect_registry_initialized();
        // Seed-1 default (arbitrary — the preview reseeds to `CombatRng::default()` (seed 0) on
        // every reset/Play anyway, see `PreviewStageReset::reset_stage`; this initial seed only
        // matters for the sliver of time between app start and the first stage reset).
        app.seed_combat_rng(1);

        add_obelisk_sim(app);

        // `.before(ensure_stage)`: this plugin and `PreviewControllerPlugin` are separate
        // structs, but Bevy orders by system reference regardless of which plugin added a
        // system, so this constraint holds even though `ensure_stage` is added later (by
        // `SkillPreviewPlugin`) — see `sync_sim_registries`'s own doc comment for why this
        // ordering matters (a same-frame edit-then-Play must see the current authored content).
        app.add_systems(
            Update,
            sync_sim_registries
                .run_if(resource_exists::<SkillLibrary>)
                .run_if(in_state(EditorMode::Skill))
                .before(ensure_stage),
        );
    }
}

/// Compose obelisk-bevy's sim sub-plugins, mirroring `ObeliskSimPlugin::build` minus
/// `ObeliskSpatialPlugin` (physics is the host's job) — see the module doc comment. The preview
/// is authoritative over its own world (server-tier: hit resolution + `CombatRng` both run here),
/// matching a headless server's composition.
fn add_obelisk_sim(app: &mut App) {
    use obelisk_bevy::{assets, combat, core, loot, net, spatial, timeline, vfx, ObeliskSet};

    app.add_plugins(assets::ObeliskAssetsPlugin)
        .add_plugins(core::ObeliskCorePlugin)
        .add_plugins(combat::ObeliskCombatPlugin)
        .add_plugins(net::ObeliskNetPlugin)
        .add_plugins(vfx::ObeliskCuePlugin)
        .add_plugins(loot::ObeliskLootPlugin);

    // Finding 1 (Task 10 review): gating the SETS (not just the systems this fn explicitly adds
    // below) means every system anyone assigns `.in_set(ObeliskSet::_)` — including
    // `ObeliskCorePlugin`'s own `tick_effects_system`/`tick_cooldowns` (`.in_set(TickEffects)`,
    // added by a different plugin, added above via `add_obelisk_sim`'s own `add_plugins` calls) —
    // is skipped outside Skill mode too, with no need to chase every current and future obelisk-
    // bevy internal system individually.
    app.configure_sets(
        FixedUpdate,
        (
            ObeliskSet::Validate,
            ObeliskSet::Advance,
            ObeliskSet::Projectiles,
            ObeliskSet::ResolveHits,
            ObeliskSet::TickEffects,
        )
            .chain()
            .run_if(in_state(EditorMode::Skill)),
    );

    app.add_observer(timeline::advance::on_hitbox_world_hit);
    app.add_systems(
        FixedUpdate,
        (
            timeline::advance::validate_casts.in_set(ObeliskSet::Validate),
            (
                timeline::advance::advance_casts,
                (
                    timeline::advance::end_hitboxes,
                    timeline::advance::tick_emitters,
                )
                    .chain_ignore_deferred(),
                timeline::triggered::advance_triggered_execs,
            )
                .in_set(ObeliskSet::Advance),
            (
                spatial::projectile::move_projectiles,
                // Arena rule obelisk doesn't know: the flat floor (see `report_ground_hits`).
                report_ground_hits,
            )
                .chain()
                .in_set(ObeliskSet::Projectiles),
            (
                spatial::detect::detect_overlaps,
                spatial::detect::resolve_beam_hits,
            )
                .in_set(ObeliskSet::ResolveHits),
        ),
    );

    // `.before`/`.after` are plain ordering constraints, not set membership, so these two don't
    // inherit the `ObeliskSet` gate above automatically — gated explicitly (Finding 1).
    app.add_systems(
        FixedUpdate,
        refresh_spatial_pipeline
            .before(ObeliskSet::Validate)
            .run_if(in_state(EditorMode::Skill)),
    );
    app.add_systems(
        FixedUpdate,
        refresh_spatial_pipeline_pre_detect
            .after(ObeliskSet::Projectiles)
            .before(ObeliskSet::ResolveHits)
            .run_if(in_state(EditorMode::Skill)),
    );
    app.add_systems(Update, refresh_spatial_pipeline.run_if(in_state(EditorMode::Skill)));
}

/// The v2 translation of v1's `derive_vfx_cues`: obelisk's cue-firing systems (`vfx.rs`) decide
/// whether a slot fires at all by looking up `CastTimeline::vfx_cues[slot] -> cue_id`; the Skill
/// panel's Presentation region (Task 9) authors bindings directly into `CastTimeline::cues`,
/// KEYED BY THE SAME SLOT NAME (`"on_cast"`, `"on_window_bolt"`, ... — see `cue_slots::cue_slots`
/// and every `CueBinding` insert in `templates.rs`). So every `cues` key contributes an identity
/// slot → cue_id entry (v2's `cues` map IS ALREADY keyed by slot name, unlike v1's named-lane
/// indirection).
///
/// The result is the UNION of those identity entries with the timeline's own AUTHORED `vfx_cues`
/// — an authored slot may legitimately have NO `cues` binding (obelisk-arena's firebolt authors
/// `on_end_bolt` purely as the trail-teardown TRIGGER: the cue must fire so the client can
/// despawn the Follow cosmetic, but binds no visual of its own). Replacing the authored map
/// outright silently killed such slots in the preview — the bolt's teardown cue never fired and
/// the cosmetic sailed through the dummy/floor while the sim had long since resolved the hit
/// (see `tests/skill_preview.rs::unbound_vfx_cue_slots_still_fire`). Identity entries win on a
/// shared slot key (a panel-authored binding is the fresher intent for that slot).
pub fn derive_vfx_cues(tl: &CastTimeline) -> std::collections::HashMap<String, String> {
    let mut slots: std::collections::HashMap<String, String> =
        tl.cues.keys().map(|k| (k.clone(), k.clone())).collect();
    for (slot, cue_id) in &tl.vfx_cues {
        slots
            .entry(slot.clone())
            .or_insert_with(|| cue_id.clone());
    }
    slots
}

/// Keep obelisk-bevy's OWN sim resources (`SkillRegistry`, `CastTimelineHandles` /
/// `Assets<CastTimeline>`) synced from `SkillLibrary` — the editor's in-memory source of truth,
/// including unsaved edits (the same "what you author is what plays" spirit v1's single
/// `EditedSkill` resource had, just now covering every loaded skill instead of one). Runs
/// whenever the library changes; cheap at editor content-library scale (Task 8's own precedent).
/// Ordered before `(ensure_stage, start_preview)` so a Play the same frame a skill was edited
/// always casts the CURRENT authored content.
fn sync_sim_registries(
    library: Res<SkillLibrary>,
    mut registry: ResMut<SkillRegistry>,
    mut handles: ResMut<CastTimelineHandles>,
    mut timelines: ResMut<Assets<CastTimeline>>,
    mut synced: Local<std::collections::HashMap<String, u64>>,
) {
    if !library.is_changed() {
        return;
    }
    // HANDLE STABILITY. The egui Skill panel takes `ResMut<SkillLibrary>` while drawing, so
    // `library.is_changed()` is true EVERY frame a skill is open — not just on real edits. The
    // old body re-`add`ed every timeline unconditionally on any change, minting a NEW asset +
    // handle PER FRAME: per-frame asset/GC churn, `AssetEvent` spam, and a standing race for
    // anything holding a timeline handle across ticks (obelisk re-resolves per tick today, but
    // nothing guarantees that forever). So: skip skills whose synced content is unchanged
    // (hash-gated — per-frame panel noise becomes a no-op), write CHANGED content in-place
    // through the EXISTING handle (in-flight casts keep resolving and pick up live edits), and
    // add assets only for NEW skills. See
    // `tests/skill_preview.rs::per_frame_library_writes_keep_timeline_handles_stable`.
    registry.0.clear();
    for (id, entry) in &library.skills {
        registry.0.insert(id.clone(), entry.rules.clone());
        let mut tl = entry.timeline.clone();
        tl.vfx_cues = derive_vfx_cues(&tl);
        let hash = ron::ser::to_string(&tl)
            .map(|s| crate::skill::library::hash_bytes(s.as_bytes()))
            .unwrap_or(0);
        let existing = handles.0.get(id).cloned();
        if synced.get(id) == Some(&hash) && existing.is_some() {
            continue; // unchanged content, stable handle — nothing to do
        }
        match existing.and_then(|h| timelines.get_mut(&h).map(|_| h)) {
            Some(h) => {
                if let Some(asset) = timelines.get_mut(&h) {
                    *asset = tl;
                }
            }
            None => {
                let handle = timelines.add(tl);
                handles.0.insert(id.clone(), handle);
            }
        }
        synced.insert(id.clone(), hash);
    }
    // Skills removed from the library drop their handle (and the asset with it).
    handles.0.retain(|id, _| library.skills.contains_key(id));
    synced.retain(|id, _| library.skills.contains_key(id));
}

// ---------------------------------------------------------------------------
// Persistent stage lifecycle (ported from arena_editor::preview_controller).
// ---------------------------------------------------------------------------

/// Registers the preview lifecycle: a persistent floor at Startup + the ensure/Play/Reset
/// handlers. NOT registered by default from `SkillModePlugin` unless the `obelisk` feature host
/// wants the full preview (see `super::SkillPreviewPlugin`).
pub struct PreviewControllerPlugin;

impl Plugin for PreviewControllerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Playhead>()
            .init_resource::<PreviewCastSkill>()
            // Finding 1 (Task 10 review): the stage is scoped to `EditorMode::Skill` — a fresh
            // stage spawns on every entry, torn down on exit, mirroring `MeshModelPlugin`'s
            // pre-existing `OnEnter`/`OnExit(EditorMode::Blockout)` precedent (`src/modeling/
            // mod.rs`). Un-gated, the stage's floor/caster/dummy colliders and sim ticked in
            // EVERY editor mode: dead-zone click-selection near the origin, contaminated
            // Insert-mode placement raycasts, permanent visual clutter, and unconditional
            // physics/sim cost outside Skill mode.
            .add_systems(OnEnter(EditorMode::Skill), spawn_preview_floor)
            .add_systems(OnExit(EditorMode::Skill), despawn_stage)
            .add_systems(
                Update,
                (
                    // Chained: the stage must exist (commands applied at the sync point) before
                    // Play's cast looks for the caster.
                    (ensure_stage, start_preview).chain(),
                    reset_stage_on_reset,
                    sync_playhead,
                    clear_playhead_on_reset,
                    keep_editor_camera_during_play,
                )
                    .run_if(in_state(EditorMode::Skill)),
            );
    }
}

/// Spawn the stage floor: `OnEnter(EditorMode::Skill)` (Finding 1, Task 10 review — a fresh stage
/// on every Skill-mode entry, torn down on exit by [`despawn_stage`]; not `GameEntity`, so a Reset
/// WITHIN a Skill-mode session heals it in place instead — see `reset_stage_on_reset`). Windowed,
/// it also gets a visible slab matching the collider; headless test apps have no
/// `StandardMaterial` assets and skip it.
pub fn spawn_preview_floor(
    mut commands: Commands,
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
) {
    spawn_arena_floor(&mut commands);
    if let (Some(mut meshes), Some(mut materials)) = (meshes, materials) {
        commands.spawn((
            Name::new("PreviewFloorVisual"),
            PreviewStageFloor,
            Mesh3d(meshes.add(Cuboid::new(40.0, 1.0, 40.0))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.25, 0.25, 0.28),
                ..default()
            })),
            // Top face 1 mm below world 0 so it doesn't z-fight the editor grid while editing.
            Transform::from_xyz(0.0, -0.501, 0.0),
        ));
    }
}

/// The desired dummy layout for the currently open skill (spec's dummy auto-sync rule): a
/// `can_chain` skill with `chain_count > 0` stages `chain_count + 1` dummies, each within the
/// timeline's `chain_radius` of the one before it (so a chain hop always finds its next victim);
/// every other skill (including `GroundPoint` ones — the ground marker IS marker 1, see
/// [`ground_marker`], so "a dummy under the aim point" is already satisfied by the default
/// single dummy) gets exactly one. `None` (nothing open) also gets exactly one, so the stage
/// always shows a duel.
fn dummy_layout(entry: Option<&SkillEntry>) -> Vec<Vec3> {
    let Some(entry) = entry else {
        return vec![SPAWN_MARKERS[1]];
    };
    let dmg = &entry.rules.damage;
    if dmg.can_chain && dmg.chain_count > 0 {
        let want = dmg.chain_count as usize + 1;
        // Each successive dummy within chain_radius of the one before it (a hop's search radius
        // is centered on the STRIKE position, i.e. the previous victim) — 80% of chain_radius per
        // step, floored at 1.0 so a tiny authored radius still produces a sane, distinct layout.
        let step = (entry.timeline.chain_radius * 0.8).max(1.0);
        (0..want)
            .map(|i| SPAWN_MARKERS[1] + Vec3::Z * step * i as f32)
            .collect()
    } else {
        vec![SPAWN_MARKERS[1]]
    }
}

/// The stage's "aim/ground marker" point — what a `GroundPoint` acquisition resolves to (see
/// [`resolve_stage_acquisition`]). Fixed at marker 1: the same spot the default (non-chaining)
/// dummy stands, so "a dummy under the aim point for GroundPoint skills" holds by construction —
/// no separate marker gizmo entity is needed.
fn ground_marker() -> Vec3 {
    SPAWN_MARKERS[1]
}

/// The PERSISTENT STAGE (UX spec P3): the caster + dummies exist for the whole Skill-mode session
/// (spawned `OnEnter(EditorMode::Skill)`, torn down `OnExit` by [`despawn_stage`] — Finding 1,
/// Task 10 review), visible while editing — synced to the CURRENTLY OPEN skill (`library.open`;
/// `None` is a legal idle state — defaults to a single dummy). NOT `GameEntity`: a Reset WITHIN a
/// Skill-mode session heals/repositions instead of despawning (see `reset_stage_on_reset`).
pub fn ensure_stage(
    library: Res<SkillLibrary>,
    casters: Query<(), With<PreviewCaster>>,
    dummies: Query<Entity, With<PreviewDummy>>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut commands: Commands,
) {
    type MeshMat<'w> = Option<(ResMut<'w, Assets<Mesh>>, ResMut<'w, Assets<StandardMaterial>>)>;
    let mut meshmat: MeshMat = meshes.zip(materials);
    if casters.is_empty() {
        let caster = make_arena_combatant(&mut commands, "preview_caster", Faction::Player, SPAWN_MARKERS[0]);
        commands.entity(caster).insert((
            PreviewCaster,
            StagePost(SPAWN_MARKERS[0]),
            // Visibility on the root: a parent without InheritedVisibility silently unrenders the
            // rig subtree (Bevy B0004) — also required for the capsule mesh below to render, and
            // for `rig::spawn_preview_rig_scene` (Task 10, generic — no hardcoded rig asset here;
            // see that module) to hang a real scene under the caster if a host ever registers one.
            Visibility::default(),
        ));
        // A capsule stand-in body: v1 hung a `character.glb` rig under the caster instead (see
        // `preview_rig.rs`), but this editor has no canonical player-rig asset to hardcode (no
        // `register_gltf_library` call exists anywhere in this repo yet — see `rig.rs`'s module
        // doc). Give the caster the SAME visible-capsule treatment the dummy gets below, so the
        // stage always renders a duel even with no rig; a host that registers a real rig's GLTF
        // library can layer a scene under `PreviewCaster` itself (`rig.rs`'s anim-graph plumbing
        // picks up any `AnimationPlayer` that appears there automatically).
        if let Some((meshes, materials)) = meshmat.as_mut() {
            commands.entity(caster).insert((
                Mesh3d(meshes.add(Capsule3d::new(PLAYER_CAPSULE_RADIUS, PLAYER_CAPSULE_LENGTH))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(0.25, 0.45, 0.8),
                    ..default()
                })),
            ));
        }
    }

    let open_entry = library.open.as_ref().and_then(|id| library.skills.get(id));
    let layout = dummy_layout(open_entry);
    let have = dummies.iter().count();
    for (i, pos) in layout.iter().enumerate().skip(have) {
        let name = if i == 0 { "preview_dummy".to_string() } else { format!("preview_dummy_{}", i + 1) };
        let dummy = make_arena_combatant(&mut commands, &name, Faction::Enemy, *pos);
        commands
            .entity(dummy)
            .insert((PreviewDummy, StagePost(*pos), Visibility::default()));
        if let Some((meshes, materials)) = meshmat.as_mut() {
            commands.entity(dummy).insert((
                Mesh3d(meshes.add(Capsule3d::new(PLAYER_CAPSULE_RADIUS, PLAYER_CAPSULE_LENGTH))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(0.7, 0.25, 0.2),
                    ..default()
                })),
            ));
        }
    }
    for (i, e) in dummies.iter().enumerate() {
        if i >= layout.len() {
            commands.entity(e).despawn();
        }
    }
}

/// `OnExit(EditorMode::Skill)` (Finding 1, Task 10 review): tear down the whole stage. Hard-
/// despawns the floor (collider + visual), the caster, every dummy, and any in-flight [`Hitbox`]
/// (a live projectile at the moment of a mode switch) — all pure physics/logic entities with no
/// `bevy_vfx`/`bevy_effect` driver, so an immediate despawn is exactly what `reset_stage` already
/// does to hitboxes on every ordinary Reset (see [`PreviewStageReset::reset_stage`]) and carries
/// no grace-ladder risk.
///
/// A live [`PreviewCosmetic`] is handled differently: it's EXPIRED in place (`life.elapsed =
/// life.duration`, the same idiom `reset_stage` uses), never hard-despawned here. A cosmetic can
/// carry a live `VfxSystem`/`EffectPlayback` driver — hard-despawning it synchronously is exactly
/// the bevy_vfx dead-entity-command panic class Finding 2 just fixed a different path into (the
/// generic `ResetCommand`'s `GameEntity` despawn pass); this system must not reopen it via a
/// second path. `cosmetics::reap_preview_cosmetics` is deliberately left un-gated by `EditorMode`
/// (see that plugin's own doc comment) specifically so it can still walk an expired cosmetic
/// through its two-render-frame grace ladder after the stage — and the mode — that spawned it is
/// already gone.
#[allow(clippy::type_complexity)]
pub fn despawn_stage(
    mut commands: Commands,
    stage: Query<Entity, Or<(With<PreviewStageFloor>, With<PreviewCaster>, With<PreviewDummy>, With<Hitbox>)>>,
    mut cosmetics: Query<&mut CosmeticLifetime, With<PreviewCosmetic>>,
) {
    for e in &stage {
        commands.entity(e).despawn();
    }
    for mut life in &mut cosmetics {
        life.elapsed = life.duration;
        // Teardown wants the INSTANT clear, not the natural particle drain.
        life.drain = Some(0.0);
    }
}

/// Everything a stage reset / stage cast needs, bundled (shared by the scrub restart, Play, and
/// the editor's Reset).
#[derive(bevy::ecs::system::SystemParam)]
#[allow(clippy::type_complexity)]
pub struct PreviewStageReset<'w, 's> {
    pub commands: Commands<'w, 's>,
    casters: Query<'w, 's, (Entity, &'static Transform), With<PreviewCaster>>,
    dummies: Query<'w, 's, (Entity, &'static Transform), (With<PreviewDummy>, Without<PreviewCaster>)>,
    stage: Query<
        'w,
        's,
        (
            &'static mut obelisk_bevy::prelude::Attributes,
            &'static StagePost,
            &'static mut Position,
            &'static mut LinearVelocity,
        ),
    >,
    hitboxes: Query<'w, 's, Entity, With<Hitbox>>,
    cosmetics: Query<'w, 's, &'static mut CosmeticLifetime>,
    rng: ResMut<'w, CombatRng>,
    cooldowns: ResMut<'w, Cooldowns>,
    previewing: ResMut<'w, PreviewCastSkill>,
}

impl PreviewStageReset<'_, '_> {
    /// Reset the stage to a clean, deterministic instant: despawn in-flight hitboxes, expire
    /// cosmetics in place (the grace ladder despawns them — see `cosmetics.rs`), interrupt any
    /// live cast, heal + refill everyone, reposition to home posts, reseed the combat RNG
    /// (`CombatRng::default()`, seed 0), clear cooldowns, clear which skill is "previewing"
    /// (`cosmetics::on_preview_cue`'s cue-binding lookup key).
    pub fn reset_stage(&mut self) {
        for e in self.hitboxes.iter() {
            self.commands.entity(e).try_despawn();
        }
        for mut life in self.cosmetics.iter_mut() {
            life.elapsed = life.duration;
            // Reset wants the INSTANT clear (replay from t=0), not the natural drain.
            life.drain = Some(0.0);
        }
        for (e, _) in self.casters.iter() {
            self.commands.entity(e).interrupt_cast();
        }
        for (mut attrs, post, mut pos, mut vel) in self.stage.iter_mut() {
            attrs.0.current_life = attrs.0.max_life.base;
            attrs.0.current_mana = attrs.0.max_mana.base;
            pos.0 = post.0;
            vel.0 = Vec3::ZERO;
        }
        *self.rng = CombatRng::default();
        *self.cooldowns = Default::default();
        self.previewing.0 = None;
    }
}

/// Which skill is currently being previewed on the stage (the last skill [`stage_cast`]
/// successfully cast) — the key `cosmetics::on_preview_cue` resolves a fired `CueEvent::cue_id`
/// against (there is no `skill_id` on `CueEvent` itself). `None` until the first cast; cleared on
/// every [`PreviewStageReset::reset_stage`] (Play, editor Reset). v2 has no single "the edited
/// skill" resource (`SkillLibrary` can hold many), so this is the port's own minimal analogue —
/// scoped to "whatever is actually resolving on the stage" rather than "whatever the panel has
/// open," which can differ if the author switches skills mid-cast.
#[derive(Resource, Default)]
pub struct PreviewCastSkill(pub Option<String>);

/// The stage's resolved cast verb for one `Acquisition` branch — see
/// [`resolve_stage_acquisition`]. `pub(crate)`: Task 12's `skill::proxies` reuses this (and
/// [`StageAimContext`]/[`resolve_stage_acquisition`] below) to resolve a `WindowAnchor::CastPoint`
/// window's preview position, rather than re-deriving the same acquisition/fallback-walk logic by
/// hand a second time.
pub(crate) enum StageAim {
    Entity(Entity),
    Point(Vec3),
    Direction(Vec3),
}

/// Everything [`resolve_stage_acquisition`] needs to know about the stage's current geometry.
/// `pub(crate)` — see [`StageAim`]'s doc comment.
pub(crate) struct StageAimContext {
    pub(crate) caster_pos: Vec3,
    pub(crate) dummy: Option<(Entity, Vec3)>,
}

/// **Stage-provided acquisition resolution** (Task 10): resolve the timeline's authored
/// `Acquisition` into a concrete [`StageAim`], walking `AcqFallback` chains when the primary
/// branch's requirement can't be met on the stage. Mirrors `timeline::advance::resolve_acquisition`
/// (the REAL sim-side check `validate_casts` runs) closely enough that the branch this fn picks
/// is the one the live sim will actually accept — but this is a HOST-side pre-resolution, not a
/// substitute: the sim still validates range/filter/LOS for real when the cast lands.
/// - `Aim` → the lofted ballistic direction at the first dummy (or the ground marker with no
///   dummy) — [`preview_aim`].
/// - `HitscanEntity` → the first dummy entity, if one exists, is in `range`, and passes `filter`
///   against the caster's (Player) faction; else the fallback.
/// - `GroundPoint` → the stage's aim/ground marker point, if in `range`; else the fallback.
/// - `SelfPoint` → the caster's own position.
///
/// `pub(crate)`: reused as-is by `skill::proxies` (Task 12) to resolve a `CastPoint`-anchored
/// window's preview position — see [`StageAim`]'s doc comment.
pub(crate) fn resolve_stage_acquisition(acq: &Acquisition, tl: &CastTimeline, ctx: &StageAimContext) -> Option<StageAim> {
    match acq {
        Acquisition::Aim => {
            let to = ctx.dummy.map(|(_, p)| p).unwrap_or_else(ground_marker);
            Some(StageAim::Direction(preview_aim(tl, ctx.caster_pos, to)))
        }
        Acquisition::SelfPoint => Some(StageAim::Point(ctx.caster_pos)),
        Acquisition::HitscanEntity { range, filter, fallback } => {
            let hit = ctx.dummy.filter(|(_, pos)| {
                pos.distance(ctx.caster_pos) <= *range
                    && passes_filter(*filter, Faction::Player, Faction::Enemy, false)
            });
            match hit {
                Some((e, _)) => Some(StageAim::Entity(e)),
                None => resolve_stage_fallback(fallback, tl, ctx),
            }
        }
        Acquisition::GroundPoint { range, fallback } => {
            let marker = ground_marker();
            if marker.distance(ctx.caster_pos) <= *range {
                Some(StageAim::Point(marker))
            } else {
                resolve_stage_fallback(fallback, tl, ctx)
            }
        }
    }
}

fn resolve_stage_fallback(fallback: &AcqFallback, tl: &CastTimeline, ctx: &StageAimContext) -> Option<StageAim> {
    match fallback {
        AcqFallback::Fizzle => None,
        AcqFallback::Then(next) => resolve_stage_acquisition(next, tl, ctx),
    }
}

/// Cast `tl` (registered under `skill_id` in `CastTimelineHandles` by `sync_sim_registries`,
/// which always runs before this — see `PreviewControllerPlugin`'s system ordering) on the stage
/// caster via the resolved [`StageAim`]. `charge` is the cast's charge byte (`None` = uncharged
/// 1.0×). Records `skill_id` into [`PreviewCastSkill`] on success, so the cosmetics observer
/// knows which timeline's `cues` map to resolve fired `CueEvent`s against.
pub fn stage_cast(reset: &mut PreviewStageReset, skill_id: &str, tl: &CastTimeline, charge: Option<u8>) {
    let Some((caster, caster_tf)) = reset.casters.iter().next() else {
        return;
    };
    let ctx = StageAimContext {
        caster_pos: caster_tf.translation,
        dummy: reset.dummies.iter().next().map(|(e, tf)| (e, tf.translation)),
    };
    let Some(aim) = resolve_stage_acquisition(&tl.acquisition, tl, &ctx) else {
        warn!(
            "stage_cast: skill '{skill_id}' acquisition fizzled on the stage (no branch \
             resolved) — skipping preview cast"
        );
        return;
    };
    reset.previewing.0 = Some(skill_id.to_string());
    reset.commands.entity(caster).grant_skill(skill_id.to_string());
    let mut ec = reset.commands.entity(caster);
    match aim {
        StageAim::Entity(target) => match charge {
            Some(c) => {
                ec.cast_skill_at_charged(skill_id.to_string(), target, c);
            }
            None => {
                ec.cast_skill_at(skill_id.to_string(), target);
            }
        },
        StageAim::Point(point) => match charge {
            Some(c) => {
                ec.cast_skill_at_point_charged(skill_id.to_string(), point, c);
            }
            None => {
                ec.cast_skill_at_point(skill_id.to_string(), point);
            }
        },
        StageAim::Direction(dir) => {
            let dir3 = Dir3::new(dir).unwrap_or(Dir3::X);
            match charge {
                Some(c) => {
                    ec.cast_skill_dir_charged(skill_id.to_string(), dir3, c);
                }
                None => {
                    ec.cast_skill_dir(skill_id.to_string(), dir3);
                }
            }
        }
    }
}

/// On a `GameStartedEvent` (Play): reset the stage and cast the CURRENTLY OPEN skill on it — the
/// same deterministic path a scrub restart uses (`scrub::restart_cast`), at live speed. A no-op
/// if no skill is open. `scrub` is `Option` because a from-scratch test harness may compose the
/// stage without `PreviewScrubPlugin` (e.g. `tests/skill_preview.rs`); `None` there just means
/// Play always casts uncharged (`None` = 1.0x), same as before Task 11 threaded the charge
/// slider through.
pub fn start_preview(
    mut started: MessageReader<GameStartedEvent>,
    library: Res<SkillLibrary>,
    scrub: Option<Res<super::scrub::ScrubSim>>,
    mut reset: PreviewStageReset,
) {
    if started.read().next().is_none() {
        return;
    }
    reset.reset_stage();
    let Some(id) = library.open.as_ref() else {
        return;
    };
    let Some(entry) = library.skills.get(id) else {
        return;
    };
    let charge = scrub.map(|s| s.charge);
    stage_cast(&mut reset, id, &entry.timeline, charge);
}

/// The editor's Reset heals + repositions the persistent stage (it is not `GameEntity`, so the
/// upstream despawn pass leaves it alone).
pub fn reset_stage_on_reset(mut ev: MessageReader<GameResetEvent>, mut reset: PreviewStageReset) {
    if ev.read().next().is_some() {
        reset.reset_stage();
    }
}

/// While Playing, upstream `sync_camera_states` (if the host has one — see `bevy_editor_game`'s
/// own convention) deactivates the editor camera and activates only `GameCamera`-tagged cameras.
/// The skill preview deliberately provides NONE: Play must not move the camera — the duel is
/// watched from exactly the editor camera's current view (also the view the `bevy_vfx` billboard
/// pipeline renders on). So while Playing with no `GameCamera` in the world, re-assert the editor
/// camera as active.
pub fn keep_editor_camera_during_play(
    game_state: Res<State<GameState>>,
    game_cameras: Query<(), (With<GameCamera>, Without<EditorCamera>)>,
    mut editor_cameras: Query<&mut Camera, With<EditorCamera>>,
) {
    if *game_state.get() != GameState::Playing || !game_cameras.is_empty() {
        return;
    }
    for mut cam in &mut editor_cameras {
        if !cam.is_active {
            cam.is_active = true;
        }
    }
}

/// The timeline scrubber mirror: the `PreviewCaster`'s live `ActiveCast` (phase/elapsed/total
/// effective duration), or an idle default when no cast is in flight. Cleared on Reset.
#[derive(Resource, Default)]
pub struct Playhead {
    pub active: bool,
    pub phase: Option<SkillPhase>,
    pub elapsed: f32,
    pub total: f32,
}

pub fn sync_playhead(mut ph: ResMut<Playhead>, q: Query<&ActiveCast, With<PreviewCaster>>) {
    if let Ok(ac) = q.single() {
        ph.active = true;
        ph.phase = Some(ac.phase);
        ph.elapsed = ac.elapsed;
        ph.total = ac.total_duration();
    } else {
        ph.active = false;
        ph.phase = None;
    }
}

pub fn clear_playhead_on_reset(mut ph: ResMut<Playhead>, mut ev: MessageReader<GameResetEvent>) {
    if ev.read().next().is_some() {
        *ph = Playhead::default();
    }
}

//! Gizmo-editable window stage proxies (Task 12): an EPHEMERAL viewport visualization of the
//! currently-SELECTED `CollisionWindow`'s shape, resolved to its stage anchor position, drawn as
//! a `bevy_gizmos` outline with (for Sphere/Capsule) one draggable radius handle. Selection is set
//! by `panel::behavior`'s window-card toggle (`SkillSelection`, below) — never `SceneEntity`/
//! `GameEntity`, exists only in `EditorMode::Skill`, despawned the instant the window is
//! deselected/closed or the mode is exited.
//!
//! ## Scope decision (brief: "report your scope decision")
//!
//! The editor's existing transform-gizmo system (`src/gizmos/transform.rs`) is a full
//! translate/rotate/scale state machine keyed off `Selected`/`TransformOperation`, hardwired to
//! `EditorMode::Edit`, operating on entities the rest of the editor treats as `SceneEntity`. None
//! of that fits here: a window proxy is never `Selected`/`SceneEntity`, only ever exists in
//! `EditorMode::Skill`, and what's being edited is a single scalar (`CollisionShape::{Sphere,
//! Capsule}`'s `radius`) or a `Vec3` field on a `CollisionWindow` living inside `SkillLibrary`, not
//! a `Transform` on a scene object. Reusing that machinery would mean bolting Skill-mode-specific
//! branches onto a system already carrying five operations' worth of state, or fabricating a
//! `Selected`+`SceneEntity` just to ride its rails — both a worse fit than a small, purpose-built
//! system that follows the SAME idiom (screen-space-projected mouse-delta drag, e.g.
//! `calculate_axis_movement`) without the surrounding machinery.
//!
//! **v1 (this task) delivers:**
//! - The selected window's shape ALWAYS renders as a viewport gizmo outline at its resolved stage
//!   position (sphere / capsule / cone) — [`draw_window_proxy_gizmo`].
//! - Radius (Sphere/Capsule only — a Cone has no single "radius") is draggable directly in the
//!   viewport via one handle marker — [`drag_proxy_radius`] — using the same screen-space
//!   mouse-delta projection idiom `gizmos::transform::calculate_axis_movement` established, here
//!   specialized to one scalar instead of a 3-axis translate.
//! - [`sync_window_proxy`] recomputes the proxy's position/shape from `SkillLibrary` every frame,
//!   so editing `anchor_offset` or `radius`/`height`/`angle` through the Behavior region's
//!   EXISTING numeric fields (`panel::behavior::draw_shape`/`draw_anchor`) moves or resizes the
//!   live gizmo immediately, with no extra plumbing — "the card's numeric edits update the gizmo
//!   live" holds for every field this way, not only the one with a dedicated viewport handle.
//!
//! **Follow-up, NOT delivered**: a 3-axis `anchor_offset` viewport drag handle, and a Cone
//! `range`/`angle` handle. The brief explicitly permits this split ("at least ONE
//! viewport-draggable handle... if full drag-gizmo infra is disproportionate, draw the outline +
//! document that drag-handles are a follow-up"). Radius was chosen as the one delivered handle
//! because it's a single scalar (the simplest possible drag), and because the offset dimension is
//! already fully covered — with live gizmo feedback — by the Behavior region's existing x/y/z
//! `DragValue`s, so a second, 3-axis handle's marginal value is smaller than its marginal cost.
//!
//! ## Resolved position is a preview, not a substitute for the real sim
//!
//! [`resolve_window_stage_position`] reuses `preview::stage`'s OWN acquisition resolution
//! (`resolve_stage_acquisition`/`StageAim`/`StageAimContext`, widened to `pub(crate)` for this)
//! rather than re-deriving the same fallback-walk logic by hand a second time. It answers "where
//! would this window's volume spawn if cast right now, at rest" — no `ActiveCast`, no
//! `muzzle_offset` (both only exist mid-cast) — a best-effort preview position, exactly as Task
//! 10's own host-side pre-resolution is a preview, never a substitute for the sim's authoritative
//! resolution at actual cast time.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::EguiContexts;

use obelisk_bevy::assets::{CastTimeline, CollisionShape, CollisionWindow, WindowAnchor};

use crate::editor::{EditorCamera, EditorMode};

use super::library::SkillLibrary;
use super::preview::stage::{self, StageAim, StageAimContext};

// ---------------------------------------------------------------------------
// Selection signal (set by `panel::behavior`'s window-card toggle)
// ---------------------------------------------------------------------------

/// Which `CollisionWindow` (by index into `entry.timeline.collision_windows`) is selected for
/// viewport gizmo preview, and which skill it belongs to. `for_id` is ignored — treated as no
/// selection — whenever it doesn't match `SkillLibrary.open` (same "stale state belonging to a
/// different skill is discarded" convention `SkillSaveState`/`ChipSwitchPrompt` already use), so a
/// leftover selection from a since-closed skill can never resolve a proxy against the WRONG
/// skill's timeline.
#[derive(Resource, Default, Debug, Clone, PartialEq)]
pub struct SkillSelection {
    pub for_id: Option<String>,
    pub window: Option<usize>,
}

// ---------------------------------------------------------------------------
// Proxy entity
// ---------------------------------------------------------------------------

/// Marks the ephemeral viewport gizmo proxy for the currently-selected window. Never
/// `SceneEntity`/`GameEntity` (see the module doc comment) — at most one exists at a time,
/// spawned/updated/despawned by [`sync_window_proxy`].
#[derive(Component, Debug)]
pub struct WindowProxy {
    pub window_index: usize,
}

/// The proxy's current shape — a plain snapshot of `CollisionWindow::shape`, refreshed every
/// frame by [`sync_window_proxy`] (this refresh is how a numeric-field radius/height/angle edit
/// reaches the gizmo "live" — see the module doc comment).
#[derive(Component, Debug, Clone, Copy)]
pub struct ProxyShape(pub CollisionShape);

const PROXY_COLOR: Color = Color::srgba(1.0, 0.85, 0.2, 0.9);
const HANDLE_COLOR: Color = Color::srgb(1.0, 0.25, 0.25);
/// Visual radius (world units) of the drag-handle marker sphere — deliberately constant
/// regardless of the window's own radius, so the handle doesn't get harder to grab as the shape
/// shrinks.
const HANDLE_MARKER_RADIUS: f32 = 0.06;
/// Click tolerance (screen pixels) for grabbing the radius handle.
const HANDLE_CLICK_RADIUS_PX: f32 = 16.0;

/// Minimum/maximum a dragged (or numerically-edited) radius may reach — mirrors
/// `panel::behavior::draw_shape`'s own `CollisionShape::Sphere`/`Capsule` `DragValue` range
/// (`0.05..=50.0`) so the viewport handle can never push a window's radius outside what the
/// numeric field itself already allows.
pub const MIN_PROXY_RADIUS: f32 = 0.05;
pub const MAX_PROXY_RADIUS: f32 = 50.0;

// ---------------------------------------------------------------------------
// Position resolution (pure — see the module doc comment)
// ---------------------------------------------------------------------------

/// Resolve `win`'s stage anchor position for a RESTING preview (no live cast) — see the module
/// doc comment. `caster_pos`/`dummy_pos` are plain positions (not queries), so this stays a pure,
/// directly-unit-testable function; [`sync_window_proxy`] is the thin ECS wrapper that supplies
/// them from the real stage entities.
pub(crate) fn resolve_window_stage_position(
    win: &CollisionWindow,
    tl: &CastTimeline,
    caster_pos: Vec3,
    dummy_pos: Option<Vec3>,
) -> Vec3 {
    let anchor_base = match win.anchor {
        WindowAnchor::Caster => caster_pos,
        WindowAnchor::CastPoint => {
            let ctx = StageAimContext {
                caster_pos,
                dummy: dummy_pos.map(|p| (Entity::PLACEHOLDER, p)),
            };
            match stage::resolve_stage_acquisition(&tl.acquisition, tl, &ctx) {
                Some(StageAim::Point(p)) => p,
                // `HitscanEntity` resolving to an entity can only ever be OUR dummy (the only
                // entity `ctx` carries) — use its position directly, no identity check needed.
                Some(StageAim::Entity(_)) => dummy_pos.unwrap_or(caster_pos),
                // `Direction` (an `Aim`-rooted resolution) or `None` (fizzle): the acquisition
                // can't produce a point at all. `validate_timeline` structurally rejects a
                // `CastPoint` anchor on such content (see that fn's own doc comment), so this arm
                // is "dead in valid content" — kept only as an honest, non-panicking fallback for
                // a mid-edit, momentarily-invalid timeline.
                Some(StageAim::Direction(_)) | None => caster_pos,
            }
        }
    };
    anchor_base + win.anchor_offset
}

/// Everything [`sync_window_proxy`] needs each frame, as one pure fn — most of "should a proxy
/// exist, and where/what shape" is unit-testable with no ECS `App` at all; the system below is a
/// thin `Query`/`Commands` wrapper around it.
fn resolved_proxy_state(
    library: &SkillLibrary,
    selection: &SkillSelection,
    caster_pos: Vec3,
    dummy_pos: Option<Vec3>,
) -> Option<(usize, CollisionShape, Vec3)> {
    let open_id = library.open.as_deref()?;
    if selection.for_id.as_deref() != Some(open_id) {
        return None;
    }
    let idx = selection.window?;
    let entry = library.skills.get(open_id)?;
    let win = entry.timeline.collision_windows.get(idx)?;
    let pos = resolve_window_stage_position(win, &entry.timeline, caster_pos, dummy_pos);
    Some((idx, win.shape, pos))
}

// ---------------------------------------------------------------------------
// Lifecycle system
// ---------------------------------------------------------------------------

/// `Update`, gated `in_state(EditorMode::Skill)` (see [`SkillProxyPlugin`]): spawn/refresh/despawn
/// the (at most one) window-proxy entity from `SkillSelection` + `SkillLibrary`, every frame —
/// see the module doc comment for why "every frame" is what makes numeric-field edits reach the
/// gizmo live. Updates an EXISTING proxy via a direct mutable `Query` (not `Commands`), so a
/// same-frame-ordered `draw_window_proxy_gizmo` sees the fresh position/shape with no
/// command-flush lag; `Commands` is used only for the spawn/despawn transitions.
pub fn sync_window_proxy(
    mut commands: Commands,
    library: Res<SkillLibrary>,
    selection: Res<SkillSelection>,
    caster_q: Query<&Transform, (With<stage::PreviewCaster>, Without<WindowProxy>)>,
    dummy_q: Query<&Transform, (With<stage::PreviewDummy>, Without<WindowProxy>)>,
    mut proxy_q: Query<(Entity, &mut Transform, &mut ProxyShape, &mut WindowProxy)>,
) {
    let Some(caster_pos) = caster_q.iter().next().map(|t| t.translation) else {
        // No stage yet (e.g. Skill mode just entered) — nothing to anchor a proxy against.
        for (e, ..) in &proxy_q {
            commands.entity(e).despawn();
        }
        return;
    };
    let dummy_pos = dummy_q.iter().next().map(|t| t.translation);

    match resolved_proxy_state(&library, &selection, caster_pos, dummy_pos) {
        Some((idx, shape, pos)) => {
            let mut iter = proxy_q.iter_mut();
            match iter.next() {
                Some((_, mut transform, mut proxy_shape, mut proxy)) => {
                    transform.translation = pos;
                    proxy_shape.0 = shape;
                    proxy.window_index = idx;
                    // Defensive: never more than one proxy should exist, but if something
                    // upstream ever double-spawned, don't leave a stray extra behind.
                    for (extra, ..) in iter {
                        commands.entity(extra).despawn();
                    }
                }
                None => {
                    commands.spawn((
                        Name::new("SkillWindowProxy"),
                        WindowProxy { window_index: idx },
                        ProxyShape(shape),
                        Transform::from_translation(pos),
                    ));
                }
            }
        }
        None => {
            for (e, ..) in &proxy_q {
                commands.entity(e).despawn();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Drawing (not headless-testable — rendering; exercised visually by the probe)
// ---------------------------------------------------------------------------

/// `Update`, gated `in_state(EditorMode::Skill)`, chained after [`sync_window_proxy`]: draw the
/// proxy's shape outline, plus (Sphere/Capsule) its radius-handle marker.
pub fn draw_window_proxy_gizmo(mut gizmos: Gizmos, proxy_q: Query<(&Transform, &ProxyShape)>) {
    for (transform, shape) in &proxy_q {
        let center = transform.translation;
        match shape.0 {
            CollisionShape::Sphere { radius } => {
                gizmos.sphere(Isometry3d::from_translation(center), radius, PROXY_COLOR);
                draw_radius_handle(&mut gizmos, center, radius);
            }
            CollisionShape::Capsule { radius, height } => {
                draw_capsule_outline(&mut gizmos, center, radius, height);
                draw_radius_handle(&mut gizmos, center, radius);
            }
            CollisionShape::Cone { angle, range } => {
                draw_cone_outline(&mut gizmos, center, angle, range);
            }
        }
    }
}

/// World position of the radius drag handle: along the resolved anchor's world +X axis.
/// `anchor_offset` is itself authored in world axes, never caster-rotated (see
/// `timeline::advance`'s `anchor_base + win.anchor_offset`), so a fixed world-axis handle position
/// is consistent with how the shape is actually placed, not an arbitrary choice.
fn radius_handle_world_pos(center: Vec3, radius: f32) -> Vec3 {
    center + Vec3::X * radius
}

fn draw_radius_handle(gizmos: &mut Gizmos, center: Vec3, radius: f32) {
    gizmos.sphere(
        Isometry3d::from_translation(radius_handle_world_pos(center, radius)),
        HANDLE_MARKER_RADIUS,
        HANDLE_COLOR,
    );
}

/// Two horizontal rings (top/bottom, matching the world-Y-standing convention every other stage
/// capsule in this editor uses — `stage::make_arena_combatant`) plus four vertical side lines.
fn draw_capsule_outline(gizmos: &mut Gizmos, center: Vec3, radius: f32, height: f32) {
    let half = height * 0.5;
    let top = center + Vec3::Y * half;
    let bottom = center - Vec3::Y * half;
    // `Isometry3d::IDENTITY`'s circle lies in the XY plane (bevy_gizmos' own doc comment); rotate
    // 90° about X to get a HORIZONTAL ring (XZ plane) around the vertical capsule.
    let horizontal = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    gizmos.circle(Isometry3d::new(top, horizontal), radius, PROXY_COLOR);
    gizmos.circle(Isometry3d::new(bottom, horizontal), radius, PROXY_COLOR);
    for i in 0..4 {
        let angle = i as f32 * std::f32::consts::FRAC_PI_2;
        let offset = Vec3::new(angle.cos(), 0.0, angle.sin()) * radius;
        gizmos.line(top + offset, bottom + offset, PROXY_COLOR);
    }
}

/// A cone drawn along world +X from `apex` (no live cast/aim direction exists at rest — see the
/// module doc comment; this is a visual approximation, not a substitute for the sim's own
/// aim-facing cone at cast time): a base ring at `range`, sized by the half-angle, plus four lines
/// from the apex to the ring.
fn draw_cone_outline(gizmos: &mut Gizmos, apex: Vec3, angle_deg: f32, range: f32) {
    let half_angle = (angle_deg.to_radians() * 0.5).clamp(0.01, std::f32::consts::FRAC_PI_2 - 0.001);
    let base_radius = (range * half_angle.tan()).max(0.01);
    let base_center = apex + Vec3::X * range;
    // Rotate the default XY-plane circle 90° about Y so its normal is +X — perpendicular to the
    // cone's (world +X) axis.
    let ring_rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
    gizmos.circle(Isometry3d::new(base_center, ring_rot), base_radius, PROXY_COLOR);
    for i in 0..4 {
        let angle = i as f32 * std::f32::consts::FRAC_PI_2;
        let offset = Vec3::new(0.0, angle.cos(), angle.sin()) * base_radius;
        gizmos.line(apex, base_center + offset, PROXY_COLOR);
    }
}

// ---------------------------------------------------------------------------
// Radius drag (the one viewport-draggable handle this task delivers)
// ---------------------------------------------------------------------------

/// Persists across frames whether the radius handle is currently being dragged — a plain click
/// only starts a drag when it lands within [`HANDLE_CLICK_RADIUS_PX`] of the handle; once active,
/// the drag continues tracking mouse movement until release regardless of where the cursor
/// wanders (mirrors ordinary click-drag UX elsewhere; the alternative — re-checking proximity or
/// UI-hover every frame — would make the drag stutter/cancel if the cursor crosses the panel).
#[derive(Resource, Default)]
pub struct ProxyDragState {
    active: bool,
}

/// Project a 2D pixel-space mouse `delta` onto the on-screen direction from `screen_center` to
/// `screen_axis_point` (the SAME world axis, sampled one world unit away), returning the
/// equivalent WORLD-space delta along that axis. Pure screen math, no camera/ECS — the same idiom
/// `gizmos::transform::calculate_axis_movement` uses for its 3-axis translate/scale drags, here
/// specialized to the one scalar (radius) this task's handle drags. Returns `0.0` for a
/// degenerate (near-zero-length) screen projection of the axis (camera looking straight down it).
pub(crate) fn project_pixel_delta_to_world_units(screen_center: Vec2, screen_axis_point: Vec2, delta: Vec2) -> f32 {
    let screen_axis = screen_axis_point - screen_center;
    let len = screen_axis.length();
    if len < 0.001 {
        return 0.0;
    }
    delta.dot(screen_axis / len) / len
}

/// Clamp a radius after adding a world-space `delta` — see [`MIN_PROXY_RADIUS`]/[`MAX_PROXY_RADIUS`].
pub(crate) fn apply_radius_drag(old_radius: f32, world_delta: f32) -> f32 {
    (old_radius + world_delta).clamp(MIN_PROXY_RADIUS, MAX_PROXY_RADIUS)
}

/// `Update`, gated `in_state(EditorMode::Skill)`, chained after [`draw_window_proxy_gizmo`]: the
/// ONE viewport-draggable handle this task delivers (see the module doc comment's scope
/// decision) — Sphere/Capsule radius only. A left-click within [`HANDLE_CLICK_RADIUS_PX`] of the
/// rendered handle marker starts a drag; holding + moving the mouse resizes the radius
/// (screen-space-projected onto the handle's world +X axis); release ends it. Writes straight
/// into `SkillLibrary` — the SAME source of truth the Behavior region's numeric field writes —
/// and flips `dirty_timeline`, so Save/validation see it exactly like any other edit.
///
/// Not headless-testable (real mouse/camera/viewport input) — see the module doc comment; the
/// pixel-to-world projection ([`project_pixel_delta_to_world_units`]) and the clamped radius
/// update ([`apply_radius_drag`]) are extracted as pure fns and unit-tested directly instead.
#[allow(clippy::too_many_arguments)] // one Bevy system param per resource/query touched; same
                                      // rationale as `skill::skill_probe`/`panel::behavior::window_card`.
pub fn drag_proxy_radius(
    mouse: Res<ButtonInput<MouseButton>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    mut contexts: EguiContexts,
    camera_q: Query<(&Camera, &GlobalTransform), With<EditorCamera>>,
    proxy_q: Query<(&Transform, &ProxyShape)>,
    window_q: Query<&Window, With<PrimaryWindow>>,
    mut drag: ResMut<ProxyDragState>,
    mut library: ResMut<SkillLibrary>,
    selection: Res<SkillSelection>,
) {
    if mouse.just_released(MouseButton::Left) {
        drag.active = false;
    }

    let Ok((camera, camera_tf)) = camera_q.single() else {
        return;
    };
    let Some((transform, shape)) = proxy_q.iter().next() else {
        drag.active = false;
        return;
    };
    let radius = match shape.0 {
        CollisionShape::Sphere { radius } | CollisionShape::Capsule { radius, .. } => radius,
        CollisionShape::Cone { .. } => {
            drag.active = false;
            return;
        }
    };
    let center = transform.translation;

    if !drag.active {
        let ui_blocking = contexts
            .ctx_mut()
            .map(|ctx| ctx.wants_pointer_input() || ctx.is_pointer_over_area())
            .unwrap_or(false);
        if ui_blocking || !mouse.just_pressed(MouseButton::Left) {
            return;
        }
        let Ok(window) = window_q.single() else {
            return;
        };
        let Some(cursor) = window.cursor_position() else {
            return;
        };
        let Ok(handle_screen) = camera.world_to_viewport(camera_tf, radius_handle_world_pos(center, radius)) else {
            return;
        };
        if cursor.distance(handle_screen) > HANDLE_CLICK_RADIUS_PX {
            return;
        }
        drag.active = true;
    }

    let delta = mouse_motion.delta;
    if delta == Vec2::ZERO {
        return;
    }
    let Ok(center_screen) = camera.world_to_viewport(camera_tf, center) else {
        return;
    };
    let Ok(axis_screen) = camera.world_to_viewport(camera_tf, center + Vec3::X) else {
        return;
    };
    let world_delta = project_pixel_delta_to_world_units(center_screen, axis_screen, delta);
    let new_radius = apply_radius_drag(radius, world_delta);

    let Some(open_id) = library.open.clone() else {
        return;
    };
    if selection.for_id.as_deref() != Some(open_id.as_str()) {
        return;
    }
    let Some(idx) = selection.window else {
        return;
    };
    if let Some(entry) = library.skills.get_mut(&open_id)
        && let Some(win) = entry.timeline.collision_windows.get_mut(idx)
    {
        match &mut win.shape {
            CollisionShape::Sphere { radius } => *radius = new_radius,
            CollisionShape::Capsule { radius, .. } => *radius = new_radius,
            CollisionShape::Cone { .. } => return,
        }
        entry.dirty_timeline = true;
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

/// `OnExit(EditorMode::Skill)`: despawn any live proxy and reset the selection — mirrors
/// `stage::despawn_stage`/`scrub::reset_scrub_on_mode_exit`'s own teardown precedent (Tasks
/// 10/11), so a stale proxy/selection can't linger into another editor mode or a later Skill-mode
/// session.
fn teardown_window_proxy(
    mut commands: Commands,
    q: Query<Entity, With<WindowProxy>>,
    mut selection: ResMut<SkillSelection>,
    mut drag: ResMut<ProxyDragState>,
) {
    for e in &q {
        commands.entity(e).despawn();
    }
    *selection = SkillSelection::default();
    *drag = ProxyDragState::default();
}

/// Bundles Task 12's selection resource + the proxy lifecycle/drawing/dragging systems.
/// Registered from `SkillModePlugin` (engine/logic, not UI, per this module family's convention —
/// see `crate::skill`'s own doc comment), NOT from `EditorGizmosPlugin` (that plugin is compiled
/// unconditionally; everything here is `#[cfg(feature = "obelisk")]`).
pub struct SkillProxyPlugin;

impl Plugin for SkillProxyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SkillSelection>()
            .init_resource::<ProxyDragState>()
            .add_systems(
                Update,
                (sync_window_proxy, draw_window_proxy_gizmo, drag_proxy_radius)
                    .chain()
                    .run_if(in_state(EditorMode::Skill)),
            )
            .add_systems(OnExit(EditorMode::Skill), teardown_window_proxy);
    }
}

// ---------------------------------------------------------------------------
// Tests (pure fns only — see the module/fn doc comments for what's not headless-testable)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use obelisk_bevy::assets::{AcqFallback, Acquisition, HitFilter, HitMode, VolumeMotion, WindowPhase, WindowSpawn};

    fn timeline_with(acquisition: Acquisition) -> CastTimeline {
        CastTimeline {
            skill_id: "t".to_string(),
            acquisition,
            ..blank_timeline()
        }
    }

    fn blank_timeline() -> CastTimeline {
        CastTimeline {
            skill_id: "t".to_string(),
            phase_durations: obelisk_bevy::assets::PhaseDurations { windup: 0.0, active: 0.0, recovery: 0.0 },
            collision_windows: Vec::new(),
            acquisition: Acquisition::default(),
            vfx_cues: Default::default(),
            chain_radius: 6.0,
            chargeable: false,
            max_hold: 1.0,
            cues: Default::default(),
        }
    }

    fn window(anchor: WindowAnchor, offset: Vec3) -> CollisionWindow {
        CollisionWindow {
            id: "w".to_string(),
            spawn: WindowSpawn::Scheduled { phase: WindowPhase::Active, offset: 0.0 },
            anchor,
            anchor_offset: offset,
            strikes: true,
            active_duration: 1.0,
            shape: CollisionShape::Sphere { radius: 0.5 },
            motion: VolumeMotion::Static,
            motion_direction: Default::default(),
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::OncePerTarget,
            rehit_interval: None,
            emitter: None,
        }
    }

    const CASTER: Vec3 = Vec3::new(-4.0, 0.59, 0.0);
    const DUMMY: Vec3 = Vec3::new(4.0, 0.59, 0.0);

    #[test]
    fn caster_anchor_ignores_acquisition_entirely() {
        let win = window(WindowAnchor::Caster, Vec3::new(0.0, 1.0, 1.5));
        // Acquisition::Aim, which produces no point at all — irrelevant for a Caster anchor.
        let tl = timeline_with(Acquisition::Aim);

        let pos = resolve_window_stage_position(&win, &tl, CASTER, Some(DUMMY));

        assert_eq!(pos, CASTER + Vec3::new(0.0, 1.0, 1.5));
    }

    #[test]
    fn cast_point_with_self_point_acquisition_resolves_to_the_caster() {
        let win = window(WindowAnchor::CastPoint, Vec3::ZERO);
        let tl = timeline_with(Acquisition::SelfPoint);

        let pos = resolve_window_stage_position(&win, &tl, CASTER, None);

        assert_eq!(pos, CASTER);
    }

    #[test]
    fn cast_point_with_ground_point_in_range_resolves_to_the_ground_marker() {
        let win = window(WindowAnchor::CastPoint, Vec3::new(0.0, 3.0, 0.0));
        let tl = timeline_with(Acquisition::GroundPoint { range: 20.0, fallback: AcqFallback::Fizzle });

        let pos = resolve_window_stage_position(&win, &tl, CASTER, Some(DUMMY));

        // `stage::ground_marker()` is fixed at `SPAWN_MARKERS[1]` — the dummy's own default spot.
        assert_eq!(pos, stage::SPAWN_MARKERS[1] + Vec3::new(0.0, 3.0, 0.0));
    }

    #[test]
    fn cast_point_with_ground_point_out_of_range_falls_back() {
        let win = window(WindowAnchor::CastPoint, Vec3::ZERO);
        let tl = timeline_with(Acquisition::GroundPoint {
            range: 1.0, // far short of the ~8-unit stage duel gap
            fallback: AcqFallback::Then(Box::new(Acquisition::SelfPoint)),
        });

        let pos = resolve_window_stage_position(&win, &tl, CASTER, Some(DUMMY));

        assert_eq!(pos, CASTER, "out-of-range GroundPoint must walk its Then(SelfPoint) fallback");
    }

    #[test]
    fn cast_point_on_a_point_incapable_acquisition_falls_back_to_caster_without_panicking() {
        // `Aim` never produces a point; `validate_timeline` rejects this combination structurally
        // in real content, but the resolver must still degrade sanely for a mid-edit instant.
        let win = window(WindowAnchor::CastPoint, Vec3::ZERO);
        let tl = timeline_with(Acquisition::Aim);

        let pos = resolve_window_stage_position(&win, &tl, CASTER, Some(DUMMY));

        assert_eq!(pos, CASTER);
    }

    // -- resolved_proxy_state (selection + library plumbing, still pure) -----------------------

    fn library_with_window(id: &str, win: CollisionWindow) -> SkillLibrary {
        use crate::skill::library::SkillEntry;
        use std::path::PathBuf;
        let mut library = SkillLibrary::default();
        let (rules, _) = crate::skill::templates::strike_template(id);
        let mut tl = blank_timeline();
        tl.skill_id = id.to_string();
        tl.collision_windows.push(win);
        library.skills.insert(
            id.to_string(),
            SkillEntry {
                rules,
                timeline: tl,
                rules_path: PathBuf::new(),
                timeline_path: PathBuf::new(),
                dirty_rules: false,
                dirty_timeline: false,
                disk_hash: (0, 0),
            },
        );
        library.open = Some(id.to_string());
        library
    }

    #[test]
    fn no_selection_resolves_to_nothing() {
        let library = library_with_window("a", window(WindowAnchor::Caster, Vec3::ZERO));
        let selection = SkillSelection { for_id: Some("a".to_string()), window: None };

        assert!(resolved_proxy_state(&library, &selection, CASTER, Some(DUMMY)).is_none());
    }

    #[test]
    fn selection_for_a_different_skill_is_ignored() {
        let library = library_with_window("a", window(WindowAnchor::Caster, Vec3::ZERO));
        let selection = SkillSelection { for_id: Some("stale_other_skill".to_string()), window: Some(0) };

        assert!(resolved_proxy_state(&library, &selection, CASTER, Some(DUMMY)).is_none());
    }

    #[test]
    fn valid_selection_resolves_the_windows_shape_and_position() {
        let win = window(WindowAnchor::Caster, Vec3::new(0.0, 1.0, 1.5));
        let library = library_with_window("a", win);
        let selection = SkillSelection { for_id: Some("a".to_string()), window: Some(0) };

        let (idx, shape, pos) = resolved_proxy_state(&library, &selection, CASTER, Some(DUMMY)).expect("resolves");

        assert_eq!(idx, 0);
        // `CollisionShape` has no `PartialEq` impl — match its one field instead.
        assert!(matches!(shape, CollisionShape::Sphere { radius } if (radius - 0.5).abs() < 1e-6), "{shape:?}");
        assert_eq!(pos, CASTER + Vec3::new(0.0, 1.0, 1.5));
    }

    #[test]
    fn out_of_range_window_index_resolves_to_nothing() {
        let library = library_with_window("a", window(WindowAnchor::Caster, Vec3::ZERO));
        let selection = SkillSelection { for_id: Some("a".to_string()), window: Some(5) };

        assert!(resolved_proxy_state(&library, &selection, CASTER, Some(DUMMY)).is_none());
    }

    // -- drag math (pure) ------------------------------------------------------------------

    #[test]
    fn pixel_projection_converts_screen_delta_to_world_units() {
        // 50 screen px == 1 world unit along the axis; a 25px delta along that same direction is
        // 0.5 world units.
        let world_delta = project_pixel_delta_to_world_units(Vec2::new(100.0, 100.0), Vec2::new(150.0, 100.0), Vec2::new(25.0, 0.0));
        assert!((world_delta - 0.5).abs() < 1e-5, "{world_delta}");
    }

    #[test]
    fn pixel_projection_is_negative_moving_the_opposite_way() {
        let world_delta = project_pixel_delta_to_world_units(Vec2::new(100.0, 100.0), Vec2::new(150.0, 100.0), Vec2::new(-25.0, 0.0));
        assert!((world_delta + 0.5).abs() < 1e-5, "{world_delta}");
    }

    #[test]
    fn pixel_projection_ignores_perpendicular_motion() {
        let world_delta = project_pixel_delta_to_world_units(Vec2::new(100.0, 100.0), Vec2::new(150.0, 100.0), Vec2::new(0.0, 40.0));
        assert!(world_delta.abs() < 1e-5, "{world_delta}");
    }

    #[test]
    fn pixel_projection_degenerate_axis_is_zero() {
        // Camera looking straight down the axis — its screen projection collapses to a point.
        let world_delta = project_pixel_delta_to_world_units(Vec2::new(100.0, 100.0), Vec2::new(100.0, 100.0), Vec2::new(25.0, 10.0));
        assert_eq!(world_delta, 0.0);
    }

    #[test]
    fn radius_drag_adds_the_world_delta() {
        assert!((apply_radius_drag(1.0, 0.5) - 1.5).abs() < 1e-5);
        assert!((apply_radius_drag(1.0, -0.5) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn radius_drag_clamps_at_the_minimum() {
        assert_eq!(apply_radius_drag(0.1, -10.0), MIN_PROXY_RADIUS);
    }

    #[test]
    fn radius_drag_clamps_at_the_maximum() {
        assert_eq!(apply_radius_drag(40.0, 100.0), MAX_PROXY_RADIUS);
    }
}

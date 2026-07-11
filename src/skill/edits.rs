//! Pure Behavior-region timeline mutations (Task 7), TDD'd inline against
//! `obelisk_bevy::assets::validate_timeline`. No egui, no ECS — the panel
//! (`crate::skill::panel::behavior`) wires these to buttons/drags, same split as
//! `crate::skill::library`'s lifecycle ops (pure fns; the caller flips `dirty_timeline`
//! once at the end of the region, mirroring the Rules region's `changed: bool` idiom).
//!
//! Adapted from obelisk-arena's `crates/arena_editor/src/edits.rs` (v1: `WindowTemplate` +
//! `add_window_from_template`/`set_window_start`) — the pattern (pure, tested mutation
//! fns; an archetype enum driving a "+ window" picker) carries over unchanged. The
//! CONTENT is new: v1 had no `Acquisition`/`Emitter`/`Template` spawn kind at all, so
//! `add_emitter`/`add_emitter_with_new_template`/`remove_window`'s emitter-reference
//! guard/`set_spawn_kind` have no v1 analog — they exist to keep every mutation landing
//! on a timeline `validate_timeline` still accepts (Task 11's emitter/Template rules,
//! Task 10's CastPoint-reachability rule).
//!
//! `remove_emitter` (phase-3 whole-branch review fix) is the one deliberate exception to
//! that "stays valid" invariant: it always succeeds when an emitter is present, even when
//! doing so leaves the emitter's former `Template` target transiently unreferenced (a
//! `validate_timeline` rejection). See its doc comment for why refusing instead would
//! deadlock it against `remove_window`'s/`set_spawn_kind`'s own emitter-reference guards.

use bevy::prelude::*;

use obelisk_bevy::assets::{
    CastTimeline, CollisionShape, CollisionWindow, Emitter, HitFilter, HitMode, VolumeMotion,
    WindowAnchor, WindowPhase, WindowSpawn,
};

// ---------------------------------------------------------------------------
// Window archetypes — "+ window" picker
// ---------------------------------------------------------------------------

/// The four window shape archetypes the Behavior region's "+ window" picker offers —
/// deliberately the SAME four shapes `crate::skill::templates`'s skill archetypes author
/// (so a window added mid-timeline looks identical to one a fresh archetype template
/// would produce), minus the whole-skill wrapper (rules + acquisition): adding a window
/// to an EXISTING timeline must never silently change that timeline's acquisition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowArchetype {
    Strike,
    Projectile,
    Zone,
    Beam,
}

impl WindowArchetype {
    pub const ALL: [WindowArchetype; 4] =
        [Self::Strike, Self::Projectile, Self::Zone, Self::Beam];

    pub fn label(&self) -> &'static str {
        match self {
            Self::Strike => "Strike (melee sweep)",
            Self::Projectile => "Projectile (flying bolt)",
            Self::Zone => "Zone (damage field)",
            Self::Beam => "Beam (hitscan strike)",
        }
    }

    fn id_base(&self) -> &'static str {
        match self {
            Self::Strike => "strike",
            Self::Projectile => "bolt",
            Self::Zone => "zone",
            Self::Beam => "beam",
        }
    }

    /// This archetype's `CollisionWindow` shape (anchor, shape, motion, hit_filter,
    /// hit_mode, rehit_interval) — field-for-field identical to the matching branch of
    /// `crate::skill::templates`'s per-archetype window, except `id` (caller-supplied,
    /// uniqueness-checked by `add_window_from_archetype`) and `spawn` (always freshly
    /// `Scheduled` at `Active`+0 — a window added via this picker starts on-schedule; the
    /// author can flip it to `Template` afterward via `set_spawn_kind`).
    fn build(&self, id: String) -> CollisionWindow {
        let spawn = WindowSpawn::Scheduled { phase: WindowPhase::Active, offset: 0.0 };
        match self {
            Self::Strike => CollisionWindow {
                id,
                spawn,
                anchor: WindowAnchor::Caster,
                anchor_offset: Vec3::new(0.0, 1.0, 1.5),
                strikes: true,
                active_duration: 0.15,
                shape: CollisionShape::Sphere { radius: 1.5 },
                motion: VolumeMotion::Static,
                motion_direction: Default::default(),
                hit_filter: HitFilter::Enemies,
                hit_mode: HitMode::OncePerTarget,
                rehit_interval: None,
                emitter: None,
                paints: None,
            },
            Self::Projectile => CollisionWindow {
                id,
                spawn,
                anchor: WindowAnchor::Caster,
                anchor_offset: Vec3::ZERO,
                strikes: true,
                active_duration: 2.0,
                shape: CollisionShape::Sphere { radius: 0.4 },
                motion: VolumeMotion::Linear { speed: 25.0 },
                motion_direction: Default::default(),
                hit_filter: HitFilter::Enemies,
                hit_mode: HitMode::FirstOnly,
                rehit_interval: None,
                emitter: None,
                paints: None,
            },
            Self::Zone => CollisionWindow {
                id,
                spawn,
                anchor: WindowAnchor::CastPoint,
                anchor_offset: Vec3::ZERO,
                strikes: true,
                active_duration: 2.0,
                shape: CollisionShape::Sphere { radius: 3.0 },
                motion: VolumeMotion::Static,
                motion_direction: Default::default(),
                hit_filter: HitFilter::Enemies,
                hit_mode: HitMode::EveryTick,
                rehit_interval: Some(0.5),
                emitter: None,
                paints: None,
            },
            Self::Beam => CollisionWindow {
                id,
                spawn,
                anchor: WindowAnchor::Caster,
                anchor_offset: Vec3::ZERO,
                strikes: true,
                active_duration: 0.1,
                shape: CollisionShape::Sphere { radius: 0.5 },
                motion: VolumeMotion::Beam,
                motion_direction: Default::default(),
                hit_filter: HitFilter::Enemies,
                hit_mode: HitMode::FirstOnly,
                rehit_interval: None,
                emitter: None,
                paints: None,
            },
        }
    }
}

/// Pick a window id not already present in `tl`, starting from `base` and appending
/// `_2`, `_3`, ... on collision — same idiom as `crate::skill::library::unique_id`.
pub fn unique_window_id(tl: &CastTimeline, base: &str) -> String {
    if !tl.collision_windows.iter().any(|w| w.id == base) {
        return base.to_string();
    }
    for i in 2.. {
        let candidate = format!("{base}_{i}");
        if !tl.collision_windows.iter().any(|w| w.id == candidate) {
            return candidate;
        }
    }
    unreachable!("a CastTimeline can't hold usize::MAX windows")
}

/// Append a new `Scheduled` window built from `archetype`, auto-ided uniquely against
/// `tl`'s existing window ids. Returns its index.
pub fn add_window_from_archetype(tl: &mut CastTimeline, archetype: WindowArchetype) -> usize {
    let id = unique_window_id(tl, archetype.id_base());
    tl.collision_windows.push(archetype.build(id));
    tl.collision_windows.len() - 1
}

// ---------------------------------------------------------------------------
// Emitters
// ---------------------------------------------------------------------------

/// Wire `tl.collision_windows[window_idx]` to emit into an EXISTING `Template` window
/// named `target_window_id`, at a sensible default rate/jitter. Refused (message names
/// the blocker, mirroring `validate_timeline`'s own wording) if:
/// - `window_idx` is out of range;
/// - the source window is itself a `Template` (Task 11's Template->Template recursion
///   guard — a Template window is never itself alive on its own, so an emitter on it
///   could never tick);
/// - `target_window_id` doesn't name an existing window, or names one that isn't a
///   `Template` (an emitter may only instantiate a Template window).
pub fn add_emitter(tl: &mut CastTimeline, window_idx: usize, target_window_id: &str) -> Result<(), String> {
    let Some(source) = tl.collision_windows.get(window_idx) else {
        return Err(format!("no window at index {window_idx}"));
    };
    if source.spawn == WindowSpawn::Template {
        return Err(format!(
            "'{}' is a Template and may not itself carry an emitter",
            source.id
        ));
    }
    let Some(target) = tl.collision_windows.iter().find(|w| w.id == target_window_id) else {
        return Err(format!("no window named '{target_window_id}'"));
    };
    if target.spawn != WindowSpawn::Template {
        return Err(format!(
            "'{target_window_id}' is not a Template — an emitter may only instantiate a \
             Template window"
        ));
    }

    tl.collision_windows[window_idx].emitter = Some(Emitter {
        rate: 10.0,
        jitter: 0.5,
        window: target_window_id.to_string(),
    });
    Ok(())
}

/// Simpler authoring path (brief: "simpler authoring"): create a FRESH `Template` window
/// — a copy of `window_idx`'s own shape (a shard inheriting its parent's silhouette is a
/// better starting default than a bare sphere; the author can re-shape it afterward) with
/// `spawn` forced to `Template` and its own `emitter` cleared (guards the recursion rule
/// even though a freshly-cloned Scheduled window would already have `emitter: None`) —
/// and wire `window_idx`'s emitter at it in one step. Returns the new Template window's
/// index. Assumes `window_idx` is valid (the panel only ever calls this from inside a
/// `0..len` loop over `tl.collision_windows`, same invariant `rules.rs`'s trigger-card
/// loop relies on for its own by-index mutation).
pub fn add_emitter_with_new_template(tl: &mut CastTimeline, window_idx: usize) -> usize {
    let base = format!("{}_shard", tl.collision_windows[window_idx].id);
    let new_id = unique_window_id(tl, &base);

    let mut shard = tl.collision_windows[window_idx].clone();
    shard.id = new_id.clone();
    shard.spawn = WindowSpawn::Template;
    shard.emitter = None;
    tl.collision_windows.push(shard);
    let new_idx = tl.collision_windows.len() - 1;

    tl.collision_windows[window_idx].emitter = Some(Emitter {
        rate: 10.0,
        jitter: 0.5,
        window: new_id,
    });
    new_idx
}

/// Clear `tl.collision_windows[window_idx]`'s `emitter` field — the inverse of `add_emitter`/
/// `add_emitter_with_new_template`. Refused (message names the blocker) if:
/// - `window_idx` is out of range;
/// - the window carries no emitter to remove.
///
/// Unlike `remove_window`'s conservative "refuse rather than cascade" reading, this fn does
/// NOT refuse when clearing the reference leaves its `Template` target unreferenced by any
/// other window's emitter (`validate_timeline` would then reject the result: "is a Template
/// but is never referenced by an emitter"). That transient invalid state is intentional, not
/// an oversight: `remove_window`'s own guards ("a Template referenced by another window's
/// emitter" / "carries an emitter") and `set_spawn_kind`'s ("flip FROM Template while still
/// referenced") all refuse to touch a Template or its carrier WHILE the emitter reference is
/// live. This fn is the ONLY way to sever that reference without deleting a whole window — if
/// it also refused whenever the target would end up unreferenced, all three fns would
/// deadlock each other (nothing could ever unblock the other two; see
/// `remove_emitter_then_remove_window_on_the_orphan_restores_validity` below for the proof).
/// The author's natural next click is `remove_window` on the now-unreferenced Template (no
/// longer blocked), repointing another emitter at it (`add_emitter`), or
/// `set_spawn_kind(.., false)` to fold it back to `Scheduled` (also no longer blocked) — the
/// panel's live-validation inline card (see `panel::behavior`'s module doc comment) surfaces
/// the orphan in the meantime, same as any other `validate_timeline` rejection.
pub fn remove_emitter(tl: &mut CastTimeline, window_idx: usize) -> Result<(), String> {
    let Some(window) = tl.collision_windows.get(window_idx) else {
        return Err(format!("no window at index {window_idx}"));
    };
    if window.emitter.is_none() {
        return Err(format!("'{}' has no emitter to remove", window.id));
    }

    tl.collision_windows[window_idx].emitter = None;
    Ok(())
}

// ---------------------------------------------------------------------------
// Remove / spawn-kind flip
// ---------------------------------------------------------------------------

/// Remove `tl.collision_windows[idx]`. Refused (message names the blocker) if:
/// - `idx` is out of range;
/// - the window is a `Template` referenced by another window's emitter — removing it
///   would leave that emitter dangling (`validate_timeline` would reject the result: "no
///   window named '...'");
/// - the window itself CARRIES an emitter — removing it would leave its Template target
///   unreferenced by anyone (`validate_timeline` would reject THAT: "is a Template but is
///   never referenced by an emitter"). The brief leaves "the caller decides" for this
///   case; this fn takes the conservative reading — refuse rather than silently cascading
///   a second removal, so the author explicitly removes the Template (or repoints the
///   emitter) first and always sees exactly one thing change per click.
pub fn remove_window(tl: &mut CastTimeline, idx: usize) -> Result<(), String> {
    let Some(window) = tl.collision_windows.get(idx) else {
        return Err(format!("no window at index {idx}"));
    };
    let id = window.id.clone();

    if window.spawn == WindowSpawn::Template
        && let Some(referrer) = tl
            .collision_windows
            .iter()
            .find(|w| w.emitter.as_ref().is_some_and(|e| e.window == id))
    {
        return Err(format!(
            "'{id}' is a Template referenced by '{}'s emitter — remove or repoint that \
             emitter first",
            referrer.id
        ));
    }

    if window.emitter.is_some() {
        return Err(format!(
            "'{id}' carries an emitter — remove the emitter (or the Template window it \
             targets) first"
        ));
    }

    tl.collision_windows.remove(idx);
    Ok(())
}

/// Rename `tl.collision_windows[idx]`'s id to `new_id`, repointing any OTHER window's
/// `emitter.window` that targeted the old id so the timeline keeps validating (a window
/// card's id field is a free-text edit — silently orphaning an emitter reference on
/// rename would be a `validate_timeline` rejection the author never asked for). Refused
/// if `idx` is out of range, `new_id` is empty, or another window already has that id.
/// A no-op `Ok(())` (no rename, no repoint) when `new_id` equals the window's current id.
pub fn rename_window(tl: &mut CastTimeline, idx: usize, new_id: &str) -> Result<(), String> {
    if new_id.is_empty() {
        return Err("window id can't be empty".to_string());
    }
    let Some(window) = tl.collision_windows.get(idx) else {
        return Err(format!("no window at index {idx}"));
    };
    let old_id = window.id.clone();
    if old_id == new_id {
        return Ok(());
    }
    if tl.collision_windows.iter().any(|w| w.id == new_id) {
        return Err(format!("a window named '{new_id}' already exists"));
    }

    tl.collision_windows[idx].id = new_id.to_string();
    for w in tl.collision_windows.iter_mut() {
        if let Some(em) = &mut w.emitter
            && em.window == old_id
        {
            em.window = new_id.to_string();
        }
    }
    Ok(())
}

/// Flip `tl.collision_windows[idx]`'s `spawn` between `Scheduled` (`Active`+0 default,
/// when flipping in) and `Template`. Refused (message names the blocker) if:
/// - `idx` is out of range;
/// - flipping TO `Template` while the window still carries an emitter itself (Task 11's
///   recursion guard);
/// - flipping FROM `Template` while some other window's emitter still targets this
///   window's id (that emitter would then target a non-Template window — also a
///   `validate_timeline` rejection). The caller must repoint/remove the emitter first.
pub fn set_spawn_kind(tl: &mut CastTimeline, idx: usize, to_template: bool) -> Result<(), String> {
    let Some(window) = tl.collision_windows.get(idx) else {
        return Err(format!("no window at index {idx}"));
    };
    let id = window.id.clone();

    if to_template {
        if window.emitter.is_some() {
            return Err(format!(
                "'{id}' carries an emitter — a Template window may not itself carry one; \
                 remove it first"
            ));
        }
        tl.collision_windows[idx].spawn = WindowSpawn::Template;
    } else {
        if let Some(referrer) = tl
            .collision_windows
            .iter()
            .find(|w| w.emitter.as_ref().is_some_and(|e| e.window == id))
        {
            return Err(format!(
                "'{id}' is referenced by '{}'s emitter — repoint or remove that emitter before \
                 making it Scheduled",
                referrer.id
            ));
        }
        tl.collision_windows[idx].spawn = WindowSpawn::Scheduled { phase: WindowPhase::Active, offset: 0.0 };
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::templates::{projectile_template, zone_template};
    use obelisk_bevy::assets::validate_timeline;

    fn fixture() -> CastTimeline {
        let (_, tl) = projectile_template("t");
        assert!(validate_timeline(&tl).is_ok(), "fixture must start valid");
        tl
    }

    /// A `GroundPoint`-acquisition fixture — the base `WindowArchetype::Zone` needs, since
    /// its window anchors on `CastPoint` (`add_window_from_archetype` never touches
    /// `tl.acquisition`, by design: adding a window must not silently change what a
    /// timeline's acquisition can produce, so a Zone window landing on an `Aim`-only
    /// timeline is correctly a `validate_timeline` rejection, exercised separately below).
    fn point_producing_fixture() -> CastTimeline {
        let (_, tl) = zone_template("t");
        assert!(validate_timeline(&tl).is_ok(), "fixture must start valid");
        tl
    }

    // -- add_window_from_archetype -----------------------------------------------------

    #[test]
    fn add_window_from_archetype_appends_and_validates() {
        for archetype in WindowArchetype::ALL {
            let mut tl = if archetype == WindowArchetype::Zone {
                point_producing_fixture()
            } else {
                fixture()
            };
            let before = tl.collision_windows.len();
            let idx = add_window_from_archetype(&mut tl, archetype);
            assert_eq!(idx, before);
            assert_eq!(tl.collision_windows.len(), before + 1);
            assert!(
                validate_timeline(&tl).is_ok(),
                "{archetype:?} window must leave the timeline valid: {:?}",
                validate_timeline(&tl)
            );
        }
    }

    /// The flip side of the fixture split above: a Zone window's `CastPoint` anchor landing
    /// on an `Aim`-only (never-produces-a-point) timeline IS correctly rejected by
    /// `validate_timeline` — `add_window_from_archetype` doesn't (and shouldn't) fix that up
    /// on the caller's behalf; the panel surfaces the rejection via the live-validation
    /// inline card message (see `panel::behavior`).
    #[test]
    fn zone_window_on_a_point_incapable_acquisition_is_correctly_rejected() {
        let mut tl = fixture(); // Acquisition::Aim
        add_window_from_archetype(&mut tl, WindowArchetype::Zone);
        assert!(validate_timeline(&tl).is_err());
    }

    #[test]
    fn add_window_from_archetype_uniques_the_id_on_collision() {
        let mut tl = fixture();
        // The fixture's own window is "bolt" — adding a Projectile (base "bolt") twice must
        // not collide with it or with itself.
        let i1 = add_window_from_archetype(&mut tl, WindowArchetype::Projectile);
        let i2 = add_window_from_archetype(&mut tl, WindowArchetype::Projectile);
        let ids: Vec<&str> = tl.collision_windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids.iter().collect::<std::collections::HashSet<_>>().len(), 3, "ids must be unique: {ids:?}");
        assert_ne!(tl.collision_windows[i1].id, tl.collision_windows[i2].id);
    }

    // -- add_emitter ----------------------------------------------------------------------

    #[test]
    fn add_emitter_wires_an_existing_template_positive() {
        let mut tl = fixture();
        let template_idx = add_window_from_archetype(&mut tl, WindowArchetype::Strike);
        set_spawn_kind(&mut tl, template_idx, true).expect("flip to Template");
        let template_id = tl.collision_windows[template_idx].id.clone();

        let source_idx = 0; // the fixture's scheduled "bolt" window
        let result = add_emitter(&mut tl, source_idx, &template_id);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(tl.collision_windows[source_idx].emitter.as_ref().unwrap().window, template_id);
        assert!(validate_timeline(&tl).is_ok(), "{:?}", validate_timeline(&tl));
    }

    #[test]
    fn add_emitter_refuses_unknown_target() {
        let mut tl = fixture();
        let result = add_emitter(&mut tl, 0, "does_not_exist");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no window named"));
    }

    #[test]
    fn add_emitter_refuses_non_template_target() {
        let mut tl = fixture();
        add_window_from_archetype(&mut tl, WindowArchetype::Strike);
        let scheduled_id = tl.collision_windows[1].id.clone();
        let result = add_emitter(&mut tl, 0, &scheduled_id);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a Template"));
    }

    #[test]
    fn add_emitter_refuses_source_that_is_itself_a_template() {
        let mut tl = fixture();
        let source_idx = add_window_from_archetype(&mut tl, WindowArchetype::Strike);
        set_spawn_kind(&mut tl, source_idx, true).expect("flip source to Template");

        let target_idx = add_window_from_archetype(&mut tl, WindowArchetype::Projectile);
        set_spawn_kind(&mut tl, target_idx, true).expect("flip target to Template");
        let target_id = tl.collision_windows[target_idx].id.clone();

        let result = add_emitter(&mut tl, source_idx, &target_id);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("may not itself carry"));
    }

    // -- add_emitter_with_new_template -----------------------------------------------------

    #[test]
    fn add_emitter_with_new_template_creates_and_wires_a_shard() {
        let mut tl = fixture();
        let before = tl.collision_windows.len();
        let source_idx = 0;
        let new_idx = add_emitter_with_new_template(&mut tl, source_idx);

        assert_eq!(tl.collision_windows.len(), before + 1);
        assert_eq!(new_idx, before);
        assert_eq!(tl.collision_windows[new_idx].spawn, WindowSpawn::Template);
        assert!(tl.collision_windows[new_idx].emitter.is_none());
        assert_eq!(
            tl.collision_windows[source_idx].emitter.as_ref().unwrap().window,
            tl.collision_windows[new_idx].id
        );
        assert!(validate_timeline(&tl).is_ok(), "{:?}", validate_timeline(&tl));
    }

    // -- remove_emitter ---------------------------------------------------------------------

    #[test]
    fn remove_emitter_clears_the_field_positive() {
        let mut tl = fixture();
        add_emitter_with_new_template(&mut tl, 0);
        assert!(tl.collision_windows[0].emitter.is_some(), "fixture must start with an emitter");

        let result = remove_emitter(&mut tl, 0);
        assert!(result.is_ok(), "{result:?}");
        assert!(tl.collision_windows[0].emitter.is_none());
    }

    /// Documents the orphan-state design decision from the doc comment above: clearing the
    /// ONLY emitter that referenced a Template window leaves that Template transiently
    /// invalid — `remove_emitter` does NOT cascade-fix it (delete it, or flip it back to
    /// Scheduled) on the caller's behalf.
    #[test]
    fn remove_emitter_leaves_an_unreferenced_template_transiently_invalid() {
        let mut tl = fixture();
        add_emitter_with_new_template(&mut tl, 0);
        remove_emitter(&mut tl, 0).expect("fixture's window 0 has an emitter");

        let err = validate_timeline(&tl).expect_err("the now-orphaned Template must fail validation");
        assert!(err.contains("never referenced by an emitter"), "{err}");
    }

    /// The orphan documented above is never a dead end: once the emitter is cleared, the
    /// previously-blocked `remove_window` on the (now-unreferenced) Template succeeds,
    /// restoring validity — proving the three emitter/Template fns don't deadlock each other
    /// (see `remove_emitter`'s doc comment).
    #[test]
    fn remove_emitter_then_remove_window_on_the_orphan_restores_validity() {
        let mut tl = fixture();
        let template_idx = add_emitter_with_new_template(&mut tl, 0);
        remove_emitter(&mut tl, 0).expect("fixture's window 0 has an emitter");

        let result = remove_window(&mut tl, template_idx);
        assert!(result.is_ok(), "{result:?}");
        assert!(validate_timeline(&tl).is_ok(), "{:?}", validate_timeline(&tl));
    }

    #[test]
    fn remove_emitter_refuses_when_none_present() {
        let mut tl = fixture();
        let result = remove_emitter(&mut tl, 0);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no emitter"));
    }

    #[test]
    fn remove_emitter_refuses_out_of_range() {
        let mut tl = fixture();
        let result = remove_emitter(&mut tl, 99);
        assert!(result.is_err());
    }

    // -- remove_window ----------------------------------------------------------------------

    #[test]
    fn remove_window_positive_when_unreferenced_and_no_emitter() {
        let mut tl = fixture();
        add_window_from_archetype(&mut tl, WindowArchetype::Strike);
        let before = tl.collision_windows.len();
        let result = remove_window(&mut tl, 1);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(tl.collision_windows.len(), before - 1);
        assert!(validate_timeline(&tl).is_ok());
    }

    #[test]
    fn remove_window_refuses_a_template_referenced_by_an_emitter() {
        let mut tl = fixture();
        add_emitter_with_new_template(&mut tl, 0);
        let template_idx = tl
            .collision_windows
            .iter()
            .position(|w| w.spawn == WindowSpawn::Template)
            .unwrap();
        let result = remove_window(&mut tl, template_idx);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("referenced by"));
    }

    #[test]
    fn remove_window_refuses_a_window_that_carries_an_emitter() {
        let mut tl = fixture();
        add_emitter_with_new_template(&mut tl, 0);
        let result = remove_window(&mut tl, 0);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("carries an emitter"));
    }

    #[test]
    fn remove_window_refuses_out_of_range() {
        let mut tl = fixture();
        let result = remove_window(&mut tl, 99);
        assert!(result.is_err());
    }

    // -- rename_window -----------------------------------------------------------------------

    #[test]
    fn rename_window_positive_and_repoints_emitters() {
        let mut tl = fixture();
        let template_idx = add_emitter_with_new_template(&mut tl, 0);
        let old_template_id = tl.collision_windows[template_idx].id.clone();

        let result = rename_window(&mut tl, template_idx, "shard_v2");
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(tl.collision_windows[template_idx].id, "shard_v2");
        assert_eq!(tl.collision_windows[0].emitter.as_ref().unwrap().window, "shard_v2");
        assert_ne!(old_template_id, "shard_v2");
        assert!(validate_timeline(&tl).is_ok(), "{:?}", validate_timeline(&tl));
    }

    #[test]
    fn rename_window_refuses_duplicate_id() {
        let mut tl = fixture();
        add_window_from_archetype(&mut tl, WindowArchetype::Strike);
        let taken = tl.collision_windows[0].id.clone();
        let result = rename_window(&mut tl, 1, &taken);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));
    }

    #[test]
    fn rename_window_refuses_empty_id() {
        let mut tl = fixture();
        let result = rename_window(&mut tl, 0, "");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn rename_window_refuses_out_of_range() {
        let mut tl = fixture();
        assert!(rename_window(&mut tl, 99, "x").is_err());
    }

    #[test]
    fn rename_window_same_id_is_a_noop_ok() {
        let mut tl = fixture();
        let id = tl.collision_windows[0].id.clone();
        assert!(rename_window(&mut tl, 0, &id).is_ok());
    }

    // -- set_spawn_kind ----------------------------------------------------------------------

    #[test]
    fn set_spawn_kind_flips_scheduled_to_template_positive() {
        let mut tl = fixture();
        add_window_from_archetype(&mut tl, WindowArchetype::Strike);
        let idx = 1;
        // A lone, unreferenced Template is itself invalid (never referenced) — so wire an
        // emitter at it right after flipping, to check the flip mutation in isolation first.
        let result = set_spawn_kind(&mut tl, idx, true);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(tl.collision_windows[idx].spawn, WindowSpawn::Template);
    }

    #[test]
    fn set_spawn_kind_refuses_to_template_when_it_carries_an_emitter() {
        let mut tl = fixture();
        add_emitter_with_new_template(&mut tl, 0);
        // window 0 now carries an emitter; flipping IT to Template must be refused.
        let result = set_spawn_kind(&mut tl, 0, true);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("may not itself carry"));
    }

    #[test]
    fn set_spawn_kind_flips_template_to_scheduled_when_unreferenced() {
        let mut tl = fixture();
        add_window_from_archetype(&mut tl, WindowArchetype::Strike);
        set_spawn_kind(&mut tl, 1, true).unwrap();
        let result = set_spawn_kind(&mut tl, 1, false);
        assert!(result.is_ok(), "{result:?}");
        assert!(matches!(tl.collision_windows[1].spawn, WindowSpawn::Scheduled { .. }));
        assert!(validate_timeline(&tl).is_ok());
    }

    #[test]
    fn set_spawn_kind_refuses_template_to_scheduled_when_still_referenced() {
        let mut tl = fixture();
        add_emitter_with_new_template(&mut tl, 0);
        let template_idx = tl
            .collision_windows
            .iter()
            .position(|w| w.spawn == WindowSpawn::Template)
            .unwrap();
        let result = set_spawn_kind(&mut tl, template_idx, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("referenced by"));
    }

    #[test]
    fn set_spawn_kind_refuses_out_of_range() {
        let mut tl = fixture();
        assert!(set_spawn_kind(&mut tl, 99, true).is_err());
    }
}

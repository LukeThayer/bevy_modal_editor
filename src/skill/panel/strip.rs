//! The Skill-mode scrub strip (Task 11 — ported from arena_editor's `panel.rs` strip-painting
//! portion + `timeline_geom.rs`, obelisk-arena @ `f6472e4`): the painted phase/window strip with
//! the scrubber + playhead, event markers, the (replay) control, and the charge slider.
//!
//! **v1 -> v2 schema adaptations:**
//! - No `WindowPhase::Chained`/`resolved_window_span` recursion: v2 deleted authored window
//!   chaining entirely (see `obelisk_bevy::assets::CastTimeline`'s own doc comment) — every
//!   window is either `Scheduled` (a fixed `(start, end)` — [`window_span`]) or `Template`
//!   (never on the phase schedule, only an emitter instantiates it; drawn with no fixed span of
//!   its own).
//! - [`base_span`] (v1's `strip_span`) has zero visibility into a rules-triggered sub-cast (a
//!   `TriggeredExec` is a wholly separate skill/timeline) — the strip's actual RENDERED extent
//!   is `base_span(..).max(scrub.dynamic_end)`, the latter empirically discovered as the sim
//!   runs (see `crate::skill::preview::scrub::extend_dynamic_end`), not computed from authored
//!   data. [`draw_scrub_strip`] paints the region between them as a visually distinct trailing
//!   band.
//! - The charge slider and event markers are new (v1 kept charge on the panel's timeline tab
//!   directly; v2 has no equivalent event log outside test harnesses, see
//!   `crate::skill::preview::scrub`'s module doc comment).
//!
//! **Task 11 review fix — INTERACTIVE range vs RENDERED range.** The strip used to map a
//! click/drag straight to the RENDERED range (`base.max(dynamic_end)`), which made the trailing
//! sub-cast region permanently unreachable: `dynamic_end` only grows when `drive_scrub`'s seek
//! loop (`while clock < target`) runs at least one iteration, which requires `target` to sit
//! STRICTLY past the sim's current clock — but the far edge of the rendered range always equals
//! the CURRENT `dynamic_end`/clock by construction, so the loop always ran zero iterations and
//! the region froze one tick past `base` forever. [`strip_click_to_target`] now maps clicks
//! against a wider INTERACTIVE range, hard-capped at `base + MAX_TRAILING_SECS` — the exact same
//! ceiling `drive_scrub`'s own `hard_cap` already allows `target` to reach — so a single
//! far-edge gesture can always drive the seek loop forward and grow `dynamic_end`, whether or not
//! anything has been discovered yet. [`draw_scrub_strip`] uses this same wider span as the ONE
//! pixel-mapping denominator for every element (phase/window bars, markers, playhead), not just
//! the click formula, so what's drawn always lines up with what's clickable; the consequence
//! (chosen deliberately over incremental-headroom growth, which needs multiple drags to reach a
//! distant sub-cast and is less predictable UX) is that a short base skill's phases/windows
//! occupy a small fraction of the strip's width — mitigated by shading and labeling the
//! not-yet-simulated tail so that compression reads as intentional, not broken.
//!
//! [`draw_scrub_strip`] is pure UI: it never touches `ScrubSim` directly (the panel that calls it
//! — `crate::skill::draw_skill_panel` — is an EXCLUSIVE `world: &mut World` fn that snapshots
//! resources as immutable locals before the egui window closure and writes mutations back only
//! after the closure ends, the same clone-out/write-back convention every other region in this
//! panel follows). A strip drag/click writes `*new_target`, the (replay) button sets
//! `*replay_clicked`, and the slider writes `*new_charge`; the caller applies them to the real
//! `ScrubSim` resource.

use bevy_egui::egui;

use obelisk_bevy::assets::{CastTimeline, CollisionWindow, PhaseDurations, WindowPhase, WindowSpawn};
use obelisk_bevy::prelude::charge_mult;

use crate::skill::library::SkillEntry;
use crate::skill::preview::{
    MarkerKind, Playhead, ScrubMarkers, ScrubMode, ScrubSim, MAX_TRAILING_SECS,
};

const STRIP_H: f32 = 40.0;
const PHASE_COLORS: [egui::Color32; 3] = [
    egui::Color32::from_rgb(60, 80, 130),
    egui::Color32::from_rgb(130, 70, 60),
    egui::Color32::from_rgb(60, 110, 80),
];

/// Total authored duration of the three phases (clamping negatives to 0).
pub fn total_duration(d: &PhaseDurations) -> f32 {
    d.windup.max(0.0) + d.active.max(0.0) + d.recovery.max(0.0)
}

/// The `[windup, active, recovery]` phase spans as `(start, end)` absolute-time pairs.
pub fn phase_spans(d: &PhaseDurations) -> [(f32, f32); 3] {
    let w = d.windup.max(0.0);
    let a = d.active.max(0.0);
    let r = d.recovery.max(0.0);
    [(0.0, w), (w, w + a), (w + a, w + a + r)]
}

/// Absolute `(start, end)` of a SCHEDULED window given the phase durations. `Template` windows
/// (v2: never self-schedule — only an emitter instantiates them) have no fixed span of their
/// own: `None`.
pub fn window_span(d: &PhaseDurations, w: &CollisionWindow) -> Option<(f32, f32)> {
    let WindowSpawn::Scheduled { phase, offset } = w.spawn else {
        return None;
    };
    let phase_start = match phase {
        WindowPhase::Windup => 0.0,
        WindowPhase::Active => d.windup.max(0.0),
        WindowPhase::Recovery => d.windup.max(0.0) + d.active.max(0.0),
    };
    let start = phase_start + offset.max(0.0);
    Some((start, start + w.active_duration.max(0.0)))
}

/// The BASE time span the strip covers from AUTHORED data alone: the phase total, extended to
/// the latest scheduled window's close. See the module doc comment for why this is blind to any
/// rules-triggered sub-cast (the "dynamic end" the caller must separately max in).
pub fn base_span(tl: &CastTimeline) -> f32 {
    tl.collision_windows
        .iter()
        .filter_map(|w| window_span(&tl.phase_durations, w))
        .map(|(_, e)| e)
        .fold(total_duration(&tl.phase_durations), f32::max)
}

/// Map an absolute time `t` (over `[0, span]`) into a pixel `x` in `[left, left + width]`.
/// Degenerate `span <= 0` pins to `left`.
pub fn time_to_x(t: f32, span: f32, left: f32, width: f32) -> f32 {
    if span <= 0.0 {
        left
    } else {
        left + (t / span).clamp(0.0, 1.0) * width
    }
}

/// Map a strip click/drag's normalized pointer fraction `rel_x` (`[0, 1]` across the strip's
/// pixel rect) to an absolute sim-time seek target.
///
/// This is the strip's INTERACTIVE range, deliberately wider than its RENDERED range (`base`
/// paired with the empirically-discovered `dynamic_end` — see the module doc comment). Task 11
/// review: clamping the click target to the rendered range made the trailing sub-cast region
/// permanently unreachable, since the far edge of that range always equals the sim's OWN current
/// clock by construction, so `drive_scrub`'s seek loop (`while clock < target`) always ran zero
/// iterations there.
///
/// The interactive span is hard-capped at `base + MAX_TRAILING_SECS` — the SAME ceiling
/// `drive_scrub`'s own `hard_cap` already enforces on `target` — so a runaway drag can never ask
/// the engine to seek further than it is willing to run, and a single far-edge gesture can always
/// reach anywhere the engine can go. `dynamic_end` is folded in via `.max` defensively: it can
/// never legitimately exceed the cap (`extend_dynamic_end` clamps it too), but this keeps the
/// interactive range from ever being narrower than what's already been rendered.
pub fn strip_click_to_target(rel_x: f32, base: f32, dynamic_end: f32) -> f32 {
    let interactive_span = (base + MAX_TRAILING_SECS).max(dynamic_end);
    rel_x.clamp(0.0, 1.0) * interactive_span
}

fn marker_color(kind: MarkerKind) -> egui::Color32 {
    match kind {
        MarkerKind::WindowOpened => egui::Color32::from_rgb(120, 200, 255),
        MarkerKind::Hit => egui::Color32::from_rgb(255, 90, 90),
        MarkerKind::Ended => egui::Color32::from_rgb(200, 200, 200),
        MarkerKind::Trigger => egui::Color32::from_rgb(255, 210, 60),
    }
}

fn marker_label(kind: MarkerKind) -> &'static str {
    match kind {
        MarkerKind::WindowOpened => "window opened",
        MarkerKind::Hit => "hit",
        MarkerKind::Ended => "window ended",
        MarkerKind::Trigger => "trigger fired",
    }
}

/// Draw the scrub strip for the currently open `entry`: the phase/window bars, the two-band
/// trailing region (discovered vs undiscovered — see the module doc comment), event markers, the
/// playhead, the (replay) control, and the charge slider. See the module doc comment for the
/// out-param write-back convention and the interactive-vs-rendered range split.
#[allow(clippy::too_many_arguments)]
pub fn draw_scrub_strip(
    ui: &mut egui::Ui,
    entry: &SkillEntry,
    scrub: &ScrubSim,
    markers: &ScrubMarkers,
    playhead: &Playhead,
    new_target: &mut Option<f32>,
    replay_clicked: &mut bool,
    new_charge: &mut Option<u8>,
) {
    let tl = &entry.timeline;
    let base = base_span(tl).max(0.0001);
    // RENDERED range: what's actually known -- authored data (`base`) maxed with however far the
    // sim has empirically run this scrub session (`dynamic_end`). Phase/window bars, markers, and
    // the "discovered" trailing band all represent times within this range.
    let rendered_end = base.max(scrub.dynamic_end);
    // INTERACTIVE range: the single pixel-mapping denominator for EVERYTHING below (bars,
    // markers, playhead, AND the click formula) — deliberately wider than `rendered_end` so a
    // far-edge gesture can always reach into not-yet-simulated trailing time. See the module doc
    // comment (Task 11 review) for why this must be one shared scale, not two.
    let interactive_span = (base + MAX_TRAILING_SECS).max(rendered_end);

    let (rect, strip_resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), STRIP_H),
        egui::Sense::click_and_drag(),
    );
    if (strip_resp.clicked() || strip_resp.dragged())
        && let Some(pos) = strip_resp.interact_pointer_pos()
    {
        let rel_x = (pos.x - rect.left()) / rect.width().max(1.0);
        *new_target = Some(strip_click_to_target(rel_x, base, scrub.dynamic_end));
    }
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, egui::Color32::from_rgb(24, 24, 28));

    // The trailing region past `base` is TWO visually distinct bands (Task 11 review): what the
    // sim has actually run (`base..rendered_end`, dim warm -- may hold a revealed sub-cast) and
    // what it hasn't yet (`rendered_end..interactive_span`, dim cool + labeled -- dragging there
    // is the "simulate ahead" gesture that grows `dynamic_end`). Without this split a short base
    // skill's own content would read as a bare sliver with no explanation why.
    if rendered_end > base {
        let x0 = time_to_x(base, interactive_span, rect.left(), rect.width());
        let x1 = time_to_x(rendered_end, interactive_span, rect.left(), rect.width());
        p.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom())),
            0.0,
            egui::Color32::from_rgb(45, 38, 30),
        );
    }
    if interactive_span > rendered_end {
        let x0 = time_to_x(rendered_end, interactive_span, rect.left(), rect.width());
        p.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(rect.right(), rect.bottom())),
            0.0,
            egui::Color32::from_rgb(20, 24, 30),
        );
        p.text(
            egui::pos2(x0 + 4.0, rect.top() + 3.0),
            egui::Align2::LEFT_TOP,
            "drag to simulate ahead",
            egui::FontId::proportional(9.0),
            egui::Color32::from_rgb(130, 140, 155),
        );
    }

    for (i, (s, e)) in phase_spans(&tl.phase_durations).iter().enumerate() {
        let x0 = time_to_x(*s, interactive_span, rect.left(), rect.width());
        let x1 = time_to_x(*e, interactive_span, rect.left(), rect.width());
        p.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.center().y)),
            0.0,
            PHASE_COLORS[i],
        );
    }
    for w in &tl.collision_windows {
        let Some((ws, we)) = window_span(&tl.phase_durations, w) else {
            continue;
        };
        let x0 = time_to_x(ws, interactive_span, rect.left(), rect.width());
        let x1 = time_to_x(we, interactive_span, rect.left(), rect.width());
        p.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x0, rect.center().y + 2.0),
                egui::pos2(x1.max(x0 + 2.0), rect.bottom()),
            ),
            2.0,
            egui::Color32::from_rgb(220, 180, 60),
        );
    }

    // Event markers (window opens / hits / ends / trigger firings) — ticks along the strip's
    // bottom edge, colored by kind, sourced from the scrub session's own recorder.
    for marker in &markers.0 {
        let x = time_to_x(marker.time, interactive_span, rect.left(), rect.width());
        p.line_segment(
            [egui::pos2(x, rect.bottom() - 8.0), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(2.0, marker_color(marker.kind)),
        );
    }
    if let Some(pos) = strip_resp.hover_pos() {
        const HOVER_PX: f32 = 4.0;
        if let Some(hovered) = markers.0.iter().min_by(|a, b| {
            let da = (time_to_x(a.time, interactive_span, rect.left(), rect.width()) - pos.x).abs();
            let db = (time_to_x(b.time, interactive_span, rect.left(), rect.width()) - pos.x).abs();
            da.total_cmp(&db)
        }) {
            let x = time_to_x(hovered.time, interactive_span, rect.left(), rect.width());
            if (x - pos.x).abs() <= HOVER_PX {
                strip_resp.clone().on_hover_text(format!(
                    "{} ({}) @ {:.2}s",
                    marker_label(hovered.kind),
                    hovered.label,
                    hovered.time
                ));
            }
        }
    }

    if playhead.active && playhead.total > 0.0 {
        let x = time_to_x(playhead.elapsed, interactive_span, rect.left(), rect.width());
        p.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(230, 70, 70)),
        );
    } else if scrub.mode != ScrubMode::Idle {
        // The scrub head shows the SIM's actual clock (the frozen truth), not the request. Using
        // `interactive_span` (not just `rendered_end`) lets it actually sit IN the trailing
        // region while a far seek is landing there, instead of pinning to the old range's edge.
        let x = time_to_x(scrub.clock, interactive_span, rect.left(), rect.width());
        p.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(240, 160, 50)),
        );
    }

    ui.horizontal(|ui| {
        if ui
            .button("\u{27f3} replay")
            .on_hover_text("re-run the cast at 1x from the start (deterministic)")
            .clicked()
        {
            *replay_clicked = true;
        }
        match scrub.mode {
            ScrubMode::Frozen => {
                ui.label(
                    egui::RichText::new(format!("frozen at {:.2}s -- drag to seek", scrub.clock))
                        .small(),
                );
            }
            ScrubMode::Replaying | ScrubMode::Seeking => {
                ui.label(egui::RichText::new(format!("{:.2}s", scrub.clock)).small());
            }
            ScrubMode::Idle => {
                ui.label(
                    egui::RichText::new("drag the strip to scrub the REAL sim; replay re-runs it")
                        .small()
                        .weak(),
                );
            }
        }
        ui.separator();
        ui.label("charge");
        let mut charge = scrub.charge as u32;
        if ui
            .add(egui::Slider::new(&mut charge, 0..=255).show_value(false))
            .on_hover_text(
                "cast charge: 85 ~= tap (1.0x), 255 = full hold (2.0x) -- scales speed AND \
                 damage; feeds both the scrub cast and Play",
            )
            .changed()
        {
            *new_charge = Some(charge as u8);
        }
        ui.label(egui::RichText::new(format!("{:.2}x", charge_mult(Some(scrub.charge)))).small());
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Vec3;
    use obelisk_bevy::assets::{
        Acquisition, CollisionShape, HitFilter, HitMode, VolumeMotion, WindowAnchor,
    };

    fn pd(windup: f32, active: f32, recovery: f32) -> PhaseDurations {
        PhaseDurations { windup, active, recovery }
    }

    fn window(spawn: WindowSpawn, active_duration: f32) -> CollisionWindow {
        CollisionWindow {
            id: "w".into(),
            spawn,
            anchor: WindowAnchor::Caster,
            anchor_offset: Vec3::ZERO,
            strikes: true,
            active_duration,
            shape: CollisionShape::Sphere { radius: 0.5 },
            motion: VolumeMotion::Static,
            motion_direction: Default::default(),
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::OncePerTarget,
            rehit_interval: None,
            emitter: None,
        }
    }

    fn timeline(phase_durations: PhaseDurations, windows: Vec<CollisionWindow>) -> CastTimeline {
        CastTimeline {
            skill_id: "x".into(),
            phase_durations,
            collision_windows: windows,
            acquisition: Acquisition::default(),
            vfx_cues: Default::default(),
            chain_radius: 6.0,
            chargeable: false,
            max_hold: 1.0,
            cues: Default::default(),
            charge_cues: Vec::new(),
        }
    }

    #[test]
    fn phase_spans_and_total_duration() {
        let d = pd(0.3, 0.1, 0.2);
        assert_eq!(phase_spans(&d), [(0.0, 0.3), (0.3, 0.4), (0.4, 0.6)]);
        assert_eq!(total_duration(&d), 0.6);
    }

    #[test]
    fn time_to_x_maps_and_clamps() {
        assert_eq!(time_to_x(0.0, 0.6, 10.0, 100.0), 10.0);
        assert_eq!(time_to_x(0.6, 0.6, 10.0, 100.0), 110.0);
        assert_eq!(time_to_x(0.3, 0.6, 0.0, 100.0), 50.0);
        assert_eq!(time_to_x(5.0, 0.0, 7.0, 100.0), 7.0);
    }

    #[test]
    fn window_span_offsets_from_its_phase() {
        let d = pd(0.3, 0.1, 0.2);
        assert_eq!(
            window_span(&d, &window(WindowSpawn::Scheduled { phase: WindowPhase::Active, offset: 0.0 }, 0.1)),
            Some((0.3, 0.4))
        );
        // 0.3 + 0.1 + 0.05 accumulates f32 rounding, so compare within epsilon.
        let (s, _) = window_span(
            &d,
            &window(WindowSpawn::Scheduled { phase: WindowPhase::Recovery, offset: 0.05 }, 0.1),
        )
        .unwrap();
        assert!((s - 0.45).abs() < 1e-5);
    }

    #[test]
    fn window_span_is_none_for_a_template_window() {
        let d = pd(0.3, 0.1, 0.2);
        assert_eq!(window_span(&d, &window(WindowSpawn::Template, 0.1)), None);
    }

    #[test]
    fn base_span_extends_to_the_latest_scheduled_windows_close() {
        let d = pd(0.3, 0.1, 0.2);
        let tl = timeline(d.clone(), vec![]);
        assert_eq!(base_span(&tl), 0.6, "no windows: phase total");

        let tl = timeline(
            d.clone(),
            vec![window(WindowSpawn::Scheduled { phase: WindowPhase::Active, offset: 0.0 }, 2.0)],
        );
        assert!(
            (base_span(&tl) - 2.3).abs() < 1e-6,
            "a long window extends the strip to its close"
        );
    }

    #[test]
    fn base_span_ignores_template_windows_own_span() {
        // A Template window contributes no fixed span of its own (v2: only an emitter
        // instantiates it) — base_span falls back to the phase total.
        let d = pd(0.3, 0.1, 0.2);
        let tl = timeline(d, vec![window(WindowSpawn::Template, 5.0)]);
        assert_eq!(base_span(&tl), 0.6);
    }

    // -----------------------------------------------------------------------
    // Task 11 review: the strip's headline feature — scrub PAST the base timeline to reveal a
    // triggered sub-cast — was permanently unreachable, because the click formula clamped its
    // target to the RENDERED range (`base.max(dynamic_end)`), and `dynamic_end` only grows when
    // `drive_scrub`'s seek loop runs at least one iteration past the current clock. At the far
    // edge, the old formula always requested exactly the current clock, so the loop ran zero
    // iterations forever. This test exercises the WIDGET'S OWN click->target math directly (not
    // a `ScrubSim.target` poke) and must fail red against the pre-fix clamp.
    // -----------------------------------------------------------------------

    #[test]
    fn far_edge_click_reaches_past_the_rendered_range() {
        let base = 0.6;
        let dynamic_end = base; // freshly restarted: nothing discovered past base yet
        let far = strip_click_to_target(1.0, base, dynamic_end);
        assert!(
            far > base,
            "a far-edge click/drag must be able to request a target PAST the base span so \
             `drive_scrub`'s seek loop actually runs and grows `dynamic_end` -- got {far} for \
             base {base}"
        );
    }
}

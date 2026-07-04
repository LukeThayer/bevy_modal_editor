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
use crate::skill::preview::{MarkerKind, Playhead, ScrubMarkers, ScrubMode, ScrubSim};

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

/// Draw the scrub strip for the currently open `entry`: the phase/window bars, the trailing
/// sub-cast region, event markers, the playhead, the (replay) control, and the charge slider.
/// See the module doc comment for the out-param write-back convention.
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
    let span = base.max(scrub.dynamic_end);

    let (rect, strip_resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), STRIP_H),
        egui::Sense::click_and_drag(),
    );
    if (strip_resp.clicked() || strip_resp.dragged())
        && let Some(pos) = strip_resp.interact_pointer_pos()
    {
        let t = ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0) * span;
        *new_target = Some(t);
    }
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, egui::Color32::from_rgb(24, 24, 28));

    // The trailing sub-cast region (past the base span) reads as a dim, visually distinct band
    // from the authored phases/windows to its left — see the module doc comment.
    if span > base {
        let x0 = time_to_x(base, span, rect.left(), rect.width());
        p.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(rect.right(), rect.bottom())),
            0.0,
            egui::Color32::from_rgb(45, 38, 30),
        );
    }

    for (i, (s, e)) in phase_spans(&tl.phase_durations).iter().enumerate() {
        let x0 = time_to_x(*s, span, rect.left(), rect.width());
        let x1 = time_to_x(*e, span, rect.left(), rect.width());
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
        let x0 = time_to_x(ws, span, rect.left(), rect.width());
        let x1 = time_to_x(we, span, rect.left(), rect.width());
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
        let x = time_to_x(marker.time, span, rect.left(), rect.width());
        p.line_segment(
            [egui::pos2(x, rect.bottom() - 8.0), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(2.0, marker_color(marker.kind)),
        );
    }
    if let Some(pos) = strip_resp.hover_pos() {
        const HOVER_PX: f32 = 4.0;
        if let Some(hovered) = markers.0.iter().min_by(|a, b| {
            let da = (time_to_x(a.time, span, rect.left(), rect.width()) - pos.x).abs();
            let db = (time_to_x(b.time, span, rect.left(), rect.width()) - pos.x).abs();
            da.total_cmp(&db)
        }) {
            let x = time_to_x(hovered.time, span, rect.left(), rect.width());
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
        let x = time_to_x(playhead.elapsed, span, rect.left(), rect.width());
        p.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(230, 70, 70)),
        );
    } else if scrub.mode != ScrubMode::Idle {
        // The scrub head shows the SIM's actual clock (the frozen truth), not the request.
        let x = time_to_x(scrub.clock, span, rect.left(), rect.width());
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
}

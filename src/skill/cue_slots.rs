//! Pure cue-slot enumeration for the Presentation region (Task 9).
//!
//! `cue_slots` walks a `CastTimeline` and produces the ordered list of cue keys the
//! Presentation region should render one row for — mirroring exactly the slot vocabulary and
//! legality table documented on `obelisk_bevy::assets::CastTimeline::cues`/`CueBinding` (itself
//! transcribed from the design doc's §3.2 "Cue table", normative for which `CueBinding` fields
//! are legal to author on each slot):
//!
//! | Slot | `attach` legal | `anim` legal |
//! |---|---|---|
//! | `on_cast` | no (world-anchored) | yes |
//! | `on_window_{id}` | yes | no |
//! | `on_end_{id}` | no (world-anchored) | no |
//! | `emit_{id}` | yes | no |
//! | `on_hit` | no (world-anchored) | no |
//!
//! Ordering (timeline order, matching the brief): `on_cast`; then, for every
//! `CollisionWindow` in `collision_windows` order (Scheduled AND Template alike — a Template
//! window's row is NOT dead weight, even though its first-ever spawn is always `emitted` (fires
//! `emit_{id}`, never `on_window_{id}`, per `CastTimeline::cues`'s doc comment): `end_hitboxes`'
//! chain-hop arm (Task 12, `timeline/advance.rs`) re-strikes the SAME window id via
//! `spawn_window_hitbox` with `ChainPayload { emitted: false, hop: hb.hop + 1, .. }` regardless
//! of that window's own `spawn` kind, so a chaining beam's hop 2+ through a Template window
//! fires `on_window_{id}` for real — the row is live authorable data on hop 2+, not merely a
//! hand-editing convenience; `on_end_{id}` DOES fire for every emitted instance too —
//! `vfx.rs::cue_on_end` fires on every `HitboxEnded` regardless of `emitted`), `on_window_{id}`
//! then `on_end_{id}`; then, walking the SAME window order again, each emitter-carrying window's
//! `emit_{target_id}` (deduped — two windows emitting the same `Template` id list that `emit_`
//! slot once); finally `on_hit`.

use std::collections::HashSet;

use obelisk_bevy::assets::CastTimeline;

/// One cue slot the Presentation region renders a row for. Pure data — `attach_legal`/
/// `anim_legal` are this module's only opinion; `panel::presentation` reads them to decide
/// which pickers to show for the row (see the module doc comment's legality table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueSlot {
    /// The `CastTimeline::cues` map key this slot binds (`"on_cast"`, `"on_window_bolt"`, ...).
    pub slot_id: String,
    /// Human-readable row label.
    pub label: String,
    pub attach_legal: bool,
    pub anim_legal: bool,
    /// Whether `CueAttach::Bone` is offered (caster-anchored slots — the effect can ride a
    /// named rig joint).
    pub bone_legal: bool,
}

fn on_cast_slot() -> CueSlot {
    CueSlot { slot_id: "on_cast".to_string(), label: "On Cast".to_string(), attach_legal: false, anim_legal: true, bone_legal: true }
}

fn on_window_slot(id: &str) -> CueSlot {
    CueSlot {
        slot_id: format!("on_window_{id}"),
        label: format!("On Window: {id}"),
        attach_legal: true,
        anim_legal: false,
        bone_legal: false,
    }
}

fn on_end_slot(id: &str) -> CueSlot {
    CueSlot {
        slot_id: format!("on_end_{id}"),
        label: format!("On End: {id}"),
        attach_legal: false,
        anim_legal: false,
        bone_legal: false,
    }
}

fn emit_slot(id: &str) -> CueSlot {
    CueSlot { slot_id: format!("emit_{id}"), label: format!("Emit: {id}"), attach_legal: true, anim_legal: false, bone_legal: false }
}

fn on_hit_slot() -> CueSlot {
    CueSlot { slot_id: "on_hit".to_string(), label: "On Hit".to_string(), attach_legal: false, anim_legal: false, bone_legal: false }
}

/// The ordered set of cue slots `timeline` offers for presentation binding — see the module
/// doc comment for ordering/legality. Pure: reads only `timeline.collision_windows`, never
/// `timeline.cues` itself (a slot is offered whether or not it's currently bound).
pub fn cue_slots(timeline: &CastTimeline) -> Vec<CueSlot> {
    let mut slots = vec![on_cast_slot()];

    for window in &timeline.collision_windows {
        slots.push(on_window_slot(&window.id));
        slots.push(on_end_slot(&window.id));
    }

    let mut seen_emit_targets = HashSet::new();
    for window in &timeline.collision_windows {
        if let Some(emitter) = &window.emitter
            && seen_emit_targets.insert(emitter.window.clone())
        {
            slots.push(emit_slot(&emitter.window));
        }
    }

    slots.push(on_hit_slot());
    slots
}

#[cfg(test)]
mod tests {
    use super::*;

    use obelisk_bevy::assets::{
        Acquisition, CollisionShape, CollisionWindow, Emitter, HitFilter, HitMode, PhaseDurations,
        VolumeMotion, WindowPhase, WindowSpawn,
    };

    fn blank_timeline() -> CastTimeline {
        CastTimeline {
            skill_id: "test".to_string(),
            phase_durations: PhaseDurations { windup: 0.0, active: 0.0, recovery: 0.0 },
            collision_windows: Vec::new(),
            acquisition: Acquisition::default(),
            vfx_cues: Default::default(),
            chain_radius: 6.0,
            chargeable: false,
            max_hold: 1.0,
            cues: Default::default(),
            charge_cues: Vec::new(),
        }
    }

    fn scheduled_window(id: &str, emitter: Option<Emitter>) -> CollisionWindow {
        CollisionWindow {
            id: id.to_string(),
            spawn: WindowSpawn::Scheduled { phase: WindowPhase::Active, offset: 0.0 },
            anchor: Default::default(),
            anchor_offset: Default::default(),
            strikes: true,
            active_duration: 1.0,
            shape: CollisionShape::Sphere { radius: 0.5 },
            motion: VolumeMotion::Static,
            motion_direction: Default::default(),
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::OncePerTarget,
            rehit_interval: None,
            emitter,
            paints: None,
        }
    }

    fn template_window(id: &str) -> CollisionWindow {
        CollisionWindow {
            id: id.to_string(),
            spawn: WindowSpawn::Template,
            anchor: Default::default(),
            anchor_offset: Default::default(),
            strikes: true,
            active_duration: 1.0,
            shape: CollisionShape::Sphere { radius: 0.3 },
            motion: VolumeMotion::Static,
            motion_direction: Default::default(),
            hit_filter: HitFilter::Enemies,
            hit_mode: HitMode::OncePerTarget,
            rehit_interval: None,
            emitter: None,
            paints: None,
        }
    }

    fn ids(slots: &[CueSlot]) -> Vec<&str> {
        slots.iter().map(|s| s.slot_id.as_str()).collect()
    }

    // --- fireball-pair-style: one scheduled window, no emitter ---

    #[test]
    fn fireball_pair_style_ordering() {
        let mut tl = blank_timeline();
        tl.collision_windows = vec![scheduled_window("bolt", None)];

        let slots = cue_slots(&tl);
        assert_eq!(ids(&slots), vec!["on_cast", "on_window_bolt", "on_end_bolt", "on_hit"]);
    }

    #[test]
    fn empty_timeline_is_just_cast_and_hit() {
        let tl = blank_timeline();
        let slots = cue_slots(&tl);
        assert_eq!(ids(&slots), vec!["on_cast", "on_hit"]);
    }

    // --- blizzard-style: emitter-carrying scheduled window + its Template target ---

    #[test]
    fn blizzard_style_emit_slot_present_after_window_pairs() {
        let mut tl = blank_timeline();
        tl.collision_windows = vec![
            scheduled_window("storm", Some(Emitter { rate: 5.0, jitter: 0.5, window: "shard".to_string() })),
            template_window("shard"),
        ];

        let slots = cue_slots(&tl);
        assert_eq!(
            ids(&slots),
            vec!["on_cast", "on_window_storm", "on_end_storm", "on_window_shard", "on_end_shard", "emit_shard", "on_hit"]
        );
    }

    #[test]
    fn emit_target_referenced_by_two_emitters_lists_once() {
        let mut tl = blank_timeline();
        tl.collision_windows = vec![
            scheduled_window("storm_a", Some(Emitter { rate: 5.0, jitter: 0.5, window: "shard".to_string() })),
            scheduled_window("storm_b", Some(Emitter { rate: 3.0, jitter: 0.2, window: "shard".to_string() })),
            template_window("shard"),
        ];

        let slots = cue_slots(&tl);
        let emit_count = slots.iter().filter(|s| s.slot_id == "emit_shard").count();
        assert_eq!(emit_count, 1, "{:?}", ids(&slots));
        // Still ordered after every window's on_window_/on_end_ pair, before on_hit.
        assert_eq!(
            ids(&slots),
            vec![
                "on_cast",
                "on_window_storm_a",
                "on_end_storm_a",
                "on_window_storm_b",
                "on_end_storm_b",
                "on_window_shard",
                "on_end_shard",
                "emit_shard",
                "on_hit",
            ]
        );
    }

    // --- legality flags ---

    #[test]
    fn on_cast_allows_anim_not_attach() {
        let tl = blank_timeline();
        let slots = cue_slots(&tl);
        let on_cast = slots.iter().find(|s| s.slot_id == "on_cast").unwrap();
        assert!(on_cast.anim_legal);
        assert!(!on_cast.attach_legal);
    }

    #[test]
    fn on_window_and_emit_allow_attach_not_anim() {
        let mut tl = blank_timeline();
        tl.collision_windows = vec![
            scheduled_window("storm", Some(Emitter { rate: 5.0, jitter: 0.5, window: "shard".to_string() })),
            template_window("shard"),
        ];
        let slots = cue_slots(&tl);

        let on_window = slots.iter().find(|s| s.slot_id == "on_window_storm").unwrap();
        assert!(on_window.attach_legal);
        assert!(!on_window.anim_legal);

        let emit = slots.iter().find(|s| s.slot_id == "emit_shard").unwrap();
        assert!(emit.attach_legal);
        assert!(!emit.anim_legal);
    }

    #[test]
    fn on_end_and_on_hit_allow_neither() {
        let mut tl = blank_timeline();
        tl.collision_windows = vec![scheduled_window("bolt", None)];
        let slots = cue_slots(&tl);

        let on_end = slots.iter().find(|s| s.slot_id == "on_end_bolt").unwrap();
        assert!(!on_end.attach_legal);
        assert!(!on_end.anim_legal);

        let on_hit = slots.iter().find(|s| s.slot_id == "on_hit").unwrap();
        assert!(!on_hit.attach_legal);
        assert!(!on_hit.anim_legal);
    }

    #[test]
    fn labels_are_human_readable() {
        let mut tl = blank_timeline();
        tl.collision_windows = vec![scheduled_window("bolt", None)];
        let slots = cue_slots(&tl);
        assert_eq!(slots[0].label, "On Cast");
        assert_eq!(slots[1].label, "On Window: bolt");
        assert_eq!(slots[2].label, "On End: bolt");
        assert_eq!(slots[3].label, "On Hit");
    }
}

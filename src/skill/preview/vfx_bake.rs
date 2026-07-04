//! CPU-bake a cue's `ParamSource::Charge` binding into a `bevy_vfx` [`VfxSystem`] before insert
//! (Task 10 — ported/adapted from `arena_editor::vfx_bind`, obelisk-arena `f6472e4`) — **the
//! "VfxParam seam"** the task brief refers to.
//!
//! `apply_modulated_param` maps a named authoring param onto the first emitter's `bevy_vfx`
//! module stack (`"scale"` → `SetSize`, `"emission"` → `SpawnModule::Rate`, `"color"` → scale
//! `SetColor` RGB) — ported VERBATIM from v1 (`bevy_vfx::data`'s shape is unchanged between the
//! two editors, confirmed by direct comparison). Baking happens on the CPU before the
//! `VfxSystem` is inserted, same as v1.
//!
//! **Relationship to `VfxSystem::params` (schema note).** `VfxSystem` also carries a `params:
//! Vec<VfxParam>` list (`crates/bevy_vfx/src/data.rs`) — Task 9's Presentation region reads it
//! (`discoverable_params`) to suggest legal charge-param NAMES in the picker. That list is
//! metadata only; nothing in `bevy_vfx`'s runtime consumes it to affect a running particle system
//! (its own doc comment: "future: bindable from game code"). This module is what makes naming a
//! param real: `apply_modulated_param`'s hardcoded name convention (`scale`/`emission`/`color`) is
//! the ACTUAL seam between an authored `CueParam.param` string and a visible effect, independent
//! of whatever names happen to appear in `VfxSystem::params`. A param named something else in the
//! picker (or not discoverable at all) is a documented no-op here — see the Task 9 review ticket
//! this task's report answers ("cue effect-name resolution order") and the port report's
//! concerns section for the analogous param-name caveat.
//!
//! **v1 → v2 delta:** v1's `bake_bindings` resolved each binding through `arena_skills::
//! resolve_binding` (a `min`/`max` lerp `VfxParamBinding` carried). v2's `obelisk_bevy::assets::
//! CueParam` has no `min`/`max` — just `{ param: String, source: ParamSource }` — so there is
//! nothing to lerp; the raw charge fraction (`ev.charge` normalized 0..1, see
//! `cosmetics::charge_fraction`) is applied directly. This is a straightforward simplification of
//! the same mechanism, not a behavior gap: a v1 author who left `min: 0.0, max: 1.0` (the
//! identity range) got byte-identical behavior to this direct application.

use bevy::color::LinearRgba;
use bevy_vfx::data::{ColorSource, EmitterDef, InitModule, ScalarRange, SpawnModule, VfxSystem};

fn set_size(em: &mut EmitterDef, v: f32) {
    for m in em.init.iter_mut() {
        if let InitModule::SetSize(r) = m {
            *r = ScalarRange::Constant(v);
            return;
        }
    }
    em.init.push(InitModule::SetSize(ScalarRange::Constant(v)));
}

fn scale_color(em: &mut EmitterDef, mult: f32) {
    for m in em.init.iter_mut() {
        if let InitModule::SetColor(ColorSource::Constant(c)) = m {
            *c = LinearRgba::rgb(c.red * mult, c.green * mult, c.blue * mult);
            return;
        }
    }
    em.init.push(InitModule::SetColor(ColorSource::Constant(LinearRgba::rgb(
        mult, mult, mult,
    ))));
}

/// Bake a single resolved `value` into the first emitter under the named `param`. Unknown params
/// and empty-emitter systems are no-ops.
pub fn apply_modulated_param(system: &mut VfxSystem, param: &str, value: f32) {
    let Some(em) = system.emitters.first_mut() else {
        return;
    };
    match param {
        "scale" => set_size(em, value),
        "emission" => em.spawn = SpawnModule::Rate(value),
        "color" => scale_color(em, value),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_scale_inserts_or_replaces_set_size() {
        let mut system = VfxSystem::default();
        apply_modulated_param(&mut system, "scale", 0.7);
        let em = system.emitters.first().unwrap();
        let sizes: Vec<f32> = em
            .init
            .iter()
            .filter_map(|m| match m {
                InitModule::SetSize(ScalarRange::Constant(v)) => Some(*v),
                _ => None,
            })
            .collect();
        assert_eq!(sizes, vec![0.7]);
    }

    #[test]
    fn apply_emission_sets_spawn_rate() {
        let mut system = VfxSystem::default();
        apply_modulated_param(&mut system, "emission", 120.0);
        match system.emitters.first().unwrap().spawn {
            SpawnModule::Rate(r) => assert_eq!(r, 120.0),
            _ => panic!("expected SpawnModule::Rate"),
        }
    }

    #[test]
    fn apply_color_scales_existing_constant() {
        let mut system = VfxSystem::default();
        system.emitters[0].init.push(InitModule::SetColor(ColorSource::Constant(LinearRgba::rgb(
            1.0, 1.0, 1.0,
        ))));
        apply_modulated_param(&mut system, "color", 0.5);
        let found = system.emitters[0].init.iter().find_map(|m| match m {
            InitModule::SetColor(ColorSource::Constant(c)) => Some(*c),
            _ => None,
        });
        assert_eq!(found, Some(LinearRgba::rgb(0.5, 0.5, 0.5)));
    }

    #[test]
    fn unknown_param_is_a_no_op() {
        let mut system = VfxSystem::default();
        let before = system.clone();
        apply_modulated_param(&mut system, "nonsense", 1.0);
        assert_eq!(system, before);
    }
}

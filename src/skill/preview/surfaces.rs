//! Preview surface-patch visuals (Task 3 — the editor counterpart of obelisk-arena's
//! `crates/arena_game/src/client/surfaces.rs`, adapted to the preview stage).
//!
//! The preview runs the real obelisk surfaces sim (Task 2), so a painted `SurfacePatch` is a LOCAL
//! sim entity that already carries its own `Transform` — unlike the arena's REPLICATED patch (which
//! needs a `Position`→`Transform` mirror), this hangs visuals directly under it, no pose mirror.
//!
//! Each freshly-painted patch gets a [`SurfacePatchVisual`]-marked CHILD carrying a tinted
//! [`ForwardDecal`] (the projected splat — tint/texture from the `SurfaceRegistry` `[visuals]`
//! block, Option-safe defaults) plus, when the surface authors one, a looping vfx child (the
//! `VfxLibrary` preset resolved through cosmetics' shared `resolve_vfx_effect`). Both children
//! despawn WITH the patch: obelisk patch despawns are plain recursive `despawn`
//! (decay/consume/evict/reset), so neither child carries its own lifetime.
//!
//! The decal MATERIAL is the depth-TESTED fork ([`decal_material::DepthTestedDecalMaterial`]) — a
//! near-verbatim clone of bevy's `ForwardDecalMaterial` minus the `depth_compare = Always` override,
//! so the frost/scorch ground no longer draws THROUGH the preview dummy/caster standing in it (see
//! that module's docs). The `ForwardDecal` MARKER is KEPT: its on-add hook is material-agnostic (it
//! only sets the shared quad mesh), so it drives the forked material's mesh unchanged.
//!
//! HEADLESS SAFETY (why this diverges from the arena's single-bundle spawn): `ForwardDecal` needs
//! `PbrPlugin`'s render infrastructure — the private `ForwardDecalMesh` resource its on-add hook
//! reads (inserted by `ForwardDecalPlugin`) and the `Assets<DepthTestedDecalMaterial>` store
//! `PreviewSurfacesPlugin`'s `MaterialPlugin` inserts. The editor's own `tests/skill_preview.rs`
//! harness runs the surfaces sim on a `MinimalPlugins` app with NEITHER, so the [`SurfacePatchVisual`]
//! marker child is spawned UNCONDITIONALLY (the render-independent proof the attach ran + the
//! teardown handle) and the `ForwardDecal` + material are attached to it only where the material
//! store exists (the real windowed editor). See the module's own headless test for which assertion
//! ships.

use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};
// `ForwardDecal` is kept for its MATERIAL-AGNOSTIC quad-mesh on-add hook (bevy_pbr 0.18.0
// `decal/forward.rs::forward_decal_set_mesh` only sets the shared rotated-`Rectangle`
// `ForwardDecalMesh` on the entity — it never touches the material type), so it drives the mesh for
// our forked material unchanged. The MATERIAL itself is the depth-tested fork (`decal_material.rs`).
use bevy::pbr::decal::ForwardDecal;
use bevy::prelude::*;

use bevy_vfx::VfxLibrary;
use obelisk_bevy::surfaces::{SurfacePatch, SurfaceRegistry};

use super::decal_material::{DepthTestedDecalExt, DepthTestedDecalMaterial};
use super::stage::PreviewStageFloor;
use crate::editor::EditorMode;

/// Marks a spawned preview surface-patch visual CHILD (the decal carrier). Queried by tests to
/// prove the attach ran WITHOUT depending on `ForwardDecal` (which is render-resource-dependent and
/// not constructible on the headless test harness — see the module doc comment). Children despawn
/// with the patch (recursive `despawn`), so this carries no lifetime of its own.
#[derive(Component)]
pub struct SurfacePatchVisual;

/// Session-scoped staged ground state (spec §9 / D12's stage-setup direction): pre-painted patches
/// the designer placed via the palette, re-applied on EVERY stage reset (Play, editor Reset, scrub
/// restart — all re-sim from t=0) so surface-GATED casts (the frost-spire pattern) are testable and
/// the scrubber stays honest. Re-application lives at the END of
/// [`super::stage::PreviewStageReset::reset_stage`], AFTER the Task-2 clear — one funnel covers all
/// three replay entry points. Never serialized — pure session state.
#[derive(Resource, Default)]
pub struct StagedPaints(pub Vec<StagedPaint>);

/// One staged pre-paint: a `surface` type to paint at a world `position` (the painter/owner is the
/// preview caster, resolved at re-apply time — see `reset_stage`).
#[derive(Clone, Debug)]
pub struct StagedPaint {
    pub surface: String,
    pub position: Vec3,
}

/// Push a staged pre-paint, DEDUPED on `(surface, position)`. The palette can re-stage the same
/// spot (a double-select, or re-picking the row) but the durable [`StagedPaints`] list must not
/// grow — the caller still fires the instant paint either way (obelisk's own paint dedups the live
/// patch). Extracted from the palette's egui handler so the guard is unit-testable in isolation.
/// Returns `true` if a new entry was pushed, `false` if an identical one already existed.
pub fn push_staged_dedup(staged: &mut StagedPaints, surface: &str, pos: Vec3) -> bool {
    if staged.0.iter().any(|s| s.surface == surface && s.position == pos) {
        return false;
    }
    staged.0.push(StagedPaint {
        surface: surface.to_string(),
        position: pos,
    });
    true
}

/// The default decal tint (white, 0.8 alpha) when a surface authors no `[visuals].color`.
const DEFAULT_DECAL_COLOR: Color = Color::srgba(1.0, 1.0, 1.0, 0.8);
/// The default decal texture key when a surface authors no `[visuals].decal`. `asset_server.load`
/// is LAZY, so a host whose asset root lacks this file simply renders an untextured tint rather
/// than panicking (the editor's asset root varies by host — arena_editor points it at the arena
/// workspace's `assets/`; a given test root need not carry this file).
const DEFAULT_DECAL_TEXTURE: &str = "textures/decal_splat.png";

/// Attach the decal (+ optional looping vfx) to every freshly-painted patch. `Added<SurfacePatch>`
/// fires the frame the sim spawns the patch; the patch already carries its `Transform`, so the
/// children hang directly under it (no `Position`→`Transform` mirror — unlike the replicated arena
/// patch). Children despawn WITH the patch (obelisk despawns are recursive), so neither carries a
/// lifetime.
///
/// Every input beyond the patch query is Option/Option-guarded so this is a clean no-op on the
/// headless test harness: a missing `SurfaceRegistry` falls back to the neutral splat, a missing
/// `VfxLibrary` skips the vfx child, and — the load-bearing guard — a missing
/// `Assets<DepthTestedDecalMaterial>` store spawns the bare [`SurfacePatchVisual`] marker WITHOUT
/// the render-resource-dependent `ForwardDecal` (whose on-add hook would panic there; see the module
/// doc comment).
pub fn attach_patch_visuals(
    q: Query<(Entity, &SurfacePatch, &Transform), Added<SurfacePatch>>,
    registry: Option<Res<SurfaceRegistry>>,
    asset_server: Res<AssetServer>,
    mut decal_materials: Option<ResMut<Assets<DepthTestedDecalMaterial>>>,
    // Per-surface-type decal material cache (see the attach block for the static-registry caveat).
    mut material_cache: Local<std::collections::HashMap<String, Handle<DepthTestedDecalMaterial>>>,
    vfx: Option<Res<VfxLibrary>>,
    // Ground-snap the decals (see the attach block): a downward ray onto the STATIC stage floor.
    spatial: SpatialQuery,
    floor_query: Query<(), With<PreviewStageFloor>>,
    mut commands: Commands,
) {
    for (e, patch, transform) in &q {
        let visuals = registry
            .as_ref()
            .and_then(|r| r.0.get(&patch.surface))
            .and_then(|s| s.visuals.clone())
            .unwrap_or_default();

        // The sim patch has a `Transform` but no `Visibility` — give it one so its visual children
        // inherit visibility (mirrors the arena's own patch-parent `Visibility` insert).
        commands.entity(e).insert(Visibility::default());

        // Ground-snap the decal (+ vfx) to the floor. bevy 0.18 `ForwardDecal` is a FLAT +Y quad:
        // scale.y is INERT (there is no Y extent to grow — the old `y_span` scale did nothing) and
        // `depth_fade_factor` bounds the projection (1.0 => ~1 m). The material is now the DEPTH-TESTED
        // fork (`decal_material.rs`), so unlike stock `ForwardDecal` the quad IS occluded by nearer
        // opaque geometry (the dummy/caster no longer show frost through them) — which makes flush
        // ground-snapping doubly important: an ELEVATED quad would float, parallax-smear at grazing
        // angles, AND now z-fight / vanish against the floor. Snapping the VISUAL flush to the ground
        // keeps only sub-1m receivers (the floor, feet) catching it. The SIM patch keeps its authored Y
        // (gameplay is `SURFACE_Y_TOLERANCE`-based); this offset lives on the render child alone.
        let patch_pos = transform.translation;
        let origin = patch_pos + Vec3::Y * 2.0;
        let ground_y = spatial
            .cast_ray_predicate(
                origin,
                Dir3::NEG_Y,
                50.0,
                true,
                &SpatialQueryFilter::default(),
                // STATIC stage floor ONLY — never a combatant capsule the patch happens to sit
                // under (predicate `true` = a hit to consider, `false` = skip and keep travelling).
                &|entity| floor_query.contains(entity),
            )
            .map(|hit| origin.y - hit.distance)
            // Flat-stage fallback: the floor's top face is world Y = 0 (see `spawn_arena_floor`).
            .unwrap_or(0.0);
        // Child-LOCAL Y that lands the child's WORLD Y on the ground + a 1 cm bias off the floor.
        // With the depth-tested fork, that +0.01 m ALSO doubles as the depth-test-winning z-offset:
        // the decal sits just in front of the floor plane so the standard depth comparison lets it
        // draw OVER the floor (no z-fight) while still being occluded by taller geometry above it.
        let visual_y = ground_y - patch_pos.y + 0.01;

        // The decal child: ALWAYS `SurfacePatchVisual`-marked (render-independent). `ForwardDecal`
        // + its material are attached only where the material store exists (the windowed editor) —
        // the headless harness has none, and inserting `ForwardDecal` there would panic in its
        // on-add hook (missing `ForwardDecalMesh`).
        let child = commands
            .spawn((
                Name::new(format!("SurfaceDecal({})", patch.surface)),
                SurfacePatchVisual,
                // Ground-snapped (child-local `visual_y`), XZ = diameter. scale.y is 1.0: the quad
                // has NO Y extent to scale (see the ground-snap note above — `y_span` is gone for
                // good; a raw scale never reached the floor, it only stretched a flat quad).
                Transform::from_xyz(0.0, visual_y, 0.0)
                    .with_scale(Vec3::new(patch.radius * 2.0, 1.0, patch.radius * 2.0)),
                Visibility::default(),
            ))
            .id();
        if let Some(materials) = decal_materials.as_mut() {
            let color = visuals
                .color
                .map(|c| Color::srgba(c[0], c[1], c[2], c[3]))
                .unwrap_or(DEFAULT_DECAL_COLOR);
            let texture = visuals
                .decal
                .as_deref()
                .unwrap_or(DEFAULT_DECAL_TEXTURE)
                .to_string();
            // One decal material per surface TYPE, not per patch: a surface's registry visuals are
            // STATIC at runtime, so every patch of a given surface shares one handle.
            // NOTE: a future hot-reload of the surface TOMLs would mutate a type's visuals and MUST
            // invalidate this cache (drop the changed surface's entry) — no reload path exists today.
            let material = material_cache
                .entry(patch.surface.clone())
                .or_insert_with(|| {
                    materials.add(DepthTestedDecalMaterial {
                        base: StandardMaterial {
                            base_color: color,
                            base_color_texture: Some(asset_server.load(&texture)),
                            alpha_mode: AlphaMode::Blend,
                            perceptual_roughness: 1.0,
                            ..default()
                        },
                        // The depth-tested fork: identical to bevy's `ForwardDecalMaterialExt` but
                        // the pipeline keeps its STANDARD depth test, so nearer opaque geometry (the
                        // preview dummy/caster) occludes the frost instead of the decal drawing over it.
                        extension: DepthTestedDecalExt {
                            depth_fade_factor: 1.0,
                        },
                    })
                })
                .clone();
            commands
                .entity(child)
                .insert((ForwardDecal, MeshMaterial3d(material)));
        }
        commands.entity(e).add_child(child);

        // Optional looping vfx (e.g. a burning surface's embers): reuse cosmetics' shared
        // `resolve_vfx_effect` to turn the authored name into a live `VfxSystem`, then parent it
        // under the patch with NO lifetime component — it loops for the patch's life and despawns
        // with the parent. A surface authors no `CueParam`s, so pass `&[]`/`0.0`.
        if let (Some(vfx_name), Some(vfx_lib)) = (visuals.vfx.as_deref(), vfx.as_ref()) {
            if let Some(system) = super::cosmetics::resolve_vfx_effect(vfx_lib, vfx_name, &[], 0.0) {
                let fx = commands
                    .spawn((
                        Name::new(format!("SurfaceVfx({})", patch.surface)),
                        // Same ground-snap as the decal so embers sit on the floor, not at the
                        // patch's authored (elevated) Y.
                        Transform::from_xyz(0.0, visual_y, 0.0),
                        Visibility::default(),
                        system,
                    ))
                    .id();
                commands.entity(e).add_child(fx);
            }
        }
    }
}

/// Wires the preview surface-patch visuals: the windowed-only `MaterialPlugin` for the depth-tested
/// decal fork ([`super::decal_material::DepthTestedDecalMaterial`]) + the attach system, scoped to
/// `EditorMode::Skill` (patches only ever exist on the Skill-mode stage).
///
/// This plugin is the editor's WINDOWED-ONLY composition point for preview decals — it is added only
/// via `SkillPreviewPlugin` (the real editor); the headless `tests/skill_preview.rs` harness adds
/// `attach_patch_visuals` DIRECTLY and never adds this plugin, so it never registers the
/// `DepthTestedDecalMaterial` store and the decal stays a clean no-op there (mirroring the arena's
/// `app_windowed.rs`-only `MaterialPlugin` registration). Unlike the old stock `ForwardDecalMaterial`
/// add, this registration is LOAD-BEARING: `PbrPlugin`/`ForwardDecalPlugin` supply the `ForwardDecal`
/// quad-mesh hook + the `bevy_pbr::decal::forward` shader library the fork reuses, but NOT this custom
/// material type — so the guard here is pure idempotency, not a defensive no-op.
pub struct PreviewSurfacesPlugin;

impl Plugin for PreviewSurfacesPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<MaterialPlugin<DepthTestedDecalMaterial>>() {
            app.add_plugins(MaterialPlugin::<DepthTestedDecalMaterial>::default());
        }
        app.add_systems(Update, attach_patch_visuals.run_if(in_state(EditorMode::Skill)));
    }
}

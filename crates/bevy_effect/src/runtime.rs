//! Effect playback runtime: trigger evaluation, action execution, tweens,
//! and the collision feed for `OnCollision` triggers.

use std::collections::HashMap;

use avian3d::prelude::*;
use bevy::light::ClusteredDecal;
use bevy::prelude::*;

use crate::data::*;
use crate::gltf::{GltfSource, GltfSourcePlugin};
use crate::primitive::PrimitiveShape;
use bevy_vfx::VfxLibrary;

/// Radius of the small sphere collider attached to helper entities spawned by
/// effects (particles, effect containers) so editors can pick them via
/// raycast. Mirrors the editor's light-collider radius.
const HELPER_COLLIDER_RADIUS: f32 = 0.5;

/// Maximum `SpawnEffect` nesting depth. A chain of child effects spawned via
/// `EffectAction::SpawnEffect` may be at most this many hops deep; spawns past
/// the cap are dropped with a `warn!`.
///
/// NOTE: this guard is NEW behavior added during the extraction into
/// `bevy_effect` (a self-spawning preset previously spawned unboundedly, one
/// level per frame). It is deliberately EXCLUDED from the Task-2
/// characterization contract — the characterization suite never nests deeper
/// than one level. See `tests::spawn_effect_recursion_stops_at_depth_cap`.
pub const MAX_SPAWN_EFFECT_DEPTH: u8 = 8;

/// Library of named effect presets.
#[derive(Resource, Default)]
pub struct EffectLibrary {
    pub effects: HashMap<String, EffectMarker>,
}

/// Core effect runtime plugin: registers the reflected effect types, the
/// [`EffectLibrary`] resource, and the playback systems.
///
/// Does NOT do any disk I/O — call [`crate::load_effects_from_dir`] (or your
/// own loader) to populate the library, and layer auto-save on top if desired
/// (as `bevy_modal_editor` does).
pub struct EffectPlugin;

impl Plugin for EffectPlugin {
    fn build(&self, app: &mut App) {
        // `EffectAction::SpawnGltf` inserts `GltfSource`; make sure its
        // materializing systems are present without double-adding when the
        // host app already installed them.
        if !app.is_plugin_added::<GltfSourcePlugin>() {
            app.add_plugins(GltfSourcePlugin);
        }

        app.register_type::<EffectMarker>()
            .register_type::<EffectStep>()
            .register_type::<EffectTrigger>()
            .register_type::<EffectAction>()
            .register_type::<RigidBodyKind>()
            .register_type::<SpawnLocation>()
            .register_type::<TweenProperty>()
            .register_type::<EasingType>()
            .register_type::<PrimitiveShape>()
            .init_resource::<EffectLibrary>()
            .add_systems(
                Update,
                (
                    rebuild_effect_playback.run_if(any_with_component::<EffectMarker>),
                    detect_effect_collisions
                        .before(advance_effects)
                        .run_if(any_with_component::<EffectPlayback>),
                    advance_effects.run_if(any_with_component::<EffectPlayback>),
                    advance_tweens
                        .after(advance_effects)
                        .run_if(any_with_component::<EffectPlayback>),
                ),
            );
    }
}

// ---------------------------------------------------------------------------
// Playback rebuild
// ---------------------------------------------------------------------------

/// Insert `EffectPlayback` on any `EffectMarker` entity that doesn't have one.
fn rebuild_effect_playback(
    mut commands: Commands,
    query: Query<Entity, (With<EffectMarker>, Without<EffectPlayback>)>,
) {
    for entity in &query {
        commands.entity(entity).insert(EffectPlayback::default());
    }
}

// ---------------------------------------------------------------------------
// Collision detection for effect children
// ---------------------------------------------------------------------------

/// Check `Collisions` for entities spawned by effects and populate
/// `EffectPlayback::collision_tags` so `OnCollision` triggers can fire.
fn detect_effect_collisions(
    collisions: Collisions,
    effect_children: Query<(Entity, &EffectChild)>,
    mut effects: Query<&mut EffectPlayback>,
) {
    for (child_entity, child) in &effect_children {
        // Check if this child is currently colliding with anything
        let mut hit_point = None;
        for contact_pair in collisions.collisions_with(child_entity) {
            if !contact_pair.is_touching() {
                continue;
            }
            // Grab the first contact point as the collision location
            if hit_point.is_none() {
                for manifold in &contact_pair.manifolds {
                    if let Some(cp) = manifold.points.first() {
                        hit_point = Some(cp.point);
                        break;
                    }
                }
            }
            break;
        }

        if hit_point.is_some() {
            if let Ok(mut playback) = effects.get_mut(child.effect_entity) {
                if playback.state == PlaybackState::Playing {
                    playback.collision_tags.insert(child.tag.clone());
                    if let Some(pt) = hit_point {
                        playback.last_collision_point = Some(pt);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Spawn location helper
// ---------------------------------------------------------------------------

fn resolve_spawn_location(
    at: &SpawnLocation,
    effect_transform: &GlobalTransform,
    playback: &EffectPlayback,
) -> Vec3 {
    match at {
        SpawnLocation::Offset(offset) => effect_transform.translation() + *offset,
        SpawnLocation::CollisionPoint => playback
            .last_collision_point
            .unwrap_or_else(|| effect_transform.translation()),
    }
}

// ---------------------------------------------------------------------------
// Playback advance
// ---------------------------------------------------------------------------

/// Main playback tick: advance time, check triggers, execute actions.
pub(crate) fn advance_effects(
    mut commands: Commands,
    time: Res<Time>,
    mut effects: Query<(Entity, &EffectMarker, &mut EffectPlayback, &GlobalTransform)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    vfx_library: Res<VfxLibrary>,
    effect_library: Res<EffectLibrary>,
    asset_server: Res<AssetServer>,
) {
    for (effect_entity, marker, mut playback, effect_transform) in &mut effects {
        if playback.state != PlaybackState::Playing {
            continue;
        }

        let dt = time.delta_secs();
        playback.elapsed += dt;

        // Collect events emitted this frame for cross-step triggering
        let mut new_events: Vec<String> = Vec::new();

        for (step_idx, step) in marker.steps.iter().enumerate() {
            // Repeatable triggers can fire multiple times
            let is_repeatable = matches!(
                &step.trigger,
                EffectTrigger::RepeatingInterval { .. } | EffectTrigger::AfterIdleTimeout { .. }
            );
            if !is_repeatable && playback.fired_steps.contains(&step_idx) {
                continue;
            }

            let should_fire = match &step.trigger {
                EffectTrigger::AtTime(t) => playback.elapsed >= *t,
                EffectTrigger::OnCollision { tag } => playback.collision_tags.contains(tag),
                EffectTrigger::OnEffectEvent(name) => playback.pending_events.contains(name),
                EffectTrigger::AfterRule { source_rule, delay } => {
                    playback
                        .rule_fire_times
                        .get(source_rule)
                        .map(|t| playback.elapsed >= t + delay)
                        .unwrap_or(false)
                }
                EffectTrigger::RepeatingInterval { interval, max_count } => {
                    let count = playback.repeat_counts.get(&step.name).copied().unwrap_or(0);
                    let within_max = max_count.map_or(true, |max| count < max);
                    within_max && playback.elapsed >= (count + 1) as f32 * interval
                }
                EffectTrigger::OnSpawn => {
                    !playback.fired_steps.contains(&step_idx) && playback.elapsed < dt * 2.0
                }
                EffectTrigger::AfterIdleTimeout { timeout } => {
                    playback.last_fire_time > 0.0
                        && playback.elapsed - playback.last_fire_time >= *timeout
                }
            };

            if !should_fire {
                continue;
            }

            playback.fired_steps.insert(step_idx);

            // Record fire time for rule chaining
            let current_elapsed = playback.elapsed;
            playback
                .rule_fire_times
                .insert(step.name.clone(), current_elapsed);
            playback.last_fire_time = current_elapsed;

            // Increment repeat count for repeating triggers
            if is_repeatable {
                *playback
                    .repeat_counts
                    .entry(step.name.clone())
                    .or_insert(0) += 1;
            }

            for action in &step.actions {
                execute_action(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    &vfx_library,
                    &effect_library,
                    &asset_server,
                    effect_entity,
                    effect_transform,
                    &mut playback,
                    action,
                    &mut new_events,
                );
            }
        }

        // Merge new events so subsequent steps can see them next frame
        playback.pending_events.extend(new_events);
    }

    // Clear pending events and collision tags at end of frame (they only live one tick)
    for (_, _, mut playback, _) in &mut effects {
        if playback.state == PlaybackState::Playing {
            playback.pending_events.clear();
            playback.collision_tags.clear();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_action(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    vfx_library: &VfxLibrary,
    effect_library: &EffectLibrary,
    asset_server: &AssetServer,
    effect_entity: Entity,
    effect_transform: &GlobalTransform,
    playback: &mut EffectPlayback,
    action: &EffectAction,
    new_events: &mut Vec<String>,
) {
    match action {
        EffectAction::SpawnPrimitive {
            tag,
            shape,
            offset,
            material: _,
            rigid_body,
        } => {
            let world_pos = effect_transform.translation() + *offset;
            let mesh_handle = meshes.add(shape.create_mesh());
            let mat_handle = materials.add(shape.create_material());
            let collider = shape.create_collider();

            let mut entity_cmds = commands.spawn((
                EffectChild {
                    effect_entity,
                    tag: tag.clone(),
                },
                Mesh3d(mesh_handle),
                MeshMaterial3d(mat_handle),
                Transform::from_translation(world_pos),
                collider,
            ));

            if let Some(rb_kind) = rigid_body {
                entity_cmds.insert(rb_kind.to_rigid_body());
            }

            let child = entity_cmds.id();
            playback.spawned.insert(tag.clone(), child);
        }
        EffectAction::SpawnParticle { tag, preset, at } => {
            let pos = resolve_spawn_location(at, effect_transform, playback);

            let system = vfx_library
                .effects
                .get(preset)
                .cloned()
                .unwrap_or_default();

            let child = commands
                .spawn((
                    EffectChild {
                        effect_entity,
                        tag: tag.clone(),
                    },
                    system,
                    Transform::from_translation(pos),
                    Visibility::default(),
                    Collider::sphere(HELPER_COLLIDER_RADIUS),
                ))
                .id();

            playback.spawned.insert(tag.clone(), child);
        }
        EffectAction::SetVelocity { tag, velocity } => {
            if let Some(&child_entity) = playback.spawned.get(tag) {
                commands
                    .entity(child_entity)
                    .insert(LinearVelocity(*velocity));
            }
        }
        EffectAction::ApplyImpulse { tag, impulse } => {
            if let Some(&child_entity) = playback.spawned.get(tag) {
                commands
                    .entity(child_entity)
                    .insert(LinearVelocity(*impulse));
            }
        }
        EffectAction::Despawn { tag } => {
            if let Some(child_entity) = playback.spawned.remove(tag) {
                commands.entity(child_entity).despawn();
            }
        }
        EffectAction::EmitEvent(name) => {
            new_events.push(name.clone());
        }
        EffectAction::SetGravity { tag, enabled } => {
            if let Some(&child_entity) = playback.spawned.get(tag) {
                if *enabled {
                    commands
                        .entity(child_entity)
                        .insert(GravityScale(1.0));
                } else {
                    commands
                        .entity(child_entity)
                        .insert(GravityScale(0.0));
                }
            }
        }
        EffectAction::SpawnGltf {
            tag,
            path,
            at,
            scale,
            rigid_body,
        } => {
            let pos = resolve_spawn_location(at, effect_transform, playback);

            let mut entity_cmds = commands.spawn((
                EffectChild {
                    effect_entity,
                    tag: tag.clone(),
                },
                GltfSource {
                    path: path.clone(),
                    scene_index: 0,
                },
                Transform::from_translation(pos).with_scale(*scale),
            ));

            if let Some(rb_kind) = rigid_body {
                entity_cmds.insert(rb_kind.to_rigid_body());
            }

            let child = entity_cmds.id();
            playback.spawned.insert(tag.clone(), child);
        }
        EffectAction::SpawnDecal {
            tag,
            texture_path,
            at,
            scale,
        } => {
            let pos = resolve_spawn_location(at, effect_transform, playback);
            let texture = if texture_path.is_empty() {
                None
            } else {
                Some(asset_server.load(texture_path.clone()))
            };

            let child = commands
                .spawn((
                    EffectChild {
                        effect_entity,
                        tag: tag.clone(),
                    },
                    ClusteredDecal {
                        base_color_texture: texture,
                        ..default()
                    },
                    Transform::from_translation(pos).with_scale(*scale),
                ))
                .id();
            playback.spawned.insert(tag.clone(), child);
        }
        EffectAction::SpawnEffect {
            tag,
            preset,
            at,
            inherit_velocity,
        } => {
            // Recursion depth guard — see `MAX_SPAWN_EFFECT_DEPTH` for why
            // this is excluded from the characterization contract.
            if playback.spawn_depth >= MAX_SPAWN_EFFECT_DEPTH {
                warn!(
                    "SpawnEffect: recursion depth cap ({}) reached — dropping spawn of preset '{}'",
                    MAX_SPAWN_EFFECT_DEPTH, preset
                );
                return;
            }

            let pos = resolve_spawn_location(at, effect_transform, playback);

            if let Some(effect_marker) = effect_library.effects.get(preset).cloned() {
                let mut entity_cmds = commands.spawn((
                    EffectChild {
                        effect_entity,
                        tag: tag.clone(),
                    },
                    effect_marker,
                    EffectPlayback {
                        state: PlaybackState::Playing,
                        spawn_depth: playback.spawn_depth + 1,
                        ..default()
                    },
                    Transform::from_translation(pos),
                    Visibility::default(),
                ));

                if *inherit_velocity {
                    // Copy parent's velocity if it has one — deferred, will be
                    // picked up if the parent had LinearVelocity. For now we
                    // just mark it; a more complete implementation would query
                    // the parent's velocity.
                    entity_cmds.insert(LinearVelocity::ZERO);
                }

                let child = entity_cmds.id();
                playback.spawned.insert(tag.clone(), child);
            } else {
                warn!("SpawnEffect: preset '{}' not found in effect library", preset);
            }
        }
        EffectAction::InsertComponent {
            target_tag,
            component_type,
            field_values,
        } => {
            if let Some(&child_entity) = playback.spawned.get(target_tag) {
                commands.queue(InsertComponentFromEffect {
                    entity: child_entity,
                    component_type: component_type.clone(),
                    field_values: field_values.clone(),
                });
            }
        }
        EffectAction::RemoveComponent {
            target_tag,
            component_type,
        } => {
            if let Some(&child_entity) = playback.spawned.get(target_tag) {
                commands.queue(RemoveComponentFromEffect {
                    entity: child_entity,
                    component_type: component_type.clone(),
                });
            }
        }
        EffectAction::TweenValue {
            target_tag,
            property,
            from,
            to,
            duration,
            easing,
        } => {
            if let Some(&child_entity) = playback.spawned.get(target_tag) {
                playback.active_tweens.push(ActiveTween {
                    entity: child_entity,
                    property: property.clone(),
                    from: *from,
                    to: *to,
                    start_time: playback.elapsed,
                    duration: *duration,
                    easing: *easing,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Reflection-based component commands
// ---------------------------------------------------------------------------

struct InsertComponentFromEffect {
    entity: Entity,
    component_type: String,
    #[allow(dead_code)]
    field_values: HashMap<String, String>,
}

impl Command for InsertComponentFromEffect {
    fn apply(self, world: &mut World) {
        let type_registry_arc = world.resource::<AppTypeRegistry>().clone();

        // Find type and create default instance
        let result = {
            let guard = type_registry_arc.read();
            let mut found = None;
            for reg in guard.iter() {
                if reg.type_info().type_path_table().short_path() == self.component_type {
                    if let Some(rd) = reg.data::<ReflectDefault>() {
                        found = Some((reg.type_id(), rd.default()));
                    }
                    break;
                }
            }
            found
        };

        let Some((type_id, default_val)) = result else {
            warn!(
                "InsertComponentFromEffect: type '{}' not found or has no ReflectDefault",
                self.component_type
            );
            return;
        };

        // Insert the component using ReflectComponent
        let reflect_component = {
            let guard = type_registry_arc.read();
            let Some(registration) = guard.get(type_id) else {
                return;
            };
            let Some(rc) = registration.data::<ReflectComponent>() else {
                warn!(
                    "InsertComponentFromEffect: type '{}' has no ReflectComponent",
                    self.component_type
                );
                return;
            };
            rc.clone()
        };
        let guard = type_registry_arc.read();
        reflect_component.insert(
            &mut world.entity_mut(self.entity),
            default_val.as_ref(),
            &guard,
        );
    }
}

struct RemoveComponentFromEffect {
    entity: Entity,
    component_type: String,
}

impl Command for RemoveComponentFromEffect {
    fn apply(self, world: &mut World) {
        let type_registry_arc = world.resource::<AppTypeRegistry>().clone();

        let reflect_component = {
            let guard = type_registry_arc.read();
            let mut type_id = None;
            for reg in guard.iter() {
                if reg.type_info().type_path_table().short_path() == self.component_type {
                    type_id = Some(reg.type_id());
                    break;
                }
            }
            let Some(tid) = type_id else {
                warn!(
                    "RemoveComponentFromEffect: type '{}' not found in type registry",
                    self.component_type
                );
                return;
            };
            let Some(registration) = guard.get(tid) else {
                return;
            };
            let Some(rc) = registration.data::<ReflectComponent>() else {
                warn!(
                    "RemoveComponentFromEffect: type '{}' has no ReflectComponent",
                    self.component_type
                );
                return;
            };
            rc.clone()
        };

        let Ok(mut entity_mut) = world.get_entity_mut(self.entity) else {
            return;
        };
        reflect_component.remove(&mut entity_mut);
    }
}

// ---------------------------------------------------------------------------
// Tween system
// ---------------------------------------------------------------------------

pub(crate) fn advance_tweens(
    mut effects: Query<&mut EffectPlayback>,
    mut transforms: Query<&mut Transform>,
    mut point_lights: Query<&mut PointLight>,
) {
    for mut playback in &mut effects {
        if playback.state != PlaybackState::Playing {
            continue;
        }

        let elapsed = playback.elapsed;
        playback.active_tweens.retain(|tween| {
            let t = ((elapsed - tween.start_time) / tween.duration).clamp(0.0, 1.0);
            let value = tween.from + (tween.to - tween.from) * tween.easing.eval(t);

            match &tween.property {
                TweenProperty::Scale => {
                    if let Ok(mut tr) = transforms.get_mut(tween.entity) {
                        tr.scale = Vec3::splat(value);
                    }
                }
                TweenProperty::LightIntensity => {
                    if let Ok(mut light) = point_lights.get_mut(tween.entity) {
                        light.intensity = value;
                    }
                }
                TweenProperty::Opacity | TweenProperty::Custom(_) => {
                    // Opacity would need material asset access; Custom is future.
                    // For now these are no-ops at runtime.
                }
            }

            t < 1.0
        });
    }
}

/// Despawn all children belonging to an effect and reset its playback state.
pub fn cleanup_effect(commands: &mut Commands, playback: &mut EffectPlayback) {
    for (_tag, entity) in playback.spawned.drain() {
        commands.entity(entity).despawn();
    }
    playback.elapsed = 0.0;
    playback.fired_steps.clear();
    playback.pending_events.clear();
    playback.collision_tags.clear();
    playback.last_collision_point = None;
    playback.rule_fire_times.clear();
    playback.last_fire_time = 0.0;
    playback.active_tweens.clear();
    playback.repeat_counts.clear();
    playback.state = PlaybackState::Stopped;
}

/// GRACEFULLY stop an effect: halt trigger firing (`Playing` → `Stopped`) and stop every spawned
/// vfx child EMITTING — without despawning anything, so live particles finish their authored
/// lifetimes and fade out as designed. The graceful sibling of [`cleanup_effect`]: `stop_effect`
/// first, wait out the drain (see [`max_spawned_particle_lifetime`]), then [`cleanup_effect`] to
/// despawn the (by then empty) children. `playback.spawned` is deliberately NOT drained here —
/// the final cleanup still owns the despawn.
pub fn stop_effect(commands: &mut Commands, playback: &mut EffectPlayback) {
    playback.state = PlaybackState::Stopped;
    playback.active_tweens.clear();
    for (_tag, entity) in playback.spawned.iter() {
        // Harmless on non-vfx children (a stray marker on a light/mesh is inert).
        if let Ok(mut ec) = commands.get_entity(*entity) {
            ec.insert(bevy_vfx::VfxEmissionStopped);
        }
    }
}

/// The longest any live particle of this effect's spawned vfx children can still take to age out
/// (the max of their [`bevy_vfx::VfxSystem::max_particle_lifetime`]s) — how long a caller must
/// wait after [`stop_effect`] before [`cleanup_effect`] despawns nothing visible. Children
/// without a `VfxSystem` (lights, meshes, nested effects' roots) contribute 0.
pub fn max_spawned_particle_lifetime(
    playback: &EffectPlayback,
    systems: &Query<&bevy_vfx::VfxSystem>,
) -> f32 {
    playback
        .spawned
        .values()
        .filter_map(|e| systems.get(*e).ok())
        .map(|s| s.max_particle_lifetime())
        .fold(0.0, f32::max)
}

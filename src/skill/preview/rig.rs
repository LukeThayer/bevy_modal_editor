//! Preview rig anim-graph plumbing (Task 10 — adapted from arena_editor's `preview_rig.rs`,
//! obelisk-arena `f6472e4`).
//!
//! **Deliberate v2 scope adaptation.** v1 hardcoded a specific rig asset (`character.glb`,
//! registered as a GLTF library in `main.rs`) plus its exact clip-name vocabulary
//! (`PREVIEW_CLIPS`, 11 short names like `"casting_idle"`), because arena_editor IS the game's
//! own editor and ships that exact rig. `bevy_modal_editor` is a general-purpose editor with no
//! canonical player-rig asset — grep confirms there is no `register_gltf_library` call anywhere
//! in this repo, so `AnimationLibrary` (from `bevy_editor_game`) is empty by default. Hardcoding
//! `character.glb` here would 404.
//!
//! So this module ports only the GENERIC half: build one `AnimationGraph` node per clip actually
//! present in `AnimationLibrary` (keyed by its full `"{gltf_name}::{clip_name}"` name — see
//! `asset_libraries::index_gltf` — which is ALSO exactly what the Skill panel's Presentation
//! region's "Anim (editor-only)" picker already stores in `CueBinding.anim`, Task 9's
//! `anim_clip_options`), and attach it to whatever `AnimationPlayer` the scene loader spawns
//! under the `PreviewCaster` — WHATEVER put it there. Nothing in this crate currently does (the
//! caster gets a plain capsule mesh, see `stage::ensure_stage`), so this machinery is inert until
//! a host registers a real rig's GLTF library and hangs its scene under `PreviewCaster` (out of
//! this task's scope) — exactly the "first consumer" framing the task brief uses: the PLUMBING is
//! what Task 10 owns, not a specific asset.

use bevy::prelude::*;
use bevy_editor_game::AnimationLibrary;
use std::collections::HashMap;

use super::stage::PreviewCaster;

/// The built preview `AnimationGraph` handle + per-clip node indices, keyed by the clip's FULL
/// `AnimationLibrary` key (e.g. `"wizard::Cast"`) — the same string authors pick via the
/// Presentation region's Anim dropdown, so a cue binding's `anim` name resolves here directly,
/// with no short-name/suffix translation needed (unlike v1 — see the module doc comment).
#[derive(Resource, Default)]
pub struct PreviewAnimGraph {
    pub graph: Option<Handle<AnimationGraph>>,
    pub nodes: HashMap<String, AnimationNodeIndex>,
}

impl PreviewAnimGraph {
    pub fn ready(&self) -> bool {
        self.graph.is_some()
    }
}

/// Once `AnimationLibrary` has any clips indexed, (re)build one `AnimationGraph` adding every
/// clip under the root. Idempotent per clip-set size: re-runs (and rebuilds, since the node
/// index map would otherwise silently go stale) whenever the library's clip COUNT grows — a new
/// GLTF library registered mid-session gets picked up; clips already resolved keep their slot
/// only incidentally (the graph is rebuilt from scratch, which is fine — nothing holds an
/// `AnimationNodeIndex` across frames except this resource itself).
pub fn build_preview_anim_graph(
    lib: Res<AnimationLibrary>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut state: ResMut<PreviewAnimGraph>,
    mut built_for: Local<usize>,
) {
    if lib.clips.is_empty() || lib.clips.len() == *built_for {
        return;
    }
    let mut graph = AnimationGraph::new();
    let root = graph.root;
    let mut nodes = HashMap::new();
    for (name, clip) in &lib.clips {
        let node = graph.add_clip(clip.clone(), 1.0, root);
        nodes.insert(name.clone(), node);
    }
    *built_for = lib.clips.len();
    if nodes.is_empty() {
        return;
    }
    state.nodes = nodes;
    state.graph = Some(graphs.add(graph));
}

/// Attach the built graph to any freshly-spawned `AnimationPlayer` found under a `PreviewCaster`
/// (the GLTF scene loader creates one inside a rig's tree, whenever a host hangs one there).
/// Seeds every clip looping muted-at-rest so cue-driven weight changes need no initial `play`.
pub fn attach_preview_anim_graph(
    mut commands: Commands,
    state: Res<PreviewAnimGraph>,
    casters: Query<Entity, With<PreviewCaster>>,
    children: Query<&Children>,
    pending: Query<Entity, (With<AnimationPlayer>, Without<AnimationGraphHandle>)>,
    mut players: Query<&mut AnimationPlayer>,
) {
    let Some(graph) = state.graph.clone() else {
        return;
    };
    for caster in &casters {
        let mut stack = vec![caster];
        while let Some(e) = stack.pop() {
            if pending.contains(e) {
                if let Ok(mut player) = players.get_mut(e) {
                    for node in state.nodes.values() {
                        player.play(*node).repeat().set_weight(0.0);
                    }
                }
                commands.entity(e).insert(AnimationGraphHandle(graph.clone()));
            }
            if let Ok(cs) = children.get(e) {
                stack.extend(cs.iter());
            }
        }
    }
}

/// Drive one clip node to `weight` (looping). The cosmetics observer calls this per cue-bound
/// anim row.
pub fn drive_anim_clip(player: &mut AnimationPlayer, node: AnimationNodeIndex, weight: f32) {
    player.play(node).repeat().set_weight(weight);
}

/// Depth-first search a rig subtree for the first `AnimationPlayer`. Returns `None` if the rig
/// has no player (e.g. no scene was ever hung under the caster — the common case today).
pub fn find_anim_player(root: Entity, children: &Query<&Children>, players: &Query<&mut AnimationPlayer>) -> Option<Entity> {
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if players.contains(e) {
            return Some(e);
        }
        if let Ok(cs) = children.get(e) {
            stack.extend(cs.iter());
        }
    }
    None
}

/// Wires the preview rig: the anim-graph state resource + the build/attach systems.
pub struct PreviewRigPlugin;

impl Plugin for PreviewRigPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PreviewAnimGraph>().add_systems(
            Update,
            (build_preview_anim_graph, attach_preview_anim_graph).chain(),
        );
    }
}

//! `SkillLibrary` + content-root registration/scanning (Task 5).
//!
//! A **content root** is a directory with four conventional subtrees (spec §3.3):
//! - `config/skills/*.toml` — skill RULES (`stat_core::Skill`), single-skill or
//!   `[[skills]]` array format (same as `stat_core::config::load_skills_dir`).
//! - `assets/skills/<id>.cast.ron` — the matching BEHAVIOR timeline
//!   (`obelisk_bevy::assets::CastTimeline`).
//! - `assets/effects/*.fx.ron` — `bevy_effect::EffectLibrary` presets.
//! - `assets/vfx/*.vfx.ron` — `bevy_vfx::VfxLibrary` presets.
//!
//! `register_obelisk_content` (the `RegisterObeliskContentExt` app extension) queues a
//! root; `scan_registered_content_roots` (a `Startup` system, after the `PreStartup`
//! library inits) scans every queued root into all four libraries in one pass. The
//! first root ever registered is the default write target for newly authored skills
//! (`SkillLibrary::default_root`).

use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use bevy::prelude::*;

use obelisk_bevy::assets::{Acquisition, CastTimeline, PhaseDurations};
use stat_core::Skill;

use bevy_effect::load_effects_from_dir;
use bevy_vfx::VfxLibrary;

use crate::effects::EffectLibrary;

use super::templates::ensure_starter_effects;

// ---------------------------------------------------------------------------
// SkillEntry / SkillLibrary
// ---------------------------------------------------------------------------

/// One loaded skill: paired rules (`stat_core::Skill`) + behavior timeline
/// (`obelisk_bevy::assets::CastTimeline`), plus disk bookkeeping.
///
/// This is Task 5's contract — Tasks 6-12 consume this exact shape.
#[derive(Debug, Clone)]
pub struct SkillEntry {
    pub rules: Skill,
    pub timeline: CastTimeline,
    pub rules_path: PathBuf,
    pub timeline_path: PathBuf,
    pub dirty_rules: bool,
    pub dirty_timeline: bool,
    /// `(hash of rules file bytes, hash of timeline file bytes)` at last load/save —
    /// Task 8's stale-check anchor. When no timeline file existed at load time (see
    /// [`SkillEntry::timeline_flagged`]) the timeline half is `0`.
    pub disk_hash: (u64, u64),
}

impl SkillEntry {
    /// True when `timeline` is a blank placeholder because `timeline_path` doesn't
    /// exist on disk — the "rules-only skill on disk" case the brief calls out as
    /// needing a flag. Rather than a dedicated bool field, the flag IS
    /// `!timeline_path.is_file()`: it's always correct by construction, can't drift
    /// out of sync with the entry's own paths, and both the palette and Task 8's
    /// stale-check can compute it directly.
    pub fn timeline_flagged(&self) -> bool {
        !self.timeline_path.is_file()
    }
}

/// The editor's live skill library: every skill known from every registered content
/// root, plus which one (if any) is open in the panel.
#[derive(Resource, Default)]
pub struct SkillLibrary {
    pub skills: BTreeMap<String, SkillEntry>,
    pub roots: Vec<PathBuf>,
    pub open: Option<String>,
}

impl SkillLibrary {
    /// The default write target for newly authored skills — the first root ever
    /// registered (spec §3.3). `None` if no content root has been registered yet.
    pub fn default_root(&self) -> Option<&Path> {
        self.roots.first().map(PathBuf::as_path)
    }
}

/// Where a skill id's rules TOML lives under content root `root`.
pub fn rules_path_for(root: &Path, id: &str) -> PathBuf {
    root.join("config").join("skills").join(format!("{id}.toml"))
}

/// Where a skill id's `.cast.ron` timeline lives under content root `root`.
pub fn timeline_path_for(root: &Path, id: &str) -> PathBuf {
    root.join("assets").join("skills").join(format!("{id}.cast.ron"))
}

fn blank_timeline(skill_id: &str) -> CastTimeline {
    CastTimeline {
        skill_id: skill_id.to_string(),
        phase_durations: PhaseDurations {
            windup: 0.0,
            active: 0.0,
            recovery: 0.0,
        },
        collision_windows: Vec::new(),
        acquisition: Acquisition::default(),
        vfx_cues: Default::default(),
        // Mirrors obelisk-bevy's own (private) `default_chain_radius`/`default_max_hold`
        // — see `obelisk_bevy::assets::CastTimeline` doc comments.
        chain_radius: 6.0,
        chargeable: false,
        max_hold: 1.0,
        cues: Default::default(),
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

// ---------------------------------------------------------------------------
// Content-root registration
// ---------------------------------------------------------------------------

/// Content roots queued by `RegisterObeliskContentExt::register_obelisk_content`,
/// consumed once by `scan_registered_content_roots` at `Startup`.
#[derive(Resource, Default)]
pub struct PendingContentRoots(pub Vec<PathBuf>);

/// `App` extension for queuing an obelisk content root — see the module docs for the
/// root's expected shape.
pub trait RegisterObeliskContentExt {
    /// Queue `root` for scanning at `Startup`. One registration feeds `SkillLibrary`,
    /// `EffectLibrary`, and `VfxLibrary` from the root's `config/skills/`,
    /// `assets/effects/`, and `assets/vfx/` subtrees respectively. The first root
    /// registered (across the whole app) becomes `SkillLibrary::default_root`.
    fn register_obelisk_content(&mut self, root: impl Into<PathBuf>) -> &mut Self;
}

impl RegisterObeliskContentExt for App {
    fn register_obelisk_content(&mut self, root: impl Into<PathBuf>) -> &mut Self {
        self.world_mut()
            .get_resource_or_insert_with(PendingContentRoots::default)
            .0
            .push(root.into());
        self
    }
}

/// Scan `root`'s `config/skills/*.toml` files into a fresh `SkillEntry` map. Pure
/// (besides the disk reads) — no `SkillLibrary` mutation, so it's directly testable
/// and directly reusable for both the `Startup` scan and the palette's "Rescan
/// content" action.
pub fn scan_content_root(root: &Path) -> BTreeMap<String, SkillEntry> {
    let mut skills = BTreeMap::new();
    let skills_dir = root.join("config").join("skills");
    let Ok(entries) = std::fs::read_dir(&skills_dir) else {
        return skills;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            warn!("Failed to read skill rules file: {:?}", path);
            continue;
        };

        // Same two-shape fallback as `stat_core::config::load_skills_dir`: try the
        // `[[skills]]` array format first, then a single top-level skill.
        let parsed: Vec<Skill> = match stat_core::config::parse_skills(&content) {
            Ok(map) => map.into_values().collect(),
            Err(_) => match toml::from_str::<Skill>(&content) {
                Ok(skill) => vec![skill],
                Err(e) => {
                    warn!("Failed to parse skill rules {:?}: {}", path, e);
                    continue;
                }
            },
        };
        let rules_hash = hash_bytes(content.as_bytes());

        for rules in parsed {
            let id = rules.id.clone();
            let timeline_path = timeline_path_for(root, &id);
            let (timeline, timeline_hash) = match std::fs::read_to_string(&timeline_path) {
                Ok(tl_content) => match ron::de::from_str::<CastTimeline>(&tl_content) {
                    Ok(tl) => (tl, hash_bytes(tl_content.as_bytes())),
                    Err(e) => {
                        warn!("Failed to parse timeline {:?}: {}", timeline_path, e);
                        (blank_timeline(&id), 0)
                    }
                },
                Err(_) => (blank_timeline(&id), 0),
            };

            skills.insert(
                id,
                SkillEntry {
                    rules,
                    timeline,
                    rules_path: path.clone(),
                    timeline_path,
                    dirty_rules: false,
                    dirty_timeline: false,
                    disk_hash: (rules_hash, timeline_hash),
                },
            );
        }
    }

    skills
}

/// Scan `root` and merge it into all three libraries — the single entry point both
/// the `Startup` system and the palette's "Rescan content" action call.
pub fn scan_and_merge_root(
    root: &Path,
    skill_library: &mut SkillLibrary,
    effect_library: &mut EffectLibrary,
    vfx_library: &mut VfxLibrary,
) {
    let scanned = scan_content_root(root);
    skill_library.skills.extend(scanned);
    if !skill_library.roots.iter().any(|r| r == root) {
        skill_library.roots.push(root.to_path_buf());
    }
    load_effects_from_dir(effect_library, &root.join("assets").join("effects"));
    crate::vfx::load_vfx_presets_from_dir(vfx_library, &root.join("assets").join("vfx"));
}

/// `Startup` system: ensures the starter Effect presets exist (fresh-install safety —
/// the archetype templates' cue bindings reference only these), then scans every
/// content root queued via `register_obelisk_content`.
pub(super) fn scan_registered_content_roots(
    pending: Res<PendingContentRoots>,
    mut skill_library: ResMut<SkillLibrary>,
    mut effect_library: ResMut<EffectLibrary>,
    mut vfx_library: ResMut<VfxLibrary>,
) {
    ensure_starter_effects(&mut effect_library);
    for root in &pending.0 {
        scan_and_merge_root(root, &mut skill_library, &mut effect_library, &mut vfx_library);
    }
}

// ---------------------------------------------------------------------------
// Lifecycle ops (pure fns — palette rows call these)
// ---------------------------------------------------------------------------

/// Ids of skills whose rules `conditions[]` name `id` as their `trigger_skill` — the
/// back-reference check delete/rename confirms against ("3 skills trigger this —
/// really delete?").
pub fn skills_referencing(id: &str, library: &SkillLibrary) -> Vec<String> {
    library
        .skills
        .iter()
        .filter(|(other_id, entry)| {
            other_id.as_str() != id
                && entry.rules.conditions.iter().any(|c| c.trigger_skill == id)
        })
        .map(|(other_id, _)| other_id.clone())
        .collect()
}

/// Pick an id not already present in `library`, starting from `base` and appending
/// `_2`, `_3`, ... on collision. Public: also used by the "New Skill" palette row to
/// turn a free-typed query/archetype label into a fresh, collision-free skill id.
pub fn unique_id(base: &str, library: &SkillLibrary) -> String {
    if !library.skills.contains_key(base) {
        return base.to_string();
    }
    for i in 2.. {
        let candidate = format!("{base}_{i}");
        if !library.skills.contains_key(&candidate) {
            return candidate;
        }
    }
    unreachable!("BTreeMap can't hold usize::MAX entries")
}

/// Insert a freshly authored (unsaved, dirty) skill entry, pathed under
/// `write_root`. Used both by "New Skill (archetype)" and `duplicate_skill` — Task 8
/// owns actually writing the files this points at. Returns `false` (no-op) if
/// `rules.id` is already taken.
pub fn insert_new_skill(
    library: &mut SkillLibrary,
    rules: Skill,
    timeline: CastTimeline,
    write_root: &Path,
) -> bool {
    let id = rules.id.clone();
    if library.skills.contains_key(&id) {
        return false;
    }
    library.skills.insert(
        id.clone(),
        SkillEntry {
            rules,
            timeline,
            rules_path: rules_path_for(write_root, &id),
            timeline_path: timeline_path_for(write_root, &id),
            dirty_rules: true,
            dirty_timeline: true,
            disk_hash: (0, 0),
        },
    );
    true
}

/// Duplicate `source_id`'s entry under a fresh id derived from it (`{source_id}_copy`,
/// `{source_id}_copy_2`, ...), re-pathed under `write_root`. Returns the new id, or
/// `None` if `source_id` doesn't exist.
pub fn duplicate_skill(library: &mut SkillLibrary, source_id: &str, write_root: &Path) -> Option<String> {
    let source = library.skills.get(source_id)?.clone();
    let new_id = unique_id(&format!("{source_id}_copy"), library);

    let mut rules = source.rules;
    rules.id = new_id.clone();
    let mut timeline = source.timeline;
    timeline.skill_id = new_id.clone();

    library.skills.insert(
        new_id.clone(),
        SkillEntry {
            rules,
            timeline,
            rules_path: rules_path_for(write_root, &new_id),
            timeline_path: timeline_path_for(write_root, &new_id),
            dirty_rules: true,
            dirty_timeline: true,
            disk_hash: (0, 0),
        },
    );
    Some(new_id)
}

/// Rename `old_id`'s entry to `new_id` (updates `rules.id`/`timeline.skill_id` and
/// re-paths within the same parent directories). No-ops (returns `false`) if
/// `old_id` is missing, `new_id` is already taken, or they're equal. Does NOT check
/// `skills_referencing` — callers own the confirm UX; this stays an unconditional,
/// pure-ish operation.
pub fn rename_skill(library: &mut SkillLibrary, old_id: &str, new_id: &str) -> bool {
    if old_id == new_id || library.skills.contains_key(new_id) {
        return false;
    }
    let Some(mut entry) = library.skills.remove(old_id) else {
        return false;
    };

    entry.rules.id = new_id.to_string();
    entry.timeline.skill_id = new_id.to_string();
    entry.rules_path = entry
        .rules_path
        .parent()
        .map(|p| p.join(format!("{new_id}.toml")))
        .unwrap_or_else(|| PathBuf::from(format!("{new_id}.toml")));
    entry.timeline_path = entry
        .timeline_path
        .parent()
        .map(|p| p.join(format!("{new_id}.cast.ron")))
        .unwrap_or_else(|| PathBuf::from(format!("{new_id}.cast.ron")));
    entry.dirty_rules = true;
    entry.dirty_timeline = true;

    library.skills.insert(new_id.to_string(), entry);
    if library.open.as_deref() == Some(old_id) {
        library.open = Some(new_id.to_string());
    }
    true
}

/// Delete `id`'s entry. Returns `false` if it didn't exist. Does NOT check
/// `skills_referencing` — see `rename_skill` docs.
pub fn delete_skill(library: &mut SkillLibrary, id: &str) -> bool {
    let removed = library.skills.remove(id).is_some();
    if removed && library.open.as_deref() == Some(id) {
        library.open = None;
    }
    removed
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A scratch directory under the OS temp dir, cleaned up on drop.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "bevy_modal_editor_skill_test_{tag}_{}_{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("create temp root");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_rules_toml(root: &Path, filename: &str, ids_and_names: &[(&str, &str)]) {
        let dir = root.join("config").join("skills");
        std::fs::create_dir_all(&dir).unwrap();
        let mut toml = String::new();
        for (id, name) in ids_and_names {
            toml.push_str(&format!(
                "[[skills]]\nid = \"{id}\"\nname = \"{name}\"\n\n[skills.damage]\nweapon_effectiveness = 1.0\n\n"
            ));
        }
        std::fs::write(dir.join(filename), toml).unwrap();
    }

    fn write_timeline_ron(root: &Path, id: &str) {
        use crate::skill::templates::strike_template;
        let (_, tl) = strike_template(id);
        let dir = root.join("assets").join("skills");
        std::fs::create_dir_all(&dir).unwrap();
        let ron_str = ron::ser::to_string_pretty(&tl, Default::default()).unwrap();
        std::fs::write(dir.join(format!("{id}.cast.ron")), ron_str).unwrap();
    }

    #[test]
    fn scan_root_pairs_rules_with_timelines() {
        let root = TempRoot::new("scan_pairs");
        write_rules_toml(
            root.path(),
            "skills.toml",
            &[("bolt", "Bolt"), ("rules_only", "Rules Only")],
        );
        write_timeline_ron(root.path(), "bolt");
        // "rules_only" deliberately gets no matching .cast.ron.

        let skills = scan_content_root(root.path());

        assert_eq!(skills.len(), 2);
        let bolt = skills.get("bolt").expect("bolt scanned");
        assert!(!bolt.timeline_flagged(), "bolt has a matching timeline on disk");
        assert_ne!(bolt.disk_hash.1, 0, "bolt's timeline hash is populated");

        let rules_only = skills.get("rules_only").expect("rules_only scanned");
        assert!(
            rules_only.timeline_flagged(),
            "rules_only has no matching .cast.ron — must be flagged"
        );
        assert_eq!(rules_only.timeline.collision_windows.len(), 0, "blank timeline");
        assert_eq!(rules_only.disk_hash.1, 0, "no timeline file read");
    }

    #[test]
    fn back_reference_check_finds_triggering_skills() {
        let mut library = SkillLibrary::default();

        let mut trigger = Skill {
            id: "explosion".into(),
            name: "Explosion".into(),
            ..Default::default()
        };
        trigger.conditions = vec![];

        let mut firebolt = Skill {
            id: "firebolt".into(),
            name: "Firebolt".into(),
            ..Default::default()
        };
        firebolt.conditions = vec![stat_core::SkillCondition {
            trigger_skill: "explosion".into(),
            ..Default::default()
        }];

        let unrelated = Skill {
            id: "heal".into(),
            name: "Heal".into(),
            ..Default::default()
        };

        for skill in [trigger, firebolt, unrelated] {
            let id = skill.id.clone();
            library.skills.insert(
                id.clone(),
                SkillEntry {
                    rules: skill,
                    timeline: blank_timeline(&id),
                    rules_path: PathBuf::new(),
                    timeline_path: PathBuf::new(),
                    dirty_rules: false,
                    dirty_timeline: false,
                    disk_hash: (0, 0),
                },
            );
        }

        let referencing = skills_referencing("explosion", &library);
        assert_eq!(referencing, vec!["firebolt".to_string()]);
        assert!(skills_referencing("heal", &library).is_empty());
    }

    #[test]
    fn duplicate_rename_delete_round_trip() {
        let mut library = SkillLibrary::default();
        let write_root = PathBuf::from("/tmp/does_not_need_to_exist_for_this_test");

        let (rules, timeline) = crate::skill::templates::strike_template("bolt");
        assert!(insert_new_skill(&mut library, rules, timeline, &write_root));
        assert!(library.skills.contains_key("bolt"));

        let dup_id = duplicate_skill(&mut library, "bolt", &write_root).expect("duplicate");
        assert_eq!(dup_id, "bolt_copy");
        assert!(library.skills.get(&dup_id).unwrap().dirty_rules);

        assert!(rename_skill(&mut library, &dup_id, "bolt2"));
        assert!(library.skills.contains_key("bolt2"));
        assert!(!library.skills.contains_key(&dup_id));
        assert_eq!(library.skills["bolt2"].rules.id, "bolt2");

        assert!(delete_skill(&mut library, "bolt2"));
        assert!(!library.skills.contains_key("bolt2"));
        assert!(library.skills.contains_key("bolt"));
    }
}

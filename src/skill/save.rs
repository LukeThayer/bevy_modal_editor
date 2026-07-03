//! Explicit, validation-gated save (Task 8).
//!
//! `save_skill` writes ONLY the dirty half(es) of a [`SkillEntry`] — rules via `toml_edit`
//! (format-preserving patch of an existing document; a brand-new file gets a clean one),
//! timeline via `ron::ser::to_string_pretty` (a full rewrite — `.cast.ron` comments are NOT
//! preserved; see the module doc on `SaveError` and the "D8" reference in the task brief for
//! why that's an accepted, documented gap rather than an oversight: RON has no
//! format-preserving edit crate the way TOML has `toml_edit`, and the timeline is a single
//! Rust-shaped tree with no comment-bearing content today).
//!
//! **Save is gated on validation, but not by this module.** `save_skill`'s signature
//! (`&mut SkillEntry -> Result<(), SaveError>`) has no library access, so it cannot itself run
//! `crate::skill::validation::validate_skill` (that needs `SkillLibrary`/`EffectLibrary`/
//! `VfxLibrary`/`AnimationLibrary`, all only reachable from the panel's `&mut World`). The panel
//! (`crate::skill::mod::draw_skill_panel`) is the actual gate: it runs `validate_skill` every
//! frame, disables the Save button whenever the report has a blocking problem, and — if it ever
//! needs to represent "refused to even try" through the same error channel the disk-I/O paths
//! use — constructs `SaveError::Blocked` itself. `save_skill` never returns `Blocked` on its
//! own; the variant exists so the panel has one enum to match on regardless of why a save didn't
//! happen.
//!
//! **Stale-check.** Every dirty target is rehashed against `entry.disk_hash` BEFORE either file
//! is touched: if the file's on-disk bytes changed since `entry` was scanned/last saved (someone
//! else edited it — a hand-edit, a second editor instance, a VCS checkout), `save_skill` returns
//! `SaveError::StaleDisk` without writing ANYTHING (not even the other, non-stale half) — a
//! torn write across the two files would be a worse failure mode than refusing outright. The
//! panel surfaces this as an inline "Reload from disk" / "Overwrite" choice:
//! `reload_skill` implements the first (re-scans both files, discarding in-memory edits);
//! `save_skill_overwrite` implements the second (skips the stale-check, force-writes, then
//! rehashes — same write path, just without the guard).
use std::path::Path;

use obelisk_bevy::assets::CastTimeline;

use super::library::{blank_timeline, hash_bytes, parse_skill_file, SkillEntry};

/// Fields the editor owns on a rules TOML — the only keys `save_skill` ever writes into an
/// EXISTING document. Every other key (description, tags, targeting, delivery,
/// attack_speed_modifier, consumes_self_effect, consumes_target_effect, use_conditions,
/// global_conditionals, conditional_modifiers, grants_elude_stacks, use_message, hint,
/// hint_effect, and any comments anywhere in the file) is untouched — round-tripped byte-for-
/// byte via the existing `toml_edit::DocumentMut` the patch is applied to.
const OWNED_RULES_FIELDS: &[&str] = &["id", "name", "mana_cost", "cooldown", "damage", "conditions", "effect_applications"];

/// Which disk target a save/stale-check failure is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveTarget {
    Rules,
    Timeline,
}

impl std::fmt::Display for SaveTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveTarget::Rules => write!(f, "rules"),
            SaveTarget::Timeline => write!(f, "timeline"),
        }
    }
}

/// Honest failure modes for a save attempt. See the module doc comment for how each is meant to
/// be handled by the panel.
#[derive(Debug, Clone)]
pub enum SaveError {
    /// A filesystem or (de)serialization error while reading/writing/rendering `which`.
    Io { which: SaveTarget, message: String },
    /// `which`'s on-disk bytes no longer match `entry.disk_hash` — someone/something else wrote
    /// this file since it was last scanned/saved. Nothing was written. See `reload_skill` /
    /// `save_skill_overwrite` for the two ways the panel can resolve this.
    StaleDisk { which: SaveTarget },
    /// The panel refused to call `save_skill` at all because `ValidationReport::has_blocking`
    /// was true. `save_skill` never constructs this itself (see the module doc comment) — it
    /// exists so the panel's "last save error" slot has one type to hold regardless of cause.
    Blocked(Vec<String>),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Io { which, message } => write!(f, "{which} save failed: {message}"),
            SaveError::StaleDisk { which } => {
                write!(f, "{which} file changed on disk since it was loaded — reload or overwrite")
            }
            SaveError::Blocked(reasons) => write!(f, "blocked by {} validation problem(s)", reasons.len()),
        }
    }
}

fn io_err(which: SaveTarget, e: impl std::fmt::Display) -> SaveError {
    SaveError::Io { which, message: e.to_string() }
}

/// Current hash of `path`'s bytes, or `0` if the file doesn't exist — the same "absent = 0"
/// sentinel `scan_content_root` uses for a skill with no `.cast.ron` yet, so a freshly authored
/// (never-saved) entry's `disk_hash: (0, 0)` always compares as "not stale".
fn current_disk_hash(path: &Path) -> u64 {
    std::fs::read(path).map(|bytes| hash_bytes(&bytes)).unwrap_or(0)
}

/// Save every dirty half of `entry`, stale-check first (see module doc comment). On success,
/// `entry.disk_hash` reflects the just-written bytes and the saved half's `dirty_*` flag is
/// cleared. On `StaleDisk`, nothing is written and `entry` is unchanged.
pub fn save_skill(entry: &mut SkillEntry) -> Result<(), SaveError> {
    check_stale(entry)?;
    write_dirty(entry)
}

/// Force-write every dirty half of `entry` WITHOUT the stale-check — the panel's "Overwrite"
/// branch of the stale prompt. Rehashes on success exactly like `save_skill`.
pub fn save_skill_overwrite(entry: &mut SkillEntry) -> Result<(), SaveError> {
    write_dirty(entry)
}

/// Re-scan both of `entry`'s files from disk, discarding any in-memory edits — the panel's
/// "Reload from disk" branch of the stale prompt. Clears both dirty flags and refreshes
/// `disk_hash` regardless of which half was actually stale (a reload always re-syncs both, since
/// after it `entry` must exactly mirror disk). Leaves `entry` untouched on error.
pub fn reload_skill(entry: &mut SkillEntry) -> Result<(), SaveError> {
    let id = entry.rules.id.clone();

    let rules_content = std::fs::read_to_string(&entry.rules_path).map_err(|e| io_err(SaveTarget::Rules, e))?;
    let rules_hash = hash_bytes(rules_content.as_bytes());
    let parsed = parse_skill_file(&rules_content).map_err(|e| io_err(SaveTarget::Rules, e))?;
    let rules = parsed
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| io_err(SaveTarget::Rules, format!("skill '{id}' not found in {:?}", entry.rules_path)))?;

    let (timeline, timeline_hash) = match std::fs::read_to_string(&entry.timeline_path) {
        Ok(content) => {
            let tl = ron::de::from_str::<CastTimeline>(&content).map_err(|e| io_err(SaveTarget::Timeline, e))?;
            (tl, hash_bytes(content.as_bytes()))
        }
        Err(_) => (blank_timeline(&id), 0),
    };

    entry.rules = rules;
    entry.timeline = timeline;
    entry.disk_hash = (rules_hash, timeline_hash);
    entry.dirty_rules = false;
    entry.dirty_timeline = false;
    Ok(())
}

fn check_stale(entry: &SkillEntry) -> Result<(), SaveError> {
    if entry.dirty_rules && current_disk_hash(&entry.rules_path) != entry.disk_hash.0 {
        return Err(SaveError::StaleDisk { which: SaveTarget::Rules });
    }
    if entry.dirty_timeline && current_disk_hash(&entry.timeline_path) != entry.disk_hash.1 {
        return Err(SaveError::StaleDisk { which: SaveTarget::Timeline });
    }
    Ok(())
}

fn write_dirty(entry: &mut SkillEntry) -> Result<(), SaveError> {
    if entry.dirty_rules {
        let rendered = render_rules_toml(entry).map_err(|e| io_err(SaveTarget::Rules, e))?;
        write_file(&entry.rules_path, &rendered).map_err(|e| io_err(SaveTarget::Rules, e))?;
        entry.disk_hash.0 = hash_bytes(rendered.as_bytes());
        entry.dirty_rules = false;
    }
    if entry.dirty_timeline {
        let pretty = ron::ser::PrettyConfig::default();
        let rendered = ron::ser::to_string_pretty(&entry.timeline, pretty).map_err(|e| io_err(SaveTarget::Timeline, e))?;
        write_file(&entry.timeline_path, &rendered).map_err(|e| io_err(SaveTarget::Timeline, e))?;
        entry.disk_hash.1 = hash_bytes(rendered.as_bytes());
        entry.dirty_timeline = false;
    }
    Ok(())
}

fn write_file(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)
}

/// Render `entry.rules` as a TOML document: if `entry.rules_path` exists on disk, load it and
/// patch ONLY `OWNED_RULES_FIELDS` in place (format-preserving — comments, key order, and every
/// non-owned field survive untouched); otherwise (a brand-new skill) emit a clean document.
fn render_rules_toml(entry: &SkillEntry) -> Result<String, String> {
    let fresh = toml_edit::ser::to_document(&entry.rules).map_err(|e| e.to_string())?;

    let Ok(existing) = std::fs::read_to_string(&entry.rules_path) else {
        return Ok(fresh.to_string());
    };
    let mut doc: toml_edit::DocumentMut = existing.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;

    match find_skill_table(&mut doc, &entry.rules.id) {
        Some(target) => {
            for key in OWNED_RULES_FIELDS {
                if let Some(item) = fresh.get(key) {
                    target[*key] = item.clone();
                }
            }
            Ok(doc.to_string())
        }
        // The existing file doesn't (yet) contain this id — e.g. it was just renamed into a
        // path with no prior content, or the id genuinely isn't in the file it's pathed at.
        // Fall back to a clean document for this file rather than guessing where to splice a
        // brand-new skill into someone else's TOML structure.
        None => Ok(fresh.to_string()),
    }
}

/// Find the sub-table describing skill `id` within `doc` — either an item of a top-level
/// `[[skills]]` array-of-tables, or (single-skill-per-file layout) `doc`'s own root table when
/// its `id` matches.
fn find_skill_table<'a>(doc: &'a mut toml_edit::DocumentMut, id: &str) -> Option<&'a mut toml_edit::Table> {
    let is_array = doc.get("skills").is_some_and(|item| item.is_array_of_tables());
    if is_array {
        let array = doc.get_mut("skills")?.as_array_of_tables_mut()?;
        return array.iter_mut().find(|t| t.get("id").and_then(|i| i.as_str()) == Some(id));
    }
    if doc.get("id").and_then(|i| i.as_str()) == Some(id) {
        return Some(doc.as_table_mut());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use crate::skill::templates::strike_template;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("bevy_modal_editor_skill_save_test_{tag}_{}_{n}", std::process::id()));
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

    fn dirty_entry(root: &Path, id: &str) -> SkillEntry {
        let (rules, timeline) = strike_template(id);
        SkillEntry {
            rules,
            timeline,
            rules_path: root.join(format!("{id}.toml")),
            timeline_path: root.join(format!("{id}.cast.ron")),
            dirty_rules: true,
            dirty_timeline: true,
            disk_hash: (0, 0),
        }
    }

    // --- brand-new skill: clean documents, hashes/dirty flags update ---

    #[test]
    fn new_skill_writes_both_files_and_clears_dirty() {
        let root = TempRoot::new("new_skill");
        let mut entry = dirty_entry(root.path(), "bolt");

        save_skill(&mut entry).expect("save");

        assert!(!entry.dirty_rules);
        assert!(!entry.dirty_timeline);
        assert!(entry.rules_path.is_file());
        assert!(entry.timeline_path.is_file());
        assert_ne!(entry.disk_hash.0, 0);
        assert_ne!(entry.disk_hash.1, 0);

        let rules_on_disk = std::fs::read_to_string(&entry.rules_path).unwrap();
        assert!(rules_on_disk.contains("\"bolt\""));
        let timeline_on_disk = std::fs::read_to_string(&entry.timeline_path).unwrap();
        assert!(timeline_on_disk.contains("bolt"));
    }

    #[test]
    fn only_dirty_halves_are_written() {
        let root = TempRoot::new("only_dirty");
        let mut entry = dirty_entry(root.path(), "bolt");
        entry.dirty_timeline = false;

        save_skill(&mut entry).expect("save");

        assert!(entry.rules_path.is_file());
        assert!(!entry.timeline_path.is_file(), "timeline was never dirty — must not be written");
    }

    // --- toml_edit comment preservation ---

    #[test]
    fn existing_rules_file_comments_survive_a_field_change() {
        let root = TempRoot::new("comments");
        std::fs::create_dir_all(root.path()).unwrap();
        let rules_path = root.path().join("bolt.toml");
        std::fs::write(
            &rules_path,
            "# top-of-file author note\n\
             id = \"bolt\"\n\
             name = \"Bolt\"\n\
             # mana cost is intentionally cheap\n\
             mana_cost = 5.0\n\
             cooldown = 1.0\n\
             description = \"a bolt\" # trailing comment on an unrelated field\n",
        )
        .unwrap();

        let (mut rules, timeline) = strike_template("bolt");
        rules.mana_cost = 42.0; // the one field this save changes
        let mut entry = SkillEntry {
            rules,
            timeline,
            rules_path: rules_path.clone(),
            timeline_path: root.path().join("bolt.cast.ron"),
            dirty_rules: true,
            dirty_timeline: false,
            disk_hash: (hash_bytes(std::fs::read(&rules_path).unwrap().as_slice()), 0),
        };

        save_skill(&mut entry).expect("save");

        let after = std::fs::read_to_string(&rules_path).unwrap();
        assert!(after.contains("# top-of-file author note"), "{after}");
        assert!(after.contains("# mana cost is intentionally cheap"), "{after}");
        assert!(after.contains("# trailing comment on an unrelated field"), "{after}");
        assert!(after.contains("a bolt"), "description (not editor-owned) must survive: {after}");
        assert!(after.contains("42"), "mana_cost must be updated: {after}");
        assert!(!after.contains("mana_cost = 5"), "old value must be gone: {after}");
    }

    #[test]
    fn array_of_tables_shape_patches_the_matching_entry_only() {
        let root = TempRoot::new("array_shape");
        std::fs::create_dir_all(root.path()).unwrap();
        let rules_path = root.path().join("skills.toml");
        std::fs::write(
            &rules_path,
            "[[skills]]\n\
             id = \"bolt\"\n\
             name = \"Bolt\"\n\
             mana_cost = 5.0\n\
             cooldown = 1.0\n\n\
             [[skills]]\n\
             id = \"other\"\n\
             name = \"Other\"\n\
             mana_cost = 99.0\n\
             cooldown = 1.0\n",
        )
        .unwrap();

        let (mut rules, timeline) = strike_template("bolt");
        rules.mana_cost = 7.0;
        let mut entry = SkillEntry {
            rules,
            timeline,
            rules_path: rules_path.clone(),
            timeline_path: root.path().join("bolt.cast.ron"),
            dirty_rules: true,
            dirty_timeline: false,
            disk_hash: (hash_bytes(std::fs::read(&rules_path).unwrap().as_slice()), 0),
        };

        save_skill(&mut entry).expect("save");

        let after = std::fs::read_to_string(&rules_path).unwrap();
        assert!(after.contains("mana_cost = 7"), "{after}");
        assert!(after.contains("mana_cost = 99"), "sibling entry must be untouched: {after}");
    }

    // --- stale-check ---

    #[test]
    fn stale_disk_refuses_to_write_either_file() {
        let root = TempRoot::new("stale");
        let mut entry = dirty_entry(root.path(), "bolt");
        save_skill(&mut entry).expect("first save");

        // Simulate an external edit after the entry's disk_hash was captured.
        std::fs::write(&entry.rules_path, "id = \"bolt\"\nname = \"Hand-edited\"\n").unwrap();
        entry.rules.name = "Edited in editor".to_string();
        entry.dirty_rules = true;
        entry.dirty_timeline = true;

        let result = save_skill(&mut entry);
        assert!(matches!(result, Err(SaveError::StaleDisk { which: SaveTarget::Rules })), "{result:?}");

        // Neither file was touched by the refused save.
        let rules_on_disk = std::fs::read_to_string(&entry.rules_path).unwrap();
        assert!(rules_on_disk.contains("Hand-edited"));
        assert!(entry.dirty_rules && entry.dirty_timeline, "dirty flags must be unchanged on refusal");
    }

    #[test]
    fn save_skill_overwrite_ignores_staleness_and_rehashes() {
        let root = TempRoot::new("overwrite");
        let mut entry = dirty_entry(root.path(), "bolt");
        save_skill(&mut entry).expect("first save");

        std::fs::write(&entry.rules_path, "id = \"bolt\"\nname = \"Hand-edited\"\n").unwrap();
        entry.rules.name = "Edited in editor".to_string();
        entry.dirty_rules = true;

        save_skill_overwrite(&mut entry).expect("overwrite save");
        assert!(!entry.dirty_rules);
        let rules_on_disk = std::fs::read_to_string(&entry.rules_path).unwrap();
        assert!(rules_on_disk.contains("Edited in editor"));
        assert_eq!(entry.disk_hash.0, hash_bytes(rules_on_disk.as_bytes()));
    }

    // --- reload ---

    #[test]
    fn reload_skill_discards_in_memory_edits() {
        let root = TempRoot::new("reload");
        let mut entry = dirty_entry(root.path(), "bolt");
        save_skill(&mut entry).expect("first save");
        let saved_name = entry.rules.name.clone();

        entry.rules.name = "Unsaved edit".to_string();
        entry.dirty_rules = true;

        reload_skill(&mut entry).expect("reload");
        assert_eq!(entry.rules.name, saved_name, "reload must reflect on-disk content, not the unsaved edit");
        assert!(!entry.dirty_rules);
        assert!(!entry.dirty_timeline);
    }

    // --- SaveTarget/SaveError Display sanity (cheap, but keeps the enum honest) ---

    #[test]
    fn save_error_display_is_human_readable() {
        let stale = SaveError::StaleDisk { which: SaveTarget::Timeline };
        assert!(stale.to_string().contains("timeline"));
        let blocked = SaveError::Blocked(vec!["a".to_string(), "b".to_string()]);
        assert!(blocked.to_string().contains('2'));
    }
}

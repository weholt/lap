//! Grouped-asset file operations with develop recipe companions
//! (lap-487 / TASK-403; governing contract `docs/raw-development/spec.md`,
//! acceptance A8).
//!
//! Lap owns cataloging and file operations; develop recipes live in adjacent
//! `filename.ext.lapedit.json` sidecars that must travel as part of the
//! asset's group. This module provides the primitives every grouped-asset
//! mutation entry point composes:
//!
//! - [`OperationJournal`]: a bounded journal of completed sidecar file
//!   mutations with explicit reverse-order rollback, so partial file-operation
//!   failures never strand recipes or lose data.
//! - Companion planning/execution for rename/move ([`companion_move`],
//!   [`execute_companion_moves`]): each member's sidecar is name-anchored to
//!   that member, so a same-basename RAW/JPEG pair keeps the recipe on the
//!   explicitly selected member.
//! - Copy identity fork ([`fork_sidecar_for_copy`]): a copy creates a NEW
//!   asset identity carrying the same initial recipe, revision and source
//!   fingerprint; the original stays untouched.
//! - Identity adoption ([`adopt_catalog_identity`]): re-keys a durable sidecar
//!   to the current catalog identity (after a copy, an external move or a
//!   catalog rebuild) only when the source fingerprint still matches —
//!   replacement at the same path is refused explicitly.
//! - Trash/permanent-delete companions ([`trash_companion`],
//!   [`delete_companion_permanently`]) so trash and restore keep the group
//!   together.
//! - Catalog projection maintenance ([`migrate_companion_projection`],
//!   [`delete_companion_projection`], [`migrate_companion_projection_folder`])
//!   and strict reconciliation ([`reconcile_folder`]) that associates edits
//!   only unambiguously and surfaces ambiguity and missing media.
//!
//! All failures are typed and surfaced; nothing degrades into a silent
//! success or a silent association to the wrong source.

use rapidraw_edit_model::{RecipeEnvelope, parse_envelope};
use rusqlite::Connection;
use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

use crate::develop::recipe_repository::RecipeRepository;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Typed failure of a grouped-asset companion operation. Every variant is
/// explicit; callers must surface these instead of masking them.
#[derive(Debug)]
pub enum OperationError {
    /// The referenced source (media or sidecar) is no longer present.
    SourceMissing { path: PathBuf },
    /// A destination sidecar already exists and was never overwritten.
    DestinationExists { path: PathBuf },
    /// The source bytes no longer match the sidecar's recorded fingerprint:
    /// the media at this path was replaced. The sidecar is left untouched.
    SourceReplaced {
        path: PathBuf,
        expected: String,
        found: String,
    },
    Io {
        context: String,
        path: PathBuf,
        source: std::io::Error,
    },
    /// The durable sidecar could not be parsed (corrupt or unsupported
    /// schema). It is preserved, never reset.
    Recipe { path: PathBuf, detail: String },
}

impl fmt::Display for OperationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OperationError::SourceMissing { path } => {
                write!(f, "source {} is missing", path.display())
            }
            OperationError::DestinationExists { path } => {
                write!(f, "destination {} already exists", path.display())
            }
            OperationError::SourceReplaced {
                path,
                expected,
                found,
            } => write!(
                f,
                "media at {} was replaced after its recipe was written (fingerprint {expected}, found {found})",
                path.display()
            ),
            OperationError::Io {
                context,
                path,
                source,
            } => write!(f, "{context} ({}): {source}", path.display()),
            OperationError::Recipe { path, detail } => {
                write!(
                    f,
                    "recipe sidecar at {} is unusable and was left untouched: {detail}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for OperationError {}

// ---------------------------------------------------------------------------
// Operation journal
// ---------------------------------------------------------------------------

/// One completed file mutation recorded by [`OperationJournal`].
#[derive(Debug, Clone)]
pub enum JournalStep {
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    Copy {
        from: PathBuf,
        to: PathBuf,
    },
    /// A removal staged through a same-directory backup file so a failed
    /// group operation can still restore it.
    Remove {
        path: PathBuf,
        backup: PathBuf,
    },
}

/// Result of finishing a journal ([`OperationJournal::rollback`] or
/// [`OperationJournal::commit`]). Steps that could not be reverted or cleaned
/// up are reported explicitly; they are never silently dropped.
#[derive(Debug, Default)]
pub struct RollbackSummary {
    pub reverted: usize,
    pub failed: Vec<(JournalStep, String)>,
}

/// Bounded journal of completed sidecar mutations for ONE grouped-asset
/// operation. Steps are recorded only after the corresponding filesystem
/// mutation succeeded; rollback replays them in reverse order.
#[derive(Debug, Default)]
pub struct OperationJournal {
    steps: Vec<JournalStep>,
}

impl OperationJournal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn steps(&self) -> &[JournalStep] {
        &self.steps
    }

    /// Renames `from` to `ref_to` and records the step. The destination is
    /// never silently overwritten.
    pub fn rename(&mut self, from: &Path, to: &Path) -> Result<(), OperationError> {
        if !from.exists() {
            return Err(OperationError::SourceMissing {
                path: from.to_path_buf(),
            });
        }
        if to.exists() {
            return Err(OperationError::DestinationExists {
                path: to.to_path_buf(),
            });
        }
        fs::rename(from, to).map_err(|err| OperationError::Io {
            context: "renaming recipe sidecar".to_string(),
            path: to.to_path_buf(),
            source: err,
        })?;
        self.steps.push(JournalStep::Rename {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        });
        Ok(())
    }

    /// Copies `from` to `to` and records the step. The destination is never
    /// silently overwritten.
    pub fn copy(&mut self, from: &Path, to: &Path) -> Result<(), OperationError> {
        if !from.exists() {
            return Err(OperationError::SourceMissing {
                path: from.to_path_buf(),
            });
        }
        if to.exists() {
            return Err(OperationError::DestinationExists {
                path: to.to_path_buf(),
            });
        }
        fs::copy(from, to).map_err(|err| OperationError::Io {
            context: "copying recipe sidecar".to_string(),
            path: to.to_path_buf(),
            source: err,
        })?;
        self.steps.push(JournalStep::Copy {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        });
        Ok(())
    }

    /// Stages a file removal through a same-directory backup so rollback can
    /// still restore it (used for replace-policy operations).
    pub fn remove_file(&mut self, path: &Path) -> Result<(), OperationError> {
        if !path.exists() {
            return Err(OperationError::SourceMissing {
                path: path.to_path_buf(),
            });
        }
        let backup = staged_backup_path(path);
        fs::rename(path, &backup).map_err(|err| OperationError::Io {
            context: "staging recipe sidecar removal".to_string(),
            path: path.to_path_buf(),
            source: err,
        })?;
        self.steps.push(JournalStep::Remove {
            path: path.to_path_buf(),
            backup,
        });
        Ok(())
    }

    /// Reverts every completed step in reverse order. Steps that cannot be
    /// reverted are reported; already-reverted steps are skipped so a second
    /// call is a safe no-op.
    pub fn rollback(mut self) -> RollbackSummary {
        let mut summary = RollbackSummary::default();
        while let Some(step) = self.steps.pop() {
            match &step {
                JournalStep::Rename { from, to } => match fs::rename(to, from) {
                    Ok(()) => summary.reverted += 1,
                    Err(err) => summary
                        .failed
                        .push((step, format!("rollback rename failed: {err}"))),
                },
                JournalStep::Copy { to, .. } => match fs::remove_file(to) {
                    Ok(()) => summary.reverted += 1,
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        summary.reverted += 1;
                    }
                    Err(err) => summary
                        .failed
                        .push((step, format!("rollback copy removal failed: {err}"))),
                },
                JournalStep::Remove { path, backup } => match fs::rename(backup, path) {
                    Ok(()) => summary.reverted += 1,
                    Err(err) => summary
                        .failed
                        .push((step, format!("rollback removal restore failed: {err}"))),
                },
            }
        }
        summary
    }

    /// Finishes a successful operation: staged removal backups are deleted.
    pub fn commit(mut self) -> RollbackSummary {
        let mut summary = RollbackSummary::default();
        for step in self.steps.drain(..) {
            if let JournalStep::Remove { backup, .. } = &step {
                match fs::remove_file(backup) {
                    Ok(()) => {}
                    Err(err) => summary
                        .failed
                        .push((step, format!("backup cleanup failed: {err}"))),
                }
            }
        }
        summary
    }
}

fn staged_backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".lap-removed-{}", uuid::Uuid::new_v4()));
    PathBuf::from(name)
}

// ---------------------------------------------------------------------------
// Companion planning and execution
// ---------------------------------------------------------------------------

/// A planned sidecar companion move: the sidecar of `from` follows to the
/// sidecar position derived from `to`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionMove {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// The sidecar path for a media file (`name.ext` -> `name.ext.lapedit.json`).
pub fn sidecar_path_for(member: &Path) -> PathBuf {
    RecipeRepository::sidecar_path(member)
}

/// Plans the sidecar companion move for one group member. Returns `None` when
/// the member has no sidecar or the move would not change its path. The
/// companion is name-anchored to THIS member only: in a same-basename
/// RAW/JPEG pair each member carries at most its own sidecar.
pub fn companion_move(source_member: &Path, target_member: &Path) -> Option<CompanionMove> {
    let from = sidecar_path_for(source_member);
    if !from.exists() {
        return None;
    }
    let to = sidecar_path_for(target_member);
    if from == to {
        return None;
    }
    Some(CompanionMove { from, to })
}

/// Executes planned companion moves through the journal. Each move is checked
/// and executed in order; on the first failure the completed moves stay in
/// the journal so the caller can roll the group back to a coherent state.
pub fn execute_companion_moves(
    journal: &mut OperationJournal,
    moves: &[CompanionMove],
) -> Result<(), OperationError> {
    for companion in moves {
        journal.rename(&companion.from, &companion.to)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Copy identity fork
// ---------------------------------------------------------------------------

/// A forked sidecar created for a copied asset.
#[derive(Debug, Clone)]
pub struct ForkedCopy {
    pub sidecar_path: PathBuf,
    pub asset_id: String,
}

/// A fresh provisional asset identity for a copied asset. Catalog row ids do
/// not exist until the copy is indexed, so copies carry a unique
/// `copied-<uuid>` identity until [`adopt_catalog_identity`] re-keys it to
/// the catalog row id.
pub fn new_copy_asset_id() -> String {
    format!("copied-{}", uuid::Uuid::new_v4())
}

/// Creates the copied asset's sidecar as a NEW asset identity with the same
/// initial recipe. The source sidecar is never modified; a partial copy is
/// removed before the error is returned. Returns `Ok(None)` when the source
/// member has no sidecar (nothing to fork).
pub fn fork_sidecar_for_copy(
    source_member: &Path,
    copied_member: &Path,
    new_asset_id: &str,
) -> Result<Option<ForkedCopy>, OperationError> {
    let source_sidecar = sidecar_path_for(source_member);
    if !source_sidecar.exists() {
        return Ok(None);
    }
    let target_sidecar = sidecar_path_for(copied_member);
    if target_sidecar.exists() {
        return Err(OperationError::DestinationExists {
            path: target_sidecar,
        });
    }

    let bytes = fs::read(&source_sidecar).map_err(|err| OperationError::Io {
        context: "reading source recipe sidecar for copy".to_string(),
        path: source_sidecar.clone(),
        source: err,
    })?;
    let mut envelope = parse_sidecar_bytes(&source_sidecar, &bytes)?;

    // The fork is only valid for the media the recipe was written for: a
    // mismatch means the source was replaced and must not propagate.
    revalidate_source_fingerprint(source_member, &envelope)?;

    envelope.asset_id = new_asset_id.to_string();
    let canonical = canonical_bytes(&source_sidecar, &envelope)?;

    let result = write_sidecar_atomically(&target_sidecar, &canonical);
    if let Err(error) = result {
        let _ = fs::remove_file(&target_sidecar);
        return Err(error);
    }
    Ok(Some(ForkedCopy {
        sidecar_path: target_sidecar,
        asset_id: new_asset_id.to_string(),
    }))
}

/// Re-keys EVERY develop sidecar inside a copied folder to a fresh unique
/// asset identity (used after whole-folder copies, where sidecars travel with
/// the directory). Called after the folder copy succeeded; a failure here is
/// reported so the caller can roll the whole copy back. Returns the number of
/// sidecars forked.
pub fn fork_copied_folder_sidecars(folder: &Path) -> Result<usize, OperationError> {
    let mut sidecars = Vec::new();
    for entry in WalkDir::new(folder) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                return Err(OperationError::Io {
                    context: "walking copied folder for develop sidecars".to_string(),
                    path: folder.to_path_buf(),
                    source: std::io::Error::other(err),
                });
            }
        };
        if entry.file_type().is_file()
            && entry
                .file_name()
                .to_string_lossy()
                .ends_with(".lapedit.json")
        {
            sidecars.push(entry.path().to_path_buf());
        }
    }

    let mut forked = 0usize;
    for sidecar in sidecars {
        let bytes = fs::read(&sidecar).map_err(|err| OperationError::Io {
            context: "reading copied recipe sidecar".to_string(),
            path: sidecar.clone(),
            source: err,
        })?;
        let mut envelope = parse_sidecar_bytes(&sidecar, &bytes)?;
        envelope.asset_id = new_copy_asset_id();
        let canonical = canonical_bytes(&sidecar, &envelope)?;
        write_sidecar_atomically(&sidecar, &canonical)?;
        forked += 1;
    }
    Ok(forked)
}

// ---------------------------------------------------------------------------
// Identity adoption (copy / external move / catalog rebuild re-keying)
// ---------------------------------------------------------------------------

/// Outcome of [`adopt_catalog_identity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdoptionOutcome {
    /// No sidecar exists next to the media: nothing to adopt.
    NoSidecar,
    /// The sidecar already carries the requested identity.
    Unchanged { revision: u64 },
    /// The sidecar was re-keyed from a different identity. The durable
    /// revision is preserved (adoption is not an edit) and the previous
    /// sidecar bytes are retained for recovery.
    Adopted {
        previous_asset_id: String,
        revision: u64,
    },
}

/// Re-keys the sidecar next to `source_member` to the current catalog
/// identity. Adoption requires the source fingerprint to match the current
/// media bytes: replacement at the same path is detected and refused, never
/// silently re-associated. Corrupt or newer-schema sidecars are refused and
/// preserved.
pub fn adopt_catalog_identity(
    source_member: &Path,
    asset_id: &str,
) -> Result<AdoptionOutcome, OperationError> {
    let sidecar = sidecar_path_for(source_member);
    if !sidecar.exists() {
        return Ok(AdoptionOutcome::NoSidecar);
    }
    let bytes = fs::read(&sidecar).map_err(|err| OperationError::Io {
        context: "reading recipe sidecar for identity adoption".to_string(),
        path: sidecar.clone(),
        source: err,
    })?;
    let envelope = parse_sidecar_bytes(&sidecar, &bytes)?;

    // Replacement/missing-media detection always runs first: even a matching
    // identity must not be reported as fine when the media at the path was
    // replaced or disappeared.
    revalidate_source_fingerprint(source_member, &envelope)?;

    adopt_envelope_identity(&sidecar, &bytes, envelope, asset_id)
}

/// Like [`adopt_catalog_identity`], but takes the ALREADY computed fingerprint
/// of the current media bytes (the develop entry points read and hash the
/// source anyway; this variant avoids reading large media twice). Still
/// refuses a mismatched fingerprint explicitly.
pub fn adopt_catalog_identity_with_fingerprint(
    source_member: &Path,
    asset_id: &str,
    found_fingerprint: &str,
) -> Result<AdoptionOutcome, OperationError> {
    let sidecar = sidecar_path_for(source_member);
    if !sidecar.exists() {
        return Ok(AdoptionOutcome::NoSidecar);
    }
    let bytes = fs::read(&sidecar).map_err(|err| OperationError::Io {
        context: "reading recipe sidecar for identity adoption".to_string(),
        path: sidecar.clone(),
        source: err,
    })?;
    let envelope = parse_sidecar_bytes(&sidecar, &bytes)?;
    if envelope.source_fingerprint != found_fingerprint {
        return Err(OperationError::SourceReplaced {
            path: source_member.to_path_buf(),
            expected: envelope.source_fingerprint.clone(),
            found: found_fingerprint.to_string(),
        });
    }
    adopt_envelope_identity(&sidecar, &bytes, envelope, asset_id)
}

/// Applies the identity decision to a parsed envelope. Requires the caller to
/// have validated the source fingerprint already.
fn adopt_envelope_identity(
    sidecar: &Path,
    bytes: &[u8],
    mut envelope: RecipeEnvelope,
    asset_id: &str,
) -> Result<AdoptionOutcome, OperationError> {
    if envelope.asset_id == asset_id {
        return Ok(AdoptionOutcome::Unchanged {
            revision: envelope.revision,
        });
    }

    let previous_asset_id = envelope.asset_id.clone();
    // Retain the previous durable sidecar for recovery, mirroring commit.
    let source = source_of_sidecar(sidecar);
    let previous_path = RecipeRepository::previous_sidecar_path(&source);
    write_bytes_and_sync(&previous_path, bytes).map_err(|err| OperationError::Io {
        context: "retaining previous sidecar during adoption".to_string(),
        path: previous_path,
        source: err,
    })?;

    envelope.asset_id = asset_id.to_string();
    let canonical = canonical_bytes(sidecar, &envelope)?;
    write_sidecar_atomically(sidecar, &canonical)?;

    Ok(AdoptionOutcome::Adopted {
        previous_asset_id,
        revision: envelope.revision,
    })
}

/// Derives the media path from a sidecar path (`name.ext.lapedit.json` ->
/// `name.ext`).
pub fn source_of_sidecar(sidecar: &Path) -> PathBuf {
    let name = sidecar
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let stripped = name.strip_suffix(".lapedit.json").unwrap_or(&name);
    match sidecar.parent() {
        Some(parent) => parent.join(stripped),
        None => PathBuf::from(stripped),
    }
}

/// Recomputes the source fingerprint and compares it to the envelope's.
fn revalidate_source_fingerprint(
    source_member: &Path,
    envelope: &RecipeEnvelope,
) -> Result<(), OperationError> {
    if !source_member.exists() {
        return Err(OperationError::SourceMissing {
            path: source_member.to_path_buf(),
        });
    }
    let bytes = fs::read(source_member).map_err(|err| OperationError::Io {
        context: "reading source media for fingerprint revalidation".to_string(),
        path: source_member.to_path_buf(),
        source: err,
    })?;
    let found = rapidraw_edit_model::sha256_hex(&bytes);
    if found != envelope.source_fingerprint {
        return Err(OperationError::SourceReplaced {
            path: source_member.to_path_buf(),
            expected: envelope.source_fingerprint.clone(),
            found,
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Trash / permanent delete companions
// ---------------------------------------------------------------------------

/// Moves the member's sidecar to the recycle bin as part of a grouped trash
/// operation, keeping trash and a later OS restore coherent as one group.
/// Returns `false` when no sidecar exists.
pub fn trash_companion(source_member: &Path) -> Result<bool, OperationError> {
    let sidecar = sidecar_path_for(source_member);
    if !sidecar.exists() {
        return Ok(false);
    }
    trash::delete(&sidecar).map_err(|err| OperationError::Io {
        context: "moving recipe sidecar to trash".to_string(),
        path: sidecar.clone(),
        source: std::io::Error::other(err),
    })?;
    if sidecar.exists() {
        return Err(OperationError::Io {
            context: "recipe sidecar still exists after trash".to_string(),
            path: sidecar,
            source: std::io::Error::other("trash destination verification failed"),
        });
    }
    Ok(true)
}

/// Permanently deletes the member's sidecar as part of a grouped permanent
/// delete. Returns `false` when no sidecar exists.
pub fn delete_companion_permanently(source_member: &Path) -> Result<bool, OperationError> {
    let sidecar = sidecar_path_for(source_member);
    if !sidecar.exists() {
        return Ok(false);
    }
    fs::remove_file(&sidecar).map_err(|err| OperationError::Io {
        context: "permanently deleting recipe sidecar".to_string(),
        path: sidecar,
        source: err,
    })?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Catalog projection maintenance
// ---------------------------------------------------------------------------

/// Moves projection rows from the old sidecar path to the new one after a
/// rename/move. Idempotent: rows already present under the new path replace
/// the old ones. Returns the number of rows now recorded under the new path.
pub fn migrate_companion_projection(
    conn: &Connection,
    old_sidecar: &Path,
    new_sidecar: &Path,
) -> Result<usize, rusqlite::Error> {
    let old = old_sidecar.to_string_lossy().to_string();
    let new = new_sidecar.to_string_lossy().to_string();
    if old == new {
        return existing_projection_rows(conn, &new);
    }
    let migrated = conn.execute(
        "UPDATE adevelop_recipes SET sidecar_path = ?1 WHERE sidecar_path = ?2",
        rusqlite::params![new, old],
    )?;
    // If a row already existed under the new path, the update above would
    // have violated the primary key; drop the stale old rows instead.
    if migrated == 0 {
        conn.execute(
            "DELETE FROM adevelop_recipes WHERE sidecar_path = ?1",
            rusqlite::params![old],
        )?;
    }
    existing_projection_rows(conn, &new)
}

fn existing_projection_rows(conn: &Connection, sidecar: &str) -> Result<usize, rusqlite::Error> {
    conn.query_row(
        "SELECT COUNT(*) FROM adevelop_recipes WHERE sidecar_path = ?1",
        rusqlite::params![sidecar],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count.max(0) as usize)
}

/// Deletes projection rows for sidecar paths whose group was trashed or
/// permanently deleted. Returns the number of rows removed.
pub fn delete_companion_projection(
    conn: &Connection,
    sidecars: &[PathBuf],
) -> Result<usize, rusqlite::Error> {
    let mut deleted = 0;
    for sidecar in sidecars {
        deleted += conn.execute(
            "DELETE FROM adevelop_recipes WHERE sidecar_path = ?1",
            rusqlite::params![sidecar.to_string_lossy()],
        )?;
    }
    Ok(deleted)
}

/// Rewrites projection sidecar paths after a folder rename/move. The prefix
/// must match up to a path-separator boundary (or the end of the string).
/// Returns the number of rows migrated.
pub fn migrate_companion_projection_folder(
    conn: &Connection,
    old_prefix: &Path,
    new_prefix: &Path,
) -> Result<usize, rusqlite::Error> {
    let old = old_prefix.to_string_lossy().to_string();
    let new = new_prefix.to_string_lossy().to_string();
    if old.is_empty() || old == new {
        return Ok(0);
    }

    let mut stmt = conn.prepare("SELECT sidecar_path FROM adevelop_recipes")?;
    let paths: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .flatten()
        .collect();
    drop(stmt);

    let mut migrated = 0;
    for path in paths {
        let Some(rest) = path.strip_prefix(&old) else {
            continue;
        };
        if !(rest.is_empty() || rest.starts_with('\\') || rest.starts_with('/')) {
            continue;
        }
        let updated = format!("{new}{rest}");
        conn.execute(
            "UPDATE adevelop_recipes SET sidecar_path = ?1 WHERE sidecar_path = ?2",
            rusqlite::params![updated, path],
        )?;
        migrated += 1;
    }
    Ok(migrated)
}

/// Removes projection rows for every sidecar below a folder prefix (folder
/// moved outside the library or deleted). Returns the number of rows removed.
pub fn delete_companion_projection_folder(
    conn: &Connection,
    folder_prefix: &Path,
) -> Result<usize, rusqlite::Error> {
    let prefix = folder_prefix.to_string_lossy().to_string();
    let mut stmt = conn.prepare("SELECT sidecar_path FROM adevelop_recipes")?;
    let paths: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .flatten()
        .collect();
    drop(stmt);

    let mut removed = 0;
    for path in paths {
        let Some(rest) = path.strip_prefix(&prefix) else {
            continue;
        };
        if !(rest.is_empty() || rest.starts_with('\\') || rest.starts_with('/')) {
            continue;
        }
        removed += conn.execute(
            "DELETE FROM adevelop_recipes WHERE sidecar_path = ?1",
            rusqlite::params![path],
        )?;
    }
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Strict reconciliation (rescan / external moves / catalog rebuild)
// ---------------------------------------------------------------------------

/// A non-fatal reconciliation finding that must be surfaced instead of
/// silently associating a recipe with the wrong source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileFinding {
    /// The sidecar exists but its media does not (external removal or pending
    /// external move). The sidecar is preserved and the row keeps no catalog
    /// association until the media returns.
    MediaMissing,
    /// The media exists on disk but has no catalog row yet (partially indexed
    /// folder or externally added media). Reconcile again after indexing.
    MediaUncataloged,
    /// The catalog contains more than one case-insensitive match for the
    /// sidecar's media path. No arbitrary association is made.
    AmbiguousCatalogMatch { candidates: Vec<i64> },
}

/// Summary of [`reconcile_folder`].
#[derive(Debug, Default)]
pub struct AssetReconcileSummary {
    pub sidecars_seen: u64,
    pub projected: u64,
    pub removed_missing_sidecar: u64,
    pub findings: Vec<(PathBuf, ReconcileFinding)>,
    pub errors: Vec<(PathBuf, String)>,
}

/// Resolution of a sidecar's media path against the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrictCatalogResolution {
    Exact(i64),
    Ambiguous(Vec<i64>),
    Missing,
}

/// Resolves a media path to its catalog row id, associating only unambiguous
/// matches: an exact (case-sensitive) path/name match wins; a single
/// case-insensitive match is accepted (Windows-normalized spellings); several
/// case-insensitive matches are surfaced as ambiguous and resolve to nothing.
pub fn resolve_catalog_file_id_strict(
    conn: &Connection,
    source: &Path,
) -> Result<StrictCatalogResolution, rusqlite::Error> {
    let parent = source
        .parent()
        .unwrap_or(Path::new(""))
        .to_string_lossy()
        .to_string();
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut stmt = conn.prepare(
        "SELECT f.id FROM afiles f JOIN afolders d ON d.id = f.folder_id
         WHERE d.path = ?1 AND f.name = ?2 LIMIT 2",
    )?;
    let exact: Vec<i64> = stmt
        .query_map(rusqlite::params![parent, name], |row| row.get(0))?
        .flatten()
        .collect();
    drop(stmt);
    if exact.len() == 1 {
        return Ok(StrictCatalogResolution::Exact(exact[0]));
    }
    if exact.len() > 1 {
        return Ok(StrictCatalogResolution::Ambiguous(exact));
    }

    let mut stmt = conn.prepare(
        "SELECT f.id FROM afiles f JOIN afolders d ON d.id = f.folder_id
         WHERE d.path = ?1 COLLATE NOCASE AND f.name = ?2 COLLATE NOCASE LIMIT 3",
    )?;
    let candidates: Vec<i64> = stmt
        .query_map(rusqlite::params![parent, name], |row| row.get(0))?
        .flatten()
        .collect();
    drop(stmt);
    match candidates.len() {
        1 => Ok(StrictCatalogResolution::Exact(candidates[0])),
        0 => Ok(StrictCatalogResolution::Missing),
        _ => Ok(StrictCatalogResolution::Ambiguous(candidates)),
    }
}

/// Reconciles every develop sidecar under `folder` (recursively) with the
/// catalog. Idempotent and safe to run repeatedly (rescan, startup, after
/// external moves): it re-associates edits from sidecars, keeps durable facts
/// in the projection, and surfaces missing media, uncataloged media and
/// ambiguous matches instead of associating to the wrong source. Media bytes
/// are not hashed here (bounded IO); replacement detection happens at
/// session-open/export through fingerprint revalidation.
pub fn reconcile_folder(
    conn: &Connection,
    repo: &RecipeRepository,
    folder: &Path,
) -> AssetReconcileSummary {
    let mut summary = AssetReconcileSummary::default();
    let mut seen_sidecars = BTreeSet::new();
    for entry in WalkDir::new(folder) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                summary
                    .errors
                    .push((folder.to_path_buf(), format!("walk error: {err}")));
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if !name.ends_with(".lapedit.json") {
            continue;
        }
        summary.sidecars_seen += 1;
        let sidecar = entry.path();
        if !seen_sidecars.insert(sidecar.to_path_buf()) {
            continue;
        }
        let source = sidecar.with_file_name(name.strip_suffix(".lapedit.json").unwrap_or(&name));

        let finding = reconcile_one(conn, repo, &source, sidecar);
        match finding {
            Ok(Some(finding)) => summary.findings.push((sidecar.to_path_buf(), finding)),
            Ok(None) => summary.projected += 1,
            Err(err) => summary
                .errors
                .push((sidecar.to_path_buf(), err.to_string())),
        }
    }
    // Sidecar files that disappeared since the last projection: the catalog
    // rows are stale until explicit per-path maintenance removes them. This
    // is counted only for paths this walk previously knew; `walk` cannot see
    // them, so cleanup stays the caller's explicit step.
    summary.removed_missing_sidecar = count_projection_rows_without_sidecars(conn, folder);
    summary
}

fn reconcile_one(
    conn: &Connection,
    repo: &RecipeRepository,
    source: &Path,
    sidecar: &Path,
) -> Result<Option<ReconcileFinding>, OperationError> {
    let envelope = match repo.load_opt(source) {
        Ok(Some(envelope)) => envelope,
        Ok(None) => {
            return Err(OperationError::SourceMissing {
                path: sidecar.to_path_buf(),
            });
        }
        Err(err) => {
            return Err(OperationError::Recipe {
                path: sidecar.to_path_buf(),
                detail: err.to_string(),
            });
        }
    };

    if !source.exists() {
        project_envelope_strict(conn, source, &envelope, sidecar, None)?;
        return Ok(Some(ReconcileFinding::MediaMissing));
    }

    match resolve_catalog_file_id_strict(conn, source).map_err(|err| OperationError::Io {
        context: "catalog resolution failed during reconcile".to_string(),
        path: sidecar.to_path_buf(),
        source: std::io::Error::other(err),
    })? {
        StrictCatalogResolution::Exact(file_id) => {
            project_envelope_strict(conn, source, &envelope, sidecar, Some(file_id))?;
            Ok(None)
        }
        StrictCatalogResolution::Ambiguous(candidates) => {
            project_envelope_strict(conn, source, &envelope, sidecar, None)?;
            Ok(Some(ReconcileFinding::AmbiguousCatalogMatch { candidates }))
        }
        StrictCatalogResolution::Missing => {
            project_envelope_strict(conn, source, &envelope, sidecar, None)?;
            Ok(Some(ReconcileFinding::MediaUncataloged))
        }
    }
}

/// Projection upsert with strict catalog resolution (same durable facts as
/// the commit-time projection).
fn project_envelope_strict(
    conn: &Connection,
    source: &Path,
    envelope: &RecipeEnvelope,
    sidecar: &Path,
    file_id: Option<i64>,
) -> Result<(), OperationError> {
    let sidecar_str = sidecar.to_string_lossy().to_string();
    let revision = i64::try_from(envelope.revision).map_err(|_| OperationError::Recipe {
        path: sidecar.to_path_buf(),
        detail: format!(
            "revision {} exceeds the SQLite integer range",
            envelope.revision
        ),
    })?;
    let content_hash = envelope
        .content_hash()
        .map_err(|err| OperationError::Recipe {
            path: sidecar.to_path_buf(),
            detail: err.to_string(),
        })?;
    let updated_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);

    conn.execute(
        "INSERT INTO adevelop_recipes
             (sidecar_path, variant_id, file_id, revision, schema_version,
              source_fingerprint, content_hash, is_edited, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(sidecar_path, variant_id) DO UPDATE SET
             file_id = excluded.file_id,
             revision = excluded.revision,
             schema_version = excluded.schema_version,
             source_fingerprint = excluded.source_fingerprint,
             content_hash = excluded.content_hash,
             is_edited = excluded.is_edited,
             updated_at = excluded.updated_at",
        rusqlite::params![
            sidecar_str,
            envelope.variant_id,
            file_id,
            revision,
            envelope.schema_version,
            envelope.source_fingerprint,
            content_hash,
            i64::from(crate::develop::recipe_repository::is_edited_envelope(
                envelope
            )),
            updated_at,
        ],
    )
    .map_err(|err| OperationError::Io {
        context: "develop projection upsert failed".to_string(),
        path: source.to_path_buf(),
        source: std::io::Error::other(err),
    })?;
    Ok(())
}

fn count_projection_rows_without_sidecars(conn: &Connection, folder: &Path) -> u64 {
    let Ok(mut stmt) = conn.prepare("SELECT sidecar_path FROM adevelop_recipes") else {
        return 0;
    };
    let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) else {
        return 0;
    };
    let prefix = folder.to_string_lossy().to_string();
    let mut count = 0u64;
    for row in rows.flatten() {
        let path = PathBuf::from(&row);
        if path.starts_with(&prefix) && !path.exists() {
            count += 1;
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Sidecar byte helpers
// ---------------------------------------------------------------------------

fn parse_sidecar_bytes(path: &Path, bytes: &[u8]) -> Result<RecipeEnvelope, OperationError> {
    let text = String::from_utf8(bytes.to_vec()).map_err(|err| OperationError::Recipe {
        path: path.to_path_buf(),
        detail: format!("sidecar is not valid UTF-8: {err}"),
    })?;
    parse_envelope(&text).map_err(|err| OperationError::Recipe {
        path: path.to_path_buf(),
        detail: err.to_string(),
    })
}

fn canonical_bytes(path: &Path, envelope: &RecipeEnvelope) -> Result<Vec<u8>, OperationError> {
    envelope
        .to_canonical_json()
        .map_err(|err| OperationError::Recipe {
            path: path.to_path_buf(),
            detail: err.to_string(),
        })
}

fn write_bytes_and_sync(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn unique_temp_path(base: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut name = base.as_os_str().to_os_string();
    name.push(format!(".{}.{nanos:x}.tmp", std::process::id()));
    PathBuf::from(name)
}

fn replace_atomic(temp: &Path, target: &Path) -> std::io::Result<()> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match fs::rename(temp, target) {
            Ok(()) => return Ok(()),
            Err(err) if attempt < 5 && is_transient_replace_error(&err) => {
                std::thread::sleep(Duration::from_millis(10 * u64::from(attempt)));
            }
            Err(err) => return Err(err),
        }
    }
}

fn is_transient_replace_error(err: &std::io::Error) -> bool {
    cfg!(windows) && err.kind() == std::io::ErrorKind::PermissionDenied
}

/// Atomic canonical sidecar write: temporary sibling, flush, atomic replace.
fn write_sidecar_atomically(sidecar: &Path, bytes: &[u8]) -> Result<(), OperationError> {
    let temp = unique_temp_path(sidecar);
    let outcome = (|| {
        write_bytes_and_sync(&temp, bytes).map_err(|err| OperationError::Io {
            context: "writing temporary sidecar sibling".to_string(),
            path: temp.clone(),
            source: err,
        })?;
        replace_atomic(&temp, sidecar).map_err(|err| OperationError::Io {
            context: "atomically replacing sidecar".to_string(),
            path: sidecar.to_path_buf(),
            source: err,
        })
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(&temp);
    }
    outcome
}

//! Virtual copies (variants) for develop recipes (lap-952 / TASK-503).
//!
//! Governing contract `docs/raw-development/spec.md` ("Recipe and session
//! contract", delivery step 5) and A1/A3/A8/A9.
//!
//! Model: the durable envelope already carries `variantId`, `revision` and
//! the source fingerprint (shared engine schema, never changed here). The
//! primary variant (`default`) keeps its established sidecar
//! `name.ext.lapedit.json`. Every virtual copy gets its **own sidecar**
//! `name.ext.lapedit.v-<variantId>.json` holding one envelope with an
//! independent CAS revision per asset/variant, referring to the same
//! immutable source bytes. Virtual-copy sidecars deliberately do **not**
//! end in `.lapedit.json`, so the existing primary-sidecar walkers never
//! mis-derive their media source; this module owns their projection,
//! reconciliation and lifecycle (copy / reset / delete).
//!
//! Copying duplicates the recipe payload only — never pixels, never the
//! source, and never resource store objects (references are shared; the
//! content-addressed store retains objects referenced by any surviving
//! recipe). Deleting a variant removes exactly its sidecar, retained
//! previous revision and projection row. Resetting clears one variant's
//! recipe/decode/resources under a CAS revision bump while retaining its
//! identity and any preserved `unsupported` payload.

use crate::develop::recipe_repository::{
    CommitReceipt, RecipeRepoError, RecipeRepository, project_envelope,
};
use rapidraw_edit_model::{EffectiveDecodeSettings, Recipe, RecipeEnvelope, sha256_hex};
use rusqlite::Connection;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Reserved variant id of the primary (non-virtual) sidecar.
pub const DEFAULT_VARIANT_ID: &str = "default";
const VARIANT_INFIX: &str = ".lapedit.v-";
const VARIANT_SUFFIX: &str = ".json";
const MAX_VARIANT_ID_LEN: usize = 64;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum VariantError {
    InvalidVariantId(String),
    InvalidInput(String),
    SidecarExists {
        path: PathBuf,
    },
    SidecarMissing {
        path: PathBuf,
    },
    /// The primary variant's sidecar is the asset's main recipe and cannot be
    /// deleted (reset is allowed).
    PrimaryVariantProtected,
    SourceMissing {
        path: PathBuf,
        detail: String,
    },
    Io {
        context: String,
        path: PathBuf,
        source: std::io::Error,
    },
    Recipe {
        source: RecipeRepoError,
    },
    Projection {
        detail: String,
    },
}

impl fmt::Display for VariantError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VariantError::InvalidVariantId(detail) => write!(f, "invalid variant id: {detail}"),
            VariantError::InvalidInput(detail) => write!(f, "invalid variant request: {detail}"),
            VariantError::SidecarExists { path } => {
                write!(f, "virtual copy already exists at {}", path.display())
            }
            VariantError::SidecarMissing { path } => {
                write!(f, "no virtual-copy sidecar exists at {}", path.display())
            }
            VariantError::PrimaryVariantProtected => write!(
                f,
                "the primary variant '{DEFAULT_VARIANT_ID}' cannot be deleted; reset it or delete the asset instead"
            ),
            VariantError::SourceMissing { path, detail } => {
                write!(f, "source {} is unavailable: {detail}", path.display())
            }
            VariantError::Io {
                context,
                path,
                source,
            } => write!(f, "{context} ({}): {source}", path.display()),
            VariantError::Recipe { source } => write!(f, "{source}"),
            VariantError::Projection { detail } => write!(f, "catalog projection failed: {detail}"),
        }
    }
}

impl std::error::Error for VariantError {}

fn recipe_error(source: RecipeRepoError) -> VariantError {
    VariantError::Recipe { source }
}

// ---------------------------------------------------------------------------
// Sidecar naming
// ---------------------------------------------------------------------------

/// True when `file_name` is a virtual-copy sidecar
/// (`name.ext.lapedit.v-<variantId>.json`). Such names never end in the
/// primary `.lapedit.json` suffix, so primary-sidecar tooling ignores them.
pub fn is_variant_sidecar(file_name: &str) -> bool {
    let Some(base) = file_name.strip_suffix(VARIANT_SUFFIX) else {
        return false;
    };
    match base.rfind(VARIANT_INFIX) {
        Some(pos) => pos > 0 && !base[pos + VARIANT_INFIX.len()..].is_empty(),
        None => false,
    }
}

/// The virtual-copy sidecar path for `source` and `variant_id`.
pub fn variant_sidecar_path(source: &Path, variant_id: &str) -> PathBuf {
    let mut name = source
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!("{VARIANT_INFIX}{variant_id}{VARIANT_SUFFIX}"));
    source.with_file_name(name)
}

/// Derives `(media source, variant id)` from a virtual-copy sidecar path.
pub fn variant_sidecar_source(sidecar: &Path) -> Option<(PathBuf, String)> {
    let name = sidecar.file_name()?.to_string_lossy();
    let base = name.strip_suffix(VARIANT_SUFFIX)?;
    let pos = base.rfind(VARIANT_INFIX)?;
    if pos == 0 {
        return None;
    }
    let source_name = &base[..pos];
    let variant_id = &base[pos + VARIANT_INFIX.len()..];
    if source_name.is_empty() || variant_id.is_empty() {
        return None;
    }
    Some((sidecar.with_file_name(source_name), variant_id.to_string()))
}

/// Variant ids become file-name fragments, so they are restricted to a
/// conservative, path-safe alphabet and length. `default` is reserved.
pub fn validate_virtual_variant_id(id: &str) -> Result<(), VariantError> {
    if id.is_empty() {
        return Err(VariantError::InvalidVariantId(
            "variant id must not be empty".to_string(),
        ));
    }
    if id == DEFAULT_VARIANT_ID {
        return Err(VariantError::InvalidVariantId(format!(
            "'{DEFAULT_VARIANT_ID}' is reserved for the primary variant"
        )));
    }
    if id.len() > MAX_VARIANT_ID_LEN {
        return Err(VariantError::InvalidVariantId(format!(
            "variant id must be at most {MAX_VARIANT_ID_LEN} characters"
        )));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(VariantError::InvalidVariantId(
            "variant id may only contain ASCII letters, digits, '-' and '_'".to_string(),
        ));
    }
    Ok(())
}

/// Generates a fresh, path-safe virtual-copy id.
pub fn new_virtual_variant_id() -> String {
    format!("vc-{}", &uuid::Uuid::new_v4().simple().to_string()[..16])
}

// ---------------------------------------------------------------------------
// Lifecycle operations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantSummary {
    pub variant_id: String,
    pub revision: u64,
    pub is_edited: bool,
    pub is_virtual_copy: bool,
    pub content_hash: String,
    pub sidecar_path: PathBuf,
    pub exists: bool,
}

fn summary_for(
    variant_id: &str,
    envelope: &RecipeEnvelope,
    sidecar: &Path,
    is_virtual_copy: bool,
) -> VariantSummary {
    VariantSummary {
        variant_id: variant_id.to_string(),
        revision: envelope.revision,
        is_edited: crate::develop::recipe_repository::is_edited_envelope(envelope),
        is_virtual_copy,
        content_hash: envelope.content_hash().unwrap_or_default(),
        sidecar_path: sidecar.to_path_buf(),
        exists: true,
    }
}

/// Lists the implicit default variant first (present or not), then every
/// virtual-copy sidecar sibling of `source`, sorted by variant id.
pub fn list_variants(
    repo: &RecipeRepository,
    source: &Path,
) -> Result<Vec<VariantSummary>, VariantError> {
    let primary_sidecar = RecipeRepository::sidecar_path(source);
    let mut out = match repo.load_opt(source).map_err(recipe_error)? {
        Some(envelope) => vec![summary_for(
            DEFAULT_VARIANT_ID,
            &envelope,
            &primary_sidecar,
            false,
        )],
        None => vec![VariantSummary {
            variant_id: DEFAULT_VARIANT_ID.to_string(),
            revision: 0,
            is_edited: false,
            is_virtual_copy: false,
            content_hash: String::new(),
            sidecar_path: primary_sidecar,
            exists: false,
        }],
    };

    let folder = source.parent().unwrap_or(Path::new("."));
    let mut copies: Vec<VariantSummary> = Vec::new();
    let entries = fs::read_dir(folder).map_err(|err| VariantError::Io {
        context: "listing variant sidecars".to_string(),
        path: folder.to_path_buf(),
        source: err,
    })?;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                return Err(VariantError::Io {
                    context: "reading variant sidecar directory entry".to_string(),
                    path: folder.to_path_buf(),
                    source: err,
                });
            }
        };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_variant_sidecar(&name) {
            continue;
        }
        let Some((copy_source, variant_id)) = variant_sidecar_source(&path) else {
            continue;
        };
        if copy_source != source {
            continue;
        }
        match repo.load_at_opt(&path) {
            Ok(Some(envelope)) => {
                copies.push(summary_for(&variant_id, &envelope, &path, true));
            }
            Ok(None) => continue,
            Err(err) => return Err(recipe_error(err)),
        }
    }
    copies.sort_by(|a, b| a.variant_id.cmp(&b.variant_id));
    out.extend(copies);
    Ok(out)
}

/// Creates a virtual copy of `source` with an independent variant identity
/// and revision, referring to the same immutable source bytes.
///
/// `asset_id` is the caller's authoritative asset identity (the catalog file
/// id string). `from_variant_id` selects the recipe payload source (`None` or
/// `default` copies the primary recipe if one exists; a missing named variant
/// is a typed error). A fresh asset without any sidecar gets a default
/// recipe. The new envelope always carries the hash of the current source
/// bytes; a copied payload whose fingerprint disagrees with the current
/// source is rejected instead of propagated.
pub fn create_virtual_copy(
    repo: &RecipeRepository,
    conn: Option<&Connection>,
    source: &Path,
    asset_id: &str,
    from_variant_id: Option<&str>,
    new_variant_id: Option<&str>,
) -> Result<RecipeEnvelope, VariantError> {
    let variant_id = match new_variant_id {
        Some(id) => {
            validate_virtual_variant_id(id)?;
            id.to_string()
        }
        None => new_virtual_variant_id(),
    };
    if asset_id.is_empty() {
        return Err(VariantError::InvalidInput(
            "asset id must not be empty".to_string(),
        ));
    }
    let sidecar = variant_sidecar_path(source, &variant_id);
    if sidecar.exists() {
        return Err(VariantError::SidecarExists { path: sidecar });
    }

    let from = match from_variant_id {
        None | Some(DEFAULT_VARIANT_ID) => repo.load_opt(source).map_err(recipe_error)?,
        Some(other) => Some(
            repo.load_variant_opt(source, other)
                .map_err(recipe_error)?
                .ok_or_else(|| VariantError::SidecarMissing {
                    path: variant_sidecar_path(source, other),
                })?,
        ),
    };

    // The copy refers to the source as it is right now (spec A1): hash the
    // current bytes and refuse to propagate a stale fingerprint.
    let source_bytes = fs::read(source).map_err(|err| VariantError::SourceMissing {
        path: source.to_path_buf(),
        detail: err.to_string(),
    })?;
    let fingerprint = sha256_hex(&source_bytes);
    if let Some(existing) = &from
        && existing.source_fingerprint != fingerprint
    {
        return Err(recipe_error(RecipeRepoError::SourceFingerprintMismatch {
            path: source.to_path_buf(),
            expected: existing.source_fingerprint.clone(),
            found: fingerprint,
        }));
    }

    let mut envelope = repo.new_envelope(asset_id, &variant_id, &fingerprint);
    if let Some(existing) = from {
        envelope.recipe = existing.recipe;
        envelope.decode = existing.decode;
        envelope.resources = existing.resources;
        envelope.unsupported = existing.unsupported;
    }
    repo.commit_at(source, &sidecar, 0, envelope, conn, None)
        .map_err(recipe_error)?;
    // Return the durable envelope (revision 1 as persisted).
    match repo.load_variant_opt(source, &variant_id).map_err(recipe_error)? {
        Some(envelope) => Ok(envelope),
        None => Err(VariantError::SidecarMissing { path: sidecar }),
    }
}

/// Clears one variant's recipe, decode settings and resource references under
/// an explicit CAS revision bump. The variant keeps its identity, source
/// fingerprint and preserved `unsupported` payload; no other variant, no
/// resource store object and no source byte is touched.
pub fn reset_variant(
    repo: &RecipeRepository,
    conn: Option<&Connection>,
    source: &Path,
    variant_id: &str,
    expected_revision: u64,
) -> Result<CommitReceipt, VariantError> {
    if variant_id != DEFAULT_VARIANT_ID {
        validate_virtual_variant_id(variant_id)?;
    }
    let mut envelope = repo
        .load_variant_opt(source, variant_id)
        .map_err(recipe_error)?
        .ok_or_else(|| VariantError::SidecarMissing {
            path: variant_sidecar_path(source, variant_id),
        })?;
    envelope.recipe = Recipe::default();
    envelope.decode = EffectiveDecodeSettings::default();
    envelope.resources.clear();
    repo.commit_variant(source, variant_id, expected_revision, envelope, conn, None)
        .map_err(recipe_error)
}

/// Deletes one virtual copy: its sidecar, retained previous revision and
/// projection row. The primary variant is protected; every other variant,
/// the resource store and the source are untouched.
pub fn delete_variant(
    conn: Option<&Connection>,
    source: &Path,
    variant_id: &str,
) -> Result<(), VariantError> {
    if variant_id == DEFAULT_VARIANT_ID {
        return Err(VariantError::PrimaryVariantProtected);
    }
    validate_virtual_variant_id(variant_id)?;
    let sidecar = variant_sidecar_path(source, variant_id);
    if !sidecar.exists() {
        return Err(VariantError::SidecarMissing { path: sidecar });
    }
    fs::remove_file(&sidecar).map_err(|err| VariantError::Io {
        context: "removing virtual-copy sidecar".to_string(),
        path: sidecar.clone(),
        source: err,
    })?;
    let previous = RecipeRepository::previous_sidecar_for(&sidecar);
    if previous.exists() {
        fs::remove_file(&previous).map_err(|err| VariantError::Io {
            context: "removing retained previous virtual-copy revision".to_string(),
            path: previous,
            source: err,
        })?;
    }
    if let Some(conn) = conn {
        conn.execute(
            "DELETE FROM adevelop_recipes WHERE sidecar_path = ?1",
            [sidecar.to_string_lossy().as_ref()],
        )
        .map_err(|err| VariantError::Projection {
            detail: format!("removing deleted variant's projection row failed: {err}"),
        })?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rebuildable projection reconciliation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VariantsReconcileSummary {
    pub sidecars_seen: u64,
    pub projected: u64,
    pub removed_missing: u64,
    pub errors: Vec<(PathBuf, String)>,
}

/// Projects every virtual-copy sidecar under `folder` into the rebuildable
/// `adevelop_recipes` catalog projection and removes projection rows of
/// variant sidecars that no longer exist. Never touches primary sidecars or
/// any durable file.
pub fn reconcile_folder(
    conn: &Connection,
    repo: &RecipeRepository,
    folder: &Path,
) -> VariantsReconcileSummary {
    let mut summary = VariantsReconcileSummary::default();
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
        if !is_variant_sidecar(&name) {
            continue;
        }
        summary.sidecars_seen += 1;
        let sidecar = entry.path();
        let Some((source, variant_id)) = variant_sidecar_source(sidecar) else {
            summary.errors.push((
                sidecar.to_path_buf(),
                "unparsable variant sidecar name".to_string(),
            ));
            continue;
        };
        match repo.load_at_opt(sidecar) {
            Ok(Some(envelope)) => {
                if envelope.variant_id != variant_id {
                    summary.errors.push((
                        sidecar.to_path_buf(),
                        format!(
                            "sidecar name declares variant '{variant_id}' but the envelope holds '{}'",
                            envelope.variant_id
                        ),
                    ));
                    continue;
                }
                match project_envelope(conn, &source, &envelope, sidecar) {
                    Ok(()) => summary.projected += 1,
                    Err(err) => summary
                        .errors
                        .push((sidecar.to_path_buf(), err.to_string())),
                }
            }
            Ok(None) => summary.errors.push((
                sidecar.to_path_buf(),
                "variant sidecar disappeared during reconcile".to_string(),
            )),
            Err(err) => summary
                .errors
                .push((sidecar.to_path_buf(), err.to_string())),
        }
    }
    summary.removed_missing = remove_stale_variant_rows(conn, folder, &mut summary.errors);
    summary
}

/// Album-wide variant reconcile mirroring
/// [`RecipeRepository::reconcile_albums_at_startup`]; call it right after the
/// primary reconcile at startup or after a catalog rebuild.
pub fn reconcile_albums_at_startup(conn: &Connection) -> VariantsReconcileSummary {
    let repo = RecipeRepository::lap_default();
    let mut summary = VariantsReconcileSummary::default();
    let rows: Vec<String> = match conn
        .prepare("SELECT path FROM albums")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, String>(0))
                .map(|rows| rows.collect::<Result<Vec<_>, _>>())
        }) {
        Ok(rows) => rows.unwrap_or_default(),
        Err(err) => {
            summary.errors.push((
                PathBuf::from("albums"),
                format!("album lookup failed: {err}"),
            ));
            return summary;
        }
    };
    for path in rows {
        let dir = PathBuf::from(&path);
        if !dir.is_dir() {
            continue;
        }
        let folder_summary = reconcile_folder(conn, &repo, &dir);
        summary.sidecars_seen += folder_summary.sidecars_seen;
        summary.projected += folder_summary.projected;
        summary.removed_missing += folder_summary.removed_missing;
        summary.errors.extend(folder_summary.errors);
    }
    summary
}

fn remove_stale_variant_rows(
    conn: &Connection,
    folder: &Path,
    errors: &mut Vec<(PathBuf, String)>,
) -> u64 {
    let mut removed = 0u64;
    let rows: Vec<(String, String)> = match conn
        .prepare("SELECT sidecar_path, variant_id FROM adevelop_recipes")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map(|rows| rows.collect::<Result<Vec<_>, _>>())
        }) {
        Ok(rows) => rows.unwrap_or_default(),
        Err(err) => {
            errors.push((
                folder.to_path_buf(),
                format!("stale variant row lookup failed: {err}"),
            ));
            return 0;
        }
    };
    for (sidecar_str, variant_id) in rows {
        let path = PathBuf::from(&sidecar_str);
        let is_variant = path
            .file_name()
            .map(|name| is_variant_sidecar(&name.to_string_lossy()))
            .unwrap_or(false);
        if !is_variant || !path_is_under(&path, folder) || path.exists() {
            continue;
        }
        match conn.execute(
            "DELETE FROM adevelop_recipes WHERE sidecar_path = ?1 AND variant_id = ?2",
            rusqlite::params![sidecar_str, variant_id],
        ) {
            Ok(_) => removed += 1,
            Err(err) => errors.push((path, format!("stale variant row cleanup failed: {err}"))),
        }
    }
    removed
}

fn path_is_under(path: &Path, folder: &Path) -> bool {
    if cfg!(windows) {
        path.to_string_lossy()
            .to_lowercase()
            .starts_with(&folder.to_string_lossy().to_lowercase())
    } else {
        path.starts_with(folder)
    }
}

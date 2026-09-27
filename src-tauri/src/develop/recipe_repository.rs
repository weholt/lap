use rapidraw_edit_model::migrate::parse_envelope_value;
use rapidraw_edit_model::{ModelError, Recipe, RecipeEnvelope, SCHEMA_VERSION, parse_envelope};
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultPoint {
    BeforeTempWrite,
    AfterTempWrite,
    BeforeFlush,
    AfterFlush,
    BeforeReplace,
    AfterReplace,
    BeforeAck,
    BeforeProjection,
    AfterProjection,
}

impl FaultPoint {
    pub fn parse(value: &str) -> Option<FaultPoint> {
        match value {
            "BeforeTempWrite" => Some(FaultPoint::BeforeTempWrite),
            "AfterTempWrite" => Some(FaultPoint::AfterTempWrite),
            "BeforeFlush" => Some(FaultPoint::BeforeFlush),
            "AfterFlush" => Some(FaultPoint::AfterFlush),
            "BeforeReplace" => Some(FaultPoint::BeforeReplace),
            "AfterReplace" => Some(FaultPoint::AfterReplace),
            "BeforeAck" => Some(FaultPoint::BeforeAck),
            "BeforeProjection" => Some(FaultPoint::BeforeProjection),
            "AfterProjection" => Some(FaultPoint::AfterProjection),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            FaultPoint::BeforeTempWrite => "BeforeTempWrite",
            FaultPoint::AfterTempWrite => "AfterTempWrite",
            FaultPoint::BeforeFlush => "BeforeFlush",
            FaultPoint::AfterFlush => "AfterFlush",
            FaultPoint::BeforeReplace => "BeforeReplace",
            FaultPoint::AfterReplace => "AfterReplace",
            FaultPoint::BeforeAck => "BeforeAck",
            FaultPoint::BeforeProjection => "BeforeProjection",
            FaultPoint::AfterProjection => "AfterProjection",
        }
    }
}

#[derive(Debug, Clone)]
pub enum FaultAction {
    Error(String),
    AbortProcess(i32),
}

#[derive(Debug, Clone)]
pub struct FaultInjection {
    pub point: FaultPoint,
    pub action: FaultAction,
}

fn hit_fault(fault: Option<&FaultInjection>, point: FaultPoint) -> Result<(), RecipeRepoError> {
    if let Some(injection) = fault
        && injection.point == point
    {
        return match &injection.action {
            FaultAction::Error(message) => Err(RecipeRepoError::Injected {
                point,
                message: message.clone(),
            }),
            FaultAction::AbortProcess(code) => std::process::exit(*code),
        };
    }
    Ok(())
}

#[derive(Debug)]
pub enum RecipeRepoError {
    Io {
        context: String,
        path: PathBuf,
        source: std::io::Error,
    },
    CorruptSidecar {
        path: PathBuf,
        detail: String,
    },
    UnsupportedSchema {
        path: PathBuf,
        found: Option<u32>,
        supported_max: u32,
        preserved: serde_json::Value,
    },
    RevisionConflict {
        path: PathBuf,
        expected: u64,
        current: Option<u64>,
    },
    IdentityMismatch {
        path: PathBuf,
        detail: String,
    },
    SourceFingerprintMismatch {
        path: PathBuf,
        expected: String,
        found: String,
    },
    InvalidRevision {
        path: PathBuf,
        detail: String,
    },
    ReadOnlyDestination {
        path: PathBuf,
    },
    Projection {
        detail: String,
    },
    Model {
        path: PathBuf,
        source: rapidraw_edit_model::ModelError,
    },
    Injected {
        point: FaultPoint,
        message: String,
    },
}

impl fmt::Display for RecipeRepoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecipeRepoError::Io {
                context,
                path,
                source,
            } => {
                write!(f, "{context} ({}): {source}", path.display())
            }
            RecipeRepoError::CorruptSidecar { path, detail } => write!(
                f,
                "sidecar at {} is corrupt and was left untouched: {detail}",
                path.display()
            ),
            RecipeRepoError::UnsupportedSchema {
                path,
                found,
                supported_max,
                ..
            } => write!(
                f,
                "sidecar at {} declares schema version {found:?}, newer than the highest supported version {supported_max}; payload preserved, not reset",
                path.display()
            ),
            RecipeRepoError::RevisionConflict {
                path,
                expected,
                current,
            } => write!(
                f,
                "sidecar revision conflict at {}: expected revision {expected:?} but the durable sidecar holds {current:?}",
                path.display()
            ),
            RecipeRepoError::IdentityMismatch { path, detail } => {
                write!(
                    f,
                    "recipe identity mismatch at {}: {detail}",
                    path.display()
                )
            }
            RecipeRepoError::SourceFingerprintMismatch {
                path,
                expected,
                found,
            } => write!(
                f,
                "source fingerprint mismatch at {}: sidecar was written for source {expected} but the host supplied {found}",
                path.display()
            ),
            RecipeRepoError::InvalidRevision { path, detail } => {
                write!(f, "invalid revision at {}: {detail}", path.display())
            }
            RecipeRepoError::ReadOnlyDestination { path } => write!(
                f,
                "destination {} is read-only; save failed and existing data was left untouched",
                path.display()
            ),
            RecipeRepoError::Projection { detail } => {
                write!(f, "catalog projection failed: {detail}")
            }
            RecipeRepoError::Model { path, source } => {
                write!(f, "recipe model error at {}: {source}", path.display())
            }
            RecipeRepoError::Injected { point, message } => {
                write!(f, "injected fault at {point:?}: {message}")
            }
        }
    }
}

impl std::error::Error for RecipeRepoError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitReceipt {
    pub revision: u64,
    pub content_hash: String,
    pub sidecar_path: PathBuf,
    pub previous_revision: Option<u64>,
    pub projection_applied: bool,
    pub projection_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileOutcome {
    Projected { revision: u64 },
    RemovedMissingSidecar,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileSummary {
    pub sidecars_seen: u64,
    pub projected: u64,
    pub removed_missing: u64,
    pub errors: Vec<(PathBuf, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionRow {
    pub sidecar_path: String,
    pub variant_id: String,
    pub file_id: Option<i64>,
    pub revision: u64,
    pub schema_version: u32,
    pub source_fingerprint: String,
    pub content_hash: String,
    pub is_edited: bool,
    pub updated_at: i64,
}

pub fn is_edited_envelope(envelope: &RecipeEnvelope) -> bool {
    envelope.recipe != Recipe::default()
        || !envelope.resources.is_empty()
        || !envelope.unsupported.is_empty()
}

fn write_lock_for(sidecar: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let registry = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = registry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    map.entry(lock_key(sidecar))
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn lock_key(sidecar: &Path) -> PathBuf {
    let parent = sidecar.parent().unwrap_or(Path::new("."));
    let canonical_parent = parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf());
    canonical_parent.join(sidecar.file_name().unwrap_or_default())
}

fn read_sidecar_raw(sidecar: &Path) -> Result<Option<Vec<u8>>, RecipeRepoError> {
    match fs::read(sidecar) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(RecipeRepoError::Io {
            context: "reading sidecar".to_string(),
            path: sidecar.to_path_buf(),
            source: err,
        }),
    }
}

fn parse_sidecar_bytes(path: &Path, bytes: &[u8]) -> Result<RecipeEnvelope, RecipeRepoError> {
    let text =
        String::from_utf8(bytes.to_vec()).map_err(|err| RecipeRepoError::CorruptSidecar {
            path: path.to_path_buf(),
            detail: format!("sidecar is not valid UTF-8: {err}"),
        })?;
    match parse_envelope(&text) {
        Ok(envelope) => Ok(envelope),
        Err(ModelError::UnsupportedSchema {
            found,
            supported_max,
            preserved,
        }) => Err(RecipeRepoError::UnsupportedSchema {
            path: path.to_path_buf(),
            found,
            supported_max,
            preserved,
        }),
        Err(other) => Err(RecipeRepoError::CorruptSidecar {
            path: path.to_path_buf(),
            detail: other.to_string(),
        }),
    }
}

fn read_sidecar_envelope(
    sidecar: &Path,
) -> Result<Option<(Vec<u8>, RecipeEnvelope)>, RecipeRepoError> {
    match read_sidecar_raw(sidecar)? {
        Some(bytes) => {
            let envelope = parse_sidecar_bytes(sidecar, &bytes)?;
            Ok(Some((bytes, envelope)))
        }
        None => Ok(None),
    }
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

fn write_bytes_and_sync(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn replace_atomic(temp: &Path, target: &Path) -> std::io::Result<()> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match fs::rename(temp, target) {
            Ok(()) => return Ok(()),
            Err(err) if attempt < 5 && transient_replace_error(&err) => {
                std::thread::sleep(Duration::from_millis(10 * u64::from(attempt)));
            }
            Err(err) => return Err(err),
        }
    }
}

fn transient_replace_error(err: &std::io::Error) -> bool {
    if !cfg!(windows) {
        return false;
    }
    matches!(err.kind(), std::io::ErrorKind::PermissionDenied)
}

#[cfg(unix)]
fn sync_parent_dir_best_effort(sidecar: &Path) {
    if let Some(parent) = sidecar.parent() {
        if let Ok(handle) = File::open(parent) {
            let _ = handle.sync_all();
        }
    }
}

#[cfg(not(unix))]
fn sync_parent_dir_best_effort(_sidecar: &Path) {}

#[derive(Clone)]
pub struct RecipeRepository {
    engine_version: String,
}

impl RecipeRepository {
    pub fn new(engine_version: impl Into<String>) -> Self {
        Self {
            engine_version: engine_version.into(),
        }
    }

    pub fn lap_default() -> Self {
        Self::new(format!(
            "lap/{}/rapidraw-edit-model/{}",
            env!("CARGO_PKG_VERSION"),
            rapidraw_edit_model::MODEL_VERSION
        ))
    }

    pub fn engine_version(&self) -> &str {
        &self.engine_version
    }

    pub fn sidecar_path(source: &Path) -> PathBuf {
        let mut name = source
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        name.push(".lapedit.json");
        source.with_file_name(name)
    }

    pub fn previous_sidecar_path(source: &Path) -> PathBuf {
        let mut sidecar = Self::sidecar_path(source).into_os_string();
        sidecar.push(".prev");
        PathBuf::from(sidecar)
    }

    pub fn new_envelope(
        &self,
        asset_id: &str,
        variant_id: &str,
        source_fingerprint: &str,
    ) -> RecipeEnvelope {
        RecipeEnvelope::new(
            &self.engine_version,
            asset_id,
            variant_id,
            source_fingerprint,
        )
    }

    pub fn load_opt(&self, source: &Path) -> Result<Option<RecipeEnvelope>, RecipeRepoError> {
        let sidecar = Self::sidecar_path(source);
        match read_sidecar_raw(&sidecar)? {
            Some(bytes) => Ok(Some(parse_sidecar_bytes(&sidecar, &bytes)?)),
            None => Ok(None),
        }
    }

    pub fn load(&self, source: &Path) -> Result<RecipeEnvelope, RecipeRepoError> {
        match self.load_opt(source)? {
            Some(envelope) => Ok(envelope),
            None => Err(RecipeRepoError::Io {
                context: "sidecar does not exist".to_string(),
                path: Self::sidecar_path(source),
                source: std::io::Error::from(std::io::ErrorKind::NotFound),
            }),
        }
    }

    pub fn load_previous_opt(
        &self,
        source: &Path,
    ) -> Result<Option<RecipeEnvelope>, RecipeRepoError> {
        let previous = Self::previous_sidecar_path(source);
        match read_sidecar_raw(&previous)? {
            Some(bytes) => Ok(Some(parse_sidecar_bytes(&previous, &bytes)?)),
            None => Ok(None),
        }
    }

    pub fn current_revision(&self, source: &Path) -> Result<Option<u64>, RecipeRepoError> {
        Ok(self.load_opt(source)?.map(|envelope| envelope.revision))
    }

    pub fn commit(
        &self,
        source: &Path,
        expected_revision: u64,
        mut envelope: RecipeEnvelope,
        conn: Option<&Connection>,
        fault: Option<FaultInjection>,
    ) -> Result<CommitReceipt, RecipeRepoError> {
        let sidecar = Self::sidecar_path(source);
        let lock = write_lock_for(&sidecar);
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        let existing = read_sidecar_envelope(&sidecar)?;
        let previous_revision = existing.as_ref().map(|(_, env)| env.revision);

        if let Some((_, current)) = &existing {
            if current.revision != expected_revision {
                return Err(RecipeRepoError::RevisionConflict {
                    path: sidecar.clone(),
                    expected: expected_revision,
                    current: previous_revision,
                });
            }
            if current.asset_id != envelope.asset_id || current.variant_id != envelope.variant_id {
                return Err(RecipeRepoError::IdentityMismatch {
                    path: sidecar.clone(),
                    detail: format!(
                        "existing sidecar holds asset '{}' variant '{}'; the commit supplied asset '{}' variant '{}'",
                        current.asset_id,
                        current.variant_id,
                        envelope.asset_id,
                        envelope.variant_id
                    ),
                });
            }
            if current.source_fingerprint != envelope.source_fingerprint {
                return Err(RecipeRepoError::SourceFingerprintMismatch {
                    path: sidecar.clone(),
                    expected: current.source_fingerprint.clone(),
                    found: envelope.source_fingerprint.clone(),
                });
            }
        } else if expected_revision != 0 {
            return Err(RecipeRepoError::RevisionConflict {
                path: sidecar.clone(),
                expected: expected_revision,
                current: None,
            });
        }

        let new_revision =
            expected_revision
                .checked_add(1)
                .ok_or_else(|| RecipeRepoError::InvalidRevision {
                    path: sidecar.clone(),
                    detail: format!("revision {expected_revision} cannot be incremented"),
                })?;
        envelope.revision = new_revision;
        envelope.schema_version = SCHEMA_VERSION;
        envelope.engine_version = self.engine_version.clone();

        let bytes = envelope
            .to_canonical_json()
            .map_err(|err| RecipeRepoError::Model {
                path: sidecar.clone(),
                source: err,
            })?;
        let value = serde_json::to_value(&envelope).map_err(|err| RecipeRepoError::Model {
            path: sidecar.clone(),
            source: err.into(),
        })?;
        parse_envelope_value(value).map_err(|err| RecipeRepoError::Model {
            path: sidecar.clone(),
            source: err,
        })?;

        if sidecar.exists() {
            let metadata = fs::metadata(&sidecar).map_err(|err| RecipeRepoError::Io {
                context: "inspecting sidecar before replacement".to_string(),
                path: sidecar.clone(),
                source: err,
            })?;
            if metadata.permissions().readonly() {
                return Err(RecipeRepoError::ReadOnlyDestination {
                    path: sidecar.clone(),
                });
            }
        }

        hit_fault(fault.as_ref(), FaultPoint::BeforeTempWrite)?;

        let temp = unique_temp_path(&sidecar);
        let outcome = self.commit_after_temp(
            source,
            &sidecar,
            &temp,
            &bytes,
            existing,
            envelope,
            new_revision,
            previous_revision,
            conn,
            fault.as_ref(),
        );
        if outcome.is_err() {
            let _ = fs::remove_file(&temp);
        }
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_after_temp(
        &self,
        source: &Path,
        sidecar: &Path,
        temp: &Path,
        bytes: &[u8],
        existing: Option<(Vec<u8>, RecipeEnvelope)>,
        envelope: RecipeEnvelope,
        new_revision: u64,
        previous_revision: Option<u64>,
        conn: Option<&Connection>,
        fault: Option<&FaultInjection>,
    ) -> Result<CommitReceipt, RecipeRepoError> {
        let mut file = File::create(temp).map_err(|err| RecipeRepoError::Io {
            context: "creating temporary sidecar sibling".to_string(),
            path: temp.to_path_buf(),
            source: err,
        })?;
        file.write_all(bytes).map_err(|err| RecipeRepoError::Io {
            context: "writing temporary sidecar sibling".to_string(),
            path: temp.to_path_buf(),
            source: err,
        })?;

        hit_fault(fault, FaultPoint::AfterTempWrite)?;
        hit_fault(fault, FaultPoint::BeforeFlush)?;

        file.sync_all().map_err(|err| RecipeRepoError::Io {
            context: "flushing temporary sidecar sibling".to_string(),
            path: temp.to_path_buf(),
            source: err,
        })?;
        drop(file);

        hit_fault(fault, FaultPoint::AfterFlush)?;

        if let Some((previous_bytes, _)) = &existing {
            let previous_path = Self::previous_sidecar_path(source);
            let previous_temp = unique_temp_path(&previous_path);
            write_bytes_and_sync(&previous_temp, previous_bytes).map_err(|err| {
                RecipeRepoError::Io {
                    context: "retaining previous sidecar revision".to_string(),
                    path: previous_temp.clone(),
                    source: err,
                }
            })?;
            fs::rename(&previous_temp, &previous_path).map_err(|err| RecipeRepoError::Io {
                context: "placing previous sidecar revision".to_string(),
                path: previous_path.clone(),
                source: err,
            })?;
        }

        hit_fault(fault, FaultPoint::BeforeReplace)?;

        replace_atomic(temp, sidecar).map_err(|err| RecipeRepoError::Io {
            context: "atomically replacing sidecar".to_string(),
            path: sidecar.to_path_buf(),
            source: err,
        })?;
        sync_parent_dir_best_effort(sidecar);

        hit_fault(fault, FaultPoint::AfterReplace)?;
        hit_fault(fault, FaultPoint::BeforeAck)?;

        hit_fault(fault, FaultPoint::BeforeProjection)?;
        let (projection_applied, projection_error) = match conn {
            Some(conn) => match project_envelope(conn, source, &envelope, sidecar) {
                Ok(()) => (true, None),
                Err(err) => (false, Some(err.to_string())),
            },
            None => (false, None),
        };
        hit_fault(fault, FaultPoint::AfterProjection)?;

        let content_hash = envelope
            .content_hash()
            .map_err(|err| RecipeRepoError::Model {
                path: sidecar.to_path_buf(),
                source: err,
            })?;

        Ok(CommitReceipt {
            revision: new_revision,
            content_hash,
            sidecar_path: sidecar.to_path_buf(),
            previous_revision,
            projection_applied,
            projection_error,
        })
    }

    pub fn commit_sidecar(
        &self,
        source: &Path,
        expected_revision: u64,
        envelope: RecipeEnvelope,
    ) -> Result<CommitReceipt, RecipeRepoError> {
        self.commit(source, expected_revision, envelope, None, None)
    }

    pub fn reconcile(
        &self,
        conn: &Connection,
        source: &Path,
    ) -> Result<ReconcileOutcome, RecipeRepoError> {
        let sidecar = Self::sidecar_path(source);
        match self.load_opt(source)? {
            None => {
                let sidecar_str = sidecar.to_string_lossy().to_string();
                conn.execute(
                    "DELETE FROM adevelop_recipes WHERE sidecar_path = ?1",
                    [sidecar_str.as_str()],
                )
                .map_err(|err| RecipeRepoError::Projection {
                    detail: format!("projection cleanup failed: {err}"),
                })?;
                Ok(ReconcileOutcome::RemovedMissingSidecar)
            }
            Some(envelope) => {
                project_envelope(conn, source, &envelope, &sidecar)?;
                Ok(ReconcileOutcome::Projected {
                    revision: envelope.revision,
                })
            }
        }
    }

    pub fn reconcile_folder(
        &self,
        conn: &Connection,
        folder: &Path,
    ) -> Result<ReconcileSummary, RecipeRepoError> {
        let mut summary = ReconcileSummary::default();
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
            let source =
                sidecar.with_file_name(name.strip_suffix(".lapedit.json").unwrap_or(&name));
            match self.reconcile(conn, &source) {
                Ok(ReconcileOutcome::Projected { .. }) => summary.projected += 1,
                Ok(ReconcileOutcome::RemovedMissingSidecar) => summary.removed_missing += 1,
                Err(err) => summary
                    .errors
                    .push((sidecar.to_path_buf(), err.to_string())),
            }
        }
        Ok(summary)
    }

    pub fn reconcile_albums_at_startup(
        &self,
        conn: &Connection,
    ) -> Result<ReconcileSummary, RecipeRepoError> {
        let mut stmt =
            conn.prepare("SELECT path FROM albums")
                .map_err(|err| RecipeRepoError::Projection {
                    detail: format!("album lookup failed: {err}"),
                })?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|err| RecipeRepoError::Projection {
                detail: format!("album path query failed: {err}"),
            })?;

        let mut summary = ReconcileSummary::default();
        for row in rows {
            let path = row.map_err(|err| RecipeRepoError::Projection {
                detail: format!("album path read failed: {err}"),
            })?;
            let dir = PathBuf::from(&path);
            if !dir.is_dir() {
                continue;
            }
            match self.reconcile_folder(conn, &dir) {
                Ok(folder_summary) => {
                    summary.sidecars_seen += folder_summary.sidecars_seen;
                    summary.projected += folder_summary.projected;
                    summary.removed_missing += folder_summary.removed_missing;
                    summary.errors.extend(folder_summary.errors);
                }
                Err(err) => summary.errors.push((dir, err.to_string())),
            }
        }
        Ok(summary)
    }

    pub fn resolve_catalog_file_id(
        conn: &Connection,
        source: &Path,
    ) -> Result<Option<i64>, RecipeRepoError> {
        let parent = source
            .parent()
            .unwrap_or(Path::new(""))
            .to_string_lossy()
            .to_string();
        let name = source
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let exact = conn
            .query_row(
                "SELECT f.id FROM afiles f JOIN afolders d ON d.id = f.folder_id
                 WHERE d.path = ?1 AND f.name = ?2 LIMIT 1",
                rusqlite::params![parent, name],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|err| RecipeRepoError::Projection {
                detail: format!("catalog lookup failed: {err}"),
            })?;
        if exact.is_some() {
            return Ok(exact);
        }

        conn.query_row(
            "SELECT f.id FROM afiles f JOIN afolders d ON d.id = f.folder_id
             WHERE d.path = ?1 COLLATE NOCASE AND f.name = ?2 COLLATE NOCASE LIMIT 1",
            rusqlite::params![parent, name],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|err| RecipeRepoError::Projection {
            detail: format!("catalog lookup failed: {err}"),
        })
    }

    pub fn projection_row(
        conn: &Connection,
        sidecar_path: &Path,
        variant_id: &str,
    ) -> Result<Option<ProjectionRow>, RecipeRepoError> {
        let sidecar_str = sidecar_path.to_string_lossy().to_string();
        conn.query_row(
            "SELECT sidecar_path, variant_id, file_id, revision, schema_version,
                    source_fingerprint, content_hash, is_edited, updated_at
             FROM adevelop_recipes
             WHERE sidecar_path = ?1 AND variant_id = ?2",
            rusqlite::params![sidecar_str, variant_id],
            |row| {
                Ok(ProjectionRow {
                    sidecar_path: row.get(0)?,
                    variant_id: row.get(1)?,
                    file_id: row.get(2)?,
                    revision: row.get::<_, i64>(3)?.max(0) as u64,
                    schema_version: row.get::<_, i64>(4)?.max(0) as u32,
                    source_fingerprint: row.get(5)?,
                    content_hash: row.get(6)?,
                    is_edited: row.get::<_, i64>(7)? != 0,
                    updated_at: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(|err| RecipeRepoError::Projection {
            detail: format!("projection lookup failed: {err}"),
        })
    }
}

fn project_envelope(
    conn: &Connection,
    source: &Path,
    envelope: &RecipeEnvelope,
    sidecar: &Path,
) -> Result<(), RecipeRepoError> {
    let sidecar_str = sidecar.to_string_lossy().to_string();
    let file_id = RecipeRepository::resolve_catalog_file_id(conn, source)?;
    let revision = i64::try_from(envelope.revision).map_err(|_| RecipeRepoError::Projection {
        detail: format!(
            "revision {} exceeds the SQLite integer range",
            envelope.revision
        ),
    })?;
    let content_hash = envelope
        .content_hash()
        .map_err(|err| RecipeRepoError::Projection {
            detail: format!("content hash failed: {err}"),
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
            i64::from(is_edited_envelope(envelope)),
            updated_at,
        ],
    )
    .map_err(|err| RecipeRepoError::Projection {
        detail: format!("projection upsert failed: {err}"),
    })?;
    Ok(())
}

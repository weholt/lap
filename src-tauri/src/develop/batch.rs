//! Bounded batch development (lap-952 / TASK-503).
//!
//! Spec refs A1/A3/A7/A12: batch recipe application and batch export make one
//! explicit per-asset commit (or one explicit export) per item, surface every
//! per-item failure visibly, honour cancellation between items, stay bounded
//! in planning (max items) and memory (sequential processing, explicit export
//! edge bounds), and never count failed or skipped assets as successfully
//! exported.

use rapidraw_develop::CancelToken;
use serde::Serialize;
use std::path::PathBuf;

use super::export::{
    AssetExportInput, ExportCancelSlot, ExportCompletion, ExportEncoder, ExportReceipt,
    ExportSettings, export_developed_with,
};
use super::recipe_repository::{FaultInjection, RecipeRepository};
use super::sessions::DevelopService;
use rapidraw_edit_model::{Recipe, sha256_hex};
use rusqlite::Connection;

// ---------------------------------------------------------------------------
// Planning bounds
// ---------------------------------------------------------------------------

pub const DEFAULT_BATCH_MAX_ITEMS: usize = 256;

/// Planning bound shared by apply and export batches: a batch may never
/// contain more than `max_items` entries. Oversized requests are rejected
/// before any asset is touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchLimits {
    pub max_items: usize,
}

impl Default for BatchLimits {
    fn default() -> Self {
        Self {
            max_items: DEFAULT_BATCH_MAX_ITEMS,
        }
    }
}

/// Export-specific bounds: `max_edge` caps the longest rendered edge of every
/// item so a batch can never allocate unbounded decode/render buffers
/// (spec A12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchExportLimits {
    pub max_items: usize,
    pub max_edge: u32,
}

impl Default for BatchExportLimits {
    fn default() -> Self {
        Self {
            max_items: DEFAULT_BATCH_MAX_ITEMS,
            max_edge: 4096,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchError {
    TooManyItems { requested: usize, limit: usize },
    InvalidInput(String),
}

impl std::fmt::Display for BatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BatchError::TooManyItems { requested, limit } => write!(
                f,
                "batch of {requested} items exceeds the configured limit of {limit}"
            ),
            BatchError::InvalidInput(message) => write!(f, "invalid batch request: {message}"),
        }
    }
}

impl std::error::Error for BatchError {}

impl BatchLimits {
    fn validate(&self, requested: usize) -> Result<(), BatchError> {
        if requested > self.max_items {
            Err(BatchError::TooManyItems {
                requested,
                limit: self.max_items,
            })
        } else {
            Ok(())
        }
    }
}

impl BatchExportLimits {
    fn validate(&self, requested: usize) -> Result<(), BatchError> {
        if requested > self.max_items {
            Err(BatchError::TooManyItems {
                requested,
                limit: self.max_items,
            })
        } else {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Item inputs
// ---------------------------------------------------------------------------

/// One per-asset recipe application. Each item commits exactly one explicit
/// recipe revision to that asset's durable sidecar (CAS against
/// `expected_revision`, or the durable revision when `None`).
#[derive(Debug, Clone)]
pub struct BatchApplyItem {
    pub asset_id: String,
    pub variant_id: String,
    pub source_path: PathBuf,
    /// Full replacement recipe for the targeted variant.
    pub recipe: Recipe,
    /// Explicit optimistic-concurrency expectation; `None` uses the durable
    /// revision read just before this item's commit.
    pub expected_revision: Option<u64>,
    /// Persistence fault-injection seam (tests/diagnostics; never exposed via IPC).
    pub fault: Option<FaultInjection>,
}

/// One per-asset export. Each item renders exactly the requested committed
/// revision and writes one derivative atomically. Source bytes are read and
/// hashed per item inside the sequential loop so at most one RAW file is
/// ever held in memory (spec A12); missing media is a per-item failure.
#[derive(Debug, Clone)]
pub struct BatchExportItem {
    pub asset_id: String,
    pub variant_id: String,
    pub source_path: PathBuf,
    /// The committed revision the UI acknowledged before the batch started.
    pub requested_revision: u64,
    pub destination: PathBuf,
}

pub fn item_key(asset_id: &str, variant_id: &str) -> String {
    format!("{asset_id}:{variant_id}")
}

// ---------------------------------------------------------------------------
// Outcomes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum BatchItemStatus {
    /// One explicit durable commit for this asset/variant.
    Applied {
        revision: u64,
        content_hash: String,
        projection_applied: bool,
        projection_error: Option<String>,
    },
    /// One explicit derivative file rendered and written for this asset/variant.
    Exported { receipt: ExportReceipt },
    /// A visible per-item failure; never counted as applied/exported.
    Failed { error: String },
    /// Not attempted (the batch was cancelled before this item).
    Skipped { reason: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchItemResult {
    pub key: String,
    #[serde(flatten)]
    pub status: BatchItemStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchSummary {
    pub requested: usize,
    /// Items that completed successfully (applied or exported). Failed and
    /// skipped items are never counted here.
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub cancelled: bool,
    pub items: Vec<BatchItemResult>,
}

impl BatchSummary {
    fn new(requested: usize) -> Self {
        Self {
            requested,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            cancelled: false,
            items: Vec::new(),
        }
    }
}

/// Per-item progress callback: `(1-based index, total, result so far)`.
/// The host surfaces batch progress and can cancel through its own token.
pub type BatchProgress<'a> = &'a (dyn Fn(usize, usize, &BatchItemResult) + Send + Sync);

// ---------------------------------------------------------------------------
// Batch recipe application
// ---------------------------------------------------------------------------

pub fn apply_recipes(
    repo: &RecipeRepository,
    conn: Option<&Connection>,
    items: &[BatchApplyItem],
    cancel: &CancelToken,
    limits: BatchLimits,
    progress: BatchProgress<'_>,
) -> Result<BatchSummary, BatchError> {
    limits.validate(items.len())?;
    let mut summary = BatchSummary::new(items.len());
    let total = items.len();

    for (index, item) in items.iter().enumerate() {
        let result = if cancel.is_cancelled() {
            summary.cancelled = true;
            BatchItemResult {
                key: item_key(&item.asset_id, &item.variant_id),
                status: BatchItemStatus::Skipped {
                    reason: "cancelled".to_string(),
                },
            }
        } else {
            apply_one(repo, conn, item)
        };
        match result.status {
            BatchItemStatus::Applied { .. } => summary.succeeded += 1,
            BatchItemStatus::Failed { .. } => summary.failed += 1,
            BatchItemStatus::Skipped { .. } => summary.skipped += 1,
            BatchItemStatus::Exported { .. } => unreachable!("apply batches never export"),
        }
        progress(index + 1, total, &result);
        summary.items.push(result);
    }
    Ok(summary)
}

fn apply_one(
    repo: &RecipeRepository,
    conn: Option<&Connection>,
    item: &BatchApplyItem,
) -> BatchItemResult {
    let key = item_key(&item.asset_id, &item.variant_id);
    let fail = |error: String| BatchItemResult {
        key: key.clone(),
        status: BatchItemStatus::Failed { error },
    };
    // Every batch item re-hashes the original source bytes and rejects
    // replaced media before writing anything (spec A1).
    let source_bytes = match std::fs::read(&item.source_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return fail(format!("source {}: {err}", item.source_path.display()));
        }
    };
    let fingerprint = sha256_hex(&source_bytes);

    let durable = match repo.load_opt(&item.source_path) {
        Ok(Some(envelope)) => envelope,
        Ok(None) => {
            return fail(format!(
                "no committed recipe sidecar for {}",
                item.source_path.display()
            ));
        }
        Err(err) => return fail(err.to_string()),
    };
    if durable.asset_id != item.asset_id || durable.variant_id != item.variant_id {
        return fail(format!(
            "sidecar holds asset '{}' variant '{}'; the batch item targets asset '{}' variant '{}'",
            durable.asset_id, durable.variant_id, item.asset_id, item.variant_id
        ));
    }
    if durable.source_fingerprint != fingerprint {
        return fail(format!(
            "source at {} was replaced after its recipe was written (fingerprint {}, found {})",
            item.source_path.display(),
            durable.source_fingerprint,
            fingerprint
        ));
    }

    let expected = item.expected_revision.unwrap_or(durable.revision);
    let mut envelope = durable;
    envelope.recipe = item.recipe.clone();

    match repo.commit(
        &item.source_path,
        expected,
        envelope,
        conn,
        item.fault.clone(),
    ) {
        Ok(receipt) => BatchItemResult {
            key,
            status: BatchItemStatus::Applied {
                revision: receipt.revision,
                content_hash: receipt.content_hash,
                projection_applied: receipt.projection_applied,
                projection_error: receipt.projection_error,
            },
        },
        Err(err) => fail(err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Batch export
// ---------------------------------------------------------------------------

pub fn export_developed_batch(
    service: &DevelopService,
    items: &[BatchExportItem],
    settings: ExportSettings,
    cancel: &CancelToken,
    limits: BatchExportLimits,
    progress: BatchProgress<'_>,
) -> Result<BatchSummary, BatchError> {
    let encoder = settings.encoder();
    export_developed_batch_with(
        service,
        items,
        settings,
        cancel,
        limits,
        progress,
        encoder.as_ref(),
    )
}

pub fn export_developed_batch_with(
    service: &DevelopService,
    items: &[BatchExportItem],
    settings: ExportSettings,
    cancel: &CancelToken,
    limits: BatchExportLimits,
    progress: BatchProgress<'_>,
    encoder: &dyn ExportEncoder,
) -> Result<BatchSummary, BatchError> {
    if settings.max_edge == Some(0) {
        return Err(BatchError::InvalidInput(
            "max_edge must be at least 1 (or omitted for the bounded batch cap)".to_string(),
        ));
    }
    limits.validate(items.len())?;
    let mut summary = BatchSummary::new(items.len());
    let total = items.len();

    for (index, item) in items.iter().enumerate() {
        let key = item_key(&item.asset_id, &item.variant_id);
        let result = if cancel.is_cancelled() {
            summary.cancelled = true;
            BatchItemResult {
                key,
                status: BatchItemStatus::Skipped {
                    reason: "cancelled".to_string(),
                },
            }
        } else {
            // Memory bound: every item's longest edge is capped (an omitted
            // edge uses the batch cap), and items are processed sequentially
            // so at most one decoded source file and frame are alive at a
            // time (spec A12).
            let mut item_settings = settings.clone();
            item_settings.destination = item.destination.clone();
            item_settings.max_edge = Some(
                settings
                    .max_edge
                    .unwrap_or(limits.max_edge)
                    .min(limits.max_edge),
            );
            let slot = ExportCancelSlot::new();
            let outcome = std::fs::read(&item.source_path).map(|source_bytes| {
                (
                    AssetExportInput {
                        asset_id: item.asset_id.clone(),
                        variant_id: item.variant_id.clone(),
                        source_path: item.source_path.clone(),
                        source_bytes,
                        requested_revision: item.requested_revision,
                    },
                    item_settings.clone(),
                )
            });
            match outcome {
                Ok((input, item_settings)) => {
                    match export_developed_with(
                        service,
                        input,
                        item_settings,
                        cancel,
                        &slot,
                        encoder,
                    ) {
                        Ok(ExportCompletion::Completed { receipt }) => BatchItemResult {
                            key,
                            status: BatchItemStatus::Exported { receipt },
                        },
                        Ok(ExportCompletion::Cancelled) => {
                            summary.cancelled = true;
                            BatchItemResult {
                                key,
                                status: BatchItemStatus::Skipped {
                                    reason: "cancelled".to_string(),
                                },
                            }
                        }
                        Err(err) => BatchItemResult {
                            key,
                            status: BatchItemStatus::Failed {
                                error: err.to_string(),
                            },
                        },
                    }
                }
                Err(err) => BatchItemResult {
                    key,
                    status: BatchItemStatus::Failed {
                        error: format!("source {}: {err}", item.source_path.display()),
                    },
                },
            }
        };
        match result.status {
            BatchItemStatus::Exported { .. } => summary.succeeded += 1,
            BatchItemStatus::Failed { .. } => summary.failed += 1,
            BatchItemStatus::Skipped { .. } => summary.skipped += 1,
            BatchItemStatus::Applied { .. } => unreachable!("export batches never commit recipes"),
        }
        progress(index + 1, total, &result);
        summary.items.push(result);
    }
    Ok(summary)
}

// ---------------------------------------------------------------------------
// Tests (RED before the implementation lands, GREEN after)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::export::{ExportFormat, ExportSettings, linear_to_rgba32f};
    use crate::develop::recipe_repository::{FaultInjection, RecipeRepository};
    use crate::develop::sessions::{DevelopConfig, DevelopService};
    use crate::t_migration::ensure_develop_projection;
    use rapidraw_develop::DevelopError as EngineError;
    use rapidraw_develop::session::{
        ExportFrame, ExportJob, ExportRenderer, PreviewFrame, PreviewJob, PreviewRenderer,
        RecipeStore, SessionManagerConfig,
    };
    use rapidraw_edit_model::{Recipe, RecipeEnvelope, sha256_hex};
    use rusqlite::Connection;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    // ---------------------------------------------------------------- fixtures

    fn fixture_bytes(name: &str) -> Vec<u8> {
        fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tests/fixtures/raw-development/synthetic")
                .join(name),
        )
        .expect("synthetic fixture")
    }

    fn gradient_bytes() -> Vec<u8> {
        fixture_bytes("dng-linear-gradient-64x48.dng")
    }

    fn wide_bytes() -> Vec<u8> {
        fixture_bytes("dng-linear-wide-5000x64.dng")
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "lap_develop_batch_{}_{}_{}",
            tag,
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn repo() -> RecipeRepository {
        RecipeRepository::lap_default()
    }

    /// One committed asset; returns (source path, durable revision).
    fn committed_asset(
        dir: &Path,
        name: &str,
        asset: &str,
        bytes: &[u8],
        exposure: f64,
    ) -> (PathBuf, u64) {
        let source = dir.join(name);
        fs::write(&source, bytes).unwrap();
        let repository = repo();
        let mut envelope = repository.new_envelope(asset, "default", &sha256_hex(bytes));
        envelope.recipe.exposure = exposure;
        let receipt = repository.commit(&source, 0, envelope, None, None).unwrap();
        (source, receipt.revision)
    }

    fn apply_item(source: &Path, asset: &str, exposure: f64) -> BatchApplyItem {
        BatchApplyItem {
            asset_id: asset.to_string(),
            variant_id: "default".to_string(),
            source_path: source.to_path_buf(),
            recipe: Recipe {
                exposure,
                ..Recipe::default()
            },
            expected_revision: None,
            fault: None,
        }
    }

    fn conn_with_projection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE albums (id INTEGER PRIMARY KEY, path TEXT NOT NULL);
             CREATE TABLE afolders (
                 id INTEGER PRIMARY KEY,
                 album_id INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
                 path TEXT NOT NULL
             );
             CREATE TABLE afiles (
                 id INTEGER PRIMARY KEY,
                 folder_id INTEGER NOT NULL REFERENCES afolders(id) ON DELETE CASCADE,
                 name TEXT NOT NULL
             );",
        )
        .unwrap();
        ensure_develop_projection(&conn).unwrap();
        conn
    }

    fn noop_progress(_index: usize, _total: usize, _result: &BatchItemResult) {}

    // ------------------------------------------------------------ test doubles

    struct NoopPreviewRenderer;
    impl PreviewRenderer for NoopPreviewRenderer {
        fn render(&self, _job: &PreviewJob) -> Result<PreviewFrame, EngineError> {
            Err(EngineError::Unsupported(
                "not used in batch tests".to_string(),
            ))
        }
    }

    struct NullStore;
    impl RecipeStore for NullStore {
        fn persist(&self, _envelope: &RecipeEnvelope) -> Result<(), String> {
            Ok(())
        }
    }

    /// Deterministic resize through the shared helper.
    struct EchoExportRenderer;
    impl ExportRenderer for EchoExportRenderer {
        fn render(&self, job: &ExportJob) -> Result<ExportFrame, EngineError> {
            let dynamic = linear_to_rgba32f(&job.original.image, job.max_edge)?;
            let rgba = dynamic.to_rgba8();
            let (width, height) = rgba.dimensions();
            Ok(ExportFrame {
                width,
                height,
                rgba8: rgba.into_raw(),
            })
        }
    }

    /// Renderer that fails for items whose recipe exposure is negative,
    /// letting tests mix success and injected GPU-style failure in one batch.
    struct FailNegativeExposureRenderer;
    impl ExportRenderer for FailNegativeExposureRenderer {
        fn render(&self, job: &ExportJob) -> Result<ExportFrame, EngineError> {
            if job.envelope.recipe.exposure < 0.0 {
                return Err(EngineError::Unsupported(
                    "injected renderer failure: device lost".to_string(),
                ));
            }
            let dynamic = linear_to_rgba32f(&job.original.image, job.max_edge)?;
            let rgba = dynamic.to_rgba8();
            let (width, height) = rgba.dimensions();
            Ok(ExportFrame {
                width,
                height,
                rgba8: rgba.into_raw(),
            })
        }
    }

    fn service_with(export_renderer: Arc<dyn ExportRenderer>) -> DevelopService {
        DevelopService::with_parts(
            DevelopConfig {
                sessions: SessionManagerConfig {
                    max_sessions: 4,
                    preview_workers: 1,
                    export_workers: 1,
                    max_queued_previews: 8,
                    max_queued_exports: 4,
                    max_cached_preview_results: 2,
                },
                max_preview_edge: 4096,
                max_cached_preview_bytes: 1024 * 1024,
            },
            Arc::new(NoopPreviewRenderer),
            export_renderer,
            Arc::new(NullStore),
        )
    }

    fn export_item(
        source: &Path,
        asset: &str,
        revision: u64,
        destination: PathBuf,
    ) -> BatchExportItem {
        BatchExportItem {
            asset_id: asset.to_string(),
            variant_id: "default".to_string(),
            source_path: source.to_path_buf(),
            requested_revision: revision,
            destination,
        }
    }

    fn png_settings() -> ExportSettings {
        ExportSettings {
            destination: PathBuf::new(),
            format: ExportFormat::Png,
            jpeg_quality: 90,
            max_edge: None,
        }
    }

    fn png_dimensions(path: &Path) -> (u32, u32) {
        let reader = image::ImageReader::open(path).expect("open exported file");
        reader.into_dimensions().expect("PNG dimensions")
    }

    // ------------------------------------------------------------------ tests

    #[test]
    fn apply_recipes_commits_each_asset_explicitly_with_per_item_receipts() {
        let dir = tmp_dir("apply-ok");
        let bytes = gradient_bytes();
        let (source_a, _) = committed_asset(&dir, "a.dng", "101", &bytes, 0.1);
        let (source_b, _) = committed_asset(&dir, "b.dng", "102", &bytes, 0.2);
        let (source_c, _) = committed_asset(&dir, "c.dng", "103", &bytes, 0.3);
        let sources_before: Vec<(PathBuf, Vec<u8>)> = [&source_a, &source_b, &source_c]
            .into_iter()
            .map(|p| (p.clone(), fs::read(p).unwrap()))
            .collect();
        let conn = conn_with_projection();

        let items = vec![
            apply_item(&source_a, "101", 0.5),
            apply_item(&source_b, "102", 0.6),
            apply_item(&source_c, "103", 0.7),
        ];
        let summary = apply_recipes(
            &repo(),
            Some(&conn),
            &items,
            &CancelToken::pair().1,
            BatchLimits::default(),
            &noop_progress,
        )
        .unwrap();

        assert_eq!(summary.requested, 3);
        assert_eq!(summary.succeeded, 3);
        assert_eq!(summary.failed, 0);
        assert_eq!(summary.skipped, 0);
        assert!(!summary.cancelled);
        for (index, result) in summary.items.iter().enumerate() {
            match &result.status {
                BatchItemStatus::Applied {
                    revision,
                    content_hash,
                    projection_applied,
                    ..
                } => {
                    assert_eq!(*revision, 2, "each asset commits its own explicit revision");
                    assert!(!content_hash.is_empty());
                    assert!(*projection_applied);
                    let row = RecipeRepository::projection_row(
                        &conn,
                        &RecipeRepository::sidecar_path(&items[index].source_path),
                        "default",
                    )
                    .unwrap()
                    .unwrap();
                    assert_eq!(row.revision, 2, "projection follows the per-asset commit");
                }
                other => panic!("expected Applied, got {other:?}"),
            }
        }
        // The originals are immutable (A1).
        for (path, before) in &sources_before {
            assert_eq!(&fs::read(path).unwrap(), before);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_recipes_reports_partial_failures_and_never_counts_them_as_applied() {
        let dir = tmp_dir("apply-mixed");
        let bytes = gradient_bytes();
        let (source_ok, _) = committed_asset(&dir, "ok.dng", "201", &bytes, 0.1);
        let (source_stale, stale_revision) = committed_asset(&dir, "stale.dng", "202", &bytes, 0.1);
        let (source_missing, _) = committed_asset(&dir, "gone.dng", "203", &bytes, 0.1);
        let missing_sidecar = RecipeRepository::sidecar_path(&source_missing);
        let missing_sidecar_before = fs::read(&missing_sidecar).unwrap();
        // Simulate missing media for the third item only.
        fs::remove_file(&source_missing).unwrap();

        let mut stale_item = apply_item(&source_stale, "202", 0.9);
        stale_item.expected_revision = Some(stale_revision + 7);

        let items = vec![
            apply_item(&source_ok, "201", 0.5),
            stale_item,
            apply_item(&source_missing, "203", 0.5),
        ];
        let summary = apply_recipes(
            &repo(),
            None,
            &items,
            &CancelToken::pair().1,
            BatchLimits::default(),
            &noop_progress,
        )
        .unwrap();

        assert_eq!(summary.succeeded, 1, "only the healthy item is applied");
        assert_eq!(summary.failed, 2, "both failures are visible");
        assert_eq!(summary.skipped, 0);
        assert!(matches!(
            summary.items[0].status,
            BatchItemStatus::Applied { revision: 2, .. }
        ));
        let BatchItemStatus::Failed { error: stale_error } = &summary.items[1].status else {
            panic!("second item must fail, got {:?}", summary.items[1].status);
        };
        assert!(
            stale_error.contains("revision"),
            "stale revision is visible: {stale_error}"
        );
        let BatchItemStatus::Failed {
            error: missing_error,
        } = &summary.items[2].status
        else {
            panic!("third item must fail, got {:?}", summary.items[2].status);
        };
        assert!(
            missing_error.to_lowercase().contains("source"),
            "missing media is visible: {missing_error}"
        );
        // The failed assets are untouched.
        assert_eq!(repo().current_revision(&source_ok).unwrap(), Some(2));
        assert_eq!(repo().current_revision(&source_stale).unwrap(), Some(1));
        assert_eq!(
            fs::read(&missing_sidecar).unwrap(),
            missing_sidecar_before,
            "a failed item must not rewrite its sidecar"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_recipes_is_bounded_by_max_items_before_any_writes() {
        let dir = tmp_dir("apply-bounds");
        let bytes = gradient_bytes();
        let (source, _) = committed_asset(&dir, "a.dng", "301", &bytes, 0.1);
        let sidecar_before = fs::read(RecipeRepository::sidecar_path(&source)).unwrap();

        let items: Vec<BatchApplyItem> = (0..5)
            .map(|i| apply_item(&source, "301", 0.1 * i as f64))
            .collect();
        let err = apply_recipes(
            &repo(),
            None,
            &items,
            &CancelToken::pair().1,
            BatchLimits { max_items: 4 },
            &noop_progress,
        )
        .expect_err("oversized batches are rejected before any write");
        assert!(
            matches!(
                err,
                BatchError::TooManyItems {
                    requested: 5,
                    limit: 4
                }
            ),
            "expected TooManyItems, got {err:?}"
        );
        assert_eq!(
            fs::read(RecipeRepository::sidecar_path(&source)).unwrap(),
            sidecar_before,
            "a rejected batch must not touch any asset"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_recipes_cancellation_skips_remaining_without_touching_them() {
        let dir = tmp_dir("apply-cancel");
        let bytes = gradient_bytes();
        let (source_a, _) = committed_asset(&dir, "a.dng", "401", &bytes, 0.1);
        let (source_b, _) = committed_asset(&dir, "b.dng", "402", &bytes, 0.1);
        let b_sidecar_before = fs::read(RecipeRepository::sidecar_path(&source_b)).unwrap();

        let (cancel_source, cancel) = CancelToken::pair();
        let items = vec![
            apply_item(&source_a, "401", 0.5),
            apply_item(&source_b, "402", 0.5),
        ];
        let summary = apply_recipes(
            &repo(),
            None,
            &items,
            &cancel,
            BatchLimits::default(),
            &|index, _total, _result| {
                if index == 1 {
                    cancel_source.cancel();
                }
            },
        )
        .unwrap();

        assert!(summary.cancelled);
        assert_eq!(summary.succeeded, 1);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.failed, 0);
        assert!(matches!(
            summary.items[1].status,
            BatchItemStatus::Skipped { ref reason } if reason == "cancelled"
        ));
        assert_eq!(
            fs::read(RecipeRepository::sidecar_path(&source_b)).unwrap(),
            b_sidecar_before,
            "cancelled items must not be written"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_recipes_injected_persistence_failure_is_a_visible_item_failure() {
        let dir = tmp_dir("apply-fault");
        let bytes = gradient_bytes();
        let (source, revision) = committed_asset(&dir, "a.dng", "501", &bytes, 0.1);
        let sidecar_before = fs::read(RecipeRepository::sidecar_path(&source)).unwrap();

        let mut item = apply_item(&source, "501", 0.5);
        item.fault = Some(FaultInjection {
            point: crate::develop::recipe_repository::FaultPoint::BeforeTempWrite,
            action: crate::develop::recipe_repository::FaultAction::Error(
                "injected persistence failure".to_string(),
            ),
        });
        let summary = apply_recipes(
            &repo(),
            None,
            &[item],
            &CancelToken::pair().1,
            BatchLimits::default(),
            &noop_progress,
        )
        .unwrap();

        assert_eq!(summary.failed, 1);
        assert_eq!(summary.succeeded, 0);
        let BatchItemStatus::Failed { error } = &summary.items[0].status else {
            panic!("expected a failed item, got {:?}", summary.items[0].status);
        };
        assert!(error.contains("injected persistence failure"), "{error}");
        assert_eq!(
            fs::read(RecipeRepository::sidecar_path(&source)).unwrap(),
            sidecar_before,
            "the failed commit must leave the sidecar untouched"
        );
        assert_eq!(repo().current_revision(&source).unwrap(), Some(revision));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_batch_mixed_success_never_counts_failures_as_exported() {
        let dir = tmp_dir("export-mixed");
        let bytes = gradient_bytes();
        let (source_a, rev_a) = committed_asset(&dir, "a.dng", "601", &bytes, 0.4);
        let (source_b, rev_b) = committed_asset(&dir, "b.dng", "602", &bytes, -0.6);
        let (source_c, rev_c) = committed_asset(&dir, "c.dng", "603", &bytes, 0.2);
        let (source_missing, rev_missing) = committed_asset(&dir, "gone.dng", "604", &bytes, 0.2);
        fs::remove_file(&source_missing).unwrap();

        let service = service_with(Arc::new(FailNegativeExposureRenderer));
        let items = vec![
            export_item(&source_a, "601", rev_a, dir.join("a.png")),
            export_item(&source_b, "602", rev_b, dir.join("b.png")),
            export_item(&source_c, "603", rev_c, dir.join("c.png")),
            export_item(&source_missing, "604", rev_missing, dir.join("gone.png")),
        ];
        let summary = export_developed_batch(
            &service,
            &items,
            png_settings(),
            &CancelToken::pair().1,
            BatchExportLimits::default(),
            &noop_progress,
        )
        .unwrap();

        assert_eq!(summary.requested, 4);
        assert_eq!(
            summary.succeeded, 2,
            "only completed exports count as exported"
        );
        assert_eq!(summary.failed, 2);
        assert_eq!(summary.skipped, 0);
        let exported_keys: Vec<&str> = summary
            .items
            .iter()
            .filter(|r| matches!(r.status, BatchItemStatus::Exported { .. }))
            .map(|r| r.key.as_str())
            .collect();
        assert_eq!(exported_keys.len(), summary.succeeded);
        assert!(exported_keys.contains(&"601:default"));
        assert!(exported_keys.contains(&"603:default"));
        let BatchItemStatus::Failed { error } = &summary.items[1].status else {
            panic!(
                "the negative-exposure item must fail, got {:?}",
                summary.items[1].status
            );
        };
        assert!(error.contains("injected renderer failure"), "{error}");
        let BatchItemStatus::Failed {
            error: missing_error,
        } = &summary.items[3].status
        else {
            panic!(
                "the missing-media item must fail, got {:?}",
                summary.items[3].status
            );
        };
        assert!(
            missing_error.to_lowercase().contains("source"),
            "missing media is visible: {missing_error}"
        );
        assert!(dir.join("a.png").exists());
        assert!(dir.join("c.png").exists());
        assert!(
            !dir.join("b.png").exists() && !dir.join("gone.png").exists(),
            "failed items must not produce output files"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_batch_enforces_the_edge_bound_on_every_item() {
        let dir = tmp_dir("export-bounds");
        let bytes = wide_bytes();
        let (source_a, rev_a) = committed_asset(&dir, "a.dng", "701", &bytes, 0.1);
        let (source_b, rev_b) = committed_asset(&dir, "b.dng", "702", &bytes, 0.1);

        let service = service_with(Arc::new(EchoExportRenderer));
        let items = vec![
            export_item(&source_a, "701", rev_a, dir.join("a.png")),
            export_item(&source_b, "702", rev_b, dir.join("b.png")),
        ];
        // Unbounded per-item settings: the batch cap must still apply.
        let summary = export_developed_batch(
            &service,
            &items,
            png_settings(),
            &CancelToken::pair().1,
            BatchExportLimits {
                max_items: 8,
                max_edge: 1024,
            },
            &noop_progress,
        )
        .unwrap();

        assert_eq!(summary.succeeded, 2);
        for path in [dir.join("a.png"), dir.join("b.png")] {
            let (width, _height) = png_dimensions(&path);
            assert!(
                width <= 1024,
                "every batch export must respect the edge cap, got {width}"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_batch_cancellation_skips_remaining_and_reports_it() {
        let dir = tmp_dir("export-cancel");
        let bytes = gradient_bytes();
        let (source_a, rev_a) = committed_asset(&dir, "a.dng", "801", &bytes, 0.1);
        let (source_b, rev_b) = committed_asset(&dir, "b.dng", "802", &bytes, 0.1);

        let (cancel_source, cancel) = CancelToken::pair();
        let service = service_with(Arc::new(EchoExportRenderer));
        let items = vec![
            export_item(&source_a, "801", rev_a, dir.join("a.png")),
            export_item(&source_b, "802", rev_b, dir.join("b.png")),
        ];
        let summary = export_developed_batch(
            &service,
            &items,
            png_settings(),
            &cancel,
            BatchExportLimits::default(),
            &|index, _total, _result| {
                if index == 1 {
                    cancel_source.cancel();
                }
            },
        )
        .unwrap();

        assert!(summary.cancelled);
        assert_eq!(summary.succeeded, 1);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.failed, 0);
        assert!(
            !dir.join("b.png").exists(),
            "cancelled items must not produce output files"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

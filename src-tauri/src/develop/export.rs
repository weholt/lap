//! Durable full-resolution derivative export (lap-7ae / TASK-304;
//! governing contract `docs/raw-development/spec.md`, `export_developed`
//! in "Recipe and session contract").
//!
//! Responsibilities:
//!
//! - Resolve **exactly the requested committed revision** from the durable
//!   sidecar before any work: a stale or unknown revision is a typed
//!   `RevisionMismatch`, never a silent "latest" substitution (spec A4/A1).
//! - **Source protection in Rust**: the destination is rejected before any
//!   write when it aliases the source RAW or its recipe sidecars — exact,
//!   normalized, case-insensitive (Windows), canonical (symlinks) and
//!   hardlink identity (file index) comparisons (spec: "Enforce source
//!   protection in Rust, including normalized path/alias checks").
//! - Render the committed recipe from the **original full-resolution decode**
//!   through the shared engine (`decode_original` + the bounded export
//!   queue); embedded or 8-bit previews are never a pixel source (spec A5).
//! - Encode (PNG/JPEG) and write **atomically** (temp sibling + flush +
//!   rename), leaving either the previous destination content or a complete
//!   new derivative — never a partial or successful-looking result.
//! - Report **explicit failures** (GPU unavailable, render, encode, write)
//!   and **cancellation** at every boundary; a cancelled export leaves no
//!   artifact and no success status (spec A7).

use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use image::GenericImageView;
use rapidraw_develop::gpu::{
    LutData, OffscreenGpuContext, OffscreenRenderer, OutputTarget, RenderRequest,
    get_all_adjustments_from_json,
};
use rapidraw_develop::session::{
    ExportFrame, ExportJob, ExportJobId, ExportOutcome, ExportRenderer, SessionError,
};
use rapidraw_develop::{
    CancelToken, DecodeOptions, DecodedOriginal, DevelopError as EngineError, LinearImage,
    decode_original,
};
use serde::Serialize;

use super::sessions::{DevelopService, GpuContextFactory, tonemapper_override_code};

// ---------------------------------------------------------------------------
// Data contracts (serde shapes shared with the Tauri command and frontend)
// ---------------------------------------------------------------------------

/// Output container of the derivative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    Png,
    Jpeg,
}

impl ExportFormat {
    pub fn parse(value: &str) -> Option<ExportFormat> {
        match value.to_ascii_lowercase().as_str() {
            "png" => Some(ExportFormat::Png),
            "jpeg" | "jpg" => Some(ExportFormat::Jpeg),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ExportFormat::Png => "png",
            ExportFormat::Jpeg => "jpeg",
        }
    }
}

/// Export request settings supplied by the host caller.
#[derive(Debug, Clone)]
pub struct ExportSettings {
    /// Derivative destination. Protected against source/sidecar aliasing.
    pub destination: PathBuf,
    pub format: ExportFormat,
    /// JPEG quality 1..=100 (ignored for PNG).
    pub jpeg_quality: u8,
    /// `None` renders at the original decoded dimensions; `Some(edge)`
    /// explicitly downscales so the longest edge fits (never upscales).
    pub max_edge: Option<u32>,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            destination: PathBuf::new(),
            format: ExportFormat::Png,
            jpeg_quality: 90,
            max_edge: None,
        }
    }
}

/// Host-resolved inputs for one export. The command layer resolves the
/// catalog asset and reads the untouched source bytes; the service resolves
/// the committed envelope from the durable sidecar at **exactly**
/// `requested_revision` and verifies identity and fingerprint.
#[derive(Debug, Clone)]
pub struct AssetExportInput {
    pub asset_id: String,
    pub variant_id: String,
    pub source_path: PathBuf,
    pub source_bytes: Vec<u8>,
    /// The committed revision the UI acknowledged after flushing its edits.
    /// A durable sidecar at any other revision is a typed
    /// [`ExportError::RevisionMismatch`]; export never silently resolves
    /// "latest".
    pub requested_revision: u64,
}

/// Receipt of a completed derivative export.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReceipt {
    pub asset_id: String,
    pub variant_id: String,
    /// The committed revision that was rendered (exactly the requested one).
    pub revision: u64,
    pub destination: PathBuf,
    pub format: ExportFormat,
    pub width: u32,
    pub height: u32,
    pub bytes_written: u64,
    pub source_fingerprint: String,
    /// Content hash of the committed recipe that produced this derivative.
    pub content_hash: Option<String>,
}

/// Final state of one export request.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ExportCompletion {
    Completed { receipt: ExportReceipt },
    Cancelled,
}

/// Typed export failure. Every variant is surfaced explicitly; nothing
/// degrades into an unadjusted success or a partial file (spec A7).
#[derive(Debug)]
pub enum ExportError {
    InvalidInput(String),
    /// The durable sidecar is not exactly the requested revision.
    RevisionMismatch {
        requested: u64,
        durable: Option<u64>,
    },
    IdentityMismatch(String),
    SourceReplaced {
        path: PathBuf,
        expected: String,
        found: String,
    },
    /// The destination aliases the source RAW or its recipe sidecars.
    DestinationProtected {
        destination: PathBuf,
        detail: String,
    },
    /// Capability-bound failure (GPU unavailable/lost, oversized texture,
    /// unsupported recipe feature). Never a silent fallback.
    Unsupported(String),
    Render(String),
    Encode(String),
    Io {
        context: String,
        path: PathBuf,
        source: std::io::Error,
    },
    QueueFull {
        limit: usize,
    },
    Recipe(String),
    Cancelled,
    ManagerShuttingDown,
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::InvalidInput(message) => write!(f, "invalid export request: {message}"),
            ExportError::RevisionMismatch { requested, durable } => write!(
                f,
                "export revision mismatch: requested committed revision {requested} but the durable sidecar holds {durable:?}; flush edits and re-commit before exporting"
            ),
            ExportError::IdentityMismatch(detail) => {
                write!(f, "export identity mismatch: {detail}")
            }
            ExportError::SourceReplaced {
                path,
                expected,
                found,
            } => write!(
                f,
                "source at {} was replaced after its recipe was committed (fingerprint {expected}, found {found})",
                path.display()
            ),
            ExportError::DestinationProtected {
                destination,
                detail,
            } => write!(
                f,
                "export destination {} is protected: {detail}",
                destination.display()
            ),
            ExportError::Unsupported(message) => {
                write!(f, "export unsupported: {message}")
            }
            ExportError::Render(message) => write!(f, "export render failed: {message}"),
            ExportError::Encode(message) => write!(f, "export encoding failed: {message}"),
            ExportError::Io {
                context,
                path,
                source,
            } => write!(f, "{context} ({}): {source}", path.display()),
            ExportError::QueueFull { limit } => {
                write!(f, "export queue limit of {limit} reached")
            }
            ExportError::Recipe(message) => write!(f, "committed recipe error: {message}"),
            ExportError::Cancelled => write!(f, "export cancelled"),
            ExportError::ManagerShuttingDown => {
                write!(f, "session manager is shutting down")
            }
        }
    }
}

impl std::error::Error for ExportError {}

/// Slot where the export records its engine job id, so an external cancel
/// (Tauri command) can cancel queued or in-flight engine work cooperatively.
#[derive(Default)]
pub struct ExportCancelSlot {
    engine_job: Mutex<Option<ExportJobId>>,
}

impl ExportCancelSlot {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, job_id: ExportJobId) {
        *self
            .engine_job
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(job_id);
    }

    pub fn engine_job_id(&self) -> Option<ExportJobId> {
        *self
            .engine_job
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

// ---------------------------------------------------------------------------
// Encoders
// ---------------------------------------------------------------------------

/// Encoding seam for the derivative bytes (tests inject failures here).
pub trait ExportEncoder: Send + Sync {
    fn encode(&self, frame: &ExportFrame) -> Result<Vec<u8>, ExportError>;
}

pub struct PngExportEncoder;

impl ExportEncoder for PngExportEncoder {
    fn encode(&self, frame: &ExportFrame) -> Result<Vec<u8>, ExportError> {
        let image = frame_to_dynamic(frame)?;
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .map_err(|err| ExportError::Encode(format!("PNG encoding failed: {err}")))?;
        Ok(bytes)
    }
}

pub struct JpegExportEncoder {
    /// 1..=100.
    pub quality: u8,
}

impl ExportEncoder for JpegExportEncoder {
    fn encode(&self, frame: &ExportFrame) -> Result<Vec<u8>, ExportError> {
        if self.quality == 0 {
            return Err(ExportError::InvalidInput(
                "JPEG quality must be between 1 and 100".to_string(),
            ));
        }
        let image = frame_to_dynamic(frame)?;
        let mut bytes = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, self.quality);
        image
            .write_with_encoder(encoder)
            .map_err(|err| ExportError::Encode(format!("JPEG encoding failed: {err}")))?;
        Ok(bytes)
    }
}

fn frame_to_dynamic(frame: &ExportFrame) -> Result<image::DynamicImage, ExportError> {
    if frame.rgba8.len() != frame.width as usize * frame.height as usize * 4 {
        return Err(ExportError::InvalidInput(format!(
            "export frame pixel buffer is {} bytes but {}x{} RGBA8 needs {}",
            frame.rgba8.len(),
            frame.width,
            frame.height,
            frame.width as usize * frame.height as usize * 4
        )));
    }
    let rgba = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba8.clone())
        .ok_or_else(|| {
            ExportError::InvalidInput("export frame does not match its dimensions".to_string())
        })?;
    Ok(image::DynamicImage::ImageRgba8(rgba))
}

impl ExportSettings {
    pub fn encoder(&self) -> Box<dyn ExportEncoder> {
        match self.format {
            ExportFormat::Png => Box::new(PngExportEncoder),
            ExportFormat::Jpeg => Box::new(JpegExportEncoder {
                quality: self.jpeg_quality,
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// GPU export renderer (full-resolution; geometry from the engine crate)
// ---------------------------------------------------------------------------

/// Host export renderer: renders an immutable committed snapshot at the
/// original decoded dimensions (or an explicitly requested downscale) through
/// the engine's offscreen GPU pipeline. No embedded preview ever supplies
/// pixels; a missing device is a typed failure, never an unadjusted fallback.
/// When a resource store is installed, recipe LUT references resolve from the
/// content-addressed store; missing or changed resources fail explicitly
/// (never a silently un-LUT-ed derivative).
pub struct GpuExportRenderer {
    device: OnceLock<Result<OffscreenRenderer, String>>,
    factory: Mutex<Option<GpuContextFactory>>,
    resources: Option<Arc<crate::develop::resources::ResourceStore>>,
}

impl GpuExportRenderer {
    /// Production constructor: environment-selected offscreen device.
    pub fn new() -> Self {
        Self::with_context_factory(Box::new(OffscreenGpuContext::new))
    }

    /// Constructor with an explicit context factory (tests inject a
    /// deterministically failing device via an empty backend set).
    pub fn with_context_factory(factory: GpuContextFactory) -> Self {
        Self {
            device: OnceLock::new(),
            factory: Mutex::new(Some(factory)),
            resources: None,
        }
    }

    /// Installs the content-addressed resource store used to resolve recipe
    /// LUT references at render time (chainable).
    pub fn with_resource_store(
        mut self,
        store: Option<Arc<crate::develop::resources::ResourceStore>>,
    ) -> Self {
        self.resources = store;
        self
    }

    /// Resolves the recipe's LUT payload from the resource store. A store
    /// error (missing, changed, non-portable reference) is an explicit
    /// failure, never a silent un-LUT-ed derivative.
    fn resolve_render_lut(
        &self,
        envelope: &rapidraw_edit_model::RecipeEnvelope,
    ) -> Result<Option<Arc<LutData>>, EngineError> {
        match &self.resources {
            None => Ok(None),
            Some(store) => store
                .resolve_recipe_lut(envelope)
                .map_err(|err| EngineError::Unsupported(err.to_string())),
        }
    }

    /// Verifies the recipe's lens-profile reference before any pixel work
    /// (lap-d52). A referenced profile that is missing, changed, unmapped or
    /// unverifiable fails explicitly: exports must never silently render with
    /// a different correction than the acknowledged recipe (spec A7).
    pub(crate) fn resolve_render_lens_profile(
        &self,
        envelope: &rapidraw_edit_model::RecipeEnvelope,
    ) -> Result<Option<Arc<rapidraw_develop::lens::LensDatabase>>, EngineError> {
        if envelope.recipe.lens_profile.is_none() {
            return Ok(None);
        }
        let Some(store) = &self.resources else {
            return Err(EngineError::Unsupported(
                "the recipe references a lens profile but no resource store is installed to verify it"
                    .to_string(),
            ));
        };
        store
            .resolve_recipe_lens_profile(envelope)
            .map_err(|err| EngineError::Unsupported(err.to_string()))
    }

    fn renderer(&self) -> Result<&OffscreenRenderer, EngineError> {
        loop {
            if let Some(result) = self.device.get() {
                return result
                    .as_ref()
                    .map_err(|detail| gpu_unavailable(detail.clone()));
            }
            let factory = self
                .factory
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            match factory {
                Some(factory) => {
                    let result = factory()
                        .map(OffscreenRenderer::new)
                        .map_err(|err| err.to_string());
                    let _ = self.device.set(result);
                }
                None => std::thread::yield_now(),
            }
        }
    }
}

impl Default for GpuExportRenderer {
    fn default() -> Self {
        Self::new()
    }
}

fn gpu_unavailable(detail: String) -> EngineError {
    EngineError::Unsupported(format!("GPU adapter unavailable or unusable: {detail}"))
}

impl ExportRenderer for GpuExportRenderer {
    fn render(&self, job: &ExportJob) -> Result<ExportFrame, EngineError> {
        job.cancel.check()?;

        // Lens-profile verification precedes every pixel work (lap-d52): a
        // referenced profile that is missing, changed or unverifiable fails
        // explicitly instead of silently changing the export. This runs
        // BEFORE device initialization so a lost resource never hides behind
        // a GPU capability error.
        self.resolve_render_lens_profile(&job.envelope)?;

        let renderer = self.renderer()?;

        // Lens correction warp on the full un-cropped frame, matching the
        // pinned reference order (warp -> coarse rotation -> flip -> crop);
        // the SAME engine transform the preview path uses (lap-d52): preview
        // and export can never diverge on lens correction.
        let lens_params = rapidraw_develop::LensWarpParams::from_recipe(&job.envelope.recipe);
        let lens_warped;
        let lens_corrected: &rapidraw_develop::LinearImage = if lens_params.is_identity() {
            &job.original.image
        } else {
            lens_warped = rapidraw_develop::lens_warp(&job.original.image, &lens_params);
            &lens_warped
        };

        // Recipe geometry in the engine's oriented coordinate system,
        // applied by the SAME shared helper the preview renderer uses
        // (lap-6bc): preview and export can never diverge on
        // crop/rotation/flip. The decode already applied the RAW metadata
        // orientation.
        let linear = super::sessions::apply_recipe_geometry(lens_corrected, &job.envelope.recipe)?;

        // Explicit resize only; otherwise the original decoded dimensions
        // pass through untouched (spec A5). The RGBA32F input is the same
        // shape the preview path feeds the engine.
        let base = linear_to_rgba32f(&linear, job.max_edge)?;

        let recipe_json = serde_json::to_value(&job.envelope.recipe).map_err(|err| {
            EngineError::InvalidInput(format!("recipe serialization failed: {err}"))
        })?;
        let adjustments = get_all_adjustments_from_json(
            &recipe_json,
            job.envelope.decode.is_raw,
            job.envelope
                .decode
                .tonemapper_override
                .map(tonemapper_override_code),
        );
        // Supported visible masks rasterize into the output space (layer i
        // aligns with visible mask i in `adjustments`); unsupported kinds
        // were rejected before enqueue and fail explicitly here too.
        let mask_frame = super::sessions::mask_raster_frame(
            job.original.image.dimensions(),
            &job.envelope.recipe,
            base.dimensions().0,
            base.dimensions().1,
        )?;
        let mask_bitmaps =
            super::sessions::rasterize_recipe_masks(&job.envelope.recipe, &mask_frame)?;
        // Stable per job identity and output size; the renderer's input
        // cache is single-slot, so distinct jobs never collide.
        let transform_hash = job.job_id.0.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ ((base.dimensions().0 as u64) << 32)
            ^ base.dimensions().1 as u64;

        let request = RenderRequest {
            adjustments,
            mask_bitmaps: &mask_bitmaps,
            // Resolved from the content-addressed resource store when one is
            // installed; a missing/changed resource fails explicitly above.
            // Without a store the engine fails with ResourceMissing below
            // (never a silently un-LUT-ed derivative).
            lut: self.resolve_render_lut(&job.envelope)?,
            roi: None,
        };
        let pixels = renderer
            .render(&base, transform_hash, request, OutputTarget::CpuPixels)
            .map_err(|err| EngineError::Unsupported(err.to_string()))?;
        job.cancel.check()?;
        Ok(ExportFrame {
            width: pixels.width,
            height: pixels.height,
            rgba8: pixels.pixels,
        })
    }
}

/// Converts a linear f32 image to the engine's RGBA32F input shape, applying
/// an explicit downscale-only `max_edge` fit. Shared by the GPU renderer and
/// test doubles so dimension semantics cannot drift.
pub(crate) fn linear_to_rgba32f(
    image: &LinearImage,
    max_edge: Option<u32>,
) -> Result<image::DynamicImage, EngineError> {
    let (width, height) = image.dimensions();
    let full = image::ImageBuffer::<image::Rgb<f32>, Vec<f32>>::from_raw(
        width,
        height,
        image.rgb().to_vec(),
    )
    .ok_or_else(|| {
        EngineError::InvalidInput("decoded buffer does not match its dimensions".to_string())
    })?;
    let scaled = match max_edge {
        Some(max_edge) if max_edge > 0 && (width > max_edge || height > max_edge) => {
            let (target_w, target_h) = fit_dimensions(width, height, max_edge);
            image::imageops::resize(
                &full,
                target_w,
                target_h,
                image::imageops::FilterType::Triangle,
            )
        }
        _ => full,
    };
    let (w, h) = scaled.dimensions();
    let rgba = image::ImageBuffer::from_fn(w, h, |x, y| {
        let pixel = scaled.get_pixel(x, y);
        image::Rgba([pixel[0], pixel[1], pixel[2], 1.0])
    });
    Ok(image::DynamicImage::ImageRgba32F(rgba))
}

/// Longest-edge fit used for explicit export resizing (downscale only,
/// never upscale; mirrors the preview dimension math).
pub(crate) fn fit_dimensions(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    if width <= max_edge && height <= max_edge {
        (width, height)
    } else if width >= height {
        (
            max_edge,
            ((height as f64 / width as f64) * max_edge as f64)
                .round()
                .max(1.0) as u32,
        )
    } else {
        (
            ((width as f64 / height as f64) * max_edge as f64)
                .round()
                .max(1.0) as u32,
            max_edge,
        )
    }
}

// ---------------------------------------------------------------------------
// Export entry point
// ---------------------------------------------------------------------------

/// Exports the committed recipe revision as a full-resolution derivative.
///
/// Flow: cancellation check -> source-protection guard -> committed revision
/// resolution (exactly the requested one) -> fingerprint verification ->
/// full-resolution decode through the shared engine -> bounded export queue
/// render -> encode -> atomic replace. Any failure or cancellation leaves the
/// destination untouched (previous content or absent) and reports a typed
/// outcome.
pub fn export_developed(
    service: &DevelopService,
    input: AssetExportInput,
    settings: ExportSettings,
    cancel: &CancelToken,
    slot: &ExportCancelSlot,
) -> Result<ExportCompletion, ExportError> {
    let encoder = settings.encoder();
    export_developed_with(service, input, settings, cancel, slot, &*encoder)
}

/// Like [`export_developed`] with an explicit encoder seam (tests inject
/// deterministic encoding failures).
pub fn export_developed_with(
    service: &DevelopService,
    input: AssetExportInput,
    settings: ExportSettings,
    cancel: &CancelToken,
    slot: &ExportCancelSlot,
    encoder: &dyn ExportEncoder,
) -> Result<ExportCompletion, ExportError> {
    if input.asset_id.is_empty() || input.variant_id.is_empty() {
        return Err(ExportError::InvalidInput(
            "asset id and variant id must be non-empty".to_string(),
        ));
    }
    if settings.destination.as_os_str().is_empty() {
        return Err(ExportError::InvalidInput(
            "a destination path is required".to_string(),
        ));
    }
    if settings.max_edge == Some(0) {
        return Err(ExportError::InvalidInput(
            "max_edge must be at least 1 (or omitted for original dimensions)".to_string(),
        ));
    }
    if cancel.is_cancelled() {
        return Ok(ExportCompletion::Cancelled);
    }

    // Source protection runs before any expensive work and before any write:
    // the destination must never alias the source RAW or its recipe sidecars
    // (exact, normalized, case, canonical/symlink and hardlink identities).
    ensure_destination_is_not_source(&input.source_path, &settings.destination)?;

    // Resolve exactly the requested committed revision from the durable
    // sidecar (never from caller-supplied JSON, never "latest").
    let repo = crate::develop::recipe_repository::RecipeRepository::new(String::new());
    let durable = repo
        .load_opt(&input.source_path)
        .map_err(|err| ExportError::Recipe(err.to_string()))?;
    let envelope = durable.ok_or_else(|| {
        ExportError::InvalidInput(format!(
            "no committed recipe sidecar exists for {}; export requires a committed revision",
            input.source_path.display()
        ))
    })?;
    if envelope.revision != input.requested_revision {
        return Err(ExportError::RevisionMismatch {
            requested: input.requested_revision,
            durable: Some(envelope.revision),
        });
    }
    if envelope.asset_id != input.asset_id || envelope.variant_id != input.variant_id {
        return Err(ExportError::IdentityMismatch(format!(
            "committed sidecar holds asset '{}' variant '{}'; the export requested asset '{}' variant '{}'",
            envelope.asset_id, envelope.variant_id, input.asset_id, input.variant_id
        )));
    }
    let source_fingerprint = rapidraw_edit_model::sha256_hex(&input.source_bytes);
    if envelope.source_fingerprint != source_fingerprint {
        return Err(ExportError::SourceReplaced {
            path: input.source_path.clone(),
            expected: envelope.source_fingerprint.clone(),
            found: source_fingerprint,
        });
    }
    // Masks: every visible sub-mask must be a supported non-AI kind with
    // convertible geometry (lap-78d). Unsupported kinds are preserved in the
    // recipe but fail here explicitly, naming mask and kind; derivatives
    // never silently drop them.
    if let Err(err) = rapidraw_develop::validate_masks_supported(&envelope.recipe.masks) {
        return Err(ExportError::Unsupported(err.to_string()));
    }
    // Provenance is part of the derivative receipt; a recipe that cannot be
    // hashed is not exported (nothing is written).
    let content_hash = envelope
        .content_hash()
        .map_err(|err| ExportError::Recipe(err.to_string()))?;

    // Full-resolution decode through the shared engine with the committed
    // effective decode settings and the export cancellation token. There is
    // no embedded/8-bit preview fallback anywhere in this path (spec A5).
    let decode = &envelope.decode;
    let options = DecodeOptions {
        fast_demosaic: decode.fast_demosaic,
        highlight_compression: decode.highlight_compression as f32,
        linear_mode: decode.linear_raw_mode,
        tone_mapper: decode.tonemapper_override,
        cancellation: Some(cancel.clone()),
        ..DecodeOptions::default()
    };
    let decoded: DecodedOriginal = match decode_original(&input.source_bytes, &options) {
        Ok(decoded) => decoded,
        Err(EngineError::Cancelled) => return Ok(ExportCompletion::Cancelled),
        Err(other) => {
            return Err(ExportError::Render(format!("decode: {other}")));
        }
    };
    if cancel.is_cancelled() {
        return Ok(ExportCompletion::Cancelled);
    }

    // Bounded export queue: the immutable snapshot (committed envelope +
    // decoded original) is captured at enqueue time; later commits cannot
    // change it, and preview/session activity cannot cancel it.
    let ticket = service
        .engine_manager()
        .export_snapshot(
            input.asset_id.clone(),
            input.variant_id.clone(),
            envelope,
            Arc::new(decoded),
            settings.max_edge,
        )
        .map_err(|err| map_session_error(&err))?;
    slot.set(ticket.job.job_id);
    if cancel.is_cancelled() {
        // Cancelled while queued: drop the job from the bounded queue and
        // report an outcome; nothing was rendered or written.
        service.cancel_engine_export(ticket.job.job_id);
        return Ok(ExportCompletion::Cancelled);
    }

    match ticket.outcome.recv() {
        Ok(ExportOutcome::Completed { info, frame, .. }) => {
            if info.revision != input.requested_revision
                || info.asset_id != input.asset_id
                || info.variant_id != input.variant_id
            {
                return Err(ExportError::Render(format!(
                    "export worker returned a mismatched identity (asset '{}', variant '{}', revision {})",
                    info.asset_id, info.variant_id, info.revision
                )));
            }
            // A cancellation that arrived during the render leaves the
            // destination untouched; no partial or "successful-looking"
            // result is reported (spec A7).
            if cancel.is_cancelled() {
                return Ok(ExportCompletion::Cancelled);
            }
            let bytes = encoder.encode(&frame)?;
            if cancel.is_cancelled() {
                return Ok(ExportCompletion::Cancelled);
            }
            let Some(bytes_written) =
                write_derivative_atomic(&settings.destination, &bytes, cancel)?
            else {
                // Cancelled between the temp write and the replace: the temp
                // file was removed and the destination is untouched.
                return Ok(ExportCompletion::Cancelled);
            };
            Ok(ExportCompletion::Completed {
                receipt: ExportReceipt {
                    asset_id: input.asset_id,
                    variant_id: input.variant_id,
                    revision: info.revision,
                    destination: settings.destination,
                    format: settings.format,
                    width: frame.width,
                    height: frame.height,
                    bytes_written,
                    source_fingerprint,
                    content_hash: Some(content_hash),
                },
            })
        }
        Ok(ExportOutcome::Cancelled { .. }) => Ok(ExportCompletion::Cancelled),
        Ok(ExportOutcome::Failed { error, .. }) => Err(map_session_error(&error)),
        Err(_) => Err(ExportError::Render(
            "export worker dropped the outcome channel".to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Source protection (normalized / case / canonical / hardlink aliases)
// ---------------------------------------------------------------------------

/// Rejects a destination that aliases the source RAW or either recipe
/// sidecar, before any write happens.
pub(crate) fn ensure_destination_is_not_source(
    source: &Path,
    destination: &Path,
) -> Result<(), ExportError> {
    let sidecar = crate::develop::recipe_repository::RecipeRepository::sidecar_path(source);
    let previous =
        crate::develop::recipe_repository::RecipeRepository::previous_sidecar_path(source);
    for protected in [source, sidecar.as_path(), previous.as_path()] {
        if paths_alias(protected, destination) {
            return Err(ExportError::DestinationProtected {
                destination: destination.to_path_buf(),
                detail: format!(
                    "it resolves to the protected {} (source RAW or recipe sidecar); derivatives must be written to a different file",
                    protected.display()
                ),
            });
        }
    }
    Ok(())
}

/// True when `a` and `b` denote the same file: lexical normalized equality
/// (case-insensitive on Windows, trailing dots/spaces stripped per Win32
/// naming), canonical equality (symlinks resolved; parent canonicalization
/// covers not-yet-existing destinations), or metadata identity (hard links).
pub(crate) fn paths_alias(a: &Path, b: &Path) -> bool {
    if lexically_equal(a, b) {
        return true;
    }
    let canonical_match = match (a.canonicalize(), b.canonicalize()) {
        (Ok(canonical_a), Ok(canonical_b)) => lexically_equal(&canonical_a, &canonical_b),
        _ => {
            // The destination may not exist yet; aliasing then reduces to the
            // canonical parent plus an (on Windows: case-insensitive) name.
            match (
                a.parent(),
                b.parent(),
                a.file_name(),
                b.file_name(),
                a.parent().and_then(|parent| parent.canonicalize().ok()),
                b.parent().and_then(|parent| parent.canonicalize().ok()),
            ) {
                (Some(_), Some(_), Some(a_name), Some(b_name), Some(a_parent), Some(b_parent)) => {
                    lexically_equal(&a_parent, &b_parent) && names_equal(a_name, b_name)
                }
                _ => false,
            }
        }
    };
    if canonical_match {
        return true;
    }
    // Hard-link aliases share storage identity even though their canonical
    // paths differ; compare volume/index (Windows) or dev/ino (Unix).
    fs::metadata(a).is_ok()
        && fs::metadata(b).is_ok()
        && same_file::is_same_file(a, b).unwrap_or(false)
}

#[cfg(windows)]
const PATHS_CASE_INSENSITIVE: bool = true;
#[cfg(not(windows))]
const PATHS_CASE_INSENSITIVE: bool = false;

fn normalize_component(component: &std::ffi::OsStr) -> String {
    let mut text = component.to_string_lossy().to_lowercase();
    // Win32 strips trailing dots and spaces from each path component, so
    // "photo.dng." and "photo.dng " alias "photo.dng".
    while text.ends_with('.') || text.ends_with(' ') {
        text.pop();
    }
    if PATHS_CASE_INSENSITIVE {
        text
    } else {
        component.to_string_lossy().to_string()
    }
}

fn lexically_equal(a: &Path, b: &Path) -> bool {
    let mut components_a = a.components().map(|c| normalize_component(c.as_os_str()));
    let mut components_b = b.components().map(|c| normalize_component(c.as_os_str()));
    loop {
        match (components_a.next(), components_b.next()) {
            (None, None) => return true,
            (Some(value_a), Some(value_b)) if value_a == value_b => continue,
            _ => return false,
        }
    }
}

fn names_equal(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    if PATHS_CASE_INSENSITIVE {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

// ---------------------------------------------------------------------------
// Atomic derivative output
// ---------------------------------------------------------------------------

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
            Err(err) if attempt < 5 && transient_replace_error(&err) => {
                std::thread::sleep(Duration::from_millis(10 * u64::from(attempt)));
            }
            Err(err) => return Err(err),
        }
    }
}

fn transient_replace_error(err: &std::io::Error) -> bool {
    cfg!(windows) && matches!(err.kind(), std::io::ErrorKind::PermissionDenied)
}

/// Writes `bytes` to a temporary sibling, flushes it, then atomically
/// replaces `destination`. Any failure (including cancellation between the
/// temp write and the replace) removes the temporary file and leaves the
/// previous destination content untouched. Returns `Ok(None)` when the write
/// was cancelled after the temp write (no artifact, no replace).
fn write_derivative_atomic(
    destination: &Path,
    bytes: &[u8],
    cancel: &CancelToken,
) -> Result<Option<u64>, ExportError> {
    let temp = unique_temp_path(destination);
    let outcome = (|| -> std::io::Result<()> {
        {
            let mut file = File::create(&temp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        if cancel.is_cancelled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled before replace",
            ));
        }
        replace_atomic(&temp, destination)
    })();
    match outcome {
        Ok(()) => Ok(Some(bytes.len() as u64)),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            if cancel.is_cancelled() {
                Ok(None)
            } else {
                Err(ExportError::Io {
                    context: "writing derivative".to_string(),
                    path: destination.to_path_buf(),
                    source: err,
                })
            }
        }
    }
}

fn map_session_error(error: &SessionError) -> ExportError {
    match error {
        SessionError::ExportQueueFull { limit } => ExportError::QueueFull { limit: *limit },
        SessionError::ManagerShuttingDown => ExportError::ManagerShuttingDown,
        SessionError::NotFound { .. } | SessionError::SessionClosed { .. } => {
            ExportError::InvalidInput(error.to_string())
        }
        SessionError::Render(engine_error) => match engine_error {
            EngineError::Unsupported(message) => ExportError::Unsupported(message.clone()),
            EngineError::Decode(message) => ExportError::Render(format!("decode: {message}")),
            EngineError::InvalidInput(message) => ExportError::InvalidInput(message.clone()),
            other => ExportError::Render(other.to_string()),
        },
        other => ExportError::Render(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Tests (behavior regressions for every failure boundary; RED before the
// implementation lands, GREEN after)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::recipe_repository::RecipeRepository;
    use crate::develop::sessions::{DevelopConfig, DevelopService};
    use rapidraw_develop::session::{
        PreviewFrame, PreviewJob, PreviewRenderer, RecipeStore, SessionManagerConfig,
    };
    use rapidraw_edit_model::RecipeEnvelope;
    use rapidraw_edit_model::sha256_hex;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    // ---------------------------------------------------------------- fixtures

    fn fixture_path(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/raw-development")
            .join(name)
    }

    fn gradient_bytes() -> Vec<u8> {
        fs::read(fixture_path("synthetic/dng-linear-gradient-64x48.dng")).expect("gradient fixture")
    }

    fn wide_bytes() -> Vec<u8> {
        fs::read(fixture_path("synthetic/dng-linear-wide-5000x64.dng")).expect("wide fixture")
    }

    fn fingerprint(bytes: &[u8]) -> String {
        sha256_hex(bytes)
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "lap_develop_export_{}_{}_{}",
            tag,
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_source(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        path
    }

    /// Commits one recipe revision to the durable sidecar, mirroring what the
    /// develop commit command does before an export is requested.
    fn commit_recipe(
        repo: &RecipeRepository,
        source: &Path,
        asset: &str,
        bytes: &[u8],
        exposure: f64,
    ) -> u64 {
        let current = repo.current_revision(source).unwrap().unwrap_or(0);
        let mut envelope = match repo.load(source) {
            Ok(existing) => existing,
            Err(_) => repo.new_envelope(asset, "default", &fingerprint(bytes)),
        };
        envelope.recipe.exposure = exposure;
        repo.commit_sidecar(source, current, envelope)
            .unwrap()
            .revision
    }

    // ------------------------------------------------------------ test doubles

    /// Preview renderer that is never exercised by export tests.
    struct NoopPreviewRenderer;

    impl PreviewRenderer for NoopPreviewRenderer {
        fn render(&self, _job: &PreviewJob) -> Result<PreviewFrame, EngineError> {
            Err(EngineError::Unsupported(
                "not used in export tests".to_string(),
            ))
        }
    }

    /// Store that accepts every save (export never persists recipes itself).
    struct NullStore;

    impl RecipeStore for NullStore {
        fn persist(&self, _envelope: &RecipeEnvelope) -> Result<(), String> {
            Ok(())
        }
    }

    /// What the echo renderer observed for the last job it ran.
    #[derive(Debug, Clone, PartialEq)]
    struct RecordedJob {
        revision: u64,
        exposure: f64,
        input_dimensions: (u32, u32),
        max_edge: Option<u32>,
    }

    /// Deterministic export renderer: echoes the decoded input through the
    /// shared dimension helper (so resize semantics cannot drift from the
    /// production renderer) and records what it received.
    struct EchoExportRenderer {
        observed: Mutex<Option<RecordedJob>>,
        render_count: AtomicU64,
    }

    impl EchoExportRenderer {
        fn new() -> Self {
            Self {
                observed: Mutex::new(None),
                render_count: AtomicU64::new(0),
            }
        }

        fn observed(&self) -> Option<RecordedJob> {
            self.observed.lock().unwrap().clone()
        }

        fn render_count(&self) -> u64 {
            self.render_count.load(AtomicOrdering::SeqCst)
        }
    }

    impl ExportRenderer for EchoExportRenderer {
        fn render(&self, job: &ExportJob) -> Result<ExportFrame, EngineError> {
            self.render_count.fetch_add(1, AtomicOrdering::SeqCst);
            *self.observed.lock().unwrap() = Some(RecordedJob {
                revision: job.revision,
                exposure: job.envelope.recipe.exposure,
                input_dimensions: job.original.image.dimensions(),
                max_edge: job.max_edge,
            });
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

    /// Export renderer that always fails explicitly (never pixels).
    struct FailingExportRenderer;

    impl ExportRenderer for FailingExportRenderer {
        fn render(&self, _job: &ExportJob) -> Result<ExportFrame, EngineError> {
            Err(EngineError::Unsupported(
                "injected renderer failure: device lost".to_string(),
            ))
        }
    }

    /// Export renderer that blocks (cooperatively honouring cancellation)
    /// until its token is cancelled, then reports cancellation.
    struct GatedExportRenderer;

    impl ExportRenderer for GatedExportRenderer {
        fn render(&self, job: &ExportJob) -> Result<ExportFrame, EngineError> {
            loop {
                job.cancel.check()?;
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }

    /// Encoder seam that always fails (encoder failure boundary).
    struct FailingEncoder;

    impl ExportEncoder for FailingEncoder {
        fn encode(&self, _frame: &ExportFrame) -> Result<Vec<u8>, ExportError> {
            Err(ExportError::Encode("injected encoder failure".to_string()))
        }
    }

    // ----------------------------------------------------------- constructors

    fn export_config() -> DevelopConfig {
        DevelopConfig {
            sessions: SessionManagerConfig {
                max_sessions: 4,
                preview_workers: 1,
                export_workers: 1,
                max_queued_previews: 8,
                max_queued_exports: 2,
                max_cached_preview_results: 2,
            },
            max_preview_edge: 4096,
            max_cached_preview_bytes: 1024 * 1024,
        }
    }

    fn service_with(export_renderer: Arc<dyn ExportRenderer>) -> DevelopService {
        DevelopService::with_parts(
            export_config(),
            Arc::new(NoopPreviewRenderer),
            export_renderer,
            Arc::new(NullStore),
        )
    }

    fn export_input(
        source: &Path,
        bytes: &[u8],
        asset: &str,
        requested_revision: u64,
    ) -> AssetExportInput {
        AssetExportInput {
            asset_id: asset.to_string(),
            variant_id: "default".to_string(),
            source_path: source.to_path_buf(),
            source_bytes: bytes.to_vec(),
            requested_revision,
        }
    }

    /// The durable sidecar revision (what a UI would have acknowledged after
    /// flushing its edits).
    fn committed(repo: &RecipeRepository, source: &Path) -> u64 {
        repo.current_revision(source)
            .unwrap()
            .expect("committed revision")
    }

    fn png_settings(destination: &Path) -> ExportSettings {
        ExportSettings {
            destination: destination.to_path_buf(),
            format: ExportFormat::Png,
            jpeg_quality: 90,
            max_edge: None,
        }
    }

    fn no_leftover_temps(dir: &Path) -> bool {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
    }

    fn png_dimensions(path: &Path) -> (u32, u32) {
        let reader = image::ImageReader::open(path).expect("open exported file");
        reader.into_dimensions().expect("PNG dimensions")
    }

    // ------------------------------------------------------------------ tests

    #[test]
    fn export_requires_a_committed_sidecar() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("uncommitted");
        let source = write_source(&dir, "photo.dng", &bytes);
        let service = service_with(Arc::new(EchoExportRenderer::new()));
        let destination = dir.join("photo-developed.png");

        let err = export_developed(
            &service,
            export_input(&source, &bytes, "asset-a", 0),
            png_settings(&destination),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect_err("exporting without a committed recipe must fail explicitly");
        assert!(
            matches!(err, ExportError::InvalidInput(_)),
            "expected an explicit invalid-input failure, got {err:?}"
        );
        assert!(!destination.exists());
    }

    #[test]
    fn export_resolves_exactly_the_requested_committed_revision() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("revision");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-a", &bytes, 0.5);
        commit_recipe(&repo, &source, "asset-a", &bytes, 1.5);
        assert_eq!(repo.current_revision(&source).unwrap(), Some(2));

        // Stale request (older committed revision): typed mismatch, no work.
        let renderer = Arc::new(EchoExportRenderer::new());
        let service = service_with(Arc::clone(&renderer) as Arc<dyn ExportRenderer>);
        let destination = dir.join("stale.png");
        let err = export_developed(
            &service,
            // The UI acknowledged revision 1, but the durable sidecar already
            // advanced to revision 2: the backend must refuse, never resolve
            // "latest".
            export_input(&source, &bytes, "asset-a", 1),
            png_settings(&destination),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect_err("the sidecar holds revision 2, requesting 1 must fail");
        let ExportError::RevisionMismatch { requested, durable } = err else {
            panic!("expected RevisionMismatch, got {err:?}");
        };
        assert_eq!(requested, 1);
        assert_eq!(durable, Some(2));
        assert_eq!(
            renderer.render_count(),
            0,
            "a stale revision must never reach the renderer"
        );
        assert!(!destination.exists());

        // Future revision (ahead of the durable sidecar): typed mismatch.
        let renderer = Arc::new(EchoExportRenderer::new());
        let service = service_with(Arc::clone(&renderer) as Arc<dyn ExportRenderer>);
        let err = export_developed(
            &service,
            export_input(&source, &bytes, "asset-a", 3),
            png_settings(&destination),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect_err("requesting a revision ahead of the sidecar must fail");
        assert!(
            matches!(
                err,
                ExportError::RevisionMismatch {
                    requested: 3,
                    durable: Some(2)
                }
            ),
            "{err:?}"
        );

        // Exactly the durable revision succeeds and renders that revision.
        let renderer = Arc::new(EchoExportRenderer::new());
        let service = service_with(Arc::clone(&renderer) as Arc<dyn ExportRenderer>);
        let destination = dir.join("rev2.png");
        let completion = export_developed(
            &service,
            export_input(&source, &bytes, "asset-a", committed(&repo, &source)),
            png_settings(&destination),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect("exactly-the-durable-revision export succeeds");
        let ExportCompletion::Completed { receipt } = completion else {
            panic!("expected completion, got {completion:?}");
        };
        assert_eq!(receipt.revision, 2);
        assert_eq!(receipt.width, 64);
        assert_eq!(receipt.height, 48);
        let observed = renderer.observed().expect("renderer observed the job");
        assert_eq!(
            observed.revision, 2,
            "renderer must see the committed revision"
        );
        assert!((observed.exposure - 1.5).abs() < f64::EPSILON);
        assert_eq!(png_dimensions(&destination), (64, 48));
    }

    #[test]
    fn export_rejects_source_destination_before_writing() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("selfdest");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-a", &bytes, 0.5);

        let service = service_with(Arc::new(EchoExportRenderer::new()));
        let err = export_developed(
            &service,
            export_input(&source, &bytes, "asset-a", committed(&repo, &source)),
            png_settings(&source),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect_err("the source RAW itself is not a valid destination");
        let ExportError::DestinationProtected { destination, .. } = err else {
            panic!("expected DestinationProtected, got {err:?}");
        };
        assert_eq!(destination, source);
        assert_eq!(sha256_hex(&fs::read(&source).unwrap()), fingerprint(&bytes));
        assert!(no_leftover_temps(&dir));
    }

    #[cfg(windows)]
    #[test]
    fn export_rejects_case_alias_destinations() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("casealias");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-a", &bytes, 0.5);

        let service = service_with(Arc::new(EchoExportRenderer::new()));
        let destination = dir.join("PHOTO.DNG");
        let err = export_developed(
            &service,
            export_input(&source, &bytes, "asset-a", committed(&repo, &source)),
            png_settings(&destination),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect_err("a case alias of the source is the same file on Windows");
        assert!(
            matches!(err, ExportError::DestinationProtected { .. }),
            "expected DestinationProtected, got {err:?}"
        );
        assert!(no_leftover_temps(&dir));
    }

    #[test]
    fn export_rejects_canonical_alias_destinations() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("canonical");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-a", &bytes, 0.5);

        let canonical = source.canonicalize().expect("canonical source path");
        assert_ne!(canonical, source, "canonical form should differ lexically");
        let service = service_with(Arc::new(EchoExportRenderer::new()));
        let err = export_developed(
            &service,
            export_input(&source, &bytes, "asset-a", committed(&repo, &source)),
            png_settings(&canonical),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect_err("the canonical path of the source is the same file");
        assert!(
            matches!(err, ExportError::DestinationProtected { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn export_rejects_hardlink_alias_destinations() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("hardlink");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-a", &bytes, 0.5);

        let hardlink = dir.join("alias.dng");
        match fs::hard_link(&source, &hardlink) {
            Ok(()) => {}
            Err(err) => {
                eprintln!("skipping hardlink alias test: filesystem refused hard links: {err}");
                return;
            }
        }

        let service = service_with(Arc::new(EchoExportRenderer::new()));
        let err = export_developed(
            &service,
            export_input(&source, &bytes, "asset-a", committed(&repo, &source)),
            png_settings(&hardlink),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect_err("a hard link to the source is the same file");
        assert!(
            matches!(err, ExportError::DestinationProtected { .. }),
            "{err:?}"
        );
        assert_eq!(
            sha256_hex(&fs::read(&source).unwrap()),
            fingerprint(&bytes),
            "the source bytes must be untouched"
        );
    }

    #[test]
    fn export_rejects_symlink_alias_destinations_where_supported() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("symlink");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-a", &bytes, 0.5);

        let link = dir.join("link.dng");
        #[cfg(windows)]
        let created = std::os::windows::fs::symlink_file(&source, &link);
        #[cfg(not(windows))]
        let created = std::os::unix::fs::symlink(&source, &link);
        match created {
            Ok(()) => {}
            Err(err) => {
                eprintln!("skipping symlink alias test: filesystem refused symlinks: {err}");
                return;
            }
        }

        let service = service_with(Arc::new(EchoExportRenderer::new()));
        let err = export_developed(
            &service,
            export_input(&source, &bytes, "asset-a", committed(&repo, &source)),
            png_settings(&link),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect_err("a symlink to the source resolves to the same file");
        assert!(
            matches!(err, ExportError::DestinationProtected { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn export_rejects_sidecar_destinations() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("sidecardest");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-a", &bytes, 0.5);
        let sidecar_before = fs::read(RecipeRepository::sidecar_path(&source)).unwrap();

        let service = service_with(Arc::new(EchoExportRenderer::new()));
        // Direct sidecar path and its case alias (Windows) must be rejected.
        for destination in [
            RecipeRepository::sidecar_path(&source),
            dir.join("PHOTO.DNG.LAPEDIT.JSON"),
        ] {
            let err = export_developed(
                &service,
                export_input(&source, &bytes, "asset-a", committed(&repo, &source)),
                png_settings(&destination),
                &CancelToken::pair().1,
                &ExportCancelSlot::new(),
            )
            .expect_err("the recipe sidecar is not a pixel destination");
            assert!(
                matches!(err, ExportError::DestinationProtected { .. }),
                "expected DestinationProtected for {}, got {err:?}",
                destination.display()
            );
        }
        assert_eq!(
            fs::read(RecipeRepository::sidecar_path(&source)).unwrap(),
            sidecar_before,
            "the sidecar must stay byte-identical"
        );
    }

    #[test]
    fn export_replaces_existing_destination_atomically() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("replace");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-a", &bytes, 0.5);

        let destination = dir.join("photo-developed.png");
        fs::write(&destination, b"stale previous derivative").unwrap();

        let service = service_with(Arc::new(EchoExportRenderer::new()));
        let completion = export_developed(
            &service,
            export_input(&source, &bytes, "asset-a", committed(&repo, &source)),
            png_settings(&destination),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect("replacement export succeeds");
        let ExportCompletion::Completed { receipt } = completion else {
            panic!("expected completion, got {completion:?}");
        };
        assert_eq!(receipt.destination, destination);
        let written = fs::read(&destination).unwrap();
        assert!(
            written.starts_with(b"\x89PNG"),
            "destination must be a real PNG"
        );
        assert_eq!(receipt.bytes_written as usize, written.len());
        assert!(no_leftover_temps(&dir), "no temp artifact may survive");
    }

    #[test]
    fn renderer_failure_is_explicit_and_writes_nothing() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("renderfail");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-a", &bytes, 0.5);

        let destination = dir.join("out.png");
        fs::write(&destination, b"pre-existing content").unwrap();

        let service = service_with(Arc::new(FailingExportRenderer));
        let err = export_developed(
            &service,
            export_input(&source, &bytes, "asset-a", committed(&repo, &source)),
            png_settings(&destination),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect_err("renderer failure must be explicit");
        let ExportError::Unsupported(message) = err else {
            panic!("expected Unsupported, got {err:?}");
        };
        assert!(message.contains("injected renderer failure"));
        assert_eq!(
            fs::read(&destination).unwrap(),
            b"pre-existing content",
            "a failed export must not damage an existing destination"
        );
        assert!(no_leftover_temps(&dir));
        assert_eq!(sha256_hex(&fs::read(&source).unwrap()), fingerprint(&bytes));
    }

    #[test]
    fn encoder_failure_is_explicit_and_writes_nothing() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("encodefail");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-a", &bytes, 0.5);

        let destination = dir.join("out.png");
        let service = service_with(Arc::new(EchoExportRenderer::new()));
        let err = export_developed_with(
            &service,
            export_input(&source, &bytes, "asset-a", committed(&repo, &source)),
            png_settings(&destination),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
            &FailingEncoder,
        )
        .expect_err("encoder failure must be explicit");
        let ExportError::Encode(message) = err else {
            panic!("expected Encode failure, got {err:?}");
        };
        assert!(message.contains("injected encoder failure"));
        assert!(!destination.exists(), "no destination file may appear");
        assert!(no_leftover_temps(&dir), "temp artifacts must be cleaned up");
    }

    #[test]
    fn write_failure_is_explicit_and_cleans_temp_artifacts() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("writefail");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-a", &bytes, 0.5);

        // A directory at the destination path makes the final rename fail.
        let destination = dir.join("out.png");
        fs::create_dir(&destination).unwrap();

        let service = service_with(Arc::new(EchoExportRenderer::new()));
        let err = export_developed(
            &service,
            export_input(&source, &bytes, "asset-a", committed(&repo, &source)),
            png_settings(&destination),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect_err("unwritable destination must fail explicitly");
        assert!(
            matches!(err, ExportError::Io { .. }),
            "expected a typed Io failure, got {err:?}"
        );
        assert!(
            destination.is_dir(),
            "the pre-existing directory must survive"
        );
        assert!(no_leftover_temps(&dir), "temp artifacts must be cleaned up");
    }

    #[test]
    fn cancelled_before_start_reports_cancelled_and_writes_nothing() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("cancel-early");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-a", &bytes, 0.5);

        let service = service_with(Arc::new(EchoExportRenderer::new()));
        let (cancel_source, cancel) = CancelToken::pair();
        cancel_source.cancel();
        let destination = dir.join("out.png");
        let completion = export_developed(
            &service,
            export_input(&source, &bytes, "asset-a", committed(&repo, &source)),
            png_settings(&destination),
            &cancel,
            &ExportCancelSlot::new(),
        )
        .expect("cancellation is an outcome, not an error");
        assert!(
            matches!(completion, ExportCompletion::Cancelled),
            "expected cancelled, got {completion:?}"
        );
        assert!(
            !destination.exists(),
            "a cancelled export must not leave a file"
        );
        assert!(no_leftover_temps(&dir));
    }

    #[test]
    fn cancel_during_render_reports_cancelled_and_writes_nothing() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("cancel-render");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-a", &bytes, 0.5);

        let service = Arc::new(service_with(Arc::new(GatedExportRenderer)));
        let (cancel_source, cancel) = CancelToken::pair();
        let slot = Arc::new(ExportCancelSlot::new());
        let destination = dir.join("out.png");

        let handle = {
            let service = Arc::clone(&service);
            let slot = Arc::clone(&slot);
            let input = export_input(&source, &bytes, "asset-a", committed(&repo, &source));
            let settings = png_settings(&destination);
            std::thread::spawn(move || export_developed(&service, input, settings, &cancel, &slot))
        };

        // Wait until the engine job is registered, then cancel.
        let mut waited = 0u32;
        while slot.engine_job_id().is_none() && waited < 200 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            waited += 1;
        }
        assert!(
            slot.engine_job_id().is_some(),
            "engine job must be registered"
        );
        cancel_source.cancel();
        assert!(
            service.cancel_engine_export(slot.engine_job_id().unwrap()),
            "engine job cancellation must be accepted"
        );

        let completion = handle.join().unwrap().expect("export resolves");
        assert!(
            matches!(completion, ExportCompletion::Cancelled),
            "expected cancelled, got {completion:?}"
        );
        assert!(!destination.exists());
        assert!(no_leftover_temps(&dir));
        assert_eq!(sha256_hex(&fs::read(&source).unwrap()), fingerprint(&bytes));
    }

    #[test]
    fn jpeg_export_writes_jpeg_bytes() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("jpeg");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-a", &bytes, 0.5);

        let service = service_with(Arc::new(EchoExportRenderer::new()));
        let destination = dir.join("out.jpg");
        let completion = export_developed(
            &service,
            export_input(&source, &bytes, "asset-a", committed(&repo, &source)),
            ExportSettings {
                format: ExportFormat::Jpeg,
                jpeg_quality: 80,
                ..png_settings(&destination)
            },
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect("jpeg export succeeds");
        let ExportCompletion::Completed { receipt } = completion else {
            panic!("expected completion, got {completion:?}");
        };
        assert_eq!(receipt.format, ExportFormat::Jpeg);
        let written = fs::read(&destination).unwrap();
        assert!(
            written.starts_with(&[0xFF, 0xD8, 0xFF]),
            "must be a real JPEG"
        );
        assert_eq!(receipt.bytes_written as usize, written.len());
    }

    #[test]
    fn full_resolution_export_preserves_original_dimensions() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("fullres");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-a", &bytes, 0.5);

        let renderer = Arc::new(EchoExportRenderer::new());
        let service = service_with(Arc::clone(&renderer) as Arc<dyn ExportRenderer>);
        let destination = dir.join("full.png");
        let completion = export_developed(
            &service,
            export_input(&source, &bytes, "asset-a", committed(&repo, &source)),
            png_settings(&destination),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect("full-resolution export succeeds");
        let ExportCompletion::Completed { receipt } = completion else {
            panic!("expected completion, got {completion:?}");
        };
        assert_eq!((receipt.width, receipt.height), (64, 48));
        assert_eq!(png_dimensions(&destination), (64, 48));
        let observed = renderer.observed().unwrap();
        assert_eq!(
            observed.max_edge, None,
            "full resolution passes no edge limit"
        );
    }

    #[test]
    fn wide_raw_exports_at_original_dimensions_unless_resized() {
        let bytes = wide_bytes();
        let dir = tmp_dir("wide");
        let source = write_source(&dir, "wide.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-w", &bytes, 0.25);

        // Original dimensions: the 5000-pixel-wide RAW must not be capped.
        let renderer = Arc::new(EchoExportRenderer::new());
        let service = service_with(Arc::clone(&renderer) as Arc<dyn ExportRenderer>);
        let destination = dir.join("wide-full.png");
        let completion = export_developed(
            &service,
            export_input(&source, &bytes, "asset-w", committed(&repo, &source)),
            png_settings(&destination),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect("wide export succeeds");
        let ExportCompletion::Completed { receipt } = completion else {
            panic!("expected completion, got {completion:?}");
        };
        assert_eq!(
            (receipt.width, receipt.height),
            (5000, 64),
            "a RAW wider than 4096 pixels exports its original dimensions"
        );
        assert_eq!(png_dimensions(&destination), (5000, 64));

        // Explicit resize: the longest edge fits the requested bound.
        let renderer = Arc::new(EchoExportRenderer::new());
        let service = service_with(Arc::clone(&renderer) as Arc<dyn ExportRenderer>);
        let destination = dir.join("wide-4096.png");
        let completion = export_developed(
            &service,
            export_input(&source, &bytes, "asset-w", committed(&repo, &source)),
            ExportSettings {
                max_edge: Some(4096),
                ..png_settings(&destination)
            },
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect("explicitly resized export succeeds");
        let ExportCompletion::Completed { receipt } = completion else {
            panic!("expected completion, got {completion:?}");
        };
        assert_eq!(receipt.width, 4096);
        assert_eq!(receipt.height, ((64f64 / 5000f64) * 4096f64).round() as u32);
        assert_eq!(renderer.observed().unwrap().max_edge, Some(4096));
    }

    #[test]
    fn export_rejects_unsupported_mask_kind_naming_it() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("masks-ai");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        let current = repo.current_revision(&source).unwrap().unwrap_or(0);
        let mut envelope = repo.new_envelope("asset-m", "default", &fingerprint(&bytes));
        envelope
            .recipe
            .masks
            .push(rapidraw_edit_model::MaskContainer {
                id: "mask-1".to_string(),
                name: "ai".to_string(),
                visible: true,
                sub_masks: vec![rapidraw_edit_model::SubMask {
                    id: "sub-1".to_string(),
                    kind: "ai-sky".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            });
        repo.commit_sidecar(&source, current, envelope).unwrap();

        let renderer = Arc::new(EchoExportRenderer::new());
        let service = service_with(Arc::clone(&renderer) as Arc<dyn ExportRenderer>);
        let err = export_developed(
            &service,
            export_input(&source, &bytes, "asset-m", committed(&repo, &source)),
            png_settings(&dir.join("out.png")),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect_err("unsupported mask kinds are never silently dropped from derivatives");
        match err {
            ExportError::Unsupported(message) => {
                assert!(
                    message.contains("ai-sky") && message.contains("mask-1"),
                    "the rejection must name the mask and the unsupported kind: {message}"
                );
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
        assert_eq!(
            renderer.render_count(),
            0,
            "no pixels may be produced for an unsupported recipe"
        );
    }

    #[test]
    fn export_renders_supported_masks_or_fails_gpu_only() {
        // Supported non-AI masks are rendered into derivatives. Combined with
        // crop/orientation the geometry must stay anchored to the oriented
        // frame. Without a device the failure must be an explicit GPU
        // capability error; silently un-masked output is impossible.
        let bytes = gradient_bytes();
        let dir = tmp_dir("masks-export");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");

        let masked_envelope = |with_geometry: bool| {
            let current = repo.current_revision(&source).unwrap().unwrap_or(0);
            let mut envelope = repo.new_envelope("asset-mx", "default", &fingerprint(&bytes));
            envelope.recipe.orientation_steps = 1;
            envelope.recipe.crop = Some(rapidraw_edit_model::CropRect {
                x: 0.1,
                y: 0.1,
                width: 0.6,
                height: 0.6,
            });
            let mut mask = rapidraw_edit_model::MaskContainer {
                id: "mask-1".to_string(),
                name: "radial".to_string(),
                visible: true,
                adjustments: rapidraw_edit_model::MaskLocalAdjustments {
                    exposure: -2.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            if with_geometry {
                mask.sub_masks = vec![rapidraw_edit_model::SubMask {
                    id: "sub-1".to_string(),
                    kind: "radial".to_string(),
                    geometry: Some(rapidraw_edit_model::MaskGeometry::Radial {
                        center_x: 0.35,
                        center_y: 0.5,
                        radius_x: 0.25,
                        radius_y: 0.45,
                        rotation: 0.0,
                        feather: 0.3,
                    }),
                    ..Default::default()
                }];
            } else {
                mask.sub_masks = Vec::new();
                mask.visible = false;
            }
            envelope.recipe.masks.push(mask);
            repo.commit_sidecar(&source, current, envelope).unwrap();
        };

        let service = DevelopService::with_parts(
            export_config(),
            Arc::new(NoopPreviewRenderer),
            Arc::new(GpuExportRenderer::new()),
            Arc::new(NullStore),
        );
        let run = |requested_revision: u64, name: &str| {
            export_developed(
                &service,
                export_input(&source, &bytes, "asset-mx", requested_revision),
                png_settings(&dir.join(name)),
                &CancelToken::pair().1,
                &ExportCancelSlot::new(),
            )
        };

        // Reference derivative first (sidecar revision 1, no visible mask),
        // then commit the masked envelope and export again (revision 2).
        masked_envelope(false);
        let unmasked = run(committed(&repo, &source), "unmasked.png");
        masked_envelope(true);
        let masked = run(committed(&repo, &source), "masked.png");
        match (unmasked, masked) {
            (Ok(ExportCompletion::Completed { .. }), Ok(ExportCompletion::Completed { .. })) => {
                let plain = image::ImageReader::open(dir.join("unmasked.png"))
                    .unwrap()
                    .decode()
                    .unwrap();
                let masked = image::ImageReader::open(dir.join("masked.png"))
                    .unwrap()
                    .decode()
                    .unwrap();
                assert_ne!(
                    plain.as_bytes(),
                    masked.as_bytes(),
                    "masked derivative must differ from the unmasked render"
                );
            }
            (Err(ExportError::Unsupported(m)), _) | (_, Err(ExportError::Unsupported(m)))
                if m.to_lowercase().contains("gpu") =>
            {
                // Honest capability skip on machines without an offscreen device.
            }
            (a, b) => panic!("unexpected export outcomes: unmasked={a:?} masked={b:?}"),
        }
    }

    #[test]
    fn sidecar_and_source_stay_immutable_after_export() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("immutable");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-a", &bytes, 0.5);
        let sidecar_before = fs::read(RecipeRepository::sidecar_path(&source)).unwrap();

        let service = service_with(Arc::new(EchoExportRenderer::new()));
        let completion = export_developed(
            &service,
            export_input(&source, &bytes, "asset-a", committed(&repo, &source)),
            png_settings(&dir.join("out.png")),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        )
        .expect("export succeeds");
        let ExportCompletion::Completed { receipt } = completion else {
            panic!("expected completion, got {completion:?}");
        };
        assert!(receipt.content_hash.is_some());
        assert_eq!(
            fs::read(RecipeRepository::sidecar_path(&source)).unwrap(),
            sidecar_before,
            "the committed recipe must stay byte-identical"
        );
        assert_eq!(repo.current_revision(&source).unwrap(), Some(1));
        assert_eq!(sha256_hex(&fs::read(&source).unwrap()), fingerprint(&bytes));
    }

    #[test]
    fn gpu_export_renderer_fails_explicitly_without_device() {
        // An empty backend set deterministically yields NoAdapter: the
        // unsupported-device path must produce a typed error mentioning the
        // GPU, never pixels and never an unadjusted frame.
        let renderer = GpuExportRenderer::with_context_factory(Box::new(|| {
            rapidraw_develop::gpu::OffscreenGpuContext::new_with_backends(wgpu::Backends::empty())
        }));
        let bytes = gradient_bytes();
        let decoded = decode_original(&bytes, &DecodeOptions::default()).expect("decode");
        let envelope =
            RecipeEnvelope::new("lap-test/0", "asset-a", "default", &fingerprint(&bytes));
        let (cancel_source, cancel) = CancelToken::pair();
        let _ = cancel_source;
        let job = ExportJob {
            job_id: ExportJobId(1),
            asset_id: "asset-a".to_string(),
            variant_id: "default".to_string(),
            revision: envelope.revision,
            original: Arc::new(decoded),
            envelope,
            max_edge: None,
            cancel,
        };
        let err = renderer
            .render(&job)
            .expect_err("no device means no pixels");
        let message = err.to_string().to_lowercase();
        assert!(
            matches!(err, EngineError::Unsupported(_)) && message.contains("gpu"),
            "unsupported device must be a typed explicit failure mentioning the GPU, got {err:?}"
        );
    }

    #[test]
    fn production_pipeline_exports_wide_fixture_full_resolution_or_fails_explicitly() {
        // Honest capability check on this machine: either the offscreen device
        // renders the committed recipe at the original 5000x64 dimensions, or
        // the failure is an explicit GPU capability error. A fake or
        // unadjusted success is impossible by construction.
        let bytes = wide_bytes();
        let dir = tmp_dir("gpu-wide");
        let source = write_source(&dir, "wide.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-g", &bytes, 0.5);

        let service = DevelopService::with_parts(
            export_config(),
            Arc::new(NoopPreviewRenderer),
            Arc::new(GpuExportRenderer::new()),
            Arc::new(NullStore),
        );
        let destination = dir.join("gpu-wide.png");
        let outcome = export_developed(
            &service,
            export_input(&source, &bytes, "asset-g", committed(&repo, &source)),
            png_settings(&destination),
            &CancelToken::pair().1,
            &ExportCancelSlot::new(),
        );
        match outcome {
            Ok(ExportCompletion::Completed { receipt }) => {
                assert_eq!((receipt.width, receipt.height), (5000, 64));
                assert_eq!(png_dimensions(&destination), (5000, 64));
                // The rendered derivative must be non-trivial (a real render,
                // not a constant or an unadjusted frame of the raw bytes).
                let image = image::ImageReader::open(&destination)
                    .unwrap()
                    .decode()
                    .unwrap();
                let min = image.as_bytes().iter().copied().min().unwrap();
                let max = image.as_bytes().iter().copied().max().unwrap();
                assert!(max > min, "rendered derivative must not be constant");
            }
            Ok(ExportCompletion::Cancelled) => {
                panic!("a run with no cancellation may not report cancelled");
            }
            Err(ExportError::Unsupported(message)) => {
                assert!(
                    message.to_lowercase().contains("gpu"),
                    "without a device the failure must be an explicit GPU capability error, got {message}"
                );
                assert!(!destination.exists());
            }
            Err(other) => panic!("unexpected export failure: {other}"),
        }
    }

    #[test]
    fn restart_reexport_from_committed_sidecar_is_reproducible() {
        // Simulates export before and after an application restart: two
        // independent service instances resolve the same durable sidecar at
        // the requested revision; dimensions always match the committed
        // decode, and when both render, the pixels are identical.
        let bytes = wide_bytes();
        let dir = tmp_dir("restart");
        let source = write_source(&dir, "wide.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/export");
        commit_recipe(&repo, &source, "asset-r", &bytes, 0.75);

        let run = |name: &str| {
            // A fresh service (and fresh GPU renderer) per run: the engine
            // manager, decoded buffers and devices do not survive "restart".
            let service = DevelopService::with_parts(
                export_config(),
                Arc::new(NoopPreviewRenderer),
                Arc::new(GpuExportRenderer::new()),
                Arc::new(NullStore),
            );
            // The revision is re-resolved from the durable sidecar exactly
            // like after a restart; the request pins what was acknowledged.
            export_developed(
                &service,
                export_input(
                    &source,
                    &bytes,
                    "asset-r",
                    RecipeRepository::new("lap-test/export")
                        .current_revision(&source)
                        .unwrap()
                        .expect("committed revision"),
                ),
                png_settings(&dir.join(name)),
                &CancelToken::pair().1,
                &ExportCancelSlot::new(),
            )
        };

        let first = run("restart-1.png");
        let second = run("restart-2.png");
        match (first, second) {
            (
                Ok(ExportCompletion::Completed { receipt: a }),
                Ok(ExportCompletion::Completed { receipt: b }),
            ) => {
                assert_eq!((a.width, a.height), (5000, 64));
                assert_eq!((a.width, a.height), (b.width, b.height));
                assert_eq!(a.revision, b.revision);
                assert_eq!(
                    fs::read(&a.destination).unwrap(),
                    fs::read(&b.destination).unwrap(),
                    "restart must not change the rendered derivative bytes"
                );
            }
            (Err(a), Err(b)) => {
                for err in [a, b] {
                    let ExportError::Unsupported(message) = err else {
                        panic!("only explicit GPU capability failures are acceptable, got {err:?}");
                    };
                    assert!(
                        message.to_lowercase().contains("gpu"),
                        "failures must be explicit GPU capability errors, got {message}"
                    );
                }
            }
            (other_a, other_b) => {
                panic!("inconsistent outcomes across restarts: {other_a:?} vs {other_b:?}");
            }
        }
    }

    #[test]
    fn lut_resources_resolve_explicitly_in_the_export_renderer() {
        use crate::develop::resources::{ResourceStore, lut_resource_id};
        use rapidraw_edit_model::{ResourceAlgorithm, ResourceRef};

        fn failing_gpu() -> GpuContextFactory {
            Box::new(|| {
                rapidraw_develop::gpu::OffscreenGpuContext::new_with_backends(
                    wgpu::Backends::empty(),
                )
            })
        }

        let digest = "e".repeat(64);
        let id = lut_resource_id(&digest);
        let mut envelope = RecipeEnvelope::new("lap-test/0", "asset-a", "default", &"a".repeat(64));
        envelope.recipe.lut_path = Some(format!("resource://{id}"));
        envelope.resources.insert(
            id.clone(),
            ResourceRef {
                algorithm: ResourceAlgorithm::Sha256,
                digest,
                size_bytes: Some(1),
            },
        );

        // No store: the engine's ResourceMissing check remains the backstop.
        let renderer = GpuExportRenderer::with_context_factory(failing_gpu());
        assert!(renderer.resolve_render_lut(&envelope).unwrap().is_none());

        // With a store but an absent object: explicit typed failure.
        let store = ResourceStore::open(&tmp_dir("lut-missing")).expect("store opens");
        let renderer = GpuExportRenderer::with_context_factory(failing_gpu())
            .with_resource_store(Some(Arc::new(store)));
        let err = renderer
            .resolve_render_lut(&envelope)
            .expect_err("a missing LUT resource must fail explicitly");
        assert!(
            matches!(err, EngineError::Unsupported(ref message) if message.contains("missing")),
            "missing LUT resources surface as explicit failures, got {err:?}"
        );
    }

    #[test]
    fn missing_lens_profile_fails_the_export_before_the_gpu() {
        use crate::develop::resources::{ResourceStore, attach_lens_profile};

        let store = Arc::new(ResourceStore::open(&tmp_dir("lens-export-missing")).expect("store"));
        let renderer = GpuExportRenderer::with_context_factory(Box::new(|| {
            rapidraw_develop::gpu::OffscreenGpuContext::new_with_backends(wgpu::Backends::empty())
        }))
        .with_resource_store(Some(Arc::clone(&store)));

        // A recipe without a lens profile resolves to nothing.
        let plain = RecipeEnvelope::new("lap-test/0", "asset-a", "default", &"a".repeat(64));
        assert!(
            renderer
                .resolve_render_lens_profile(&plain)
                .unwrap()
                .is_none()
        );

        // Attach a profile, then make the object disappear: the export must
        // fail naming the profile BEFORE any GPU work.
        let imported = store
            .import_lens_profile_bytes(
                b"<?xml version=\"1.0\"?><lensdatabase><lens><maker>M</maker><model>L</model><mount>T</mount></lens></lensdatabase>",
                None,
                Some("v1"),
            )
            .expect("profile imports");
        let mut envelope = RecipeEnvelope::new("lap-test/0", "asset-a", "default", &"a".repeat(64));
        attach_lens_profile(&mut envelope, &imported, "M", "L");
        let _ = &mut envelope;

        let digest = imported.digest.clone();
        let object = {
            let root = store.root().to_path_buf();
            root.join("objects").join(&digest[..2]).join(&digest)
        };
        std::fs::remove_file(&object).expect("object removed to simulate a lost resource");

        let err = renderer
            .resolve_render_lens_profile(&envelope)
            .expect_err("a missing lens profile must fail the export explicitly");
        assert!(
            matches!(err, EngineError::Unsupported(ref m) if m.to_lowercase().contains("missing")),
            "got {err:?}"
        );

        // Full render path: the profile failure must surface before the GPU
        // capability failure.
        let decoded = Arc::new(
            rapidraw_develop::decode_original(&gradient_bytes(), &Default::default())
                .expect("decode"),
        );
        let (cancel_source, cancel) = rapidraw_develop::CancelToken::pair();
        let _ = cancel_source;
        let job = ExportJob {
            job_id: rapidraw_develop::ExportJobId(1),
            asset_id: "asset-a".to_string(),
            variant_id: "default".to_string(),
            revision: envelope.revision,
            original: decoded,
            envelope,
            max_edge: None,
            cancel,
        };
        let err = renderer
            .render(&job)
            .expect_err("a missing profile must fail the export");
        let message = err.to_string().to_lowercase();
        assert!(
            message.contains("lens") && !message.contains("gpu"),
            "the profile check must precede GPU work, got {message}"
        );
    }
}

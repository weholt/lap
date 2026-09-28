//! Lap host adapter for the pinned `rapidraw-develop` bounded edit sessions
//! (lap-a52 / TASK-302; governing contract `docs/raw-development/spec.md`).
//!
//! This module is the only place where Lap's Tauri layer meets the engine's
//! session machinery. It owns:
//!
//! - [`DevelopService`]: asset-scoped session lifetimes (`open`, `render`,
//!   `commit`, `close`) over the engine `SessionManager`, with explicit
//!   asset/variant/session/revision identities on every call (spec A4).
//! - A bounded preview handle registry: rendered frames are stored server-side
//!   under an opaque handle and fetched as raw bytes; previews never travel as
//!   large base64 JSON payloads (spec, "Use bounded buffers/asset transport
//!   for previews").
//! - The revision translation between Lap's durable sidecar revisions (0 =
//!   unedited initial state) and the engine session revisions (1 = initial
//!   state): `engine revision = sidecar revision + 1`.
//! - Explicit capability reporting. A missing/unsupported GPU is a typed
//!   failure, never a silent fallback or a fake success (spec A7/A11).
//!
//! Decode boundary: opened assets are decoded with the engine's
//! [`rapidraw_develop::decode_original`] (full-dimension, linear, high
//! precision). Embedded or 8-bit previews are never a development source, and
//! Lap's LibRaw browsing path for unedited assets is untouched by this module.
//!
//! The engine is consumed as an immutable local git dependency pinned at
//! [`ENGINE_GIT_REVISION`]; see `docs/raw-development/engine-lock.json`.

/// The exact engine revision Lap consumes. This constant must match
/// `docs/raw-development/engine-lock.json` (`engine.revision`) and the
/// `rev=` recorded for both engine crates in `src-tauri/Cargo.lock`
/// (enforced by `cargo_lock_pins_engine_revision`).
pub const ENGINE_GIT_REVISION: &str = "de4fdbd76723c76145b477a2b8874226512475f1";

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use rapidraw_develop::gpu::{
    LutData, OffscreenGpuContext, OffscreenRenderer, OutputTarget, RenderRequest,
    get_all_adjustments_from_json,
};
use rapidraw_develop::session::{
    ClosedSession, ExportJob, ExportRenderer, OpenSessionRequest, OpenedSession, PreviewFrame,
    PreviewJob, PreviewOutcome, PreviewQuality, PreviewRenderer, PreviewRequest, PreviewTicket,
    RecipeStore, SessionError, SessionId, SessionManager, SessionManagerConfig,
};
use rapidraw_develop::{
    DecodeOptions, DecodedOriginal, DevelopError as EngineError, LinearImage, decode_original,
};
use rapidraw_edit_model::RecipeEnvelope;
use serde::Serialize;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Bounded host-side configuration wrapping the engine session bounds.
#[derive(Debug, Clone)]
pub struct DevelopConfig {
    /// Engine session/queue bounds (worker counts, queue depths, caches).
    pub sessions: SessionManagerConfig,
    /// Maximum accepted preview edge length. Rejects unbounded transport
    /// requests instead of rendering arbitrarily large frames.
    pub max_preview_edge: u32,
    /// Byte budget of the preview handle registry (bounded pixels transport).
    pub max_cached_preview_bytes: usize,
}

impl Default for DevelopConfig {
    fn default() -> Self {
        Self {
            sessions: SessionManagerConfig::default(),
            max_preview_edge: 4096,
            max_cached_preview_bytes: 128 * 1024 * 1024,
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Typed host-side failure. Every variant is explicit and surfaced to the
/// frontend; nothing degrades into a fake success (spec A7).
#[derive(Debug)]
pub enum DevelopError {
    NotFound {
        session_id: u64,
    },
    SessionClosed {
        session_id: u64,
    },
    StaleGeneration {
        session_id: u64,
        accepted: u64,
        requested: u64,
    },
    RevisionConflict {
        session_id: u64,
        expected: u64,
        current: u64,
    },
    TooManySessions {
        limit: usize,
    },
    PreviewQueueFull {
        limit: usize,
    },
    InvalidInput(String),
    /// Offscreen GPU unavailable/lost/oversized: capability-bound failure.
    Gpu(String),
    Decode(String),
    /// Feature explicitly unsupported in this slice (masks, LUT resources,
    /// durable export): reported instead of silently dropped.
    Unsupported(String),
    Persist(String),
    PreviewNotFound(String),
    SourceReplaced {
        path: PathBuf,
        expected: String,
        found: String,
    },
}

impl std::fmt::Display for DevelopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DevelopError::NotFound { session_id } => {
                write!(f, "develop session {session_id} not found")
            }
            DevelopError::SessionClosed { session_id } => {
                write!(f, "develop session {session_id} is closed")
            }
            DevelopError::StaleGeneration {
                session_id,
                accepted,
                requested,
            } => write!(
                f,
                "preview generation {requested} is stale for session {session_id}; accepted generation is {accepted}"
            ),
            DevelopError::RevisionConflict {
                session_id,
                expected,
                current,
            } => write!(
                f,
                "develop session {session_id} revision conflict: expected {expected}, current is {current}"
            ),
            DevelopError::TooManySessions { limit } => {
                write!(f, "develop session limit of {limit} reached")
            }
            DevelopError::PreviewQueueFull { limit } => {
                write!(f, "develop preview queue limit of {limit} reached")
            }
            DevelopError::InvalidInput(message) => write!(f, "invalid develop request: {message}"),
            DevelopError::Gpu(message) => write!(f, "development GPU unavailable: {message}"),
            DevelopError::Decode(message) => write!(f, "development decode failed: {message}"),
            DevelopError::Unsupported(message) => {
                write!(f, "unsupported development feature: {message}")
            }
            DevelopError::Persist(message) => write!(f, "develop recipe save failed: {message}"),
            DevelopError::PreviewNotFound(handle) => {
                write!(f, "preview handle '{handle}' expired or unknown")
            }
            DevelopError::SourceReplaced {
                path,
                expected,
                found,
            } => write!(
                f,
                "source at {} was replaced after its recipe was written (fingerprint {expected}, found {found})",
                path.display()
            ),
        }
    }
}

impl std::error::Error for DevelopError {}

// ---------------------------------------------------------------------------
// Data contracts (serde shapes shared with the Tauri commands and frontend)
// ---------------------------------------------------------------------------

/// A successfully rendered preview, identified by bounded-transport handle.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTicketMeta {
    pub session_id: u64,
    pub asset_id: String,
    pub variant_id: String,
    pub generation: u64,
    pub quality: PreviewQuality,
    pub width: u32,
    pub height: u32,
    /// Opaque handle for `develop_take_preview_frame`; pixels never travel in
    /// this JSON payload.
    pub handle: String,
    pub byte_len: usize,
}

/// Final state of one preview request as observed by the host caller.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum PreviewWait {
    Completed { ticket: PreviewTicketMeta },
    Cancelled,
    Failed { code: String, message: String },
}

/// Result of opening an edit session. `revision` is Lap's durable
/// sidecar-aligned revision (0 = unedited initial state).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedEditSession {
    pub session_id: u64,
    pub asset_id: String,
    pub variant_id: String,
    pub revision: u64,
    pub dimensions: (u32, u32),
    pub source_fingerprint: String,
    /// The committed envelope JSON (sidecar-aligned revision) the frontend
    /// edits and sends back for render/commit.
    pub envelope: serde_json::Value,
}

/// Result of closing a session. `revision` is sidecar-aligned.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosedEditSession {
    pub session_id: u64,
    pub revision: u64,
    pub released_preview_cache_entries: usize,
    pub released_preview_cache_bytes: usize,
}

/// Durable commit acknowledgment. `revision` is the new sidecar-aligned
/// revision (`expected_revision + 1`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitReceiptDto {
    pub session_id: u64,
    pub revision: u64,
    pub content_hash: Option<String>,
    pub sidecar_path: Option<PathBuf>,
    pub projection_applied: bool,
    pub projection_error: Option<String>,
}

/// GPU device status of the offscreen renderer (explicit capability report).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuStatus {
    pub available: bool,
    pub error: Option<String>,
    pub adapter_name: String,
    pub backend: String,
    pub max_texture_dimension_2d: u32,
    pub max_buffer_size: u64,
}

/// Engine identity + device capability report (spec A11/A7 evidence).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityReport {
    pub gpu: GpuStatus,
    pub engine_revision: &'static str,
    pub edit_model_version: String,
    pub schema_version: u32,
    pub preview_max_edge: u32,
}

/// Everything the host supplies to open one asset's edit session. The Tauri
/// command layer performs the catalog asset lookup and sidecar load and hands
/// the fully resolved inputs to the service.
pub struct AssetEditInput {
    pub asset_id: String,
    pub variant_id: String,
    pub source_path: PathBuf,
    /// SHA-256 of the current source bytes (host fingerprint).
    pub source_fingerprint: String,
    pub source_bytes: Vec<u8>,
    /// Committed envelope loaded from the adjacent `.lapedit.json` sidecar, if
    /// any. `None` means the asset is unedited.
    pub sidecar: Option<RecipeEnvelope>,
}

// ---------------------------------------------------------------------------
// Bounded preview handle registry
// ---------------------------------------------------------------------------

#[derive(Default)]
struct BoundedPreviewCache {
    entries: VecDeque<(String, u64, PreviewFrame)>,
    bytes: usize,
    max_bytes: usize,
}

impl BoundedPreviewCache {
    fn new(max_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            max_bytes,
        }
    }

    fn insert(&mut self, handle: String, session_id: u64, frame: PreviewFrame) {
        let len = frame.rgba8.len();
        // Evict oldest until the new frame fits; the newest frame is always
        // kept (bounded previews never grow the registry past one oversize
        // frame, which the preview edge bound already caps).
        while self.bytes + len > self.max_bytes {
            match self.entries.pop_front() {
                Some((_, _, evicted)) => self.bytes -= evicted.rgba8.len(),
                None => break,
            }
        }
        self.bytes += len;
        self.entries.push_back((handle, session_id, frame));
    }

    fn take(&mut self, handle: &str) -> Option<PreviewFrame> {
        let index = self
            .entries
            .iter()
            .position(|(candidate, _, _)| candidate == handle)?;
        let entry = self.entries.remove(index)?;
        self.entries.push_back(entry.clone());
        Some(entry.2)
    }

    fn release_session(&mut self, session_id: u64) -> (usize, usize) {
        let mut released_entries = 0usize;
        let mut released_bytes = 0usize;
        self.entries.retain(|(_, sid, frame)| {
            if *sid == session_id {
                released_entries += 1;
                released_bytes += frame.rgba8.len();
                false
            } else {
                true
            }
        });
        self.bytes -= released_bytes;
        (released_entries, released_bytes)
    }
}

fn new_preview_handle(session_id: u64) -> String {
    format!("{:016x}-{}", session_id, uuid::Uuid::new_v4().simple())
}

// ---------------------------------------------------------------------------
// Shared recipe geometry (lap-6bc): one host-side implementation for the
// preview and the export renderers, so both apply the engine's oriented
// coordinate system identically.
// ---------------------------------------------------------------------------

/// Applies the recipe's coarse geometry to a decoded linear image, in the
/// render order the engine documents: quarter turns, then the horizontal and
/// the vertical flip, then the normalized crop in the oriented frame. Invalid
/// crops are typed errors, never silently ignored.
pub(crate) fn apply_recipe_geometry(
    image: &LinearImage,
    recipe: &rapidraw_edit_model::Recipe,
) -> Result<LinearImage, EngineError> {
    let mut oriented = image.clone();
    let orientation_steps = (recipe.orientation_steps % 4) as u8;
    if orientation_steps != 0 {
        oriented = rapidraw_develop::apply_coarse_rotation(&oriented, orientation_steps);
    }
    if recipe.flip_horizontal || recipe.flip_vertical {
        oriented =
            rapidraw_develop::apply_flip(&oriented, recipe.flip_horizontal, recipe.flip_vertical);
    }
    if let Some(crop) = &recipe.crop {
        oriented = rapidraw_develop::apply_crop_normalized(&oriented, crop)?;
    }
    Ok(oriented)
}

/// Stable hash of exactly the recipe fields that change the geometry-applied
/// input pixels. The renderers' input caches are keyed by their transform
/// hash, so a geometry change at a constant output size (e.g. a 180-degree
/// turn) must change this value or previews would reuse a stale texture.
/// Lens-correction warp inputs are geometry (lap-d52): any coefficient,
/// amount, enable flag or manual distortion change re-renders the warped
/// input, while adjustment-only edits keep the cached input.
pub(crate) fn recipe_geometry_hash(recipe: &rapidraw_edit_model::Recipe) -> u64 {
    const PRIME: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut hash = (recipe.orientation_steps % 4) as u64;
    hash = hash.wrapping_mul(PRIME)
        ^ (u64::from(recipe.flip_horizontal)).rotate_left(17)
        ^ (u64::from(recipe.flip_vertical)).rotate_left(31);
    if let Some(crop) = &recipe.crop {
        for value in [crop.x, crop.y, crop.width, crop.height] {
            hash = hash.wrapping_mul(PRIME) ^ value.to_bits();
        }
    } else {
        hash = hash.wrapping_mul(PRIME) ^ 0x517C_C1B7_2722_0A95;
    }
    let lens = rapidraw_develop::LensWarpParams::from_recipe(recipe);
    for value in [
        lens.distortion,
        lens.lens_dist_k1,
        lens.lens_dist_k2,
        lens.lens_dist_k3,
        lens.lens_distortion_amount,
        lens.tca_vr,
        lens.tca_vb,
        lens.lens_tca_amount,
        lens.vig_k1,
        lens.vig_k2,
        lens.vig_k3,
        lens.lens_vignette_amount,
    ] {
        hash = hash.wrapping_mul(PRIME) ^ value.to_bits();
    }
    for flag in [
        lens.lens_distortion_enabled,
        lens.lens_tca_enabled,
        lens.lens_vignette_enabled,
    ] {
        hash = hash.wrapping_mul(PRIME) ^ (u64::from(flag)).rotate_left(11);
    }
    hash = hash.wrapping_mul(PRIME) ^ u64::from(lens.lens_model);
    hash
}

/// Builds the mask rasterization frame for a render of `out_w x out_h` from
/// a decoded source of `source_dims` with the recipe's geometry applied.
/// Mask geometry lives in the oriented, un-cropped frame (lap-78d), so the
/// frame carries the oriented full dimensions and the crop offset; crop and
/// orientation never move a mask relative to the image content.
pub(crate) fn mask_raster_frame(
    source_dims: (u32, u32),
    recipe: &rapidraw_edit_model::Recipe,
    out_w: u32,
    out_h: u32,
) -> Result<rapidraw_develop::MaskRasterFrame, EngineError> {
    let steps = recipe.orientation_steps % 4;
    let oriented = if steps == 1 || steps == 3 {
        (source_dims.1, source_dims.0)
    } else {
        source_dims
    };
    rapidraw_develop::MaskRasterFrame::new(oriented.0, oriented.1, recipe.crop, out_w, out_h)
        .map_err(|err| EngineError::InvalidInput(err.to_string()))
}

/// Rasterizes the recipe's supported visible masks for one render, mapping
/// engine errors onto typed host errors. Unsupported kinds name the mask and
/// kind; they are never silently dropped (spec A9).
pub(crate) fn rasterize_recipe_masks(
    recipe: &rapidraw_edit_model::Recipe,
    frame: &rapidraw_develop::MaskRasterFrame,
) -> Result<Vec<image::GrayImage>, EngineError> {
    rapidraw_develop::rasterize_visible_masks(&recipe.masks, frame)
        .map_err(|err| EngineError::Unsupported(err.to_string()))
}

// ---------------------------------------------------------------------------
// GPU preview renderer
// ---------------------------------------------------------------------------

/// Host preview renderer: renders a preview generation from the session's
/// linear original through the engine's offscreen GPU pipeline.
///
/// The recipe's geometry (quarter turns, flips, normalized crop) is applied
/// first, exactly like the export renderer; the oriented result is then
/// downscaled (linear f32, before any adjustment) to the requested preview
/// size so the transport stays bounded, then rendered with the recipe's
/// adjustments. Missing devices, device loss, oversized textures and missing
/// LUT resources surface as explicit engine errors, never as an unadjusted
/// frame.
///
/// When a [`ResourceStore`] is installed, LUT references are resolved from
/// the content-addressed store; missing or changed resources fail explicitly
/// (spec A7) instead of rendering un-LUT-ed pixels. Without a store the
/// engine's own `ResourceMissing` check remains the backstop.
pub struct GpuPreviewRenderer {
    renderer: OnceLock<Result<OffscreenRenderer, String>>,
    factory: Mutex<Option<GpuContextFactory>>,
    resources: Option<Arc<crate::develop::resources::ResourceStore>>,
    /// Bounded mask-bitmap cache (lap-78d). Keyed by mask geometry content
    /// plus frame; adjustment-only edits never invalidate geometry bitmaps.
    mask_cache: Mutex<rapidraw_develop::BoundedMaskCache>,
}

impl GpuPreviewRenderer {
    /// Production constructor: environment-selected offscreen device.
    pub fn new() -> Self {
        Self::with_context_factory(Box::new(OffscreenGpuContext::new))
    }

    /// Constructor with an explicit context factory (tests inject a
    /// deterministically failing device via an empty backend set).
    pub fn with_context_factory(factory: GpuContextFactory) -> Self {
        Self {
            renderer: OnceLock::new(),
            factory: Mutex::new(Some(factory)),
            resources: None,
            mask_cache: Mutex::new(rapidraw_develop::BoundedMaskCache::new()),
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

    /// The installed resource store, if any (shared with the export renderer).
    pub fn resource_store(&self) -> Option<Arc<crate::develop::resources::ResourceStore>> {
        self.resources.clone()
    }

    /// Resolves the recipe's LUT payload from the resource store. A store
    /// error (missing, changed, non-portable reference) is an explicit
    /// failure, never a silent un-LUT-ed render.
    fn resolve_render_lut(
        &self,
        envelope: &RecipeEnvelope,
    ) -> Result<Option<Arc<LutData>>, EngineError> {
        match &self.resources {
            None => Ok(None),
            Some(store) => store
                .resolve_recipe_lut(envelope)
                .map_err(|err| EngineError::Unsupported(err.to_string())),
        }
    }

    /// Verifies the recipe's lens-profile reference before any pixel work
    /// (lap-d52): the parsed profile database when a present, unchanged
    /// profile is referenced; `None` without a reference. A referenced
    /// profile is a *required* capability input — missing, changed or
    /// unmapped resources (and renders without a store that could verify
    /// them) fail explicitly instead of silently changing the correction,
    /// matching the LUT behavior and spec A7.
    pub(crate) fn resolve_render_lens_profile(
        &self,
        envelope: &RecipeEnvelope,
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
            if let Some(result) = self.renderer.get() {
                return result
                    .as_ref()
                    .map_err(|detail| gpu_unsupported(detail.clone()));
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
                    // Only the factory holder writes, so the winner's result
                    // (success or honest failure) is the permanent state.
                    let _ = self.renderer.set(result);
                }
                // Another thread is initializing; wait for its result.
                None => std::thread::yield_now(),
            }
        }
    }

    /// Probe the device and return an explicit capability report.
    pub fn probe(&self) -> GpuStatus {
        match self.renderer() {
            Ok(renderer) => {
                let caps = renderer.capabilities();
                GpuStatus {
                    available: true,
                    error: None,
                    adapter_name: caps.adapter_name,
                    backend: caps.backend,
                    max_texture_dimension_2d: caps.max_texture_dimension_2d,
                    max_buffer_size: caps.max_buffer_size,
                }
            }
            Err(error) => GpuStatus {
                available: false,
                error: Some(error.to_string()),
                ..GpuStatus::default()
            },
        }
    }
}

impl Default for GpuPreviewRenderer {
    fn default() -> Self {
        Self::new()
    }
}

fn gpu_unsupported(detail: String) -> EngineError {
    EngineError::Unsupported(format!("GPU adapter unavailable or unusable: {detail}"))
}

impl PreviewRenderer for GpuPreviewRenderer {
    fn render(&self, job: &PreviewJob) -> Result<PreviewFrame, EngineError> {
        job.cancel.check()?;

        // Lens-profile verification precedes every pixel work (lap-d52): a
        // referenced profile that is missing, changed or unverifiable fails
        // explicitly instead of silently changing the correction. This runs
        // BEFORE device initialization so a lost resource never hides behind
        // a GPU capability error.
        self.resolve_render_lens_profile(&job.envelope)?;

        let renderer = self.renderer()?;

        // Lens correction warp on the full un-cropped frame, matching the
        // pinned reference order (warp -> coarse rotation -> flip -> crop):
        // identity lens fields never touch the pixels.
        let lens_params = rapidraw_develop::LensWarpParams::from_recipe(&job.envelope.recipe);
        let lens_warped;
        let lens_corrected: &rapidraw_develop::LinearImage = if lens_params.is_identity() {
            &job.original.image
        } else {
            lens_warped = rapidraw_develop::lens_warp(&job.original.image, &lens_params);
            &lens_warped
        };

        // Recipe geometry in the engine's oriented coordinate system, applied
        // by the SAME helper the export renderer uses (lap-6bc): preview and
        // export can never diverge on crop/rotation/flip.
        let oriented = apply_recipe_geometry(lens_corrected, &job.envelope.recipe)?;
        let (width, height) = oriented.dimensions();
        let (target_w, target_h) = preview_dimensions(width, height, job.max_edge);
        let base = preview_base(&oriented, target_w, target_h)?;

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
        // Supported visible masks rasterize into the processing output
        // space (layer i aligns with visible mask i in `adjustments`).
        // Unsupported kinds fail explicitly; adjustment-only edits reuse
        // cached geometry bitmaps through the bounded cache.
        let mask_frame = mask_raster_frame(
            job.original.image.dimensions(),
            &job.envelope.recipe,
            target_w,
            target_h,
        )?;
        let mask_bitmaps = {
            let mut cache = self
                .mask_cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache
                .get_or_rasterize(&job.envelope.recipe.masks, &mask_frame, || {
                    rapidraw_develop::rasterize_visible_masks(
                        &job.envelope.recipe.masks,
                        &mask_frame,
                    )
                })
                .map_err(|err| EngineError::Unsupported(err.to_string()))?
        };
        // Stable per (session base, preview size, geometry): identical
        // previews reuse the uploaded input texture; different sessions or a
        // geometry change never collide.
        let transform_hash = job.session_id.0.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ ((target_w as u64) << 32)
            ^ target_h as u64
            ^ recipe_geometry_hash(&job.envelope.recipe);

        let request = RenderRequest {
            adjustments,
            mask_bitmaps: &mask_bitmaps,
            // Resolved from the content-addressed resource store when one is
            // installed; a missing/changed resource fails explicitly above.
            // Without a store the engine fails with ResourceMissing below
            // (explicit, never a silently un-LUT-ed render).
            lut: self.resolve_render_lut(&job.envelope)?,
            roi: None,
        };
        let pixels = renderer
            .render(&base, transform_hash, request, OutputTarget::CpuPixels)
            .map_err(|err| EngineError::Unsupported(err.to_string()))?;
        Ok(PreviewFrame {
            width: pixels.width,
            height: pixels.height,
            rgba8: pixels.pixels,
        })
    }
}

/// Engine code for the envelope's recorded tone-mapper override, shared by
/// the preview and export renderers.
pub(crate) fn tonemapper_override_code(tone_mapper: rapidraw_edit_model::ToneMapper) -> u32 {
    match tone_mapper {
        rapidraw_edit_model::ToneMapper::Agx => 1,
        rapidraw_edit_model::ToneMapper::Basic => 0,
    }
}

fn preview_dimensions(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
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

fn preview_base(
    image: &LinearImage,
    target_w: u32,
    target_h: u32,
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
    let small = if (target_w, target_h) == (width, height) {
        full
    } else {
        image::imageops::resize(
            &full,
            target_w,
            target_h,
            image::imageops::FilterType::Triangle,
        )
    };
    // High-precision input: RGBA f32 linear (opaque alpha, matching the
    // engine's rgba32f layout), the same shape the engine host feeds its
    // offscreen renderer.
    let rgba = image::ImageBuffer::from_fn(target_w, target_h, |x, y| {
        let pixel = small.get_pixel(x, y);
        image::Rgba([pixel[0], pixel[1], pixel[2], 1.0])
    });
    Ok(image::DynamicImage::ImageRgba32F(rgba))
}

// ---------------------------------------------------------------------------
// Durable store bridge
// ---------------------------------------------------------------------------
/// Context factory injected at construction (tests substitute a
/// deterministically failing device).
pub type GpuContextFactory =
    Box<dyn FnOnce() -> Result<OffscreenGpuContext, rapidraw_develop::gpu::GpuError> + Send>;

/// Connection handle handed to a projection write: derefs to a SQLite
/// connection, allowing hosts to return pooled wrappers.
pub type ProjectionConn = Box<dyn std::ops::Deref<Target = rusqlite::Connection> + 'static>;

type ProjectionConnFactory = Arc<dyn Fn() -> Result<ProjectionConn, String> + Send + Sync>;

/// Engine `RecipeStore` backed by Lap's atomic `RecipeRepository` sidecar.
///
/// Revision translation (see the module docs): the engine session counts the
/// initial state as revision 1, so `engine revision = sidecar revision + 1`.
/// `persist` receives the engine-aligned durable envelope and writes the
/// sidecar at `revision - 1` with a compare-and-swap against `revision - 2`
/// (0 = no sidecar yet). Persist receipts are retained per (asset, revision)
/// so the Tauri commit response can surface content hash and projection
/// status without a second disk read.
pub struct SidecarBackedStore {
    repo: crate::develop::recipe_repository::RecipeRepository,
    paths: Mutex<HashMap<String, PathBuf>>,
    conn_factory: Mutex<Option<ProjectionConnFactory>>,
    receipts: Mutex<HashMap<(String, u64), crate::develop::recipe_repository::CommitReceipt>>,
}

impl SidecarBackedStore {
    pub fn new(repo: crate::develop::recipe_repository::RecipeRepository) -> Self {
        Self {
            repo,
            paths: Mutex::new(HashMap::new()),
            conn_factory: Mutex::new(None),
            receipts: Mutex::new(HashMap::new()),
        }
    }

    /// Install the catalog projection connection factory (production wires
    /// Lap's pooled `open_conn`); projection failures never fail the save.
    pub fn with_conn_factory(self, factory: ProjectionConnFactory) -> Self {
        *self
            .conn_factory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(factory);
        self
    }

    pub fn register_path(&self, asset_id: &str, path: &std::path::Path) {
        self.paths
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(asset_id.to_string(), path.to_path_buf());
    }

    pub fn receipt(
        &self,
        asset_id: &str,
        sidecar_revision: u64,
    ) -> Option<crate::develop::recipe_repository::CommitReceipt> {
        self.receipts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&(asset_id.to_string(), sidecar_revision))
            .cloned()
    }
}

impl RecipeStore for SidecarBackedStore {
    fn persist(&self, envelope: &RecipeEnvelope) -> Result<(), String> {
        let asset_id = envelope.asset_id.clone();
        let source_path = {
            let paths = self
                .paths
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            paths
                .get(&asset_id)
                .cloned()
                .ok_or_else(|| format!("no source path registered for asset '{asset_id}'"))?
        };
        let sidecar_revision = envelope
            .revision
            .checked_sub(1)
            .filter(|revision| *revision >= 1)
            .ok_or_else(|| {
                format!(
                    "engine revision {} has no sidecar successor",
                    envelope.revision
                )
            })?;
        let expected = sidecar_revision - 1;

        let mut durable = envelope.clone();
        durable.revision = sidecar_revision;

        let conn = match &*self
            .conn_factory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            Some(factory) => Some(
                factory().map_err(|err| format!("catalog projection connection failed: {err}"))?,
            ),
            None => None,
        };

        let receipt = match conn {
            Some(boxed) => {
                let conn: &rusqlite::Connection = &boxed;
                self.repo
                    .commit(&source_path, expected, durable, Some(conn), None)
                    .map_err(|err| map_repo_error(&err))
            }
            None => self
                .repo
                .commit(&source_path, expected, durable, None, None)
                .map_err(|err| map_repo_error(&err)),
        }?;
        self.receipts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert((asset_id, sidecar_revision), receipt);
        Ok(())
    }
}

/// Machine-readable prefix so the service can surface a typed conflict
/// instead of an opaque persist error (spec A4).
fn map_repo_error(err: &crate::develop::recipe_repository::RecipeRepoError) -> String {
    if let crate::develop::recipe_repository::RecipeRepoError::RevisionConflict {
        expected,
        current,
        ..
    } = err
    {
        let current = current
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string());
        format!("revision-conflict:expected={expected},current={current}; {err}")
    } else {
        err.to_string()
    }
}

fn parse_persist_error(session_id: u64, message: &str) -> DevelopError {
    let prefix = "revision-conflict:expected=";
    if let Some(rest) = message.strip_prefix(prefix) {
        let leading_digits = |text: &str| -> Option<u64> {
            let digits: String = text.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                None
            } else {
                digits.parse().ok()
            }
        };
        // Format: revision-conflict:expected=<u64>,current=<u64|none>; ...
        let expected = leading_digits(rest);
        let current = rest
            .find(",current=")
            .and_then(|index| leading_digits(&rest[index + ",current=".len()..]));
        if let (Some(expected), Some(current)) = (expected, current) {
            return DevelopError::RevisionConflict {
                session_id,
                expected,
                current,
            };
        }
    }
    DevelopError::Persist(message.to_string())
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// Host-side develop service over the pinned engine's bounded sessions.
pub struct DevelopService {
    manager: SessionManager,
    sidecar: Option<Arc<SidecarBackedStore>>,
    gpu: Option<Arc<GpuPreviewRenderer>>,
    handles: Mutex<BoundedPreviewCache>,
    max_preview_edge: u32,
}

impl DevelopService {
    /// Constructor with fully injected parts (tests, alternative stores).
    /// No GPU capability probe is available for injected renderers.
    pub fn with_parts(
        config: DevelopConfig,
        preview_renderer: Arc<dyn PreviewRenderer>,
        export_renderer: Arc<dyn ExportRenderer>,
        store: Arc<dyn RecipeStore>,
    ) -> Self {
        Self::build(config, preview_renderer, export_renderer, store, None)
    }

    /// Like [`Self::with_parts`] but with the production GPU preview renderer
    /// installed for capability probing.
    pub fn with_parts_and_gpu(
        config: DevelopConfig,
        gpu: Arc<GpuPreviewRenderer>,
        export_renderer: Arc<dyn ExportRenderer>,
        store: Arc<dyn RecipeStore>,
    ) -> Self {
        Self::build(
            config,
            Arc::clone(&gpu) as Arc<dyn PreviewRenderer>,
            export_renderer,
            store,
            Some(gpu),
        )
    }

    /// Constructor wired to Lap's durable sidecar store.
    pub fn with_sidecar_store(
        config: DevelopConfig,
        preview_renderer: Arc<dyn PreviewRenderer>,
        export_renderer: Arc<dyn ExportRenderer>,
        store: Arc<SidecarBackedStore>,
    ) -> Self {
        Self::build(
            config,
            preview_renderer,
            export_renderer,
            Arc::clone(&store) as Arc<dyn RecipeStore>,
            None,
        )
        .with_sidecar(store)
    }

    /// Production constructor: GPU preview renderer, sidecar store, and the
    /// real GPU export renderer (lap-70c): durable derivative export runs
    /// through the same bounded engine export queue; a stub here would fail
    /// every `develop_export_developed` call in the application.
    pub fn with_gpu_and_sidecar_store(
        config: DevelopConfig,
        gpu: Arc<GpuPreviewRenderer>,
        store: Arc<SidecarBackedStore>,
    ) -> Self {
        let preview: Arc<dyn PreviewRenderer> = Arc::clone(&gpu) as _;
        Self::build(
            config,
            preview,
            Arc::new(
                super::export::GpuExportRenderer::new().with_resource_store(gpu.resource_store()),
            ),
            Arc::clone(&store) as Arc<dyn RecipeStore>,
            Some(gpu),
        )
        .with_sidecar(store)
    }

    fn with_sidecar(mut self, store: Arc<SidecarBackedStore>) -> Self {
        self.sidecar = Some(store);
        self
    }

    fn build(
        config: DevelopConfig,
        preview_renderer: Arc<dyn PreviewRenderer>,
        export_renderer: Arc<dyn ExportRenderer>,
        store: Arc<dyn RecipeStore>,
        gpu: Option<Arc<GpuPreviewRenderer>>,
    ) -> Self {
        let max_preview_edge = config.max_preview_edge;
        let max_cached_preview_bytes = config.max_cached_preview_bytes;
        let manager =
            SessionManager::new(config.sessions, preview_renderer, export_renderer, store)
                .expect("valid session manager configuration");
        Self {
            manager,
            sidecar: None,
            gpu,
            handles: Mutex::new(BoundedPreviewCache::new(max_cached_preview_bytes)),
            max_preview_edge,
        }
    }

    /// Opens an asset-scoped session: verifies the source fingerprint against
    /// the committed sidecar, decodes the original with the shared
    /// high-precision pipeline (envelope decode settings applied), and hands
    /// ownership to the engine session manager.
    pub fn open_session(&self, input: AssetEditInput) -> Result<OpenedEditSession, DevelopError> {
        if input.asset_id.is_empty() || input.variant_id.is_empty() {
            return Err(DevelopError::InvalidInput(
                "asset id and variant id must be non-empty".to_string(),
            ));
        }
        if let Some(sidecar) = &input.sidecar {
            if sidecar.asset_id != input.asset_id || sidecar.variant_id != input.variant_id {
                return Err(DevelopError::InvalidInput(format!(
                    "sidecar identity ({}, {}) does not match the request ({}, {})",
                    sidecar.asset_id, sidecar.variant_id, input.asset_id, input.variant_id
                )));
            }
            if sidecar.source_fingerprint != input.source_fingerprint {
                return Err(DevelopError::SourceReplaced {
                    path: input.source_path.clone(),
                    expected: sidecar.source_fingerprint.clone(),
                    found: input.source_fingerprint.clone(),
                });
            }
        }

        // Engine-aligned envelope: sidecar revision + 1 (fresh = 1).
        let engine_envelope = match &input.sidecar {
            Some(sidecar) => {
                let mut envelope = sidecar.clone();
                envelope.revision = sidecar.revision.checked_add(1).ok_or_else(|| {
                    DevelopError::InvalidInput("sidecar revision overflow".to_string())
                })?;
                envelope
            }
            None => {
                let mut envelope = RecipeEnvelope::new(
                    &format!(
                        "lap/{}/rapidraw-edit-model/{}",
                        env!("CARGO_PKG_VERSION"),
                        rapidraw_edit_model::MODEL_VERSION
                    ),
                    &input.asset_id,
                    &input.variant_id,
                    &input.source_fingerprint,
                );
                envelope.revision = 1;
                envelope
            }
        };

        // High-precision decode with the recipe's recorded effective settings;
        // there is no embedded/8-bit preview fallback.
        let decode = &engine_envelope.decode;
        let options = DecodeOptions {
            fast_demosaic: decode.fast_demosaic,
            highlight_compression: decode.highlight_compression as f32,
            linear_mode: decode.linear_raw_mode,
            tone_mapper: decode.tonemapper_override,
            ..DecodeOptions::default()
        };
        let decoded: DecodedOriginal =
            decode_original(&input.source_bytes, &options).map_err(|err| match err {
                EngineError::Decode(message) => DevelopError::Decode(message),
                other => DevelopError::Decode(other.to_string()),
            })?;

        if let Some(store) = &self.sidecar {
            store.register_path(&input.asset_id, &input.source_path);
        }

        let opened: OpenedSession = self.manager.open_session(OpenSessionRequest {
            asset_id: input.asset_id.clone(),
            variant_id: input.variant_id.clone(),
            source_fingerprint: input.source_fingerprint.clone(),
            original: Arc::new(decoded),
            envelope: engine_envelope,
        })?;

        // Sidecar-aligned envelope for the frontend.
        let mut envelope_value = match &input.sidecar {
            Some(sidecar) => serde_json::to_value(sidecar).map_err(|err| {
                DevelopError::InvalidInput(format!("sidecar serialization failed: {err}"))
            })?,
            None => {
                let mut fresh = RecipeEnvelope::new(
                    &format!(
                        "lap/{}/rapidraw-edit-model/{}",
                        env!("CARGO_PKG_VERSION"),
                        rapidraw_edit_model::MODEL_VERSION
                    ),
                    &input.asset_id,
                    &input.variant_id,
                    &input.source_fingerprint,
                );
                fresh.revision = 0;
                serde_json::to_value(&fresh).map_err(|err| {
                    DevelopError::InvalidInput(format!("envelope serialization failed: {err}"))
                })?
            }
        };
        envelope_value["revision"] = serde_json::Value::from(opened.revision.saturating_sub(1));

        Ok(OpenedEditSession {
            session_id: opened.session_id.0,
            asset_id: opened.asset_id,
            variant_id: opened.variant_id,
            revision: opened.revision.saturating_sub(1),
            dimensions: opened.dimensions,
            source_fingerprint: opened.source_fingerprint,
            envelope: envelope_value,
        })
    }

    /// Renders one preview generation. Blocks until the engine worker settles
    /// the request (coalesced generations resolve quickly as cancelled).
    /// Completed frames are stored in the bounded handle registry.
    pub fn render_preview(
        &self,
        session_id: u64,
        generation: u64,
        envelope: RecipeEnvelope,
        quality: PreviewQuality,
        max_edge: u32,
    ) -> Result<PreviewWait, DevelopError> {
        let sid = SessionId(session_id);
        let info = self
            .manager
            .session_info(sid)
            .ok_or(DevelopError::NotFound { session_id })?;
        if envelope.asset_id != info.asset_id || envelope.variant_id != info.variant_id {
            return Err(DevelopError::InvalidInput(format!(
                "envelope identity ({}, {}) does not match session asset ({}, {})",
                envelope.asset_id, envelope.variant_id, info.asset_id, info.variant_id
            )));
        }
        if max_edge == 0 || max_edge > self.max_preview_edge {
            return Err(DevelopError::InvalidInput(format!(
                "preview edge {max_edge} outside the bounded transport range 1..={}",
                self.max_preview_edge
            )));
        }
        // Masks: every visible sub-mask must be a supported non-AI kind with
        // convertible geometry (lap-78d). Unsupported kinds are preserved in
        // the recipe but fail here explicitly, naming mask and kind; they are
        // never silently dropped from previews.
        if let Err(err) = rapidraw_develop::validate_masks_supported(&envelope.recipe.masks) {
            return Err(DevelopError::Unsupported(err.to_string()));
        }

        let ticket: PreviewTicket = self.manager.render_preview(PreviewRequest {
            session_id: sid,
            generation,
            recipe: envelope,
            quality,
            max_edge,
        })?;

        match ticket.outcome.recv() {
            Ok(PreviewOutcome::Completed { info, frame, .. }) => {
                let handle = new_preview_handle(info.session_id.0);
                let meta = PreviewTicketMeta {
                    session_id: info.session_id.0,
                    asset_id: info.asset_id.clone(),
                    variant_id: info.variant_id.clone(),
                    generation: info.generation,
                    quality: info.quality,
                    width: frame.width,
                    height: frame.height,
                    byte_len: frame.rgba8.len(),
                    handle: handle.clone(),
                };
                self.handles
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(handle, info.session_id.0, frame);
                Ok(PreviewWait::Completed { ticket: meta })
            }
            Ok(PreviewOutcome::Cancelled { .. }) => Ok(PreviewWait::Cancelled),
            Ok(PreviewOutcome::Failed { error, .. }) => {
                let (code, message) = failure_parts(&error);
                Ok(PreviewWait::Failed { code, message })
            }
            Err(_) => Err(DevelopError::InvalidInput(
                "preview worker dropped the outcome channel".to_string(),
            )),
        }
    }

    /// Fetches a rendered preview frame by handle (raw RGBA8 bytes; bounded
    /// transport). Handles expire by eviction or session close.
    pub fn take_preview_frame(&self, handle: &str) -> Result<PreviewFrame, DevelopError> {
        self.handles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take(handle)
            .ok_or_else(|| DevelopError::PreviewNotFound(handle.to_string()))
    }

    /// Validates and durably persists a recipe with optimistic revision
    /// control (spec A4). Returns the new sidecar-aligned revision.
    pub fn commit_recipe(
        &self,
        session_id: u64,
        expected_revision: u64,
        envelope: RecipeEnvelope,
    ) -> Result<CommitReceiptDto, DevelopError> {
        let sid = SessionId(session_id);
        let info = self
            .manager
            .session_info(sid)
            .ok_or(DevelopError::NotFound { session_id })?;
        if envelope.asset_id != info.asset_id || envelope.variant_id != info.variant_id {
            return Err(DevelopError::InvalidInput(format!(
                "envelope identity ({}, {}) does not match session asset ({}, {})",
                envelope.asset_id, envelope.variant_id, info.asset_id, info.variant_id
            )));
        }
        let engine_expected = expected_revision
            .checked_add(1)
            .ok_or_else(|| DevelopError::InvalidInput("revision overflow".to_string()))?;
        let mut engine_envelope = envelope;
        engine_envelope.revision = engine_expected;

        match self
            .manager
            .commit_recipe(sid, engine_expected, engine_envelope)
        {
            Ok(commit) => {
                let sidecar_revision = commit.revision.saturating_sub(1);
                let receipt = self
                    .sidecar
                    .as_ref()
                    .and_then(|store| store.receipt(&info.asset_id, sidecar_revision));
                Ok(CommitReceiptDto {
                    session_id,
                    revision: sidecar_revision,
                    content_hash: receipt.as_ref().map(|receipt| receipt.content_hash.clone()),
                    sidecar_path: receipt.as_ref().map(|receipt| receipt.sidecar_path.clone()),
                    projection_applied: receipt
                        .as_ref()
                        .map(|receipt| receipt.projection_applied)
                        .unwrap_or(false),
                    projection_error: receipt
                        .as_ref()
                        .and_then(|receipt| receipt.projection_error.clone()),
                })
            }
            Err(SessionError::Persist(message)) => Err(parse_persist_error(session_id, &message)),
            Err(error) => Err(map_session_error(&error)),
        }
    }

    /// Closes a session: previews are cancelled, pending saves settle first,
    /// buffers and preview handles are released.
    pub fn close_session(&self, session_id: u64) -> Result<ClosedEditSession, DevelopError> {
        let closed: ClosedSession =
            self.manager
                .close_session(SessionId(session_id))
                .map_err(|error| match error {
                    SessionError::Persist(message) => parse_persist_error(session_id, &message),
                    other => map_session_error(&other),
                })?;
        let released = self
            .handles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .release_session(session_id);
        Ok(ClosedEditSession {
            session_id,
            revision: closed.final_revision.saturating_sub(1),
            released_preview_cache_entries: closed
                .released_preview_cache_entries
                .saturating_add(released.0),
            released_preview_cache_bytes: closed
                .released_preview_cache_bytes
                .saturating_add(released.1),
        })
    }

    /// The session's committed envelope (engine-aligned revisions).
    pub fn session_envelope(&self, session_id: u64) -> Option<RecipeEnvelope> {
        self.manager.session_envelope(SessionId(session_id))
    }

    /// Cancels one engine export job (queued: dropped; in-flight: cooperative
    /// token). Returns true when the job was found. Used by the durable
    /// derivative export to route external cancellation into the bounded
    /// export queue without touching preview domains.
    pub fn cancel_engine_export(&self, job_id: rapidraw_develop::session::ExportJobId) -> bool {
        self.manager.cancel_export(job_id)
    }

    /// Engine session-manager access for the durable export path
    /// (`develop/export.rs`): snapshot enqueue plus job cancellation run
    /// through the same bounded queues as every other session operation.
    pub(crate) fn engine_manager(&self) -> &rapidraw_develop::session::SessionManager {
        &self.manager
    }

    /// Explicit capability report: GPU status + engine identity.
    pub fn capabilities(&self) -> CapabilityReport {
        let gpu = match &self.gpu {
            Some(renderer) => renderer.probe(),
            None => GpuStatus {
                available: false,
                error: Some(
                    "no GPU preview renderer installed in this service instance".to_string(),
                ),
                ..GpuStatus::default()
            },
        };
        CapabilityReport {
            gpu,
            engine_revision: ENGINE_GIT_REVISION,
            edit_model_version: rapidraw_edit_model::MODEL_VERSION.to_string(),
            schema_version: rapidraw_edit_model::SCHEMA_VERSION,
            preview_max_edge: self.max_preview_edge,
        }
    }
}

fn failure_parts(error: &SessionError) -> (String, String) {
    match error {
        SessionError::Render(engine_error) => match engine_error {
            EngineError::Unsupported(message) => ("unsupported".to_string(), message.clone()),
            EngineError::Decode(message) => ("decode".to_string(), message.clone()),
            EngineError::InvalidInput(message) => ("invalid-input".to_string(), message.clone()),
            other => ("render".to_string(), other.to_string()),
        },
        other => ("session".to_string(), other.to_string()),
    }
}

impl From<SessionError> for DevelopError {
    fn from(error: SessionError) -> Self {
        map_session_error(&error)
    }
}

fn map_session_error(error: &SessionError) -> DevelopError {
    match error {
        SessionError::NotFound { session_id } => DevelopError::NotFound {
            session_id: session_id.0,
        },
        SessionError::SessionClosed { session_id } => DevelopError::SessionClosed {
            session_id: session_id.0,
        },
        SessionError::StaleGeneration {
            session_id,
            accepted,
            requested,
        } => DevelopError::StaleGeneration {
            session_id: session_id.0,
            accepted: *accepted,
            requested: *requested,
        },
        SessionError::RevisionConflict {
            session_id,
            current,
            expected,
        } => DevelopError::RevisionConflict {
            session_id: session_id.0,
            expected: *expected,
            current: *current,
        },
        SessionError::TooManySessions { limit } => DevelopError::TooManySessions { limit: *limit },
        SessionError::PreviewQueueFull { limit } => {
            DevelopError::PreviewQueueFull { limit: *limit }
        }
        SessionError::ExportQueueFull { limit } => {
            DevelopError::InvalidInput(format!("export queue limit of {limit} reached"))
        }
        SessionError::InvalidInput(message) => DevelopError::InvalidInput(message.clone()),
        SessionError::Render(engine_error) => match engine_error {
            EngineError::Decode(message) => DevelopError::Decode(message.clone()),
            EngineError::Unsupported(message) => DevelopError::Unsupported(message.clone()),
            other => DevelopError::InvalidInput(other.to_string()),
        },
        SessionError::Persist(message) => DevelopError::Persist(message.clone()),
        SessionError::Cancelled => DevelopError::InvalidInput("job cancelled".to_string()),
        SessionError::ManagerShuttingDown => {
            DevelopError::InvalidInput("session manager is shutting down".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ENGINE_GIT_REVISION;
    use crate::develop::recipe_repository::RecipeRepository;
    use crate::develop::sessions::{
        AssetEditInput, DevelopConfig, DevelopError, DevelopService, GpuPreviewRenderer,
        PreviewWait, SidecarBackedStore,
    };
    use rapidraw_develop::session::{
        ExportJob, ExportRenderer, PreviewFrame, PreviewJob, PreviewQuality, PreviewRenderer,
        RecipeStore, SessionManagerConfig,
    };
    use rapidraw_develop::{
        CancelToken, DecodeOptions, DecodedOriginal, DevelopError as EngineError, LinearImage,
        SessionId, decode_original,
    };
    use rapidraw_edit_model::{RecipeEnvelope, sha256_hex};
    use std::collections::HashMap;
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    // ---------------------------------------------------------------- fixtures

    fn fixture_path(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/raw-development")
            .join(name)
    }

    fn gradient_bytes() -> Vec<u8> {
        fs::read(fixture_path("synthetic/dng-linear-gradient-64x48.dng")).expect("gradient fixture")
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
            "lap_develop_sessions_{}_{}_{}",
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

    // ------------------------------------------------------------- test doubles

    /// Store that accepts every save (used with `with_parts`).
    struct NullStore;

    impl RecipeStore for NullStore {
        fn persist(&self, _envelope: &RecipeEnvelope) -> Result<(), String> {
            Ok(())
        }
    }

    /// Export renderer that refuses work explicitly (export is a later slice;
    /// it must never fake success).
    struct StubExportRenderer;

    impl ExportRenderer for StubExportRenderer {
        fn render(&self, _job: &ExportJob) -> Result<rapidraw_develop::ExportFrame, EngineError> {
            Err(EngineError::Unsupported(
                "durable export is implemented in a later slice".to_string(),
            ))
        }
    }

    /// Preview renderer with optional per-generation gates. A gated render
    /// blocks (cooperatively honouring cancellation) until its gate is
    /// released, simulating a slow/delayed host render reply.
    struct GateRenderer {
        gates: Mutex<HashMap<u64, Receiver<()>>>,
    }

    impl GateRenderer {
        fn new(generations: &[u64]) -> (Self, HashMap<u64, Sender<()>>) {
            let mut gates = HashMap::new();
            let mut senders = HashMap::new();
            for &generation in generations {
                let (tx, rx) = mpsc::channel();
                gates.insert(generation, rx);
                senders.insert(generation, tx);
            }
            (
                Self {
                    gates: Mutex::new(gates),
                },
                senders,
            )
        }
    }

    impl PreviewRenderer for GateRenderer {
        fn render(&self, job: &PreviewJob) -> Result<PreviewFrame, EngineError> {
            let rx = self.gates.lock().unwrap().remove(&job.generation);
            if let Some(rx) = rx {
                loop {
                    job.cancel.check()?;
                    match rx.recv_timeout(Duration::from_millis(5)) {
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                }
            }
            job.cancel.check()?;
            Ok(PreviewFrame {
                width: 2,
                height: 2,
                // Paint the generation into the frame so stale deliveries are
                // detectable by content.
                rgba8: [[job.generation as u8, 0, 0, 255]; 4].concat(),
            })
        }
    }

    /// Durable store that records when a save starts/finishes and sleeps in
    /// between, to prove `close` waits for acknowledged saves.
    struct SlowStore {
        delay: Duration,
        started: Mutex<Option<Instant>>,
        finished: Mutex<Option<Instant>>,
    }

    impl SlowStore {
        fn new(delay: Duration) -> Self {
            Self {
                delay,
                started: Mutex::new(None),
                finished: Mutex::new(None),
            }
        }
    }

    impl RecipeStore for SlowStore {
        fn persist(&self, _envelope: &RecipeEnvelope) -> Result<(), String> {
            *self.started.lock().unwrap() = Some(Instant::now());
            std::thread::sleep(self.delay);
            *self.finished.lock().unwrap() = Some(Instant::now());
            Ok(())
        }
    }

    // ------------------------------------------------------------ constructors

    fn test_config() -> DevelopConfig {
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
            max_cached_preview_bytes: 200,
        }
    }

    fn service_with(
        renderer: Arc<dyn PreviewRenderer>,
        store: Arc<dyn RecipeStore>,
    ) -> DevelopService {
        DevelopService::with_parts(test_config(), renderer, Arc::new(StubExportRenderer), store)
    }

    fn open_input(asset: &str, source: &Path, bytes: &[u8]) -> AssetEditInput {
        AssetEditInput {
            asset_id: asset.to_string(),
            variant_id: "default".to_string(),
            source_path: source.to_path_buf(),
            source_fingerprint: fingerprint(bytes),
            source_bytes: bytes.to_vec(),
            sidecar: None,
        }
    }

    fn open_ok(
        service: &DevelopService,
        asset: &str,
        source: &Path,
        bytes: &[u8],
    ) -> super::OpenedEditSession {
        service
            .open_session(open_input(asset, source, bytes))
            .expect("open session")
    }

    fn settled(
        service: &DevelopService,
        session_id: u64,
        generation: u64,
    ) -> super::PreviewTicketMeta {
        let envelope = service
            .session_envelope(session_id)
            .expect("session envelope");
        match service
            .render_preview(
                session_id,
                generation,
                envelope,
                PreviewQuality::Settled,
                48,
            )
            .expect("render accepted")
        {
            PreviewWait::Completed { ticket } => ticket,
            other => panic!("expected completed preview, got {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // Recipe geometry (lap-6bc / TASK-402): the shared host helper that
    // preview AND export renderers apply before rendering, and the
    // preview-path behavior that must match the export path.
    // ------------------------------------------------------------------

    fn geometry_test_image() -> LinearImage {
        let mut image = LinearImage::new(4, 3);
        for y in 0..3u32 {
            for x in 0..4u32 {
                let v = (x * 16 + y) as f32 / 255.0;
                image.set_pixel(x, y, [v, v, v]);
            }
        }
        image
    }

    fn pixel_value(image: &LinearImage, x: u32, y: u32) -> f32 {
        image.pixel(x, y)[0]
    }

    #[test]
    fn recipe_geometry_quarter_turn_maps_pixels_like_the_engine() {
        let image = geometry_test_image();
        let mut recipe = rapidraw_edit_model::Recipe::default();
        recipe.orientation_steps = 1;

        let out = super::apply_recipe_geometry(&image, &recipe).expect("valid geometry");
        let (w, h) = out.dimensions();
        assert_eq!((w, h), (3, 4), "a 90-degree turn swaps dimensions");
        // Engine Rotate90 forward map: source (sx, sy) -> oriented (h-1-sy, sx).
        // Inverse: oriented (ox, oy) -> source (oy, h-1-ox) with h = 3.
        for oy in 0..4u32 {
            for ox in 0..3u32 {
                let expected = pixel_value(&image, oy, 2 - ox);
                assert!(
                    (pixel_value(&out, ox, oy) - expected).abs() < 1e-6,
                    "oriented ({ox},{oy}) must carry source pixel ({oy},{})",
                    2 - ox
                );
            }
        }
    }

    #[test]
    fn recipe_geometry_flips_compose_after_rotation_like_the_export_path() {
        let image = geometry_test_image();
        let mut recipe = rapidraw_edit_model::Recipe::default();
        recipe.orientation_steps = 1;
        recipe.flip_horizontal = true;

        let out = super::apply_recipe_geometry(&image, &recipe).expect("valid geometry");
        let (w, h) = out.dimensions();
        assert_eq!((w, h), (3, 4));
        // Final = flip_horizontal(rotation(source)): final (fx, fy) carries
        // rotated pixel (w-1-fx, fy), i.e. source (fy, h-1-(w-1-fx)) = (fy, fx).
        for fy in 0..4u32 {
            for fx in 0..3u32 {
                let expected = pixel_value(&image, fy, fx);
                assert!((pixel_value(&out, fx, fy) - expected).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn recipe_geometry_crops_in_the_oriented_frame() {
        let image = geometry_test_image();
        let mut recipe = rapidraw_edit_model::Recipe::default();
        recipe.crop = Some(rapidraw_edit_model::CropRect {
            x: 0.5,
            y: 0.0,
            width: 0.5,
            height: 1.0,
        });

        let out = super::apply_recipe_geometry(&image, &recipe).expect("valid geometry");
        let (w, h) = out.dimensions();
        assert_eq!((w, h), (2, 3), "right half of a 4x3 image");
        for y in 0..3u32 {
            for x in 0..2u32 {
                let expected = pixel_value(&image, 2 + x, y);
                assert!((pixel_value(&out, x, y) - expected).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn recipe_geometry_without_transforms_returns_the_original() {
        let image = geometry_test_image();
        let recipe = rapidraw_edit_model::Recipe::default();
        let out = super::apply_recipe_geometry(&image, &recipe).expect("valid geometry");
        assert_eq!(out.dimensions(), image.dimensions());
        assert_eq!(out.rgb(), image.rgb());
    }

    #[test]
    fn recipe_geometry_rejects_out_of_frame_crops() {
        let image = geometry_test_image();
        let mut recipe = rapidraw_edit_model::Recipe::default();
        recipe.crop = Some(rapidraw_edit_model::CropRect {
            x: 0.8,
            y: 0.0,
            width: 0.5,
            height: 1.0,
        });
        let err = super::apply_recipe_geometry(&image, &recipe)
            .expect_err("out-of-frame crops must be rejected");
        assert!(matches!(err, EngineError::Geometry(_)), "{err:?}");
    }

    #[test]
    fn preview_geometry_hash_tracks_every_geometry_field() {
        let base = rapidraw_edit_model::Recipe::default();
        let base_hash = super::recipe_geometry_hash(&base);

        let mut rotated = base.clone();
        rotated.orientation_steps = 2;
        assert_ne!(super::recipe_geometry_hash(&rotated), base_hash);

        let mut flipped = base.clone();
        flipped.flip_horizontal = true;
        assert_ne!(super::recipe_geometry_hash(&flipped), base_hash);

        let mut cropped = base.clone();
        cropped.crop = Some(rapidraw_edit_model::CropRect {
            x: 0.1,
            y: 0.1,
            width: 0.5,
            height: 0.5,
        });
        assert_ne!(super::recipe_geometry_hash(&cropped), base_hash);

        let mut nudged = cropped.clone();
        nudged.crop.as_mut().unwrap().x = 0.2;
        assert_ne!(
            super::recipe_geometry_hash(&nudged),
            super::recipe_geometry_hash(&cropped)
        );
    }

    #[test]
    fn gpu_preview_renderer_applies_recipe_geometry_like_export() {
        // Honest capability probe: with a device the preview must reflect the
        // recipe geometry (identically to the export path's orientation ->
        // flips -> crop order); without one the failure stays explicit.
        let renderer = GpuPreviewRenderer::new();
        let bytes = gradient_bytes();
        let decoded: Arc<DecodedOriginal> =
            Arc::new(decode_original(&bytes, &DecodeOptions::default()).expect("decode"));
        let (source_w, source_h) = decoded.image.dimensions();

        let mut envelope =
            RecipeEnvelope::new("lap-test/0", "asset-a", "default", &fingerprint(&bytes));
        envelope.recipe.orientation_steps = 1;
        envelope.recipe.crop = Some(rapidraw_edit_model::CropRect {
            x: 0.5,
            y: 0.5,
            width: 0.5,
            height: 0.5,
        });

        let (cancel_source, cancel) = CancelToken::pair();
        let _ = cancel_source;
        let job = PreviewJob {
            session_id: SessionId(11),
            asset_id: "asset-a".to_string(),
            variant_id: "default".to_string(),
            generation: 1,
            quality: PreviewQuality::Settled,
            max_edge: 48,
            original: Arc::clone(&decoded),
            envelope,
            cancel,
        };
        match renderer.render(&job) {
            Ok(frame) => {
                // Oriented dims (swap for one turn), then the bottom-right
                // quarter crop halves both sides again.
                let (oriented_w, oriented_h) = (source_h, source_w);
                let cropped = super::preview_dimensions(oriented_w / 2, oriented_h / 2, 48);
                eprintln!(
                    "gpu_preview_geometry: rendered {}x{} (expected {:?})",
                    frame.width, frame.height, cropped
                );
                assert_eq!(
                    (frame.width, frame.height),
                    cropped,
                    "preview must render the oriented, cropped frame"
                );
            }
            Err(err) => {
                eprintln!("gpu_preview_geometry: no device ({err})");
                assert!(
                    matches!(err, EngineError::Unsupported(_)),
                    "without a device the failure must be a typed explicit error, got {err:?}"
                );
            }
        }
    }

    #[test]
    fn cargo_lock_pins_engine_revision() {
        let lock = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock"))
            .expect("Cargo.lock present");
        for crate_name in ["rapidraw-develop", "rapidraw-edit-model"] {
            let marker = format!("name = \"{crate_name}\"");
            let start = lock
                .find(&marker)
                .unwrap_or_else(|| panic!("{crate_name} missing from Cargo.lock"));
            let block = &lock[start..start + 800.min(lock.len() - start)];
            let source_line = block
                .lines()
                .find(|line| line.trim_start().starts_with("source = "))
                .unwrap_or_else(|| panic!("{crate_name} has no git source in Cargo.lock"));
            assert!(
                source_line.contains(&format!("rev={ENGINE_GIT_REVISION}")),
                "{crate_name} is not pinned to the engine-lock revision: {source_line}"
            );
        }
    }

    #[test]
    fn engine_revision_constant_matches_engine_lock() {
        let lock_text = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/raw-development/engine-lock.json"),
        )
        .expect("engine lock present");
        assert!(lock_text.contains(ENGINE_GIT_REVISION));
    }

    #[test]
    fn open_reports_sidecar_aligned_revision_and_dimensions() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("revision");
        let source = write_source(&dir, "photo.dng", &bytes);

        let service = service_with(Arc::new(GateRenderer::new(&[]).0), Arc::new(NullStore));

        // Fresh asset: revision 0 = unedited initial state.
        let fresh = open_ok(&service, "asset-a", &source, &bytes);
        assert_eq!(fresh.revision, 0);
        assert_eq!(fresh.asset_id, "asset-a");
        assert_eq!(fresh.variant_id, "default");
        assert_eq!(fresh.source_fingerprint, fingerprint(&bytes));
        assert!(fresh.dimensions.0 > 0 && fresh.dimensions.1 > 0);
        service.close_session(fresh.session_id).unwrap();

        // Committed asset: sidecar revision 1 surfaces as revision 1.
        let repo = RecipeRepository::new("lap-test/rapidraw-edit-model/0.1.0");
        let mut envelope = repo.new_envelope("asset-b", "default", &fingerprint(&bytes));
        envelope.recipe.exposure = 0.5;
        repo.commit_sidecar(&source, 0, envelope).unwrap();
        let committed = service
            .open_session(AssetEditInput {
                asset_id: "asset-b".to_string(),
                variant_id: "default".to_string(),
                source_path: source.clone(),
                source_fingerprint: fingerprint(&bytes),
                source_bytes: bytes.clone(),
                sidecar: Some(repo.load(&source).unwrap()),
            })
            .unwrap();
        assert_eq!(committed.revision, 1);
        let value = committed.envelope.get("recipe").expect("envelope recipe");
        assert_eq!(
            value.get("exposure").and_then(|v| v.as_f64()),
            Some(0.5),
            "opened envelope carries the committed recipe"
        );
        service.close_session(committed.session_id).unwrap();
    }

    #[test]
    fn source_replacement_is_reported_explicitly() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("replaced");
        let source = write_source(&dir, "photo.dng", &bytes);
        let repo = RecipeRepository::new("lap-test/0");
        let envelope = repo.new_envelope("asset-r", "default", &fingerprint(&bytes));
        repo.commit_sidecar(&source, 0, envelope).unwrap();

        let service = service_with(Arc::new(GateRenderer::new(&[]).0), Arc::new(NullStore));
        let mut tampered = gradient_bytes();
        tampered[100] = tampered[100].wrapping_add(1);
        let err = service
            .open_session(AssetEditInput {
                asset_id: "asset-r".to_string(),
                variant_id: "default".to_string(),
                source_path: source.clone(),
                source_fingerprint: fingerprint(&tampered),
                source_bytes: tampered,
                sidecar: Some(repo.load(&source).unwrap()),
            })
            .expect_err("replacement must fail explicitly");
        assert!(
            matches!(err, DevelopError::SourceReplaced { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn two_concurrent_sessions_render_and_commit_in_isolation() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("isolation");
        let source_a = write_source(&dir, "a.dng", &bytes);
        let source_b = write_source(&dir, "b.dng", &bytes);

        let service = service_with(Arc::new(GateRenderer::new(&[]).0), Arc::new(NullStore));
        let a = open_ok(&service, "asset-a", &source_a, &bytes);
        let b = open_ok(&service, "asset-b", &source_b, &bytes);
        assert_ne!(a.session_id, b.session_id);

        // Interleaved previews: each result must carry its own asset identity.
        let ticket_a = settled(&service, a.session_id, 1);
        let ticket_b = settled(&service, b.session_id, 1);
        assert_eq!(ticket_a.asset_id, "asset-a");
        assert_eq!(ticket_b.asset_id, "asset-b");

        let frame_a = service.take_preview_frame(&ticket_a.handle).unwrap();
        let frame_b = service.take_preview_frame(&ticket_b.handle).unwrap();
        assert_eq!(frame_a.rgba8[0], 1, "session A renders generation 1");
        assert_eq!(frame_b.rgba8[0], 1);
        // Handles are session-scoped: neither session can observe the other's
        // frame identity through its own ticket.
        assert_ne!(ticket_a.handle, ticket_b.handle);

        // Commits stay on their own sidecars.
        let mut envelope_a = service.session_envelope(a.session_id).unwrap();
        envelope_a.recipe.exposure = 1.0;
        let receipt_a = service
            .commit_recipe(a.session_id, a.revision, envelope_a)
            .unwrap();
        assert_eq!(receipt_a.revision, 1);

        let mut envelope_b = service.session_envelope(b.session_id).unwrap();
        envelope_b.recipe.exposure = 2.0;
        let receipt_b = service
            .commit_recipe(b.session_id, b.revision, envelope_b)
            .unwrap();
        assert_eq!(receipt_b.revision, 1);

        // B's session never sees A's recipe.
        let b_after = service.session_envelope(b.session_id).unwrap();
        assert_eq!(b_after.recipe.exposure, 2.0);

        service.close_session(a.session_id).unwrap();
        service.close_session(b.session_id).unwrap();
    }

    #[test]
    fn delayed_preview_reply_is_dropped_as_stale() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("delayed");
        let source = write_source(&dir, "delayed.dng", &bytes);

        let (renderer, gates) = GateRenderer::new(&[1]);
        let service = Arc::new(service_with(Arc::new(renderer), Arc::new(NullStore)));
        let session = open_ok(&service, "asset-a", &source, &bytes);

        // Generation 1 blocks inside the renderer.
        let first = {
            let service = Arc::clone(&service);
            let session_id = session.session_id;
            let envelope = service.session_envelope(session_id).unwrap();
            std::thread::spawn(move || {
                service.render_preview(session_id, 1, envelope, PreviewQuality::Settled, 48)
            })
        };
        std::thread::sleep(Duration::from_millis(150));

        // Generation 2 coalesces/cancels generation 1 and completes.
        let ticket2 = settled(&service, session.session_id, 2);
        assert_eq!(ticket2.generation, 2);

        // Release the delayed reply; its outcome must be Cancelled, never a
        // success that could overwrite generation 2. The receiver may already
        // be gone (the cancelled render returned), which is fine.
        let _ = gates.get(&1).unwrap().send(());
        let first = first.join().unwrap().expect("first request resolved");
        match first {
            PreviewWait::Cancelled => {}
            other => panic!("delayed reply must be cancelled, got {other:?}"),
        }
        assert!(
            service.take_preview_frame(&ticket2.handle).is_ok(),
            "the newest generation stays fetchable"
        );
        service.close_session(session.session_id).unwrap();
    }

    #[test]
    fn stale_generation_request_is_rejected() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("stale");
        let source = write_source(&dir, "stale.dng", &bytes);
        let service = service_with(Arc::new(GateRenderer::new(&[]).0), Arc::new(NullStore));
        let session = open_ok(&service, "asset-a", &source, &bytes);

        settled(&service, session.session_id, 3);
        let err = service
            .render_preview(
                session.session_id,
                3,
                service.session_envelope(session.session_id).unwrap(),
                PreviewQuality::Settled,
                48,
            )
            .expect_err("re-submitting the same generation is stale");
        assert!(
            matches!(err, DevelopError::StaleGeneration { .. }),
            "{err:?}"
        );
        service.close_session(session.session_id).unwrap();
    }

    #[test]
    fn close_session_cancels_previews_and_waits_for_pending_save() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("close");
        let source = write_source(&dir, "close.dng", &bytes);

        let (renderer, _gates) = GateRenderer::new(&[1]);
        let store = Arc::new(SlowStore::new(Duration::from_millis(250)));
        let service = Arc::new(service_with(
            Arc::new(renderer),
            Arc::clone(&store) as Arc<dyn RecipeStore>,
        ));
        let session = open_ok(&service, "asset-a", &source, &bytes);

        // A preview is in flight when the close/commit race begins.
        let preview = {
            let service = Arc::clone(&service);
            let session_id = session.session_id;
            let envelope = service.session_envelope(session_id).unwrap();
            std::thread::spawn(move || {
                service.render_preview(session_id, 1, envelope, PreviewQuality::Interactive, 48)
            })
        };
        std::thread::sleep(Duration::from_millis(100));

        // A durable save is pending when close arrives.
        let commit = {
            let service = Arc::clone(&service);
            let session_id = session.session_id;
            let revision = session.revision;
            let envelope = service.session_envelope(session_id).unwrap();
            std::thread::spawn(move || service.commit_recipe(session_id, revision, envelope))
        };
        while store.started.lock().unwrap().is_none() {
            std::thread::sleep(Duration::from_millis(5));
        }

        let close_started = Instant::now();
        let closed = service.close_session(session.session_id).expect("close");
        let close_returned = Instant::now();

        // The save settled before close returned.
        let save_finished = store.finished.lock().unwrap().expect("save finished");
        assert!(
            save_finished <= close_returned,
            "close returned before the pending save settled"
        );
        assert_eq!(closed.revision, 1, "final revision is sidecar-aligned");
        assert!(close_returned.duration_since(close_started) >= Duration::from_millis(100));

        // The in-flight preview was cancelled, and the session rejects new work.
        let preview = preview.join().unwrap().expect("preview resolved");
        match preview {
            PreviewWait::Cancelled => {}
            other => panic!("in-flight preview must be cancelled by close, got {other:?}"),
        }
        let commit = commit.join().unwrap().expect("commit resolved");
        assert_eq!(commit.revision, 1, "the acknowledged save is durable");

        let err = service
            .render_preview(
                session.session_id,
                2,
                service
                    .session_envelope(session.session_id)
                    .unwrap_or_default(),
                PreviewQuality::Settled,
                48,
            )
            .expect_err("closed session rejects previews");
        assert!(
            matches!(
                err,
                DevelopError::SessionClosed { .. } | DevelopError::NotFound { .. }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn two_windows_commit_same_revision_one_conflicts() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("cas");
        let source = write_source(&dir, "cas.dng", &bytes);

        let repo = RecipeRepository::new("lap-test/0");
        let envelope = repo.new_envelope("asset-c", "default", &fingerprint(&bytes));
        repo.commit_sidecar(&source, 0, envelope).unwrap();

        let store = Arc::new(SidecarBackedStore::new(repo.clone()));
        let service = DevelopService::with_sidecar_store(
            test_config(),
            Arc::new(GateRenderer::new(&[]).0),
            Arc::new(StubExportRenderer),
            store,
        );

        let open = |service: &DevelopService| {
            service
                .open_session(AssetEditInput {
                    asset_id: "asset-c".to_string(),
                    variant_id: "default".to_string(),
                    source_path: source.clone(),
                    source_fingerprint: fingerprint(&bytes),
                    source_bytes: bytes.clone(),
                    sidecar: Some(repo.load(&source).unwrap()),
                })
                .unwrap()
        };
        let window_a = open(&service);
        let window_b = open(&service);
        assert_eq!(window_a.revision, 1);
        assert_eq!(window_b.revision, 1);

        let mut envelope_a = service.session_envelope(window_a.session_id).unwrap();
        envelope_a.recipe.exposure = 1.0;
        let a = service
            .commit_recipe(window_a.session_id, window_a.revision, envelope_a)
            .expect("first window commits");
        assert_eq!(a.revision, 2);

        let mut envelope_b = service.session_envelope(window_b.session_id).unwrap();
        envelope_b.recipe.exposure = 2.0;
        let err = service
            .commit_recipe(window_b.session_id, window_b.revision, envelope_b)
            .expect_err("stale second window reports a conflict");
        assert!(
            matches!(err, DevelopError::RevisionConflict { .. }),
            "conflict must be typed, got {err:?}"
        );

        // The durable sidecar advanced exactly once.
        let durable = repo.load(&source).unwrap();
        assert_eq!(durable.revision, 2);
        assert_eq!(durable.recipe.exposure, 1.0);
    }

    #[test]
    fn bounded_preview_cache_evicts_oldest_and_unknown_handles_error() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("bounded");
        let source = write_source(&dir, "bounded.dng", &bytes);
        let service = service_with(Arc::new(GateRenderer::new(&[]).0), Arc::new(NullStore));
        let session = open_ok(&service, "asset-a", &source, &bytes);

        // Fake frames are 2x2 RGBA8 = 16 bytes; the test config bounds the
        // registry at 200 bytes, so at most 12 frames stay resident.
        let mut handles = Vec::new();
        for generation in 1..=20u64 {
            handles.push(settled(&service, session.session_id, generation).handle);
        }
        assert!(
            service.take_preview_frame(&handles[0]).is_err(),
            "the oldest handle must have been evicted by the byte budget"
        );
        assert!(service.take_preview_frame(&handles[19]).is_ok());
        let err = service
            .take_preview_frame("never-issued-handle")
            .expect_err("unknown handles are explicit errors");
        assert!(matches!(err, DevelopError::PreviewNotFound(_)), "{err:?}");

        // Closing the session releases its remaining handles.
        service.close_session(session.session_id).unwrap();
        let err = service
            .take_preview_frame(&handles[19])
            .expect_err("closed sessions release preview handles");
        assert!(matches!(err, DevelopError::PreviewNotFound(_)), "{err:?}");
    }

    #[test]
    fn masked_preview_rejects_unsupported_kind_naming_it() {
        let bytes = gradient_bytes();
        let dir = tmp_dir("masks-ai");
        let source = write_source(&dir, "masks.dng", &bytes);
        let service = service_with(Arc::new(GateRenderer::new(&[]).0), Arc::new(NullStore));
        let session = open_ok(&service, "asset-a", &source, &bytes);

        let mut envelope = service.session_envelope(session.session_id).unwrap();
        envelope
            .recipe
            .masks
            .push(rapidraw_edit_model::MaskContainer {
                id: "mask-1".to_string(),
                name: "ai".to_string(),
                visible: true,
                sub_masks: vec![rapidraw_edit_model::SubMask {
                    id: "sub-1".to_string(),
                    kind: "ai-subject".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            });
        let err = service
            .render_preview(session.session_id, 1, envelope, PreviewQuality::Settled, 48)
            .expect_err("unsupported mask kinds are never silently dropped");
        match err {
            DevelopError::Unsupported(message) => {
                assert!(
                    message.contains("ai-subject") && message.contains("mask-1"),
                    "the rejection must name the mask and the unsupported kind: {message}"
                );
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
        service.close_session(session.session_id).unwrap();
    }

    #[test]
    fn masked_preview_renders_supported_masks_or_fails_gpu_only() {
        // The supported non-AI mask path: a radial mask with local exposure
        // changes the rendered preview. Combinations with crop/orientation
        // exercise the oriented-frame mask mapping. On a machine without an
        // offscreen device the failure must be an explicit GPU capability
        // error; silently un-masked pixels are impossible by construction.
        let bytes = gradient_bytes();
        let dir = tmp_dir("masks-render");
        let source = write_source(&dir, "masks.dng", &bytes);
        let service = DevelopService::with_parts(
            test_config(),
            Arc::new(GpuPreviewRenderer::new()),
            Arc::new(StubExportRenderer),
            Arc::new(NullStore),
        );
        let session = open_ok(&service, "asset-a", &source, &bytes);
        let sid = session.session_id;

        let render = |envelope: rapidraw_edit_model::RecipeEnvelope,
                      generation: u64|
         -> Result<rapidraw_develop::session::PreviewFrame, DevelopError> {
            match service.render_preview(sid, generation, envelope, PreviewQuality::Settled, 64) {
                Ok(PreviewWait::Completed { ticket }) => service.take_preview_frame(&ticket.handle),
                Ok(PreviewWait::Failed { message, .. }) => Err(DevelopError::Unsupported(message)),
                Ok(other) => panic!("unexpected preview outcome: {other:?}"),
                Err(err) => Err(err),
            }
        };

        let masked_envelope = |crop: bool| {
            let mut envelope = service.session_envelope(sid).unwrap();
            if crop {
                envelope.recipe.orientation_steps = 1;
                envelope.recipe.crop = Some(rapidraw_edit_model::CropRect {
                    x: 0.1,
                    y: 0.1,
                    width: 0.6,
                    height: 0.6,
                });
            }
            envelope
                .recipe
                .masks
                .push(rapidraw_edit_model::MaskContainer {
                    id: "mask-1".to_string(),
                    name: "radial".to_string(),
                    visible: true,
                    adjustments: rapidraw_edit_model::MaskLocalAdjustments {
                        exposure: -2.0,
                        ..Default::default()
                    },
                    sub_masks: vec![rapidraw_edit_model::SubMask {
                        id: "sub-1".to_string(),
                        kind: "radial".to_string(),
                        geometry: Some(rapidraw_edit_model::MaskGeometry::Radial {
                            center_x: 0.3,
                            center_y: 0.5,
                            radius_x: 0.2,
                            radius_y: 0.4,
                            rotation: 0.0,
                            feather: 0.4,
                        }),
                        ..Default::default()
                    }],
                    ..Default::default()
                });
            envelope
        };

        // Plain frame without masks.
        let base = render(service.session_envelope(sid).unwrap(), 1)
            .expect("unmasked preview renders or the GPU error surfaces");

        // Masked frames: plain, and combined with crop + orientation.
        let masked = render(masked_envelope(false), 2);
        let masked_geo = render(masked_envelope(true), 3);
        for (label, outcome) in [("masked", masked), ("masked+crop/orientation", masked_geo)] {
            let frame = match outcome {
                Ok(frame) => frame,
                Err(DevelopError::Unsupported(message))
                    if message.to_lowercase().contains("gpu") =>
                {
                    // Honest capability skip: no device on this machine.
                    continue;
                }
                Err(other) => panic!("masked {label} preview failed unexpectedly: {other:?}"),
            };
            assert_ne!(
                frame.rgba8, base.rgba8,
                "masked {label} preview must differ from the unmasked render"
            );
        }
        service.close_session(sid).unwrap();
    }

    #[test]
    fn undecodable_source_is_an_explicit_error() {
        let bytes = fs::read(fixture_path("synthetic/not-a-raw.dng")).expect("fixture");
        let dir = tmp_dir("badraw");
        let source = write_source(&dir, "broken.dng", &bytes);
        let service = service_with(Arc::new(GateRenderer::new(&[]).0), Arc::new(NullStore));
        let err = service
            .open_session(open_input("asset-a", &source, &bytes))
            .expect_err("undecodable sources fail explicitly");
        assert!(matches!(err, DevelopError::Decode(_)), "{err:?}");
    }

    #[test]
    fn unsupported_gpu_device_is_an_explicit_error() {
        // An empty backend set deterministically yields NoAdapter: the
        // unsupported-device path must produce a typed error, never pixels.
        let renderer = GpuPreviewRenderer::with_context_factory(Box::new(|| {
            rapidraw_develop::gpu::OffscreenGpuContext::new_with_backends(wgpu::Backends::empty())
        }));
        let report = renderer.probe();
        assert!(!report.available, "empty backends cannot report a device");
        assert!(report.error.is_some());

        let bytes = gradient_bytes();
        let decoded = decode_original(&bytes, &DecodeOptions::default()).expect("decode");
        let envelope =
            RecipeEnvelope::new("lap-test/0", "asset-a", "default", &fingerprint(&bytes));
        let (cancel_source, cancel) = CancelToken::pair();
        let _ = cancel_source;
        let job = PreviewJob {
            session_id: SessionId(1),
            asset_id: "asset-a".to_string(),
            variant_id: "default".to_string(),
            generation: 1,
            quality: PreviewQuality::Settled,
            max_edge: 48,
            original: Arc::new(decoded),
            envelope,
            cancel,
        };
        let err = renderer
            .render(&job)
            .expect_err("no device means no pixels");
        let message = err.to_string().to_lowercase();
        assert!(
            matches!(err, EngineError::Unsupported(_)) && message.contains("gpu"),
            "unsupported device must be a typed explicit failure, got {err:?}"
        );
    }

    #[test]
    fn capability_report_carries_engine_identity() {
        let service = service_with(Arc::new(GateRenderer::new(&[]).0), Arc::new(NullStore));
        let report = service.capabilities();
        assert_eq!(report.engine_revision, ENGINE_GIT_REVISION);
        assert_eq!(
            report.edit_model_version,
            rapidraw_edit_model::MODEL_VERSION
        );
        assert_eq!(report.schema_version, rapidraw_edit_model::SCHEMA_VERSION);
    }

    #[test]
    fn lut_resources_resolve_explicitly_in_the_preview_renderer() {
        use crate::develop::resources::{ResourceStore, lut_resource_id};
        use rapidraw_edit_model::{ResourceAlgorithm, ResourceRef};

        fn failing_gpu() -> super::GpuContextFactory {
            Box::new(|| {
                rapidraw_develop::gpu::OffscreenGpuContext::new_with_backends(
                    wgpu::Backends::empty(),
                )
            })
        }

        // No store installed: resolution defers to the engine's own
        // ResourceMissing backstop (kept explicit, never a fake success).
        let renderer = GpuPreviewRenderer::with_context_factory(failing_gpu());
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
        assert!(renderer.resolve_render_lut(&envelope).unwrap().is_none());

        // With a store but an absent object: explicit typed failure.
        let store = ResourceStore::open(&tmp_dir("lut-missing")).expect("store opens");
        let renderer = GpuPreviewRenderer::with_context_factory(failing_gpu())
            .with_resource_store(Some(Arc::new(store)));
        let err = renderer
            .resolve_render_lut(&envelope)
            .expect_err("a missing LUT resource must fail explicitly");
        assert!(
            matches!(err, EngineError::Unsupported(ref message) if message.contains("missing")),
            "missing LUT resources surface as explicit failures, got {err:?}"
        );

        // A recipe without a LUT reference resolves to no payload.
        let plain = RecipeEnvelope::new("lap-test/0", "asset-a", "default", &"a".repeat(64));
        assert!(renderer.resolve_render_lut(&plain).unwrap().is_none());
    }

    #[test]
    fn real_gpu_renderer_succeeds_or_fails_explicitly() {
        // Honest capability probe on this machine: either the offscreen device
        // exists and renders the gradient through the high-precision pipeline,
        // or the failure is explicit. A fake/unadjusted success is impossible.
        let renderer = GpuPreviewRenderer::new();
        let report = renderer.probe();
        let bytes = gradient_bytes();
        let decoded: Arc<DecodedOriginal> =
            Arc::new(decode_original(&bytes, &DecodeOptions::default()).expect("decode"));
        let envelope =
            RecipeEnvelope::new("lap-test/0", "asset-a", "default", &fingerprint(&bytes));
        let (cancel_source, cancel) = CancelToken::pair();
        let _ = cancel_source;
        let job = PreviewJob {
            session_id: SessionId(7),
            asset_id: "asset-a".to_string(),
            variant_id: "default".to_string(),
            generation: 1,
            quality: PreviewQuality::Settled,
            max_edge: 48,
            original: Arc::clone(&decoded),
            envelope,
            cancel,
        };
        match renderer.render(&job) {
            Ok(frame) => {
                assert!(report.available, "a successful render implies a device");
                let (w, h) = decoded.image.dimensions();
                let expected = super::preview_dimensions(w, h, 48);
                assert_eq!((frame.width, frame.height), expected);
                let min = frame.rgba8.iter().copied().min().unwrap();
                let max = frame.rgba8.iter().copied().max().unwrap();
                assert!(max > min, "rendered preview must not be constant");
            }
            Err(err) => {
                assert!(
                    matches!(err, EngineError::Unsupported(_)),
                    "without a device the failure must be a typed explicit error, got {err:?}"
                );
            }
        }
    }

    #[test]
    fn missing_lens_profile_fails_the_preview_before_the_gpu() {
        use crate::develop::resources::ResourceStore;
        use rapidraw_develop::lens::LensWarpParams;

        fn failing_gpu() -> super::GpuContextFactory {
            Box::new(|| {
                rapidraw_develop::gpu::OffscreenGpuContext::new_with_backends(
                    wgpu::Backends::empty(),
                )
            })
        }

        let store = Arc::new(ResourceStore::open(&tmp_dir("lens-preview-missing")).expect("store"));
        let renderer = GpuPreviewRenderer::with_context_factory(failing_gpu())
            .with_resource_store(Some(Arc::clone(&store) as _));

        // A recipe without a lens profile resolves to nothing.
        let plain = RecipeEnvelope::new("lap-test/0", "asset-a", "default", &"a".repeat(64));
        assert!(
            renderer
                .resolve_render_lens_profile(&plain)
                .unwrap()
                .is_none()
        );

        // A referenced profile cannot be verified without a store.
        let renderer_without_store = GpuPreviewRenderer::with_context_factory(failing_gpu());
        let mut unverified = plain.clone();
        unverified.recipe.lens_profile = Some(rapidraw_edit_model::LensProfileRef {
            uri: format!("resource://lens/{}", "b".repeat(64)),
            maker: "TestCorp".to_string(),
            model: "Test 24-70mm f/2.8".to_string(),
            version: "v1".to_string(),
            sha256: "b".repeat(64),
        });
        let err = renderer_without_store
            .resolve_render_lens_profile(&unverified)
            .expect_err("profile verification requires a resource store");
        assert!(
            matches!(err, EngineError::Unsupported(ref m) if m.contains("resource store")),
            "got {err:?}"
        );

        // Missing object: explicit failure naming the profile, BEFORE the GPU
        // capability error — the check must precede device work.
        let profiled_envelope = {
            let imported = store
                .import_lens_profile_bytes(
                    b"<?xml version=\"1.0\"?><lensdatabase><lens><maker>M</maker><model>L</model><mount>T</mount></lens></lensdatabase>",
                    None,
                    Some("v1"),
                )
                .expect("profile imports");
            let mut envelope =
                RecipeEnvelope::new("lap-test/0", "asset-a", "default", &"a".repeat(64));
            crate::develop::resources::attach_lens_profile(&mut envelope, &imported, "M", "L");
            // Simulate a lost/pruned resource object before any render.
            let root = store.root().to_path_buf();
            std::fs::remove_file(
                root.join("objects")
                    .join(&imported.digest[..2])
                    .join(&imported.digest),
            )
            .expect("object removed");
            envelope
        };
        let err = renderer
            .resolve_render_lens_profile(&profiled_envelope)
            .expect_err("a missing lens profile object must fail explicitly");
        assert!(
            matches!(err, EngineError::Unsupported(ref m) if m.to_lowercase().contains("missing")),
            "got {err:?}"
        );

        // The warp helper is identity for neutral lens fields.
        let image = LinearImage::from_fn(4, 3, |x, y| [x as f32 / 4.0, y as f32 / 3.0, 0.5]);
        let warped =
            rapidraw_develop::lens_warp(&image, &LensWarpParams::from_recipe(&plain.recipe));
        assert_eq!(
            warped.rgb(),
            image.rgb(),
            "neutral lens fields never touch pixels"
        );

        // Full render path: the profile failure must surface before the GPU
        // failure (the injected device always fails, so only the profile
        // message can be present).
        let decoded = Arc::new(
            decode_original(&gradient_bytes(), &DecodeOptions::default()).expect("decode"),
        );
        let (cancel_source, cancel) = CancelToken::pair();
        let _ = cancel_source;
        let job = PreviewJob {
            session_id: SessionId(21),
            asset_id: "asset-a".to_string(),
            variant_id: "default".to_string(),
            generation: 1,
            quality: PreviewQuality::Settled,
            max_edge: 48,
            original: decoded,
            envelope: profiled_envelope,
            cancel,
        };
        let err = renderer
            .render(&job)
            .expect_err("a missing profile must fail the preview explicitly");
        let message = err.to_string().to_lowercase();
        assert!(
            message.contains("lens"),
            "the profile failure must be visible, got {message}"
        );
        assert!(
            !message.contains("gpu"),
            "the profile check must precede GPU work, got {message}"
        );
    }

    #[test]
    fn recipe_geometry_hash_covers_lens_correction() {
        use rapidraw_develop::lens::LensWarpParams;

        let base = rapidraw_edit_model::Recipe::default();
        let base_hash = super::recipe_geometry_hash(&base);
        assert_eq!(
            base_hash,
            super::recipe_geometry_hash(&rapidraw_edit_model::Recipe::default()),
            "equal recipes hash equally"
        );

        // Lens coefficient changes must invalidate the geometry input cache.
        let mut distorted = base.clone();
        distorted.lens_distortion_params = Some(rapidraw_edit_model::LensDistortionParams {
            k1: -0.05,
            k2: 0.0,
            k3: 0.0,
            model: 0.0,
            tca_vr: 1.0,
            tca_vb: 1.0,
            vig_k1: 0.0,
            vig_k2: 0.0,
            vig_k3: 0.0,
        });
        assert_ne!(
            base_hash,
            super::recipe_geometry_hash(&distorted),
            "lens distortion changes the geometry-applied input"
        );

        // Amount and enable changes too.
        let mut stronger = distorted.clone();
        stronger.lens_distortion_amount = 150.0;
        assert_ne!(
            super::recipe_geometry_hash(&distorted),
            super::recipe_geometry_hash(&stronger),
        );
        let mut disabled = distorted.clone();
        disabled.lens_distortion_enabled = false;
        assert_ne!(
            super::recipe_geometry_hash(&distorted),
            super::recipe_geometry_hash(&disabled),
        );

        // Non-lens edits never invalidate the geometry input cache.
        let mut exposure_edit = base.clone();
        exposure_edit.exposure = 0.7;
        assert_eq!(
            base_hash,
            super::recipe_geometry_hash(&exposure_edit),
            "adjustment-only edits reuse the geometry input"
        );

        // The warp params mirror the recipe and identity stays identity.
        assert!(LensWarpParams::from_recipe(&base).is_identity());
        assert!(!LensWarpParams::from_recipe(&distorted).is_identity());
    }

    #[test]
    fn lens_correction_warp_reaches_the_preview() {
        // Honest capability gate: with a real offscreen device, a recipe with
        // strong lens distortion must render visibly differently from the
        // same recipe without it (the warp is applied to the linear input
        // before downscaling and adjustments). Without a device the failure
        // is an explicit GPU capability error and the check is skipped.
        let renderer = GpuPreviewRenderer::new();
        if !renderer.probe().available {
            return;
        }
        let store = Arc::new(
            crate::develop::resources::ResourceStore::open(&tmp_dir("lens-preview-warp"))
                .expect("store"),
        );
        let renderer = renderer.with_resource_store(Some(store));

        let decoded = Arc::new(
            decode_original(&gradient_bytes(), &DecodeOptions::default()).expect("decode"),
        );
        let render = |envelope: RecipeEnvelope, generation: u64| {
            let (cancel_source, cancel) = CancelToken::pair();
            let _ = cancel_source;
            let job = PreviewJob {
                session_id: SessionId(31),
                asset_id: "asset-a".to_string(),
                variant_id: "default".to_string(),
                generation,
                quality: PreviewQuality::Settled,
                max_edge: 64,
                original: Arc::clone(&decoded),
                envelope,
                cancel,
            };
            renderer.render(&job).expect("lens preview renders")
        };

        let mut corrected = RecipeEnvelope::new(
            "lap-test/0",
            "asset-a",
            "default",
            &fingerprint(&gradient_bytes()),
        );
        corrected.recipe.lens_distortion_params = Some(rapidraw_edit_model::LensDistortionParams {
            k1: -0.2,
            k2: 0.05,
            k3: 0.0,
            model: 0.0,
            tca_vr: 1.0,
            tca_vb: 1.0,
            vig_k1: -0.4,
            vig_k2: 0.0,
            vig_k3: 0.0,
        });
        let with_lens = render(corrected, 1);
        let without_lens = render(
            RecipeEnvelope::new(
                "lap-test/0",
                "asset-a",
                "default",
                &fingerprint(&gradient_bytes()),
            ),
            2,
        );
        assert_ne!(
            with_lens.rgba8, without_lens.rgba8,
            "lens correction must change the rendered preview"
        );
    }
}

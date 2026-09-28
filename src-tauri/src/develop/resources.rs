//! Content-addressed development resources (lap-62b / TASK-405; lens
//! profiles added in lap-d52; governing contract
//! `docs/raw-development/spec.md`, "Persistence and compatibility", and the
//! schema's `resources` rules in `docs/raw-development/schema.md`).
//!
//! Recipes reference external render resources (LUTs and lensfun profile
//! files now; depth maps and other bitmap resources through the same map
//! later) by stable, content-derived ids in `RecipeEnvelope::resources`;
//! inline payloads are never persisted inside a recipe. Non-AI masks
//! (lap-78d) carry their geometry inline in the validated recipe schema, so
//! this slice needs no external mask resources; AI mask payloads are outside
//! the engine entirely.
//!
//! - **Content addressing.** A resource id is `lut/<sha256-hex>` or
//!   `lens/<sha256-hex>` — the SHA-256 of the resource bytes. Ids are
//!   deterministic, deduplicated and portable across assets and machines:
//!   identical content always maps to the same id, and file operations
//!   (copy/move/rebuild) re-associate recipes without duplicating payloads
//!   or sharing asset identity.
//! - **Versioned lens profiles.** A lens profile import records the
//!   lensfun database file's content hash and an explicit version label.
//!   Selecting a lens for a recipe resolves the distortion/TCA/vignetting
//!   coefficients through the engine and persists
//!   [`rapidraw_edit_model::LensProfileRef`] provenance (maker/model/
//!   version/sha256) plus the resolved parameters. Renders verify the
//!   profile object is still present and unchanged before any pixel work:
//!   a missing, changed or unmapped profile is a typed error, never a
//!   silently different export (spec A7). No lens data is bundled with Lap;
//!   profiles are locally acquired and their distribution terms remain an
//!   unresolved prerequisite (`docs/raw-development/provenance.json`).
//! - **Bounded storage.** Imports are rejected above
//!   [`ResourceStoreLimits::max_resource_bytes`] and LUT cube edges above
//!   [`ResourceStoreLimits::max_lut_edge`]; parsed payloads are cached in
//!   bounded LRUs.
//! - **Explicit limitations.** Missing, changed (tampered), corrupt,
//!   oversized and non-portable references surface as typed errors /
//!   limitation reports. Nothing silently renders without a referenced
//!   resource (spec A7).
//! - **Portable references.** `recipe.lut_path` and the lens-profile URI
//!   hold `resource://<id>` URIs; absolute paths are never portable and
//!   fail explicitly when resolved.
//!
//! Provenance: `.cube`/`.3dl` parsing semantics and the lens-correction
//! resolution semantics follow RapidRAW `src-tauri/src/lut_processing.rs` and
//! `src-tauri/src/lens_correction.rs` at revision
//! `5e30bcbb246395d391ba2e9662510641ffe68e6b`, tightened with explicit bounds
//! and error outcomes. Distribution/licensing of bundled content remains an
//! unresolved prerequisite (spec P3); no LUT or lens data is shipped here.

use std::collections::{BTreeSet, VecDeque};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rapidraw_develop::gpu::LutData;
use rapidraw_develop::lens::{LensCapabilityNotice, LensDatabase};
use rapidraw_edit_model::{
    LensDistortionParams, LensProfileRef, RecipeEnvelope, ResourceAlgorithm, ResourceRef,
    sha256_hex,
};

/// URI scheme of portable in-envelope resource references.
pub const RESOURCE_URI_SCHEME: &str = "resource://";

/// Default store bounds. A 256-edge cube is ~50 MB as f32 triplets; the byte
/// bound caps files well below the schema's 1 GiB `sizeBytes` ceiling.
pub const DEFAULT_MAX_RESOURCE_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_MAX_LUT_EDGE: u32 = 256;
pub const DEFAULT_MAX_CACHED_LUTS: usize = 8;

// ---------------------------------------------------------------------------
// Errors, statuses, limitations
// ---------------------------------------------------------------------------

/// Typed resource failure. Every outcome is explicit; callers surface these
/// instead of silently rendering without the resource.
#[derive(Debug)]
pub enum ResourceError {
    Io {
        context: String,
        path: PathBuf,
        source: std::io::Error,
    },
    /// The id is not a well-formed `lut/<64-hex>` content id.
    InvalidId { id: String },
    /// The file extension has no supported LUT format in this slice.
    UnsupportedFormat { path: PathBuf, extension: String },
    /// The payload does not parse as the claimed format.
    Corrupt { detail: String },
    /// The payload or object exceeds a configured bound.
    Oversized { detail: String },
    /// The content-addressed object is absent from the store.
    Missing { id: String },
    /// The stored object's bytes no longer match its content id (replaced,
    /// truncated or tampered after import).
    Changed {
        id: String,
        expected: String,
        found: String,
    },
    /// The recipe's LUT reference cannot be resolved from the envelope's
    /// resource map (legacy absolute path, unmapped id, digest mismatch).
    UnresolvedReference { reference: String, detail: String },
    /// A lens profile cannot be used: unknown lens, or a selected distortion
    /// model this engine does not implement. Rendered selections never
    /// silently fall back to no correction (spec A7).
    UnsupportedLens { detail: String },
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResourceError::Io {
                context,
                path,
                source,
            } => write!(f, "{context} ({}): {source}", path.display()),
            ResourceError::InvalidId { id } => {
                write!(f, "'{id}' is not a well-formed lut/<64-hex> resource id")
            }
            ResourceError::UnsupportedFormat { path, extension } => write!(
                f,
                "unsupported LUT format '{extension}' at {}",
                path.display()
            ),
            ResourceError::Corrupt { detail } => write!(f, "LUT resource is corrupt: {detail}"),
            ResourceError::Oversized { detail } => write!(f, "LUT resource is oversized: {detail}"),
            ResourceError::Missing { id } => {
                write!(f, "resource '{id}' is missing from the store")
            }
            ResourceError::Changed {
                id,
                expected,
                found,
            } => write!(
                f,
                "resource '{id}' changed after import (expected sha256 {expected}, found {found})"
            ),
            ResourceError::UnresolvedReference { reference, detail } => {
                let kind = if reference.starts_with("resource://lens/") {
                    "lens profile"
                } else {
                    "LUT"
                };
                write!(
                    f,
                    "{kind} reference '{reference}' cannot be resolved: {detail}"
                )
            }
            ResourceError::UnsupportedLens { detail } => {
                write!(f, "lens profile cannot be used: {detail}")
            }
        }
    }
}

impl std::error::Error for ResourceError {}

/// Observed state of one content-addressed object in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceStatus {
    Present {
        digest: String,
        size_bytes: u64,
    },
    Missing,
    Changed {
        expected_digest: String,
        found_digest: String,
        size_bytes: u64,
    },
}

/// One observed resource problem, reported by
/// [`ResourceStore::limitations`]. `kind` is a stable machine string:
/// `missing`, `changed`, `invalid-resource-id`, `oversized`, `unavailable`,
/// `unresolved-reference`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLimitation {
    pub id: String,
    pub kind: &'static str,
    pub detail: String,
}

// ---------------------------------------------------------------------------
// Ids and portable references
// ---------------------------------------------------------------------------

fn is_valid_resource_id(id: &str) -> bool {
    let Some(digest) = id.strip_prefix("lut/").or_else(|| id.strip_prefix("lens/")) else {
        return false;
    };
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// The content id of a LUT resource: `lut/<sha256 lowercase hex>`.
pub fn lut_resource_id(digest: &str) -> String {
    format!("lut/{digest}")
}

/// The content id of a lens-profile resource: `lens/<sha256 lowercase hex>`.
pub fn lens_resource_id(digest: &str) -> String {
    format!("lens/{digest}")
}

/// Portable in-envelope reference for a resource id.
pub fn lut_resource_uri(id: &str) -> String {
    format!("{RESOURCE_URI_SCHEME}{id}")
}

/// Parses a `resource://<id>` reference, returning the id only when it is
/// well-formed. Anything else (absolute paths, relative paths, malformed
/// ids) is not portable and returns `None`.
pub fn resource_uri_id(value: &str) -> Option<&str> {
    let id = value.strip_prefix(RESOURCE_URI_SCHEME)?;
    if is_valid_resource_id(id) {
        Some(id)
    } else {
        None
    }
}

/// Whether a recipe string field is a portable reference. Absolute and
/// relative paths are machine-local and must never propagate across assets.
pub fn is_portable_reference(value: Option<&str>) -> bool {
    match value {
        None => true,
        Some(value) => resource_uri_id(value).is_some(),
    }
}

/// Resource ids the recipe's fields actually reference (LUT path, lens
/// profile, depth maps through the same URI scheme). Sorted, deduplicated.
pub fn referenced_resource_ids(envelope: &RecipeEnvelope) -> Vec<String> {
    let mut ids = BTreeSet::new();
    for reference in [
        envelope.recipe.lut_path.as_deref(),
        envelope.recipe.lens_blur_depth_map.as_deref(),
    ] {
        if let Some(id) = reference.and_then(resource_uri_id) {
            ids.insert(id.to_string());
        }
    }
    if let Some(profile) = &envelope.recipe.lens_profile
        && let Some(id) = resource_uri_id(&profile.uri)
    {
        ids.insert(id.to_string());
    }
    ids.into_iter().collect()
}

// ---------------------------------------------------------------------------
// LUT formats (bounded parsers)
// ---------------------------------------------------------------------------

/// Supported text LUT container formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LutFormat {
    /// Autodesk/IRIDAS `.cube` (LUT_3D_SIZE header).
    Cube,
    /// Lookup `.3dl` (header-less float triplets).
    ThreeDl,
}

impl LutFormat {
    pub fn from_extension(extension: &str) -> Option<LutFormat> {
        match extension.to_ascii_lowercase().as_str() {
            "cube" => Some(LutFormat::Cube),
            "3dl" => Some(LutFormat::ThreeDl),
            _ => None,
        }
    }
}

/// A successfully imported resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedLut {
    pub id: String,
    pub digest: String,
    pub size_bytes: u64,
    pub cube_size: u32,
    pub name: Option<String>,
    /// `false` when identical content was already stored (deduplicated).
    pub newly_stored: bool,
}

impl ImportedLut {
    /// The envelope `resources` entry for this import.
    pub fn resource(&self) -> ResourceRef {
        ResourceRef {
            algorithm: ResourceAlgorithm::Sha256,
            digest: self.digest.clone(),
            size_bytes: Some(self.size_bytes),
        }
    }
}

/// Parses a bounded `.cube` document into `(cube edge, rgb f32 triplets)`.
fn parse_cube(bytes: &[u8], max_edge: u32) -> Result<(u32, Vec<f32>), ResourceError> {
    let text = std::str::from_utf8(bytes).map_err(|err| ResourceError::Corrupt {
        detail: format!(".cube documents must be UTF-8 text: {err}"),
    })?;
    let mut size: Option<u32> = None;
    let mut data: Vec<f32> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        match parts[0].to_uppercase().as_str() {
            "TITLE" | "DOMAIN_MIN" | "DOMAIN_MAX" => continue,
            "LUT_3D_SIZE" => {
                if size.is_some() {
                    return Err(ResourceError::Corrupt {
                        detail: "duplicate LUT_3D_SIZE header".to_string(),
                    });
                }
                if parts.len() < 2 {
                    return Err(ResourceError::Corrupt {
                        detail: "malformed LUT_3D_SIZE header (missing value)".to_string(),
                    });
                }
                let value: u32 = parts[1].parse().map_err(|_| ResourceError::Corrupt {
                    detail: format!("malformed LUT_3D_SIZE value '{}'", parts[1]),
                })?;
                if value < 2 {
                    return Err(ResourceError::Corrupt {
                        detail: format!("LUT_3D_SIZE must be at least 2, found {value}"),
                    });
                }
                if value > max_edge {
                    return Err(ResourceError::Oversized {
                        detail: format!("cube edge {value} exceeds the bounded maximum {max_edge}"),
                    });
                }
                size = Some(value);
            }
            _ => {
                let Some(lut_size) = size else {
                    return Err(ResourceError::Corrupt {
                        detail: "data line before LUT_3D_SIZE header".to_string(),
                    });
                };
                let _ = lut_size;
                if parts.len() < 3 {
                    return Err(ResourceError::Corrupt {
                        detail: format!("expected 3 float values, found {}", parts.len()),
                    });
                }
                let mut triplet = [0f32; 3];
                for (slot, raw) in triplet.iter_mut().zip(parts.iter().take(3)) {
                    let value: f32 = raw.parse().map_err(|_| ResourceError::Corrupt {
                        detail: format!("invalid float value '{raw}'"),
                    })?;
                    if !value.is_finite() {
                        return Err(ResourceError::Corrupt {
                            detail: format!("LUT values must be finite, found '{raw}'"),
                        });
                    }
                    *slot = value;
                }
                data.extend_from_slice(&triplet);
            }
        }
    }

    let Some(lut_size) = size else {
        return Err(ResourceError::Corrupt {
            detail: "LUT_3D_SIZE header not found".to_string(),
        });
    };
    let expected = lut_size as usize * lut_size as usize * lut_size as usize * 3;
    if data.len() != expected {
        return Err(ResourceError::Corrupt {
            detail: format!(
                "LUT data size mismatch: expected {expected} float values for size {lut_size}, found {}",
                data.len()
            ),
        });
    }
    Ok((lut_size, data))
}

/// Parses a bounded `.3dl` document (whitespace-separated triplets; any other
/// line is ignored, as in the reference parser).
fn parse_three_dl(bytes: &[u8], max_edge: u32) -> Result<(u32, Vec<f32>), ResourceError> {
    let text = std::str::from_utf8(bytes).map_err(|err| ResourceError::Corrupt {
        detail: format!(".3dl documents must be UTF-8 text: {err}"),
    })?;
    let mut data: Vec<f32> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() != 3 {
            continue;
        }
        let mut triplet = [0f32; 3];
        for (slot, raw) in triplet.iter_mut().zip(parts.iter()) {
            let value: f32 = raw.parse().map_err(|_| ResourceError::Corrupt {
                detail: format!("invalid float value '{raw}'"),
            })?;
            if !value.is_finite() {
                return Err(ResourceError::Corrupt {
                    detail: format!("LUT values must be finite, found '{raw}'"),
                });
            }
            *slot = value;
        }
        data.extend_from_slice(&triplet);
    }

    if data.is_empty() {
        return Err(ResourceError::Corrupt {
            detail: "no data found in .3dl document".to_string(),
        });
    }
    let entries = data.len() / 3;
    let size = (entries as f64).cbrt().round() as u32;
    if size as usize * size as usize * size as usize != entries {
        return Err(ResourceError::Corrupt {
            detail: format!("invalid .3dl data size: {entries} entries is not a perfect cube"),
        });
    }
    if size < 2 {
        return Err(ResourceError::Corrupt {
            detail: format!(".3dl cube size must be at least 2, found {size}"),
        });
    }
    if size > max_edge {
        return Err(ResourceError::Oversized {
            detail: format!("cube edge {size} exceeds the bounded maximum {max_edge}"),
        });
    }
    Ok((size, data))
}

/// Recovers the LUT format from the stored object content: a `.cube`
/// document is recognized by its `LUT_3D_SIZE` header, anything else is
/// parsed as `.3dl`.
fn parse_lut_auto(bytes: &[u8], max_edge: u32) -> Result<(u32, Vec<f32>), ResourceError> {
    match parse_cube(bytes, max_edge) {
        Ok(parsed) => Ok(parsed),
        Err(ResourceError::Corrupt { detail })
            if detail.contains("LUT_3D_SIZE header not found") =>
        {
            parse_three_dl(bytes, max_edge)
        }
        Err(other) => Err(other),
    }
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// Bounded store configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceStoreLimits {
    /// Maximum accepted resource payload size in bytes.
    pub max_resource_bytes: u64,
    /// Maximum LUT cube edge length.
    pub max_lut_edge: u32,
    /// Maximum number of parsed LUT payloads kept in the bounded cache.
    pub max_cached_luts: usize,
}

impl Default for ResourceStoreLimits {
    fn default() -> Self {
        Self {
            max_resource_bytes: DEFAULT_MAX_RESOURCE_BYTES,
            max_lut_edge: DEFAULT_MAX_LUT_EDGE,
            max_cached_luts: DEFAULT_MAX_CACHED_LUTS,
        }
    }
}

/// Content-addressed resource store rooted at a directory:
/// `<root>/objects/<digest[0..2]>/<digest>`. Objects are immutable once
/// written; integrity is verified against the id on every status/load.
pub struct ResourceStore {
    root: PathBuf,
    limits: ResourceStoreLimits,
    cache: Mutex<VecDeque<(String, Arc<LutData>)>>,
    lens_cache: Mutex<VecDeque<(String, Arc<LensDatabase>)>>,
}

impl ResourceStore {
    /// Opens (and creates) the default-bounded store at `root`.
    pub fn open(root: &Path) -> Result<ResourceStore, ResourceError> {
        Self::with_limits(root, ResourceStoreLimits::default())
    }

    /// Opens (and creates) the store with explicit bounds.
    pub fn with_limits(
        root: &Path,
        limits: ResourceStoreLimits,
    ) -> Result<ResourceStore, ResourceError> {
        fs::create_dir_all(root.join("objects")).map_err(|err| ResourceError::Io {
            context: "creating the resource store directory".to_string(),
            path: root.to_path_buf(),
            source: err,
        })?;
        Ok(Self {
            root: root.to_path_buf(),
            limits,
            cache: Mutex::new(VecDeque::new()),
            lens_cache: Mutex::new(VecDeque::new()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn limits(&self) -> &ResourceStoreLimits {
        &self.limits
    }

    fn object_path(&self, digest: &str) -> PathBuf {
        self.root.join("objects").join(&digest[..2]).join(digest)
    }

    /// Imports a LUT from a file. The extension selects the format; oversize
    /// files are rejected before reading.
    pub fn import_lut_file(&self, path: &Path) -> Result<ImportedLut, ResourceError> {
        let extension = path
            .extension()
            .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let format = LutFormat::from_extension(&extension).ok_or_else(|| {
            ResourceError::UnsupportedFormat {
                path: path.to_path_buf(),
                extension: extension.clone(),
            }
        })?;
        let metadata = fs::metadata(path).map_err(|err| ResourceError::Io {
            context: "inspecting the LUT file".to_string(),
            path: path.to_path_buf(),
            source: err,
        })?;
        if metadata.len() > self.limits.max_resource_bytes {
            return Err(self.oversized_bytes(metadata.len()));
        }
        let bytes = fs::read(path).map_err(|err| ResourceError::Io {
            context: "reading the LUT file".to_string(),
            path: path.to_path_buf(),
            source: err,
        })?;
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string());
        self.import_lut_bytes(&bytes, name.as_deref(), format)
    }

    /// Imports LUT bytes of the given format. Deterministic: identical bytes
    /// always produce the same id and are stored exactly once.
    pub fn import_lut_bytes(
        &self,
        bytes: &[u8],
        name: Option<&str>,
        format: LutFormat,
    ) -> Result<ImportedLut, ResourceError> {
        let len = bytes.len() as u64;
        if len > self.limits.max_resource_bytes {
            return Err(self.oversized_bytes(len));
        }
        let (cube_size, _data) = match format {
            LutFormat::Cube => parse_cube(bytes, self.limits.max_lut_edge)?,
            LutFormat::ThreeDl => parse_three_dl(bytes, self.limits.max_lut_edge)?,
        };
        let digest = sha256_hex(bytes);
        let id = lut_resource_id(&digest);
        let object = self.object_path(&digest);
        let mut newly_stored = false;
        if !object.exists() {
            if let Some(parent) = object.parent() {
                fs::create_dir_all(parent).map_err(|err| ResourceError::Io {
                    context: "creating the object shard directory".to_string(),
                    path: parent.to_path_buf(),
                    source: err,
                })?;
            }
            let temp = self.temp_object_path(&digest);
            fs::write(&temp, bytes).map_err(|err| ResourceError::Io {
                context: "writing the resource object".to_string(),
                path: temp.clone(),
                source: err,
            })?;
            if let Err(err) = fs::rename(&temp, &object) {
                let _ = fs::remove_file(&temp);
                return Err(ResourceError::Io {
                    context: "placing the resource object".to_string(),
                    path: object.clone(),
                    source: err,
                });
            }
            newly_stored = true;
        }
        Ok(ImportedLut {
            id,
            digest,
            size_bytes: len,
            cube_size,
            name: name.map(str::to_string),
            newly_stored,
        })
    }

    /// Observes one object's integrity against its content id. Reads are
    /// bounded by `max_resource_bytes`. Accepts both `lut/` and `lens/` ids.
    pub fn status(&self, id: &str) -> Result<ResourceStatus, ResourceError> {
        let digest = id
            .strip_prefix("lut/")
            .or_else(|| id.strip_prefix("lens/"))
            .ok_or_else(|| ResourceError::InvalidId { id: id.to_string() })?;
        if !is_valid_resource_id(id) {
            return Err(ResourceError::InvalidId { id: id.to_string() });
        }
        let object = self.object_path(digest);
        let metadata = match fs::metadata(&object) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ResourceStatus::Missing);
            }
            Err(err) => {
                return Err(ResourceError::Io {
                    context: "inspecting the resource object".to_string(),
                    path: object,
                    source: err,
                });
            }
        };
        let size_bytes = metadata.len();
        if size_bytes > self.limits.max_resource_bytes {
            return Err(self.oversized_bytes(size_bytes));
        }
        let bytes = fs::read(&object).map_err(|err| ResourceError::Io {
            context: "reading the resource object".to_string(),
            path: object,
            source: err,
        })?;
        let found = sha256_hex(&bytes);
        if found == digest {
            Ok(ResourceStatus::Present {
                digest: found,
                size_bytes,
            })
        } else {
            Ok(ResourceStatus::Changed {
                expected_digest: digest.to_string(),
                found_digest: found,
                size_bytes,
            })
        }
    }

    /// Loads the parsed LUT payload for a resource id. Integrity is verified
    /// against the content id on every call (missing/changed objects are
    /// typed errors even when a parsed copy is cached); successful payloads
    /// are kept in a bounded LRU cache.
    pub fn load_lut(&self, id: &str) -> Result<Arc<LutData>, ResourceError> {
        match self.status(id)? {
            ResourceStatus::Present { .. } => {}
            ResourceStatus::Missing => return Err(ResourceError::Missing { id: id.to_string() }),
            ResourceStatus::Changed {
                expected_digest,
                found_digest,
                ..
            } => {
                return Err(ResourceError::Changed {
                    id: id.to_string(),
                    expected: expected_digest,
                    found: found_digest,
                });
            }
        }
        if let Some(hit) = self.cached_lut(id) {
            return Ok(hit);
        }
        let digest = id.strip_prefix("lut/").unwrap_or_default();
        let bytes = fs::read(self.object_path(digest)).map_err(|err| ResourceError::Io {
            context: "reading the resource object".to_string(),
            path: self.object_path(digest),
            source: err,
        })?;
        // Objects are stored raw (byte-identical to the imported file); the
        // format is recovered from the content itself.
        let (size, data) = parse_lut_auto(&bytes, self.limits.max_lut_edge)?;
        let lut = Arc::new(LutData { size, data });
        self.remember_lut(id, &lut);
        Ok(lut)
    }

    fn cached_lut(&self, id: &str) -> Option<Arc<LutData>> {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let index = cache.iter().position(|(candidate, _)| candidate == id)?;
        let entry = cache.remove(index)?;
        cache.push_back(entry.clone());
        Some(entry.1)
    }

    fn remember_lut(&self, id: &str, lut: &Arc<LutData>) {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cache.iter().any(|(candidate, _)| candidate == id) {
            return;
        }
        while cache.len() >= self.limits.max_cached_luts.max(1) {
            cache.pop_front();
        }
        cache.push_back((id.to_string(), Arc::clone(lut)));
    }

    /// Resolves the render payload for a recipe's LUT reference.
    ///
    /// - No `lut_path` -> `Ok(None)`.
    /// - `resource://lut/<hex>` mapped in `envelope.resources` and present in
    ///   the store -> the parsed payload.
    /// - Absolute/legacy paths, unmapped ids, digest mismatches, missing and
    ///   changed objects -> typed errors, never `Ok(None)` for a LUT-enabled
    ///   recipe.
    pub fn resolve_recipe_lut(
        &self,
        envelope: &RecipeEnvelope,
    ) -> Result<Option<Arc<LutData>>, ResourceError> {
        let Some(reference) = envelope.recipe.lut_path.as_deref() else {
            return Ok(None);
        };
        let Some(id) = resource_uri_id(reference) else {
            return Err(ResourceError::UnresolvedReference {
                reference: reference.to_string(),
                detail: "only resource:// references are portable; legacy absolute paths must be re-imported into the resource store".to_string(),
            });
        };
        let entry = envelope.resources.get(id).ok_or_else(|| {
            ResourceError::UnresolvedReference {
                reference: reference.to_string(),
                detail: "the recipe references this resource but it is absent from the envelope resource map".to_string(),
            }
        })?;
        if entry.digest != id.strip_prefix("lut/").unwrap_or_default() {
            return Err(ResourceError::UnresolvedReference {
                reference: reference.to_string(),
                detail: format!(
                    "the envelope resource entry digest '{}' does not match its content id",
                    entry.digest
                ),
            });
        }
        self.load_lut(id).map(Some)
    }

    /// Non-fatal report of every observed resource problem in the envelope:
    /// store-level outcomes for mapped resources plus recipe references that
    /// are absent from the map. Present resources are not reported.
    pub fn limitations(&self, envelope: &RecipeEnvelope) -> Vec<ResourceLimitation> {
        let mut out = Vec::new();
        let mut unmapped = referenced_resource_ids(envelope)
            .into_iter()
            .collect::<BTreeSet<_>>();

        for id in envelope.resources.keys() {
            unmapped.remove(id);
            match self.status(id) {
                Ok(ResourceStatus::Present { .. }) => {}
                Ok(ResourceStatus::Missing) => out.push(ResourceLimitation {
                    id: id.clone(),
                    kind: "missing",
                    detail: "the resource object is absent from the store".to_string(),
                }),
                Ok(ResourceStatus::Changed {
                    expected_digest,
                    found_digest,
                    ..
                }) => out.push(ResourceLimitation {
                    id: id.clone(),
                    kind: "changed",
                    detail: format!(
                        "the stored object no longer matches its content id (expected {expected_digest}, found {found_digest})"
                    ),
                }),
                Err(ResourceError::InvalidId { .. }) => out.push(ResourceLimitation {
                    id: id.clone(),
                    kind: "invalid-resource-id",
                    detail: "the resource id is not a well-formed lut/<64-hex> or lens/<64-hex> id"
                        .to_string(),
                }),
                Err(ResourceError::Oversized { detail }) => out.push(ResourceLimitation {
                    id: id.clone(),
                    kind: "oversized",
                    detail,
                }),
                Err(err) => out.push(ResourceLimitation {
                    id: id.clone(),
                    kind: "unavailable",
                    detail: err.to_string(),
                }),
            }
        }

        for id in unmapped {
            out.push(ResourceLimitation {
                id,
                kind: "unresolved-reference",
                detail: "the recipe references this resource but it is absent from the envelope resource map".to_string(),
            });
        }
        out
    }

    fn oversized_bytes(&self, found: u64) -> ResourceError {
        ResourceError::Oversized {
            detail: format!(
                "{} bytes exceeds the bounded maximum of {} bytes",
                found, self.limits.max_resource_bytes
            ),
        }
    }

    fn temp_object_path(&self, digest: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        self.root
            .join("objects")
            .join(&digest[..2])
            .join(format!("{digest}.{}. {nanos:x}.tmp", std::process::id()))
    }
}

// ---------------------------------------------------------------------------
// Lens profile resources (lap-d52)
// ---------------------------------------------------------------------------

/// A successfully imported lens-profile (lensfun XML database file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedLensProfile {
    pub id: String,
    pub digest: String,
    pub size_bytes: u64,
    pub lens_count: usize,
    pub camera_count: usize,
    /// Explicit version label recorded at import (`None` = unversioned).
    pub version: Option<String>,
    pub name: Option<String>,
    /// `false` when identical content was already stored (deduplicated).
    pub newly_stored: bool,
}

impl ImportedLensProfile {
    /// The envelope `resources` entry for this import.
    pub fn resource(&self) -> ResourceRef {
        ResourceRef {
            algorithm: ResourceAlgorithm::Sha256,
            digest: self.digest.clone(),
            size_bytes: Some(self.size_bytes),
        }
    }

    /// The version label this profile contributes to provenance; imports
    /// without an explicit label stay explicitly `unversioned`, never
    /// invented.
    pub fn version_label(&self) -> &str {
        self.version.as_deref().unwrap_or("unversioned")
    }
}

/// One lens selection request: which stored profile, which lens in it, the
/// user-declared version label, and the capture conditions used to resolve
/// the coefficients.
#[derive(Debug, Clone, PartialEq)]
pub struct LensSelection<'a> {
    pub profile_id: &'a str,
    pub maker: &'a str,
    pub model: &'a str,
    pub version: &'a str,
    pub focal_length: f32,
    pub aperture: Option<f32>,
    pub distance: Option<f32>,
}

impl ResourceStore {
    /// Imports lensfun XML bytes as a versioned profile resource. The XML
    /// must parse (through the engine's lens module) before anything is
    /// stored; deterministic like every content-addressed import.
    pub fn import_lens_profile_bytes(
        &self,
        bytes: &[u8],
        name: Option<&str>,
        version: Option<&str>,
    ) -> Result<ImportedLensProfile, ResourceError> {
        let len = bytes.len() as u64;
        if len > self.limits.max_resource_bytes {
            return Err(self.oversized_bytes(len));
        }
        let text = std::str::from_utf8(bytes).map_err(|err| ResourceError::Corrupt {
            detail: format!("lens profiles must be UTF-8 lensfun XML documents: {err}"),
        })?;
        let database = rapidraw_develop::lens::parse_lensfun_db(text).map_err(|err| {
            ResourceError::Corrupt {
                detail: err.to_string(),
            }
        })?;
        let digest = sha256_hex(bytes);
        let id = lens_resource_id(&digest);
        let object = self.object_path(&digest);
        let mut newly_stored = false;
        if !object.exists() {
            if let Some(parent) = object.parent() {
                fs::create_dir_all(parent).map_err(|err| ResourceError::Io {
                    context: "creating the object shard directory".to_string(),
                    path: parent.to_path_buf(),
                    source: err,
                })?;
            }
            let temp = self.temp_object_path(&digest);
            fs::write(&temp, bytes).map_err(|err| ResourceError::Io {
                context: "writing the resource object".to_string(),
                path: temp.clone(),
                source: err,
            })?;
            if let Err(err) = fs::rename(&temp, &object) {
                let _ = fs::remove_file(&temp);
                return Err(ResourceError::Io {
                    context: "placing the resource object".to_string(),
                    path: object.clone(),
                    source: err,
                });
            }
            newly_stored = true;
        }
        Ok(ImportedLensProfile {
            id,
            digest,
            size_bytes: len,
            lens_count: database.lenses.len(),
            camera_count: database.cameras.len(),
            version: version.map(str::to_string),
            name: name.map(str::to_string),
            newly_stored,
        })
    }

    /// Loads the parsed lens database for a profile resource id. Integrity is
    /// verified against the content id on every call; successful parses are
    /// kept in a bounded LRU cache.
    pub fn lens_database(&self, id: &str) -> Result<Arc<LensDatabase>, ResourceError> {
        match self.status(id)? {
            ResourceStatus::Present { .. } => {}
            ResourceStatus::Missing => return Err(ResourceError::Missing { id: id.to_string() }),
            ResourceStatus::Changed {
                expected_digest,
                found_digest,
                ..
            } => {
                return Err(ResourceError::Changed {
                    id: id.to_string(),
                    expected: expected_digest,
                    found: found_digest,
                });
            }
        }
        if let Some(hit) = self.cached_lens(id) {
            return Ok(hit);
        }
        let digest = id.strip_prefix("lens/").unwrap_or_default();
        let bytes = fs::read(self.object_path(digest)).map_err(|err| ResourceError::Io {
            context: "reading the lens profile object".to_string(),
            path: self.object_path(digest),
            source: err,
        })?;
        let text = std::str::from_utf8(&bytes).map_err(|err| ResourceError::Corrupt {
            detail: format!("lens profiles must be UTF-8 lensfun XML documents: {err}"),
        })?;
        let database = Arc::new(
            rapidraw_develop::lens::parse_lensfun_db(text).map_err(|err| {
                ResourceError::Corrupt {
                    detail: err.to_string(),
                }
            })?,
        );
        self.remember_lens(id, &database);
        Ok(database)
    }

    /// Sorted, deduplicated maker list of one stored profile.
    pub fn lens_makers(&self, id: &str) -> Result<Vec<String>, ResourceError> {
        let database = self.lens_database(id)?;
        let mut makers: Vec<String> = database
            .lenses
            .iter()
            .map(|lens| lens.get_maker())
            .collect();
        makers.sort_unstable();
        makers.dedup();
        Ok(makers)
    }

    /// Reference `find_best_lens_match` against one stored profile.
    pub fn find_best_lens_match(
        &self,
        id: &str,
        maker: &str,
        model: &str,
    ) -> Result<Option<(String, String)>, ResourceError> {
        let database = self.lens_database(id)?;
        Ok(rapidraw_develop::lens::find_best_lens_match(
            &database, maker, model,
        ))
    }

    /// Reference `resolve_lens_params` against one stored profile.
    pub fn resolve_lens_params(
        &self,
        id: &str,
        maker: &str,
        model: &str,
        focal_length: f32,
        aperture: Option<f32>,
        distance: Option<f32>,
    ) -> Result<(LensDistortionParams, Vec<LensCapabilityNotice>), ResourceError> {
        let database = self.lens_database(id)?;
        rapidraw_develop::lens::resolve_lens_params(
            &database,
            maker,
            model,
            focal_length,
            aperture,
            distance,
        )
        .map_err(|err| ResourceError::UnsupportedLens {
            detail: err.to_string(),
        })
    }

    /// Selects a lens from a stored profile for a recipe: resolves the
    /// correction coefficients, writes the recipe's lens fields, and records
    /// the full provenance (maker/model/version/sha256) plus the
    /// content-addressed resource entry. `version` is the explicit label the
    /// importer declared for this profile (`unversioned` when none) — the
    /// engine never invents one. A failed selection leaves the recipe
    /// untouched.
    pub fn select_lens_profile(
        &self,
        envelope: &mut RecipeEnvelope,
        selection: &LensSelection,
    ) -> Result<(LensDistortionParams, Vec<LensCapabilityNotice>), ResourceError> {
        let (params, notices) = self.resolve_lens_params(
            selection.profile_id,
            selection.maker,
            selection.model,
            selection.focal_length,
            selection.aperture,
            selection.distance,
        )?;
        // Integrity was just verified by resolve_lens_params -> lens_database.
        let id = selection.profile_id;
        let digest = id
            .strip_prefix("lens/")
            .ok_or_else(|| ResourceError::InvalidId { id: id.to_string() })?;
        let size_bytes = fs::metadata(self.object_path(digest))
            .map(|meta| meta.len())
            .ok();
        envelope.recipe.lens_maker = Some(selection.maker.to_string());
        envelope.recipe.lens_model = Some(selection.model.to_string());
        envelope.recipe.lens_profile = Some(LensProfileRef {
            uri: format!("{RESOURCE_URI_SCHEME}{id}"),
            maker: selection.maker.to_string(),
            model: selection.model.to_string(),
            version: if selection.version.is_empty() {
                "unversioned".to_string()
            } else {
                selection.version.to_string()
            },
            sha256: digest.to_string(),
        });
        envelope
            .resources
            .entry(id.to_string())
            .or_insert(ResourceRef {
                algorithm: ResourceAlgorithm::Sha256,
                digest: digest.to_string(),
                size_bytes,
            });
        envelope.recipe.lens_distortion_params = Some(params);
        Ok((params, notices))
    }

    /// Resolves the recipe's lens-profile reference for render-time
    /// verification: the parsed database when a present, unchanged profile is
    /// referenced, `None` without a profile reference, and typed errors for
    /// missing/changed objects, unmapped references and digest disagreement.
    pub fn resolve_recipe_lens_profile(
        &self,
        envelope: &RecipeEnvelope,
    ) -> Result<Option<Arc<LensDatabase>>, ResourceError> {
        let Some(profile) = envelope.recipe.lens_profile.as_ref() else {
            return Ok(None);
        };
        let Some(id) = resource_uri_id(&profile.uri) else {
            return Err(ResourceError::UnresolvedReference {
                reference: profile.uri.clone(),
                detail: "only resource:// references are portable; legacy absolute paths must be re-imported into the resource store".to_string(),
            });
        };
        let entry = envelope.resources.get(id).ok_or_else(|| {
            ResourceError::UnresolvedReference {
                reference: profile.uri.clone(),
                detail: "the recipe references this lens profile but it is absent from the envelope resource map".to_string(),
            }
        })?;
        if entry.digest != profile.sha256 {
            return Err(ResourceError::UnresolvedReference {
                reference: profile.uri.clone(),
                detail: format!(
                    "the recipe's profile sha256 '{}' disagrees with the envelope resource entry digest '{}'",
                    profile.sha256, entry.digest
                ),
            });
        }
        self.lens_database(id).map(Some)
    }

    fn cached_lens(&self, id: &str) -> Option<Arc<LensDatabase>> {
        let mut cache = self
            .lens_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let index = cache.iter().position(|(candidate, _)| candidate == id)?;
        let entry = cache.remove(index)?;
        cache.push_back(entry.clone());
        Some(entry.1)
    }

    fn remember_lens(&self, id: &str, database: &Arc<LensDatabase>) {
        let mut cache = self
            .lens_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cache.iter().any(|(candidate, _)| candidate == id) {
            return;
        }
        while cache.len() >= self.limits.max_cached_luts.max(1) {
            cache.pop_front();
        }
        cache.push_back((id.to_string(), Arc::clone(database)));
    }
}

/// Attaches a selected lens profile to an envelope: the recipe gains the
/// portable provenance reference and identity fields, and the envelope's
/// resource map gains the content-addressed entry. Renders of this recipe
/// verify the profile object before any pixel work; missing/changed objects
/// fail explicitly instead of silently changing the export.
pub fn attach_lens_profile(
    envelope: &mut RecipeEnvelope,
    imported: &ImportedLensProfile,
    maker: &str,
    model: &str,
) {
    envelope.recipe.lens_maker = Some(maker.to_string());
    envelope.recipe.lens_model = Some(model.to_string());
    envelope.recipe.lens_profile = Some(LensProfileRef {
        uri: format!("{RESOURCE_URI_SCHEME}{}", imported.id),
        maker: maker.to_string(),
        model: model.to_string(),
        version: imported.version_label().to_string(),
        sha256: imported.digest.clone(),
    });
    envelope
        .resources
        .entry(imported.id.clone())
        .or_insert_with(|| imported.resource());
}

/// Detaches a lens profile: clears the recipe's provenance, lens selection
/// and resolved coefficients, and removes the resource map entry. LUT and
/// other resources are untouched.
pub fn detach_lens_profile(envelope: &mut RecipeEnvelope, id: &str) {
    envelope.resources.remove(id);
    let references_removed = envelope
        .recipe
        .lens_profile
        .as_ref()
        .and_then(|profile| resource_uri_id(&profile.uri))
        .is_some_and(|referenced| referenced == id);
    if references_removed {
        envelope.recipe.lens_profile = None;
        envelope.recipe.lens_maker = None;
        envelope.recipe.lens_model = None;
        envelope.recipe.lens_distortion_params = None;
    }
}

// ---------------------------------------------------------------------------
// Envelope helpers
// ---------------------------------------------------------------------------

/// Attaches an imported LUT to an envelope: the recipe references the
/// resource by portable URI and the envelope's resource map gains the
/// content-addressed entry. Renders of this recipe require the resolved
/// resource; missing/changed objects fail explicitly instead of rendering
/// un-LUT-ed pixels.
pub fn attach_lut(envelope: &mut RecipeEnvelope, imported: &ImportedLut) {
    envelope.recipe.lut_path = Some(lut_resource_uri(&imported.id));
    envelope.recipe.lut_name = imported.name.clone();
    envelope.recipe.lut_size = imported.cube_size;
    envelope
        .resources
        .insert(imported.id.clone(), imported.resource());
}

/// Detaches a LUT resource: removes the map entry and clears the recipe's
/// LUT reference when it points at this resource.
pub fn detach_lut(envelope: &mut RecipeEnvelope, id: &str) {
    envelope.resources.remove(id);
    let references_removed = envelope
        .recipe
        .lut_path
        .as_deref()
        .and_then(resource_uri_id)
        .is_some_and(|referenced| referenced == id);
    if references_removed {
        envelope.recipe.lut_path = None;
        envelope.recipe.lut_name = None;
        envelope.recipe.lut_size = 0;
    }
}

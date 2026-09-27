//! Developed-derivative cache identities, commit invalidation and viewer
//! notifications (lap-a58 / TASK-305; governing contract
//! `docs/raw-development/spec.md`).
//!
//! Responsibilities:
//!
//! - [`DerivativeIdentity`]: the recipe-sensitive cache key for every
//!   developed derivative (thumbnails, previews, exports). The key covers the
//!   source fingerprint, the recipe content hash, the referenced resource
//!   hashes, the engine revision, the color/model version, the rendered
//!   dimensions and the quality tier — a change to any of them yields a
//!   different key, so stale derivatives can never be served under a new
//!   identity.
//! - [`DevelopCommitBus`]: acknowledged-commit registry. `acknowledge` records
//!   the durable stamp (revision + content hash + source fingerprint) for an
//!   asset, bumps the per-asset generation and notifies subscribers so viewers
//!   and the thumbnail layer can invalidate affected derivatives.
//! - [`DevelopedDerivativeStore`]: bounded, content-addressed on-disk cache of
//!   developed derivative blobs. Contributions (`settle`) are only accepted
//!   when the job's stamp still matches the current acknowledged stamp (a
//!   delayed render from an older commit is rejected, spec A4), the source
//!   must still exist (missing media is an explicit error), and associations
//!   can be reconstructed after a catalog rebuild.
//! - [`RenderGate`]: shared bounded permits for developed-derivative render
//!   jobs so thumbnail/indexing and interactive/export work respect one
//!   resource limit.

/// Version tag mixed into every derivative key. Bump when the key layout
/// changes so old cache entries can never collide with new ones.
pub const DERIVATIVE_KEY_VERSION: &[u8] = b"lap-develop-derivative-v1";

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rapidraw_edit_model::{RecipeEnvelope, ResourceRef};
use serde::Serialize;

use super::sessions::ENGINE_GIT_REVISION;

// ---------------------------------------------------------------------------
// Quality tiers and derivative identity
// ---------------------------------------------------------------------------

/// Output tier of a developed derivative. Part of the cache identity: a
/// thumbnail-tier render is never served as an export and vice versa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QualityTier {
    /// Catalog/grid thumbnail (smallest bounded edge).
    Thumbnail,
    /// Fast interactive preview while dragging controls.
    Interactive,
    /// Settled preview after the last edit.
    Settled,
    /// Full-resolution durable derivative export.
    Export,
}

impl QualityTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            QualityTier::Thumbnail => "thumbnail",
            QualityTier::Interactive => "interactive",
            QualityTier::Settled => "settled",
            QualityTier::Export => "export",
        }
    }
}

/// Hash over the recipe envelope's referenced external resources (LUTs,
/// depth maps, mask bitmaps). Recipes reference resources by id; the sorted
/// (id, algorithm, digest, size) tuples make the hash order-stable.
pub fn resource_hash(resources: &BTreeMap<String, ResourceRef>) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"lap-develop-resources-v1");
    for (id, resource) in resources {
        hasher.update(id.as_bytes());
        hasher.update(b"\0");
        hasher.update(format!("{:?}", resource.algorithm).as_bytes());
        hasher.update(resource.digest.as_bytes());
        hasher.update(&resource.size_bytes.unwrap_or(0).to_le_bytes());
        hasher.update(b"\0");
    }
    hasher.finalize().to_hex().to_string()
}

/// The full identity of one developed derivative. Every field participates in
/// [`DerivativeIdentity::key`]; changing any of them changes the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DerivativeIdentity {
    /// SHA-256 of the untouched source bytes.
    pub source_fingerprint: String,
    /// Content hash of the committed recipe envelope (covers the recipe,
    /// decode settings and preserved unsupported payloads).
    pub recipe_hash: String,
    /// Hash over the envelope's referenced external resources.
    pub resource_hash: String,
    /// Pinned engine revision that renders the derivative.
    pub engine_version: String,
    /// Color/edit-model version of the recipe schema.
    pub color_version: String,
    /// Rendered pixel dimensions.
    pub width: u32,
    pub height: u32,
    /// Output tier the derivative was rendered for.
    pub quality_tier: QualityTier,
}

impl DerivativeIdentity {
    /// Builds the identity from a committed envelope and the render result
    /// shape. Fails when the envelope cannot be hashed.
    pub fn from_envelope(
        envelope: &RecipeEnvelope,
        width: u32,
        height: u32,
        quality_tier: QualityTier,
    ) -> Result<Self, String> {
        let recipe_hash = envelope
            .content_hash()
            .map_err(|err| format!("recipe content hash failed: {err}"))?;
        Ok(Self {
            source_fingerprint: envelope.source_fingerprint.clone(),
            recipe_hash,
            resource_hash: resource_hash(&envelope.resources),
            engine_version: ENGINE_GIT_REVISION.to_string(),
            color_version: rapidraw_edit_model::MODEL_VERSION.to_string(),
            width,
            height,
            quality_tier,
        })
    }

    /// Stable content-addressed cache key (hex). The layout version is mixed
    /// in so key layouts cannot collide across upgrades.
    pub fn key(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(DERIVATIVE_KEY_VERSION);
        hasher.update(b"\0source\0");
        hasher.update(self.source_fingerprint.as_bytes());
        hasher.update(b"\0recipe\0");
        hasher.update(self.recipe_hash.as_bytes());
        hasher.update(b"\0resources\0");
        hasher.update(self.resource_hash.as_bytes());
        hasher.update(b"\0engine\0");
        hasher.update(self.engine_version.as_bytes());
        hasher.update(b"\0color\0");
        hasher.update(self.color_version.as_bytes());
        hasher.update(b"\0dimensions\0");
        hasher.update(&self.width.to_le_bytes());
        hasher.update(&self.height.to_le_bytes());
        hasher.update(b"\0tier\0");
        hasher.update(self.quality_tier.as_str().as_bytes());
        hasher.finalize().to_hex().to_string()
    }
}

// ---------------------------------------------------------------------------
// Acknowledged-commit stamps and notifications
// ---------------------------------------------------------------------------

/// Durable identity of one acknowledged commit for an asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DerivativeStamp {
    pub revision: u64,
    pub content_hash: String,
    pub source_fingerprint: String,
}

/// Notification delivered to subscribers when a commit is acknowledged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitNotice {
    pub asset_id: String,
    pub stamp: DerivativeStamp,
    /// Strictly increasing per asset; receivers drop notices older than the
    /// newest one they observed.
    pub generation: u64,
}

#[derive(Default)]
struct BusState {
    stamps: HashMap<String, (DerivativeStamp, u64)>,
    subscribers: Vec<std::sync::mpsc::Sender<CommitNotice>>,
}

/// Registry of acknowledged develop commits with viewer notifications.
///
/// `acknowledge` is called by the Tauri commit command after the durable
/// sidecar write succeeded; subscribers (thumbnail invalidation, viewer
/// refresh jobs) receive a [`CommitNotice`] so affected derivatives are
/// invalidated and viewers repaint.
#[derive(Default)]
pub struct DevelopCommitBus {
    state: Mutex<BusState>,
}

impl DevelopCommitBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the acknowledged stamp for the asset, bumps its generation and
    /// notifies every subscriber. Dead subscribers are dropped.
    pub fn acknowledge(&self, asset_id: &str, stamp: DerivativeStamp) -> CommitNotice {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let generation = state
            .stamps
            .get(asset_id)
            .map(|(_, generation)| generation + 1)
            .unwrap_or(1);
        state
            .stamps
            .insert(asset_id.to_string(), (stamp.clone(), generation));
        let notice = CommitNotice {
            asset_id: asset_id.to_string(),
            stamp,
            generation,
        };
        state
            .subscribers
            .retain(|sender| sender.send(notice.clone()).is_ok());
        notice
    }

    /// The current acknowledged stamp for the asset, if any.
    pub fn stamp(&self, asset_id: &str) -> Option<DerivativeStamp> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stamps
            .get(asset_id)
            .map(|(stamp, _)| stamp.clone())
    }

    /// Seeds the stamp for an asset without notifying subscribers or bumping
    /// generations. Used when reloading persisted associations after a
    /// restart: the persisted stamp stays authoritative until a real commit
    /// is acknowledged for that asset.
    pub fn seed_if_absent(&self, asset_id: &str, stamp: DerivativeStamp) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .stamps
            .entry(asset_id.to_string())
            .or_insert((stamp, 0));
    }

    /// The current per-asset generation, if the asset ever committed.
    pub fn generation(&self, asset_id: &str) -> Option<u64> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stamps
            .get(asset_id)
            .map(|(_, generation)| *generation)
    }

    /// Subscribes to commit notices; the returned receiver gets one message
    /// per acknowledged commit.
    pub fn subscribe(&self) -> std::sync::mpsc::Receiver<CommitNotice> {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .subscribers
            .push(sender);
        receiver
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Typed derivative-cache failure. Every variant is explicit and surfaced to
/// the caller; nothing degrades into a stale or fake contribution.
#[derive(Debug)]
pub enum DerivativeCacheError {
    /// The source media vanished before the derivative could be settled.
    MissingMedia {
        path: PathBuf,
    },
    /// A delayed job tried to contribute under an outdated commit stamp; the
    /// newer acknowledged state is retained (spec A4).
    StaleContribution {
        asset_id: String,
        expected: Option<DerivativeStamp>,
        received: DerivativeStamp,
    },
    /// The shared render-permit pool is exhausted; retry later.
    Busy {
        limit: usize,
    },
    Io {
        context: String,
        source: std::io::Error,
    },
    InvalidInput(String),
}

impl std::fmt::Display for DerivativeCacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DerivativeCacheError::MissingMedia { path } => {
                write!(f, "missing media: {} does not exist", path.display())
            }
            DerivativeCacheError::StaleContribution {
                asset_id,
                expected,
                received,
            } => write!(
                f,
                "stale derivative contribution for asset {asset_id}: job was rendered for {received:?} but the acknowledged commit is {expected:?}"
            ),
            DerivativeCacheError::Busy { limit } => {
                write!(f, "derivative render gate busy: {limit} permits in use")
            }
            DerivativeCacheError::Io { context, source } => {
                write!(f, "{context}: {source}")
            }
            DerivativeCacheError::InvalidInput(message) => {
                write!(f, "invalid derivative request: {message}")
            }
        }
    }
}

impl std::error::Error for DerivativeCacheError {}

// ---------------------------------------------------------------------------
// Shared render gate
// ---------------------------------------------------------------------------

struct GateState {
    active: usize,
}

/// Bounded pool of render permits shared by developed-derivative refresh jobs
/// (thumbnails and viewer renders), so background derivative work can never
/// starve interactive/export jobs or pile up unbounded decodes.
pub struct RenderGate {
    limit: usize,
    state: Mutex<GateState>,
}

impl RenderGate {
    pub fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            state: Mutex::new(GateState { active: 0 }),
        }
    }

    /// Acquires one permit, or `None` when the pool is exhausted (explicit
    /// `Busy`, never an unbounded queue).
    pub fn try_acquire(self: &Arc<Self>) -> Option<RenderPermit> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.active >= self.limit {
            return None;
        }
        state.active += 1;
        Some(RenderPermit {
            gate: Arc::clone(self),
        })
    }

    fn release(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.active = state.active.saturating_sub(1);
    }

    pub fn active(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .active
    }
}

/// RAII permit: releases the gate slot when dropped (including on error paths).
pub struct RenderPermit {
    gate: Arc<RenderGate>,
}

impl Drop for RenderPermit {
    fn drop(&mut self) {
        self.gate.release();
    }
}

// ---------------------------------------------------------------------------
// Bounded developed-derivative store
// ---------------------------------------------------------------------------

/// A contribution to the store from a finished render job.
pub struct SettleJob {
    pub asset_id: String,
    /// The commit stamp the job rendered for; must still be current.
    pub stamp: DerivativeStamp,
    pub identity: DerivativeIdentity,
    /// The untouched source path; must still exist (missing media is
    /// explicit, never a silent keep-stale).
    pub source_path: PathBuf,
    /// Encoded derivative bytes (JPEG/PNG as produced by the job).
    pub bytes: Vec<u8>,
}

/// Reconstructed association of one asset to its developed derivative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAssociation {
    pub asset_id: String,
    pub stamp: DerivativeStamp,
    pub identity: DerivativeIdentity,
    pub identity_key: String,
    /// Whether the derivative blob exists on disk for this association.
    pub has_blob: bool,
    pub updated_at_ms: i64,
}

/// Summary of a catalog rebuild over the derivative associations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RebuildSummary {
    /// Associations whose stamp still matches the durable recipe.
    pub confirmed: usize,
    /// Associations dropped because the durable recipe moved on (or vanished).
    pub dropped_stale: usize,
    /// Durable commits with no local association yet (pending render).
    pub adopted: usize,
}

#[derive(Default)]
struct StoreState {
    associations: HashMap<String, StoredAssociation>,
}

/// Content-addressed on-disk cache of developed derivative blobs.
///
/// Layout: `<root>/index.json` holds the asset associations;
/// `<root>/blobs/<key[0..2]>/<key>.bin` holds the blobs. Writes are atomic
/// (temp file + rename). The index is a rebuildable cache: a missing or
/// corrupt index starts empty and is repaired by [`Self::rebuild_associations`]
/// or the next settled render; source sidecars (not this cache) stay the only
/// recipe authority.
pub struct DevelopedDerivativeStore {
    root: PathBuf,
    bus: Arc<DevelopCommitBus>,
    state: Mutex<StoreState>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

impl DevelopedDerivativeStore {
    /// Opens (or initializes) the store under `root`, wiring it to the commit
    /// bus so settle-time staleness checks use the authoritative stamps.
    #[allow(clippy::result_large_err)]
    pub fn open(root: PathBuf, bus: Arc<DevelopCommitBus>) -> Result<Self, DerivativeCacheError> {
        std::fs::create_dir_all(root.join("blobs")).map_err(|source| DerivativeCacheError::Io {
            context: "creating developed-derivative cache directory".to_string(),
            source,
        })?;
        let store = Self {
            root,
            bus,
            state: Mutex::new(StoreState::default()),
        };
        store.load_index();
        Ok(store)
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.json")
    }

    fn blob_path(&self, identity_key: &str) -> PathBuf {
        self.root
            .join("blobs")
            .join(&identity_key[0..2.min(identity_key.len())])
            .join(format!("{identity_key}.bin"))
    }

    /// Tolerant index load: a missing or corrupt index starts empty. The
    /// durable sidecar remains the only recipe authority; this cache is
    /// always rebuildable. Persisted associations seed the commit bus so
    /// lookups keep working after a restart (a later real commit always
    /// supersedes a seeded stamp).
    fn load_index(&self) {
        let Ok(text) = std::fs::read_to_string(self.index_path()) else {
            return;
        };
        let Ok(associations) = serde_json::from_str::<Vec<StoredAssociation>>(&text) else {
            return;
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.associations = associations
            .into_iter()
            .filter(|association| !association.asset_id.is_empty())
            .map(|association| {
                self.bus
                    .seed_if_absent(&association.asset_id, association.stamp.clone());
                (association.asset_id.clone(), association)
            })
            .collect();
    }

    #[allow(clippy::result_large_err)]
    fn persist_index(&self, state: &StoreState) -> Result<(), DerivativeCacheError> {
        let mut values: Vec<&StoredAssociation> = state.associations.values().collect();
        values.sort_by(|a, b| a.asset_id.cmp(&b.asset_id));
        let text = serde_json::to_string(&values)
            .map_err(|err| DerivativeCacheError::InvalidInput(format!("index serialize: {err}")))?;
        let temp = self.index_path().with_extension(format!(
            "json.{}.{}.tmp",
            std::process::id(),
            now_ms()
        ));
        let write = (|| -> std::io::Result<()> {
            std::fs::write(&temp, text.as_bytes())?;
            std::fs::rename(&temp, self.index_path())
        })();
        match write {
            Ok(()) => Ok(()),
            Err(source) => {
                let _ = std::fs::remove_file(&temp);
                Err(DerivativeCacheError::Io {
                    context: "persisting developed-derivative index".to_string(),
                    source,
                })
            }
        }
    }

    /// Settles a finished render job into the store.
    ///
    /// Gates, in order:
    /// 1. the source must still exist ([`DerivativeCacheError::MissingMedia`]),
    /// 2. the job's stamp must equal the current acknowledged stamp for the
    ///    asset ([`DerivativeCacheError::StaleContribution`]) — a delayed job
    ///    from an older commit can never overwrite the newer state.
    ///
    /// On success the blob is written content-addressed by the identity key
    /// and the association is persisted atomically.
    #[allow(clippy::result_large_err)]
    pub fn settle(&self, job: SettleJob) -> Result<StoredAssociation, DerivativeCacheError> {
        if job.asset_id.is_empty() || job.bytes.is_empty() {
            return Err(DerivativeCacheError::InvalidInput(
                "settle requires an asset id and non-empty bytes".to_string(),
            ));
        }
        if !Path::new(&job.source_path).exists() {
            return Err(DerivativeCacheError::MissingMedia {
                path: job.source_path,
            });
        }
        let current = self.bus.stamp(&job.asset_id);
        if current.as_ref() != Some(&job.stamp) {
            return Err(DerivativeCacheError::StaleContribution {
                asset_id: job.asset_id,
                expected: current,
                received: job.stamp,
            });
        }

        let identity_key = job.identity.key();
        let blob = self.blob_path(&identity_key);
        if let Some(parent) = blob.parent() {
            std::fs::create_dir_all(parent).map_err(|source| DerivativeCacheError::Io {
                context: "creating blob directory".to_string(),
                source,
            })?;
        }
        let temp = blob.with_extension(format!("bin.{}.{}.tmp", std::process::id(), now_ms()));
        let write = (|| -> std::io::Result<()> {
            std::fs::write(&temp, &job.bytes)?;
            std::fs::rename(&temp, &blob)
        })();
        if let Err(source) = write {
            let _ = std::fs::remove_file(&temp);
            return Err(DerivativeCacheError::Io {
                context: format!("writing derivative blob {}", blob.display()),
                source,
            });
        }

        let association = StoredAssociation {
            asset_id: job.asset_id,
            stamp: job.stamp,
            identity: job.identity,
            identity_key,
            has_blob: true,
            updated_at_ms: now_ms(),
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .associations
            .insert(association.asset_id.clone(), association.clone());
        self.persist_index(&state)?;
        Ok(association)
    }

    /// Returns the stored derivative bytes for the asset, but only when the
    /// requested stamp is still the asset's current acknowledged stamp, the
    /// association matches it, and the blob exists. Any mismatch yields
    /// `None` (the caller falls back to the undeveloped path and re-renders),
    /// never stale bytes.
    pub fn lookup_bytes(&self, asset_id: &str, expected: &DerivativeStamp) -> Option<Vec<u8>> {
        if self.bus.stamp(asset_id).as_ref() != Some(expected) {
            return None;
        }
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let association = state.associations.get(asset_id)?;
        if &association.stamp != expected || !association.has_blob {
            return None;
        }
        std::fs::read(self.blob_path(&association.identity_key)).ok()
    }

    /// The current stored association for the asset, if any.
    pub fn association(&self, asset_id: &str) -> Option<StoredAssociation> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .associations
            .get(asset_id)
            .cloned()
    }

    /// Removes the asset's association (and its blob when no other asset
    /// references the same identity). Returns true when an association was
    /// removed.
    #[allow(clippy::result_large_err)]
    pub fn invalidate_asset(&self, asset_id: &str) -> Result<bool, DerivativeCacheError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(removed) = state.associations.remove(asset_id) else {
            return Ok(false);
        };
        let key_still_referenced = state
            .associations
            .values()
            .any(|association| association.identity_key == removed.identity_key);
        if !key_still_referenced {
            let _ = std::fs::remove_file(self.blob_path(&removed.identity_key));
        }
        self.persist_index(&state)?;
        Ok(true)
    }

    /// Reconstructs associations after a catalog rebuild/rescan: `durable`
    /// maps asset ids to the current durable commit stamps (resolved from the
    /// recipe sidecars/projection). Associations matching the durable stamp
    /// are confirmed; stale ones are dropped (with their blobs when
    /// unreferenced); durable commits without a local association are adopted
    /// as pending (no blob yet — the next render fills them).
    #[allow(clippy::result_large_err)]
    pub fn rebuild_associations(
        &self,
        durable: &HashMap<String, DerivativeStamp>,
    ) -> Result<RebuildSummary, DerivativeCacheError> {
        let mut summary = RebuildSummary::default();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let stale_keys: Vec<String> = state
            .associations
            .values()
            .filter(|association| {
                durable
                    .get(&association.asset_id)
                    .map(|durable_stamp| durable_stamp != &association.stamp)
                    .unwrap_or(true)
            })
            .map(|association| association.identity_key.clone())
            .collect();
        state.associations.retain(|asset_id, association| {
            let keep = matches!(durable.get(asset_id), Some(durable_stamp) if durable_stamp == &association.stamp);
            if keep {
                summary.confirmed += 1;
            } else {
                summary.dropped_stale += 1;
            }
            keep
        });
        // Remove blobs that no surviving association references.
        for key in stale_keys {
            let still_referenced = state
                .associations
                .values()
                .any(|association| association.identity_key == key);
            if !still_referenced {
                let _ = std::fs::remove_file(self.blob_path(&key));
            }
        }

        for (asset_id, stamp) in durable {
            if state.associations.contains_key(asset_id) {
                continue;
            }
            summary.adopted += 1;
            // Pending associations carry no identity/blob yet; they are
            // recorded so lookups can report "developed, pending render"
            // instead of re-deriving the whole identity. We synthesize the
            // stored entry from the stamp alone by marking has_blob=false and
            // reusing the stamp as a placeholder identity marker through the
            // dedicated pending path below.
            state.associations.insert(
                asset_id.clone(),
                StoredAssociation {
                    asset_id: asset_id.clone(),
                    stamp: stamp.clone(),
                    identity: pending_identity(stamp),
                    identity_key: String::new(),
                    has_blob: false,
                    updated_at_ms: now_ms(),
                },
            );
        }

        self.persist_index(&state)?;
        Ok(summary)
    }
}

/// Placeholder identity for adopted (pending) associations. It is never used
/// as a cache key (identity_key stays empty) and never matches a settled
/// render's identity because its hashes are the stamp's placeholders.
fn pending_identity(stamp: &DerivativeStamp) -> DerivativeIdentity {
    DerivativeIdentity {
        source_fingerprint: stamp.source_fingerprint.clone(),
        recipe_hash: stamp.content_hash.clone(),
        resource_hash: String::new(),
        engine_version: String::new(),
        color_version: String::new(),
        width: 0,
        height: 0,
        quality_tier: QualityTier::Thumbnail,
    }
}

// ---------------------------------------------------------------------------
// Tests (behavior regressions; RED before the implementation lands, GREEN
// after)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rapidraw_edit_model::RecipeEnvelope;

    fn tmp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "lap_develop_cache_{}_{}_{}",
            tag,
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn envelope(asset: &str, fingerprint: &str) -> RecipeEnvelope {
        let mut envelope = RecipeEnvelope::new("lap-test/cache", asset, "default", fingerprint);
        envelope.revision = 1;
        envelope
    }

    fn stamp(revision: u64, content: &str, fingerprint: &str) -> DerivativeStamp {
        DerivativeStamp {
            revision,
            content_hash: content.to_string(),
            source_fingerprint: fingerprint.to_string(),
        }
    }

    fn identity(envelope: &RecipeEnvelope, width: u32, height: u32) -> DerivativeIdentity {
        DerivativeIdentity::from_envelope(envelope, width, height, QualityTier::Thumbnail)
            .expect("identity from envelope")
    }

    // ------------------------------------------------------------- identity

    #[test]
    fn identity_key_is_deterministic_for_identical_input() {
        let env = envelope("a", &"f".repeat(64));
        let a = identity(&env, 100, 80).key();
        let b = identity(&env, 100, 80).key();
        assert_eq!(a, b, "identical identities must produce identical keys");
        assert_eq!(a.len(), 64, "keys are lowercase hex blake3 digests");
    }

    #[test]
    fn identity_key_changes_with_every_identity_component() {
        let base_env = envelope("a", &"f".repeat(64));
        let base = identity(&base_env, 100, 80);

        // Source fingerprint.
        let mut env = base_env.clone();
        env.source_fingerprint = "a".repeat(64);
        let changed = identity(&env, 100, 80);
        assert_ne!(base.key(), changed.key(), "source fingerprint participates");

        // Recipe hash (a recipe edit changes the content hash).
        let recipe_hash_changed = DerivativeIdentity {
            recipe_hash: format!("{}x", &base.recipe_hash[..63]),
            ..base.clone()
        };
        assert_ne!(
            base.key(),
            recipe_hash_changed.key(),
            "recipe hash participates"
        );

        // Resource hash.
        let resource_changed = DerivativeIdentity {
            resource_hash: "different".to_string(),
            ..base.clone()
        };
        assert_ne!(
            base.key(),
            resource_changed.key(),
            "resource hash participates"
        );

        // Engine version.
        let engine_changed = DerivativeIdentity {
            engine_version: "other-engine-revision".to_string(),
            ..base.clone()
        };
        assert_ne!(
            base.key(),
            engine_changed.key(),
            "engine version participates"
        );

        // Color/model version.
        let color_changed = DerivativeIdentity {
            color_version: "0.0.0".to_string(),
            ..base.clone()
        };
        assert_ne!(
            base.key(),
            color_changed.key(),
            "color version participates"
        );

        // Dimensions.
        let width_changed = identity(&base_env, 101, 80);
        let height_changed = identity(&base_env, 100, 81);
        assert_ne!(base.key(), width_changed.key(), "width participates");
        assert_ne!(base.key(), height_changed.key(), "height participates");
        assert_ne!(
            width_changed.key(),
            height_changed.key(),
            "width and height are not interchangeable"
        );

        // Quality tier.
        let tier_changed =
            DerivativeIdentity::from_envelope(&base_env, 100, 80, QualityTier::Settled).unwrap();
        assert_ne!(base.key(), tier_changed.key(), "quality tier participates");
    }

    #[test]
    fn resource_hash_tracks_referenced_resources() {
        let empty = BTreeMap::new();
        let mut with_lut = BTreeMap::new();
        with_lut.insert(
            "lut/a".to_string(),
            ResourceRef {
                algorithm: rapidraw_edit_model::ResourceAlgorithm::Sha256,
                digest: "d".repeat(64),
                size_bytes: Some(1024),
            },
        );

        assert_ne!(
            resource_hash(&empty),
            resource_hash(&with_lut),
            "adding a resource changes the resource hash"
        );
        let before = resource_hash(&with_lut);
        with_lut.get_mut("lut/a").unwrap().digest = "e".repeat(64);
        assert_ne!(
            before,
            resource_hash(&with_lut),
            "a changed resource digest changes the resource hash"
        );
    }

    // ------------------------------------------------------------- bus

    #[test]
    fn bus_acknowledge_publishes_notices_and_bumps_generations() {
        let bus = DevelopCommitBus::new();
        let rx = bus.subscribe();

        let first = bus.acknowledge("42", stamp(1, "hash-1", &"f".repeat(64)));
        assert_eq!(first.generation, 1);
        let second = bus.acknowledge("42", stamp(2, "hash-2", &"f".repeat(64)));
        assert_eq!(second.generation, 2, "generations strictly increase");

        let notice = rx.recv().expect("subscriber receives the first notice");
        assert_eq!(notice.asset_id, "42");
        assert_eq!(notice.stamp.revision, 1);
        let notice = rx.recv().expect("subscriber receives the second notice");
        assert_eq!(notice.stamp.revision, 2);
        assert_eq!(
            bus.stamp("42"),
            Some(stamp(2, "hash-2", &"f".repeat(64))),
            "the newest acknowledged stamp is authoritative"
        );
        assert_eq!(bus.stamp("99"), None, "unknown assets have no stamp");
    }

    // ------------------------------------------------------------- gate

    #[test]
    fn render_gate_bounds_concurrent_permits_and_releases_on_drop() {
        let gate = Arc::new(RenderGate::new(2));
        let permit_a = gate.try_acquire().expect("first permit");
        let permit_b = gate.try_acquire().expect("second permit");
        assert!(
            gate.try_acquire().is_none(),
            "an exhausted gate must refuse additional jobs (Busy, never unbounded)"
        );
        drop(permit_a);
        assert_eq!(gate.active(), 1, "dropping a permit releases its slot");
        let permit_c = gate.try_acquire().expect("slot freed by drop");
        drop(permit_b);
        drop(permit_c);
        assert_eq!(gate.active(), 0);
    }

    // ------------------------------------------------------------- store

    fn write_source(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn settle_job(
        asset: &str,
        current_stamp: &DerivativeStamp,
        source: &Path,
        env: &RecipeEnvelope,
    ) -> SettleJob {
        SettleJob {
            asset_id: asset.to_string(),
            stamp: current_stamp.clone(),
            identity: identity(env, 320, 200),
            source_path: source.to_path_buf(),
            bytes: vec![1, 2, 3, 4],
        }
    }

    #[test]
    fn settle_accepts_a_current_stamp_persists_and_reloads() {
        let dir = tmp_dir("settle_ok");
        let source = write_source(&dir, "photo.dng", b"raw-bytes");
        let bus = Arc::new(DevelopCommitBus::new());
        let env = envelope("42", &"f".repeat(64));
        bus.acknowledge("42", stamp(1, "hash-1", &"f".repeat(64)));

        let store = DevelopedDerivativeStore::open(dir.join("cache"), Arc::clone(&bus))
            .expect("store opens");
        let association = store
            .settle(settle_job("42", &bus.stamp("42").unwrap(), &source, &env))
            .expect("a current contribution is accepted");
        assert!(association.has_blob);
        assert_eq!(
            store.lookup_bytes("42", &stamp(1, "hash-1", &"f".repeat(64))),
            Some(vec![1, 2, 3, 4]),
            "the settled bytes are served for the matching stamp"
        );

        // A fresh store instance rebuilds its index from disk.
        let reopened = DevelopedDerivativeStore::open(dir.join("cache"), Arc::clone(&bus))
            .expect("store reopens");
        assert_eq!(
            reopened.lookup_bytes("42", &stamp(1, "hash-1", &"f".repeat(64))),
            Some(vec![1, 2, 3, 4]),
            "associations survive a reload (index persisted)"
        );
    }

    #[test]
    fn settle_rejects_a_delayed_job_rendered_for_an_older_commit() {
        let dir = tmp_dir("settle_stale");
        let source = write_source(&dir, "photo.dng", b"raw-bytes");
        let bus = Arc::new(DevelopCommitBus::new());
        let env = envelope("42", &"f".repeat(64));
        bus.acknowledge("42", stamp(1, "hash-1", &"f".repeat(64)));

        let store = DevelopedDerivativeStore::open(dir.join("cache"), Arc::clone(&bus))
            .expect("store opens");
        // The job started for revision 1; a newer commit is acknowledged
        // before the job finishes.
        let job = settle_job("42", &stamp(1, "hash-1", &"f".repeat(64)), &source, &env);
        bus.acknowledge("42", stamp(2, "hash-2", &"f".repeat(64)));

        let err = store
            .settle(job)
            .expect_err("a delayed stale job must never overwrite the newer state");
        assert!(
            matches!(err, DerivativeCacheError::StaleContribution { ref asset_id, .. } if asset_id == "42"),
            "expected StaleContribution, got {err}"
        );
        assert_eq!(
            store.lookup_bytes("42", &stamp(1, "hash-1", &"f".repeat(64))),
            None,
            "the stale contribution is not served"
        );
        assert_eq!(
            store.lookup_bytes("42", &stamp(2, "hash-2", &"f".repeat(64))),
            None,
            "the newer stamp has no derivative yet"
        );
        assert!(
            store.association("42").is_none(),
            "no association is recorded for a rejected job"
        );
    }

    #[test]
    fn lookup_requires_the_current_stamp() {
        let dir = tmp_dir("lookup_stamp");
        let source = write_source(&dir, "photo.dng", b"raw-bytes");
        let bus = Arc::new(DevelopCommitBus::new());
        let env = envelope("42", &"f".repeat(64));
        bus.acknowledge("42", stamp(1, "hash-1", &"f".repeat(64)));
        let store = DevelopedDerivativeStore::open(dir.join("cache"), Arc::clone(&bus))
            .expect("store opens");
        store
            .settle(settle_job("42", &bus.stamp("42").unwrap(), &source, &env))
            .expect("settled");

        bus.acknowledge("42", stamp(2, "hash-2", &"f".repeat(64)));
        assert_eq!(
            store.lookup_bytes("42", &stamp(1, "hash-1", &"f".repeat(64))),
            None,
            "after a new commit the old derivative is never served, even though the blob exists"
        );
        assert_eq!(
            store.lookup_bytes("42", &stamp(2, "hash-2", &"f".repeat(64))),
            None,
            "the new stamp has no derivative until a fresh render settles"
        );
    }

    #[test]
    fn settle_reports_missing_media_explicitly() {
        let dir = tmp_dir("settle_missing");
        let source = write_source(&dir, "photo.dng", b"raw-bytes");
        let bus = Arc::new(DevelopCommitBus::new());
        let env = envelope("42", &"f".repeat(64));
        bus.acknowledge("42", stamp(1, "hash-1", &"f".repeat(64)));
        let store = DevelopedDerivativeStore::open(dir.join("cache"), Arc::clone(&bus))
            .expect("store opens");

        std::fs::remove_file(&source).expect("source removed mid-flight");
        let err = store
            .settle(settle_job("42", &bus.stamp("42").unwrap(), &source, &env))
            .expect_err("a vanished source must be an explicit missing-media error");
        assert!(
            matches!(err, DerivativeCacheError::MissingMedia { .. } if matches!(&err, DerivativeCacheError::MissingMedia { path } if path.as_path() == source.as_path())),
            "expected MissingMedia for the exact path, got {err}"
        );
        assert!(store.association("42").is_none());
    }

    #[test]
    fn invalidate_asset_drops_association_and_blob() {
        let dir = tmp_dir("invalidate");
        let source = write_source(&dir, "photo.dng", b"raw-bytes");
        let bus = Arc::new(DevelopCommitBus::new());
        let env = envelope("42", &"f".repeat(64));
        bus.acknowledge("42", stamp(1, "hash-1", &"f".repeat(64)));
        let store = DevelopedDerivativeStore::open(dir.join("cache"), Arc::clone(&bus))
            .expect("store opens");
        let association = store
            .settle(settle_job("42", &bus.stamp("42").unwrap(), &source, &env))
            .expect("settled");
        let blob_path = dir
            .join("cache")
            .join("blobs")
            .join(&association.identity_key[0..2])
            .join(format!("{}.bin", association.identity_key));
        assert!(blob_path.exists(), "blob written");

        assert_eq!(store.invalidate_asset("42").unwrap(), true);
        assert!(
            !blob_path.exists(),
            "the blob is removed with the association"
        );
        assert_eq!(
            store.invalidate_asset("42").unwrap(),
            false,
            "second invalidate is a no-op"
        );
    }

    #[test]
    fn rebuild_reconstructs_associations_from_durable_stamps() {
        let dir = tmp_dir("rebuild");
        let source_a = write_source(&dir, "a.dng", b"a-bytes");
        let source_b = write_source(&dir, "b.dng", b"b-bytes");
        let bus = Arc::new(DevelopCommitBus::new());
        let env_a = envelope("1", &"a".repeat(64));
        let env_b = envelope("2", &"b".repeat(64));
        bus.acknowledge("1", stamp(1, "hash-a", &"a".repeat(64)));
        bus.acknowledge("2", stamp(1, "hash-b", &"b".repeat(64)));
        let store = DevelopedDerivativeStore::open(dir.join("cache"), Arc::clone(&bus))
            .expect("store opens");
        store
            .settle(settle_job("1", &bus.stamp("1").unwrap(), &source_a, &env_a))
            .expect("asset 1 settled");
        store
            .settle(settle_job("2", &bus.stamp("2").unwrap(), &source_b, &env_b))
            .expect("asset 2 settled");

        // Catalog rebuild: asset 1 re-committed (revision 2), asset 2's sidecar
        // disappeared, asset 3 is newly known.
        let mut durable = HashMap::new();
        durable.insert("1".to_string(), stamp(2, "hash-a2", &"a".repeat(64)));
        durable.insert("3".to_string(), stamp(1, "hash-c", &"c".repeat(64)));

        let summary = store
            .rebuild_associations(&durable)
            .expect("rebuild succeeds");
        assert_eq!(
            summary.confirmed, 0,
            "asset 1's stored stamp no longer matches"
        );
        assert_eq!(
            summary.dropped_stale, 2,
            "stale asset 1 and vanished asset 2 are dropped"
        );
        // Both still-durable assets (1 re-committed, 3 newly known) are adopted
        // as pending; asset 2's sidecar vanished, so nothing is adopted for it.
        assert_eq!(
            summary.adopted, 2,
            "durable commits without a current blob are adopted as pending"
        );

        assert!(
            store.association("1").is_none()
                || store.association("1").unwrap().identity_key.is_empty(),
            "asset 1 is either pending or absent, never stale"
        );
        assert!(
            store.association("2").is_none(),
            "the vanished sidecar's association is gone"
        );
        let adopted = store.association("3").expect("asset 3 adopted");
        assert!(!adopted.has_blob, "adopted associations have no blob yet");

        // Blob cleanup: asset 2's blob is no longer served after the rebuild.
        assert_eq!(
            store.lookup_bytes("2", &stamp(1, "hash-b", &"b".repeat(64))),
            None
        );
    }

    #[test]
    fn rebuild_keeps_matching_associations_and_their_blobs() {
        let dir = tmp_dir("rebuild_keep");
        let source = write_source(&dir, "photo.dng", b"raw-bytes");
        let bus = Arc::new(DevelopCommitBus::new());
        let env = envelope("42", &"f".repeat(64));
        let current = stamp(1, "hash-1", &"f".repeat(64));
        bus.acknowledge("42", current.clone());
        let store = DevelopedDerivativeStore::open(dir.join("cache"), Arc::clone(&bus))
            .expect("store opens");
        store
            .settle(settle_job("42", &current, &source, &env))
            .expect("settled");

        let mut durable = HashMap::new();
        durable.insert("42".to_string(), current.clone());
        let summary = store
            .rebuild_associations(&durable)
            .expect("rebuild succeeds");
        assert_eq!(summary.confirmed, 1);
        assert_eq!(summary.dropped_stale, 0);
        assert_eq!(summary.adopted, 0);
        assert_eq!(
            store.lookup_bytes("42", &current),
            Some(vec![1, 2, 3, 4]),
            "a confirmed association keeps serving its blob"
        );
    }
}

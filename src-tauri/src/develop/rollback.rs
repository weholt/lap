//! Non-destructive develop-feature rollback switch (lap-63f / TASK-602,
//! managed continuation of lap-404.2).
//!
//! The switch is a recovery lever for the new Develop entry point: while it
//! is enabled, every mutating develop command rejects with an explicit
//! message and the frontend hides/disables the Develop panel entry. It is
//! deliberately NOT a data migration: sidecars, retained previous
//! revisions, resources and the catalog projection are all left untouched,
//! and recipes are never flattened into the original media. Disabling the
//! switch resumes editing on the same revision stream.
//!
//! The flag document is stored independently of `app-config.json` so config
//! corruption/recovery paths can never silently flip the switch. A missing
//! file means "develop enabled"; a corrupt or unsupported document is a
//! visible [`RollbackError::Corrupt`], never a silent default — mirroring
//! the recipe-side rule that older versions reject unsupported schemas
//! rather than resetting them.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Explicit rejection message returned by every gated develop command while
/// the rollback switch is enabled. Contract-pinned by
/// `src-tauri/tests/develop_rollback/entry_gate.rs`.
pub const ROLLBACK_DISABLED_MESSAGE: &str = "develop editing is disabled by the rollback switch; \
     committed sidecars, retained previous revisions, resources and the catalog projection are \
     retained untouched and no recipe is flattened into the original media";

/// Backend gate every mutating develop entry point must consult before any
/// session, sidecar or export write.
pub fn ensure_editing_available(rollback: bool) -> Result<(), String> {
    if rollback {
        Err(ROLLBACK_DISABLED_MESSAGE.to_string())
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackError {
    Io { context: String, path: PathBuf },
    Corrupt { path: PathBuf, detail: String },
}

impl fmt::Display for RollbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RollbackError::Io { context, path } => {
                write!(f, "{context} ({}): I/O error", path.display())
            }
            RollbackError::Corrupt { path, detail } => write!(
                f,
                "rollback flag document at {} is unsupported and was left untouched: {detail}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for RollbackError {}

#[derive(Debug, Serialize, Deserialize, Default)]
struct FlagDocument {
    #[serde(rename = "developRollback", default)]
    develop_rollback: bool,
}

/// File-backed rollback switch. Production points this at
/// `<app-data>/develop-rollback.json`; tests inject a temporary path.
#[derive(Debug, Clone)]
pub struct RollbackFlag {
    path: PathBuf,
}

impl RollbackFlag {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the switch state. Missing file → `Ok(false)` (develop enabled).
    /// Invalid JSON or a non-boolean `developRollback` value is an explicit
    /// [`RollbackError::Corrupt`]: the document is preserved on disk and the
    /// caller surfaces the error instead of silently resetting the switch.
    pub fn load(&self) -> Result<bool, RollbackError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(err) => {
                return Err(RollbackError::Io {
                    context: format!("reading rollback flag document: {err}"),
                    path: self.path.clone(),
                });
            }
        };
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|err| RollbackError::Corrupt {
                path: self.path.clone(),
                detail: format!("not valid JSON: {err}"),
            })?;
        // Unknown keys are tolerated (forward compatibility) but our key
        // must be a boolean when present; anything else is rejected with the
        // document preserved rather than silently defaulted.
        match value.get("developRollback") {
            None => Ok(false),
            Some(serde_json::Value::Bool(enabled)) => Ok(*enabled),
            Some(other) => Err(RollbackError::Corrupt {
                path: self.path.clone(),
                detail: format!(
                    "key 'developRollback' must be a boolean, found {other}; \
                     the document was not reset"
                ),
            }),
        }
    }

    /// Atomically persists the switch state: write a temp sibling, flush,
    /// replace. On success only this document changes.
    pub fn store(&self, enabled: bool) -> Result<(), RollbackError> {
        let document = FlagDocument {
            develop_rollback: enabled,
        };
        let text = serde_json::to_string_pretty(&document).map_err(|err| RollbackError::Io {
            context: format!("serializing rollback flag document: {err}"),
            path: self.path.clone(),
        })?;

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|err| RollbackError::Io {
                context: format!("creating rollback flag directory: {err}"),
                path: parent.to_path_buf(),
            })?;
        }

        let temp = unique_temp_path(&self.path);
        let write = || -> std::io::Result<()> {
            let mut file = fs::File::create(&temp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, &self.path)
        };
        match write() {
            Ok(()) => Ok(()),
            Err(err) => {
                let _ = fs::remove_file(&temp);
                Err(RollbackError::Io {
                    context: format!("persisting rollback flag document: {err}"),
                    path: self.path.clone(),
                })
            }
        }
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

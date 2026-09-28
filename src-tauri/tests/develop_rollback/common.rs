//! Shared helpers for the develop rollback-switch integration tests
//! (lap-63f / TASK-602, managed continuation of lap-404.2).

use lap_lib::develop::recipe_repository::RecipeRepository;
use rapidraw_edit_model::{RecipeEnvelope, sha256_hex};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SOURCE_NAME: &str = "photo.nef";
pub const ASSET: &str = "asset-A";
pub const VARIANT: &str = "default";

pub fn tmp_root(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "lap_develop_rollback_{}_{}_{}",
        tag,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn source_file(dir: &Path) -> PathBuf {
    let path = dir.join(SOURCE_NAME);
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(b"raw-bytes-for-lap-63f-rollback-tests")
        .unwrap();
    path
}

pub fn file_sha256(path: &Path) -> String {
    sha256_hex(&fs::read(path).unwrap())
}

pub fn fingerprint(source: &Path) -> String {
    file_sha256(source)
}

pub fn repo() -> RecipeRepository {
    RecipeRepository::lap_default()
}

pub fn envelope(repo: &RecipeRepository, source: &Path, exposure: f64) -> RecipeEnvelope {
    let mut env = repo.new_envelope(ASSET, VARIANT, &fingerprint(source));
    env.recipe.exposure = exposure;
    env
}

pub fn catalog_db() -> Connection {
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
    lap_lib::t_migration::ensure_develop_projection(&conn).unwrap();
    conn
}

/// Recursive directory snapshot: every regular file below `root` mapped to
/// its SHA-256, relative path normalized to forward slashes so hashes are
/// comparable across the rollback switch toggling.
pub fn tree_snapshot(root: &Path) -> BTreeMap<String, String> {
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if path.is_dir() {
                walk(&path, &rel, out);
            } else {
                out.insert(rel, file_sha256(&path));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out);
    out
}

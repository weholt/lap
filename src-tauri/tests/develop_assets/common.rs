//! Integration fixtures for grouped-asset develop operations (lap-487).
//!
//! Mirrors `tests/develop_persistence/common.rs`: real files on disk, real
//! sidecars through `RecipeRepository`, and a real SQLite catalog with the
//! `adevelop_recipes` projection table from `lap_lib::t_migration`.

use lap_lib::develop::recipe_repository::RecipeRepository;
use lap_lib::t_migration::ensure_develop_projection;
use rapidraw_edit_model::{RecipeEnvelope, sha256_hex};
use rusqlite::Connection;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn tmp_root(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "lap_develop_assets_{}_{}_{}",
        tag,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn write_source(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(bytes).unwrap();
    path
}

pub fn fingerprint(source: &Path) -> String {
    sha256_hex(&fs::read(source).unwrap())
}

pub fn repo() -> RecipeRepository {
    RecipeRepository::lap_default()
}

/// Commits an edited recipe sidecar (exposure set) for `source` and returns
/// the durable revision.
pub fn commit_recipe(source: &Path, asset_id: &str, exposure: f64) -> u64 {
    let repo = repo();
    let current = repo.current_revision(source).unwrap().unwrap_or(0);
    let mut envelope = if current == 0 {
        repo.new_envelope(asset_id, "default", &fingerprint(source))
    } else {
        repo.load(source).unwrap()
    };
    envelope.recipe.exposure = exposure;
    let receipt = repo.commit(source, current, envelope, None, None).unwrap();
    receipt.revision
}

/// SQLite catalog shaped like the Lap catalog tables the develop projection
/// resolves against (`albums` / `afolders` / `afiles`).
pub struct Catalog {
    pub conn: Connection,
}

impl Catalog {
    /// `folders` are created on disk and registered; `files` are
    /// `(folder index, file id, name)` triples.
    pub fn open(tag: &str, folder_names: &[&str], files: &[(usize, i64, &str)]) -> Catalog {
        let root = tmp_root(tag);
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
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
        conn.execute(
            "INSERT INTO albums (id, path) VALUES (1, ?1)",
            [&root.to_string_lossy()],
        )
        .unwrap();
        for (index, name) in folder_names.iter().enumerate() {
            let dir = root.join(name);
            fs::create_dir_all(&dir).unwrap();
            conn.execute(
                "INSERT INTO afolders (id, album_id, path) VALUES (?1, 1, ?2)",
                rusqlite::params![(index + 1) as i64, dir.to_string_lossy()],
            )
            .unwrap();
        }
        for (folder_index, file_id, name) in files {
            conn.execute(
                "INSERT INTO afiles (id, folder_id, name) VALUES (?1, ?2, ?3)",
                rusqlite::params![file_id, (*folder_index + 1) as i64, name],
            )
            .unwrap();
        }
        ensure_develop_projection(&conn).unwrap();
        Catalog { conn }
    }

    pub fn folder(&self, index: usize) -> PathBuf {
        let path: String = self
            .conn
            .query_row(
                "SELECT path FROM afolders WHERE id = ?1",
                rusqlite::params![(index + 1) as i64],
                |row| row.get(0),
            )
            .unwrap();
        PathBuf::from(path)
    }

    pub fn projection_file_id(&self, sidecar: &Path) -> Option<i64> {
        self.conn
            .query_row(
                "SELECT file_id FROM adevelop_recipes WHERE sidecar_path = ?1 LIMIT 1",
                rusqlite::params![sidecar.to_string_lossy()],
                |row| row.get(0),
            )
            .ok()
            .flatten()
    }

    pub fn projection_rows(&self, sidecar: &Path) -> usize {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM adevelop_recipes WHERE sidecar_path = ?1",
                rusqlite::params![sidecar.to_string_lossy()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap() as usize
    }
}

pub fn sidecar_of(member: &Path) -> PathBuf {
    RecipeRepository::sidecar_path(member)
}

pub fn sidecar_bytes(member: &Path) -> Vec<u8> {
    fs::read(sidecar_of(member)).unwrap()
}

pub fn envelope_of(member: &Path) -> RecipeEnvelope {
    repo().load(member).unwrap()
}

pub fn prev_sidecar_bytes(member: &Path) -> Option<Vec<u8>> {
    fs::read(RecipeRepository::previous_sidecar_path(member)).ok()
}

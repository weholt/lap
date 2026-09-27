use lap_lib::develop::recipe_repository::{
    FaultAction, FaultInjection, FaultPoint, RecipeRepository,
};
use rapidraw_edit_model::{RecipeEnvelope, sha256_hex};
use rusqlite::Connection;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub const CRASH_SPEC_ENV: &str = "DEV_PERSIST_CRASH_SPEC";
pub const CRASH_EXIT_OK: i32 = 70;
pub const CRASH_EXIT_POINT_NOT_REACHED: i32 = 71;
pub const SOURCE_NAME: &str = "photo.nef";
pub const ASSET: &str = "asset-A";
pub const VARIANT: &str = "default";

pub fn tmp_root(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "lap_develop_persist_{}_{}_{}",
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
    file.write_all(b"raw-bytes-for-lap-ab8-sidecar-tests")
        .unwrap();
    path
}

pub fn fingerprint(source: &Path) -> String {
    sha256_hex(&fs::read(source).unwrap())
}

pub fn repo() -> RecipeRepository {
    RecipeRepository::lap_default()
}

pub fn envelope(repo: &RecipeRepository, source: &Path, exposure: f64) -> RecipeEnvelope {
    let mut env = repo.new_envelope(ASSET, VARIANT, &fingerprint(source));
    env.recipe.exposure = exposure;
    env
}

pub fn catalog_db(dir: &Path) -> Connection {
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
    let dir_str = dir.to_string_lossy().to_string();
    conn.execute("INSERT INTO albums (id, path) VALUES (1, ?1)", [&dir_str])
        .unwrap();
    conn.execute(
        "INSERT INTO afolders (id, album_id, path) VALUES (10, 1, ?1)",
        [&dir_str],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO afiles (id, folder_id, name) VALUES (100, 10, ?1)",
        [SOURCE_NAME],
    )
    .unwrap();
    lap_lib::t_migration::ensure_develop_projection(&conn).unwrap();
    conn
}

pub fn sidecar_bytes(source: &Path) -> Vec<u8> {
    fs::read(RecipeRepository::sidecar_path(source)).unwrap()
}

pub fn canonical_bytes(envelope: &RecipeEnvelope) -> Vec<u8> {
    envelope.to_canonical_json().unwrap()
}

pub fn temp_siblings(source: &Path) -> Vec<PathBuf> {
    let dir = source.parent().unwrap();
    fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .map(|n| n.to_string_lossy().ends_with(".tmp"))
                .unwrap_or(false)
        })
        .collect()
}

pub fn prev_path(source: &Path) -> PathBuf {
    RecipeRepository::previous_sidecar_path(source)
}

pub fn prev_bytes(source: &Path) -> Option<Vec<u8>> {
    fs::read(prev_path(source)).ok()
}

pub fn set_readonly(path: &Path) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(path, perms).unwrap();
}

#[allow(clippy::permissions_set_readonly_false)]
pub fn clear_readonly(path: &Path) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_readonly(false);
    fs::set_permissions(path, perms).unwrap();
}

pub fn spawn_crash_worker(
    point: FaultPoint,
    dir: &Path,
    expected: u64,
    asset: &str,
) -> std::process::ExitStatus {
    let spec = format!(
        "{}|{}|{}|{}",
        point.as_str(),
        dir.display(),
        expected,
        asset
    );
    let exe = std::env::current_exe().unwrap();
    Command::new(exe)
        .args(["crash_worker_entrypoint", "--exact", "--nocapture"])
        .env(CRASH_SPEC_ENV, spec)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
}

pub fn run_crash_worker(spec: &str) {
    let parts: Vec<&str> = spec.splitn(4, '|').collect();
    assert_eq!(parts.len(), 4, "malformed crash spec: {spec}");
    let point =
        FaultPoint::parse(parts[0]).unwrap_or_else(|| panic!("unknown fault point {}", parts[0]));
    let dir = PathBuf::from(parts[1]);
    let expected: u64 = parts[2].parse().unwrap();
    let asset = parts[3];
    let source = dir.join(SOURCE_NAME);
    let repo = repo();
    let mut env = repo.new_envelope(asset, VARIANT, &fingerprint(&source));
    env.recipe.exposure = 0.6;
    let result = repo.commit(
        &source,
        expected,
        env,
        None,
        Some(FaultInjection {
            point,
            action: FaultAction::AbortProcess(CRASH_EXIT_OK),
        }),
    );
    eprintln!(
        "crash worker reached end without aborting at {}; commit result: {:?}",
        point.as_str(),
        result.map(|receipt| receipt.revision)
    );
    std::process::exit(CRASH_EXIT_POINT_NOT_REACHED);
}

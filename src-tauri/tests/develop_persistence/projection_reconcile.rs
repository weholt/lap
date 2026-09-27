use crate::common::{catalog_db, envelope, fingerprint, repo, source_file, tmp_root};
use lap_lib::develop::recipe_repository::{
    ProjectionRow, RecipeRepoError, RecipeRepository, ReconcileOutcome,
};
use rusqlite::Connection;
use std::fs;

fn projection_rows(conn: &Connection, sidecar_path: &str) -> Vec<ProjectionRow> {
    let mut stmt = conn
        .prepare(
            "SELECT sidecar_path, variant_id, file_id, revision, schema_version,
                    source_fingerprint, content_hash, is_edited, updated_at
             FROM adevelop_recipes WHERE sidecar_path = ?1",
        )
        .unwrap();
    stmt.query_map([sidecar_path], |row| {
        Ok(ProjectionRow {
            sidecar_path: row.get(0)?,
            variant_id: row.get(1)?,
            file_id: row.get(2)?,
            revision: row.get::<_, i64>(3)? as u64,
            schema_version: row.get::<_, i64>(4)? as u32,
            source_fingerprint: row.get(5)?,
            content_hash: row.get(6)?,
            is_edited: row.get::<_, i64>(7)? != 0,
            updated_at: row.get(8)?,
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn projection_columns(conn: &Connection) -> Vec<String> {
    let mut stmt = conn.prepare("PRAGMA table_info(adevelop_recipes)").unwrap();
    let names = stmt.query_map([], |row| row.get::<_, String>(1)).unwrap();
    names.map(Result::unwrap).collect()
}

fn seed_row(conn: &Connection, sidecar_path: &str) {
    conn.execute(
        "INSERT INTO adevelop_recipes
             (sidecar_path, variant_id, file_id, revision, schema_version,
              source_fingerprint, content_hash, is_edited, updated_at)
         VALUES (?1, 'default', NULL, 3, 1, 'seed', 'seed', 0, 0)",
        [sidecar_path],
    )
    .unwrap();
}

#[test]
fn ensure_develop_projection_is_idempotent() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE afiles (id INTEGER PRIMARY KEY, folder_id INTEGER NOT NULL, name TEXT NOT NULL);",
    )
    .unwrap();
    lap_lib::t_migration::ensure_develop_projection(&conn).unwrap();
    lap_lib::t_migration::ensure_develop_projection(&conn).unwrap();

    seed_row(&conn, "C:/somewhere/photo.nef.lapedit.json");
    let rows = projection_rows(&conn, "C:/somewhere/photo.nef.lapedit.json");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].variant_id, "default");
    assert_eq!(rows[0].revision, 3);
}

#[test]
fn projection_table_has_expected_shape() {
    let dir = tmp_root("shape");
    let conn = catalog_db(&dir);
    assert_eq!(
        projection_columns(&conn),
        vec![
            "sidecar_path",
            "variant_id",
            "file_id",
            "revision",
            "schema_version",
            "source_fingerprint",
            "content_hash",
            "is_edited",
            "updated_at"
        ]
    );
}

#[test]
fn commit_projects_revision_and_file_association() {
    let dir = tmp_root("project");
    let src = source_file(&dir);
    let repo = repo();
    let conn = catalog_db(&dir);
    let sidecar_path = RecipeRepository::sidecar_path(&src)
        .to_string_lossy()
        .to_string();

    let receipt1 = repo
        .commit(&src, 0, envelope(&repo, &src, 0.35), Some(&conn), None)
        .unwrap();
    let rows = projection_rows(&conn, &sidecar_path);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].file_id, Some(100));
    assert_eq!(rows[0].revision, 1);
    assert_eq!(rows[0].content_hash, receipt1.content_hash);
    assert!(rows[0].is_edited);
    assert_eq!(rows[0].source_fingerprint, fingerprint(&src));
    assert!(rows[0].updated_at > 0);

    let mut env2 = envelope(&repo, &src, 0.6);
    env2.revision = 2;
    let receipt2 = repo.commit(&src, 1, env2, Some(&conn), None).unwrap();
    let rows = projection_rows(&conn, &sidecar_path);
    assert_eq!(rows.len(), 1, "upsert must not duplicate rows");
    assert_eq!(rows[0].revision, 2);
    assert_eq!(rows[0].content_hash, receipt2.content_hash);
}

#[test]
fn default_recipe_commits_are_not_flagged_edited() {
    let dir = tmp_root("unedited");
    let src = source_file(&dir);
    let repo = repo();
    let conn = catalog_db(&dir);
    let sidecar_path = RecipeRepository::sidecar_path(&src)
        .to_string_lossy()
        .to_string();

    let env = repo.new_envelope("asset-A", "default", &fingerprint(&src));
    repo.commit(&src, 0, env, Some(&conn), None).unwrap();

    let rows = projection_rows(&conn, &sidecar_path);
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].is_edited);
}

#[test]
fn catalog_projection_failure_keeps_sidecar_and_reports() {
    let dir = tmp_root("projfail");
    let src = source_file(&dir);
    let repo = repo();
    let conn = catalog_db(&dir);
    conn.execute_batch("DROP TABLE adevelop_recipes").unwrap();

    let receipt = repo
        .commit(&src, 0, envelope(&repo, &src, 0.35), Some(&conn), None)
        .unwrap();
    assert!(!receipt.projection_applied);
    assert!(receipt.projection_error.is_some());
    assert_eq!(receipt.revision, 1);
    assert_eq!(repo.load(&src).unwrap().recipe.exposure, 0.35);

    lap_lib::t_migration::ensure_develop_projection(&conn).unwrap();
    let outcome = repo.reconcile(&conn, &src).unwrap();
    assert_eq!(outcome, ReconcileOutcome::Projected { revision: 1 });
    let sidecar_path = RecipeRepository::sidecar_path(&src)
        .to_string_lossy()
        .to_string();
    let rows = projection_rows(&conn, &sidecar_path);
    assert_eq!(rows[0].revision, 1);
    assert_eq!(rows[0].content_hash, receipt.content_hash);
}

#[test]
fn reconcile_recovers_sidecar_ahead_of_database() {
    let dir = tmp_root("reconcile");
    let src = source_file(&dir);
    let repo = repo();
    let conn = catalog_db(&dir);
    let sidecar_path = RecipeRepository::sidecar_path(&src)
        .to_string_lossy()
        .to_string();

    conn.execute_batch("DROP TABLE adevelop_recipes").unwrap();
    repo.commit(&src, 0, envelope(&repo, &src, 0.42), Some(&conn), None)
        .unwrap();
    lap_lib::t_migration::ensure_develop_projection(&conn).unwrap();
    assert!(projection_rows(&conn, &sidecar_path).is_empty());

    let outcome = repo.reconcile(&conn, &src).unwrap();
    assert_eq!(outcome, ReconcileOutcome::Projected { revision: 1 });
    let rows = projection_rows(&conn, &sidecar_path);
    assert_eq!(rows[0].revision, 1);
    assert_eq!(rows[0].file_id, Some(100));
    assert!(rows[0].is_edited);

    let outcome = repo.reconcile(&conn, &src).unwrap();
    assert_eq!(outcome, ReconcileOutcome::Projected { revision: 1 });
    assert_eq!(projection_rows(&conn, &sidecar_path)[0].revision, 1);
}

#[test]
fn reconcile_removes_projection_rows_for_missing_sidecars() {
    let dir = tmp_root("missing");
    let conn = catalog_db(&dir);
    let repo = repo();
    let ghost = dir.join("ghost.nef");
    let ghost_sidecar = RecipeRepository::sidecar_path(&ghost)
        .to_string_lossy()
        .to_string();
    seed_row(&conn, &ghost_sidecar);

    let outcome = repo.reconcile(&conn, &ghost).unwrap();
    assert_eq!(outcome, ReconcileOutcome::RemovedMissingSidecar);
    assert!(projection_rows(&conn, &ghost_sidecar).is_empty());
}

#[test]
fn reconcile_reports_corrupt_sidecars_without_touching_the_projection() {
    let dir = tmp_root("reconcorrupt");
    let src = source_file(&dir);
    let repo = repo();
    let conn = catalog_db(&dir);
    let sidecar_path = RecipeRepository::sidecar_path(&src)
        .to_string_lossy()
        .to_string();
    seed_row(&conn, &sidecar_path);
    fs::write(RecipeRepository::sidecar_path(&src), b"broken json {").unwrap();

    let err = repo.reconcile(&conn, &src).unwrap_err();
    assert!(matches!(err, RecipeRepoError::CorruptSidecar { .. }));

    let rows = projection_rows(&conn, &sidecar_path);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].revision, 3, "seed row must be untouched");
}

#[test]
fn reconcile_folder_walks_and_reports_errors() {
    let dir = tmp_root("walk");
    let good1 = source_file(&dir);
    let subdir = dir.join("nested");
    fs::create_dir_all(&subdir).unwrap();
    let good2_src = subdir.join("second.jpg");
    fs::write(&good2_src, b"jpeg bytes").unwrap();
    let corrupt_src = dir.join("broken.tif");
    fs::write(&corrupt_src, b"tif bytes").unwrap();

    let repo = repo();
    let conn = catalog_db(&dir);
    repo.commit(&good1, 0, envelope(&repo, &good1, 0.1), Some(&conn), None)
        .unwrap();
    let mut env2 = repo.new_envelope("asset-B", "default", &fingerprint(&good2_src));
    env2.recipe.exposure = 0.2;
    repo.commit(&good2_src, 0, env2, Some(&conn), None).unwrap();
    fs::write(
        RecipeRepository::sidecar_path(&corrupt_src),
        b"{ not parseable",
    )
    .unwrap();
    fs::write(dir.join("unrelated.txt"), b"not a sidecar").unwrap();
    fs::write(dir.join("stale.pid.tmp"), b"leftover").unwrap();

    let summary = repo.reconcile_folder(&conn, &dir).unwrap();
    assert_eq!(summary.sidecars_seen, 3);
    assert_eq!(summary.projected, 2);
    assert_eq!(summary.errors.len(), 1);
    assert_eq!(
        summary.errors[0].0,
        RecipeRepository::sidecar_path(&corrupt_src)
    );

    let good2_sidecar = RecipeRepository::sidecar_path(&good2_src)
        .to_string_lossy()
        .to_string();
    let rows = projection_rows(&conn, &good2_sidecar);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].variant_id, "default");
}

#[test]
fn startup_reconciliation_uses_album_roots_and_is_tolerant() {
    let dir = tmp_root("startup");
    let src = source_file(&dir);
    let repo = repo();
    let conn = catalog_db(&dir);
    let sidecar_path = RecipeRepository::sidecar_path(&src)
        .to_string_lossy()
        .to_string();

    repo.commit(&src, 0, envelope(&repo, &src, 0.7), Some(&conn), None)
        .unwrap();
    conn.execute(
        "UPDATE adevelop_recipes SET revision = 0 WHERE sidecar_path = ?1",
        [sidecar_path.as_str()],
    )
    .unwrap();

    let summary = repo.reconcile_albums_at_startup(&conn).unwrap();
    assert_eq!(summary.sidecars_seen, 1);
    assert_eq!(summary.projected, 1);
    assert_eq!(summary.errors.len(), 0);
    assert_eq!(projection_rows(&conn, &sidecar_path)[0].revision, 1);

    conn.execute_batch("DROP TABLE adevelop_recipes").unwrap();
    let summary = repo.reconcile_albums_at_startup(&conn).unwrap();
    assert_eq!(summary.sidecars_seen, 1);
    assert_eq!(
        summary.errors.len(),
        1,
        "projection failures must stay visible"
    );
}

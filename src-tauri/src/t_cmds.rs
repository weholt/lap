use crate::t_apple_sidecar::{
    apple_aae_sidecar_paths, build_apple_sidecar_rename_plan, collect_original_rename_db_names,
    collect_replaced_file_ids_for_targets, delete_apple_aae_sidecars, preflight_rename_plan,
    resolve_group_primary_target, rollback_copied_transfers, rollback_rename_changes,
    rollback_renamed_sidecars,
};
/**
 * Tauri commands for frontend-backend communication.
 * project: Lap
 * author:  julyx10
 * date:    2024-08-08
 */
use crate::t_config::{self, AppConfig, Library, LibraryInfo, LibraryState};
use crate::t_face;
use crate::t_image;
use crate::t_similar;
use crate::t_sqlite::{
    ACamera, ACollection, ACollectionOrder, ACollectionSelectionCount, AFile, AFileCollection,
    AFolder, ALens, ALocation, ATag, ATagFileState, ATagSelectionCount, AThumb, ATimeLine, Album,
    AlbumDisplayOrder, GroupedQueryResult, ImageSearchParams, Person, PersonPage,
    PersonPageRequest, QueryParams, SmartQueryParams,
};
use crate::t_storage;
use crate::t_utils;
use crate::{t_ai, t_common, t_sqlite};
use lap_lib::develop::asset_operations as asset_ops;

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

// cancellation token for indexing
pub struct IndexCancellation(pub Arc<Mutex<HashMap<i64, bool>>>);
pub struct ImportCancellation(pub Arc<Mutex<ImportState>>);

pub struct ImportState {
    cancelled: bool,
    running: bool,
}

impl Default for ImportState {
    fn default() -> Self {
        Self {
            cancelled: false,
            running: false,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportOrganizeFinished {
    result: Option<crate::t_utils::ImportOrganizeResult>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbRequest {
    pub file_id: i64,
    pub file_path: Option<String>,
    pub file_type: Option<i64>,
    pub orientation: Option<i32>,
    pub album_id: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportedFormatExtensions {
    pub image: Vec<String>,
    pub raw: Vec<String>,
    pub video: Vec<String>,
    pub options: Vec<ExtensionOption>,
}

// build info
include!(concat!(env!("OUT_DIR"), "/build_info.rs"));

// library

/// get app config (libraries list and current library)
#[tauri::command]
pub fn get_app_config() -> Result<AppConfig, String> {
    t_config::load_app_config()
}

fn extension_description(ext: &str) -> &'static str {
    match ext {
        "3fr" => "Hasselblad RAW",
        "3gp" => "3GPP Multimedia",
        "arw" => "Sony Alpha RAW",
        "asf" => "Advanced Systems Format",
        "avi" => "Audio Video Interleave",
        "avif" => "AV1 Image File Format",
        "bmp" => "Bitmap Image File",
        "cr2" => "Canon RAW 2",
        "cr3" => "Canon RAW 3",
        "crw" => "Canon RAW",
        "dcr" => "Kodak RAW",
        "dds" => "DirectDraw Surface",
        "dng" => "Digital Negative",
        "dpx" => "Digital Picture Exchange",
        "erf" => "Epson RAW",
        "exr" => "OpenEXR High Dynamic Range Image",
        "flv" => "Flash Video",
        "gif" => "Graphics Interchange Format",
        "hdr" => "Radiance High Dynamic Range Image",
        "heic" => "High Efficiency Image Container",
        "heif" => "High Efficiency Image File",
        "hevc" => "High Efficiency Video Coding",
        "hif" => "HEIF Image File",
        "j2c" => "JPEG 2000 Codestream",
        "j2k" => "JPEG 2000 Codestream",
        "jp2" => "JPEG 2000 Image",
        "jpc" => "JPEG 2000 Codestream",
        "jpeg" => "Joint Photographic Experts Group",
        "jfif" => "JPEG File Interchange Format",
        "jpf" => "JPEG 2000 Image",
        "jpg" => "Joint Photographic Experts Group",
        "jpx" => "JPEG 2000 Extended Image",
        "jxl" => "JPEG XL",
        "kdc" => "Kodak Digital Camera RAW",
        "m2ts" => "Blu-ray BDAV Video",
        "m4v" => "MPEG-4 Video",
        "mdc" => "Minolta RAW",
        "mef" => "Mamiya RAW",
        "mkv" => "Matroska Video",
        "mod" => "Camcorder Video",
        "mos" => "Leaf RAW",
        "mov" => "QuickTime Movie",
        "mp4" => "MPEG-4 Video",
        "mpeg" => "Moving Picture Experts Group Video",
        "mpg" => "MPEG Video",
        "mrw" => "Minolta RAW",
        "mts" => "AVCHD Video",
        "nef" => "Nikon Electronic Format",
        "nrw" => "Nikon RAW",
        "orf" => "Olympus RAW Format",
        "pef" => "Pentax Electronic File",
        "png" => "Portable Network Graphics",
        "psd" => "Photoshop Document",
        "qoi" => "Quite OK Image",
        "raf" => "Fujifilm RAW",
        "raw" => "Generic Camera RAW",
        "rgbe" => "Radiance RGBE Image",
        "rw2" => "Panasonic RAW 2",
        "rwl" => "Leica RAW",
        "sr2" => "Sony RAW 2",
        "srf" => "Sony RAW Format",
        "srw" => "Samsung RAW",
        "tga" => "Truevision Targa Image",
        "tif" => "Tagged Image File",
        "tiff" => "Tagged Image File Format",
        "tod" => "JVC HD Camcorder Video",
        "ts" => "MPEG Transport Stream",
        "webm" => "WebM Video",
        "webp" => "Web Picture Format",
        "wmv" => "Windows Media Video",
        _ => "File Format",
    }
}

fn extension_option(ext: &str) -> ExtensionOption {
    ExtensionOption {
        value: ext.to_string(),
        label: format!(
            "{} ({})",
            ext.to_ascii_uppercase(),
            extension_description(ext)
        ),
    }
}

/// get supported file format extensions for smart album filters
#[tauri::command]
pub fn get_supported_format_extensions() -> SupportedFormatExtensions {
    let image: Vec<String> = t_common::NORMAL_IMGS
        .iter()
        .chain(t_common::FFMPEG_BACKED_IMGS.iter())
        .map(|ext| ext.to_string())
        .collect();
    let raw: Vec<String> = t_common::RAW_IMGS
        .iter()
        .map(|ext| ext.to_string())
        .collect();
    let video: Vec<String> = t_common::VIDEOS.iter().map(|ext| ext.to_string()).collect();
    let mut options: Vec<ExtensionOption> = image
        .iter()
        .chain(raw.iter())
        .chain(video.iter())
        .map(|ext| extension_option(ext))
        .collect();
    options.sort_by(|a, b| a.value.cmp(&b.value));

    SupportedFormatExtensions {
        image,
        raw,
        video,
        options,
    }
}

/// Return formats whose preview pixels are generated through the bundled
/// FFmpeg sidecar, so the frontend can avoid racing the preview with a
/// thumbnail placeholder.
#[tauri::command]
pub fn get_ffmpeg_backed_image_extensions() -> Vec<String> {
    t_common::FFMPEG_BACKED_IMGS
        .iter()
        .map(|ext| (*ext).to_string())
        .collect()
}

/// Whether the host GStreamer stack can create `autoaudiosink`, which WebKitGTK
/// needs for in-webview video playback (#313). The Linux AppImage ships no
/// GStreamer of its own, so when the host lacks the element WebKitGTK crashes
/// its WebProcess on a NULL element instead of degrading gracefully. Report
/// "unavailable" only when the probe positively confirms the element is
/// missing; if the probe tool itself is absent, keep current behavior.
#[tauri::command]
pub fn check_gstreamer_available() -> bool {
    use std::sync::OnceLock;
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(probe_gstreamer_available)
}

fn probe_gstreamer_available() -> bool {
    if !cfg!(target_os = "linux") {
        return true;
    }
    match Command::new("gst-inspect-1.0")
        .args(["autoaudiosink"])
        .env("LC_ALL", "C")
        .stdout(std::process::Stdio::null())
        .output()
    {
        Ok(output) => !gstreamer_element_missing(output.status.code(), &output.stderr),
        Err(_) => true,
    }
}

fn gstreamer_element_missing(exit_code: Option<i32>, stderr: &[u8]) -> bool {
    exit_code.is_some_and(|code| code != 0)
        && String::from_utf8_lossy(stderr)
            .lines()
            .any(|line| line.trim() == "No such element or plugin 'autoaudiosink'")
}

#[cfg(test)]
mod gstreamer_probe_tests {
    use super::gstreamer_element_missing;

    #[test]
    fn only_explicit_missing_element_disables_preview() {
        let missing = b"No such element or plugin 'autoaudiosink'\n";
        assert!(gstreamer_element_missing(Some(255), missing));
        assert!(!gstreamer_element_missing(Some(0), missing));
        assert!(!gstreamer_element_missing(None, missing));
        assert!(!gstreamer_element_missing(
            Some(127),
            b"error while loading shared libraries"
        ));
        assert!(!gstreamer_element_missing(Some(1), b"unknown failure"));
    }
}

/// set last selected item index
#[tauri::command]
pub fn set_last_selected_item_index(index: i64) -> Result<(), String> {
    let mut config = t_config::load_app_config()?;
    config.last_selected_item_index = index;
    t_config::save_app_config(&config)
}

#[tauri::command]
pub fn get_db_storage_dir() -> Result<String, String> {
    t_storage::get_db_storage_dir()
}

#[tauri::command]
pub fn is_using_custom_db_storage() -> Result<bool, String> {
    t_storage::is_using_custom_db_storage()
}

fn ensure_db_storage_change_allowed(
    status_state: &State<t_face::FaceIndexingStatus>,
) -> Result<(), String> {
    if t_storage::is_db_migration_in_progress() {
        return Err("Database storage migration is already in progress.".to_string());
    }

    let is_library_indexing = t_config::get_current_library_state()
        .map(|state| state.index.status == 1)
        .unwrap_or(false);
    if is_library_indexing {
        return Err(
            "Cannot change database storage while library indexing is running.".to_string(),
        );
    }

    if *status_state.0.lock().unwrap() {
        return Err("Cannot change database storage while face indexing is running.".to_string());
    }

    if t_sqlite::has_active_thumb_background_tasks() {
        return Err(
            "Cannot change database storage while thumbnails are still being generated."
                .to_string(),
        );
    }

    Ok(())
}

#[tauri::command]
pub fn change_db_storage_dir(
    new_dir: &str,
    status_state: State<t_face::FaceIndexingStatus>,
) -> Result<String, String> {
    ensure_db_storage_change_allowed(&status_state)?;
    let result = t_storage::change_db_storage_dir(new_dir);
    if result.is_ok() {
        revalidate_db_after_path_change();
    }
    result
}

#[tauri::command]
pub fn reset_db_storage_dir(
    status_state: State<t_face::FaceIndexingStatus>,
) -> Result<String, String> {
    ensure_db_storage_change_allowed(&status_state)?;
    let result = t_storage::reset_db_storage_dir();
    if result.is_ok() {
        revalidate_db_after_path_change();
    }
    result
}

/// The db file moved to a new path without going through switch_library, so drop stale pooled
/// connections and re-evaluate the corruption mark for the new current path (create_db clears or
/// re-sets it and re-runs migrations). Errors are logged, not propagated: the move already
/// succeeded and the command's contract returns the new directory.
fn revalidate_db_after_path_change() {
    t_sqlite::clear_conn_pool();
    if let Err(e) = t_sqlite::create_db() {
        eprintln!("db storage dir changed: post-move create_db failed: {}", e);
    }
}

// ----------------------------------------------------------------------------
// Develop recipe companions for grouped-asset operations (lap-487).
//
// Every grouped-asset mutation below treats `filename.ext.lapedit.json`
// sidecars as part of the asset group (docs/raw-development/spec.md A8):
// rename/move keep identity, copy forks identity, trash/restore keep the
// group together, and the rebuildable catalog projection is maintained
// best-effort after each successful operation (startup/rescan reconciliation
// heals anything it missed).
// ----------------------------------------------------------------------------

/// Plans sidecar companion moves for grouped rename/move members: each
/// member's sidecar is name-anchored to that member only (a same-basename
/// RAW/JPEG pair never swaps recipes).
fn develop_companion_moves(members: &[(PathBuf, PathBuf)]) -> Vec<asset_ops::CompanionMove> {
    members
        .iter()
        .filter_map(|(old, new)| asset_ops::companion_move(old, new))
        .collect()
}

/// Rolls a companion journal back and reports steps that could not be
/// reverted (explicit, never silently dropped).
fn rollback_develop_journal(journal: asset_ops::OperationJournal, context: &str) {
    let summary = journal.rollback();
    if summary.reverted > 0 {
        eprintln!(
            "{context}: rolled back {} develop recipe sidecar step(s)",
            summary.reverted
        );
    }
    for (step, error) in summary.failed {
        eprintln!("{context}: develop recipe rollback step {step:?} failed: {error}");
    }
}

/// Best-effort projection maintenance: the catalog projection is rebuildable,
/// so a failure here is logged and healed by reconciliation; it never fails
/// the already-successful file operation.
fn migrate_develop_projection_logged(old: &Path, new: &Path) {
    let Ok(conn) = t_sqlite::open_conn() else {
        return;
    };
    if let Err(error) = asset_ops::migrate_companion_projection(&conn, old, new) {
        eprintln!(
            "develop projection migration failed for '{}': {error}",
            old.display()
        );
    }
}

fn delete_develop_projection_logged(sidecars: &[PathBuf]) {
    if sidecars.is_empty() {
        return;
    }
    let Ok(conn) = t_sqlite::open_conn() else {
        return;
    };
    if let Err(error) = asset_ops::delete_companion_projection(&conn, sidecars) {
        eprintln!("develop projection cleanup failed after delete: {error}");
    }
}

fn migrate_develop_projection_folder_logged(old_prefix: &Path, new_prefix: &Path) {
    let Ok(conn) = t_sqlite::open_conn() else {
        return;
    };
    if let Err(error) =
        asset_ops::migrate_companion_projection_folder(&conn, old_prefix, new_prefix)
    {
        eprintln!(
            "develop projection folder migration failed for '{}': {error}",
            old_prefix.display()
        );
    }
}

fn delete_develop_projection_folder_logged(folder_prefix: &Path) {
    let Ok(conn) = t_sqlite::open_conn() else {
        return;
    };
    if let Err(error) = asset_ops::delete_companion_projection_folder(&conn, folder_prefix) {
        eprintln!(
            "develop projection folder cleanup failed for '{}': {error}",
            folder_prefix.display()
        );
    }
}

#[tauri::command]
pub fn add_library(name: &str) -> Result<Library, String> {
    t_config::add_library(name)
}

/// hide a library
#[tauri::command]
pub fn hide_library(id: &str, hidden: bool) -> Result<(), String> {
    t_config::hide_library(id, hidden)
}

/// reorder libraries
#[tauri::command]
pub fn reorder_libraries(ids: Vec<String>) -> Result<(), String> {
    t_config::reorder_libraries(ids)
}

/// edit library name
#[tauri::command]
pub fn edit_library(id: &str, name: &str) -> Result<(), String> {
    t_config::edit_library(id, name)
}

/// remove a library (also deletes the database file)
#[tauri::command]
pub fn remove_library(id: &str) -> Result<(), String> {
    t_config::remove_library(id)
}

/// Startup integrity check result for the selected library.
#[tauri::command]
pub fn is_database_corrupted() -> bool {
    t_sqlite::is_database_corrupted()
}

/// switch to a different library
#[tauri::command]
pub async fn switch_library(app_handle: tauri::AppHandle, id: String) -> Result<(), String> {
    // The blocking task reports whether the target library turned out to be corrupt, deciding from
    // create_db's returned error itself rather than re-reading the process-global flag afterwards
    // (which a concurrent switch could resolve against a different library).
    let corrupted = tauri::async_runtime::spawn_blocking(move || -> Result<bool, String> {
        t_config::switch_library(&id)?;
        t_utils::clear_album_accessibility();
        t_sqlite::clear_conn_pool();
        match t_sqlite::create_db() {
            Ok(()) => Ok(false),
            Err(e) if e == t_sqlite::DB_CORRUPTED_MSG => Ok(true),
            Err(e) => Err(e),
        }
    })
    .await
    .map_err(|e| format!("Failed to join switch library task: {}", e))??;

    if corrupted {
        return Ok(());
    }
    t_utils::restore_album_scopes(&app_handle)?;
    tauri::async_runtime::spawn_blocking(|| -> Result<(), String> {
        let mut albums = Album::get_all_albums()
            .map_err(|e| format!("Error while checking albums after switching library: {}", e))?;
        t_utils::refresh_all_album_accessibility(&mut albums);
        Ok(())
    })
    .await
    .map_err(|e| format!("Failed to join album accessibility check: {}", e))??;
    t_utils::start_folder_mtime_sync(app_handle);
    Ok(())
}

/// get library statistics
#[tauri::command]
pub async fn get_library_info(id: String) -> Result<LibraryInfo, String> {
    tauri::async_runtime::spawn_blocking(move || t_config::get_library_info(&id))
        .await
        .map_err(|e| format!("Failed to join library info task: {}", e))?
}

/// save library state
#[tauri::command]
pub fn save_library_state(id: &str, state: LibraryState) -> Result<(), String> {
    t_config::save_library_state(id, state)
}

/// get library state
#[tauri::command]
pub fn get_library_state(id: &str) -> Result<LibraryState, String> {
    t_config::get_library_state(id)
}

/// get current library state
#[tauri::command]
pub fn get_current_library_state() -> Result<LibraryState, String> {
    t_config::get_current_library_state()
}

// album

/// get all albums
#[tauri::command]
pub fn get_all_albums(refresh_accessibility: bool) -> Result<Vec<Album>, String> {
    // Best-effort: repair stale covers so the album list never renders a
    // broken thumbnail.
    let albums =
        Album::get_all_albums().map_err(|e| format!("Error while getting all albums: {}", e))?;
    for album in &albums {
        if let Some(album_id) = album.id {
            let _ = Album::auto_set_cover(album_id);
        }
    }

    // Reload to reflect any repaired covers.
    let mut albums =
        Album::get_all_albums().map_err(|e| format!("Error while getting all albums: {}", e))?;
    if refresh_accessibility {
        t_utils::refresh_all_album_accessibility(&mut albums);
    } else {
        for album in &mut albums {
            t_utils::apply_album_accessibility(album);
        }
    }
    Ok(albums)
}

/// Get the indexed folder records used by the album sidebar search.
#[tauri::command]
pub fn get_all_album_folders() -> Result<Vec<AFolder>, String> {
    AFolder::get_all().map_err(|e| format!("Error while getting album folders: {}", e))
}

/// batch-generate thumbnails for a directory into an output folder
#[tauri::command]
pub fn generate_directory_thumbnails(
    dir_path: &str,
    output_dir: &str,
    thumbnail_size: u32,
) -> Result<t_image::BatchThumbnailStats, String> {
    t_image::generate_directory_thumbnails(dir_path, output_dir, thumbnail_size)
}

/// get one album
#[tauri::command]
pub fn get_album(album_id: i64) -> Result<Album, String> {
    let mut album = Album::get_album_by_id(album_id)
        .map_err(|e| format!("Error while getting one album: {}", e))?;
    t_utils::apply_album_accessibility(&mut album);
    Ok(album)
}

/// Recheck one album root when the user selects that album or one of its folders.
#[tauri::command]
pub fn check_album_accessibility(album_id: i64) -> Result<bool, String> {
    let mut album = Album::get_album_by_id(album_id)
        .map_err(|e| format!("Error while checking album accessibility: {}", e))?;
    t_utils::refresh_album_accessibility(&mut album);
    Ok(album.is_accessible)
}

/// recount files for an album and return updated album
#[tauri::command]
pub fn recount_album(album_id: i64) -> Result<Album, String> {
    Album::recount_album(album_id).map_err(|e| format!("Error while recounting album: {}", e))
}

#[tauri::command]
pub fn get_album_visible_counts(small_file_filter: i64) -> Result<HashMap<i64, i64>, String> {
    Album::get_visible_counts(small_file_filter)
        .map_err(|e| format!("Error while getting album visible counts: {}", e))
}

/// add an album
#[tauri::command]
pub fn add_album(app_handle: tauri::AppHandle, folder_path: &str) -> Result<Album, String> {
    t_utils::authorize_directory_scope(&app_handle, folder_path).map_err(|e| {
        format!(
            "Error while authorizing album folder '{}': {}",
            folder_path, e
        )
    })?;

    Album::add_album_to_db(folder_path)
        .map_err(|e| format!("Error while adding an album to DB: {}", e))
}

/// edit an album
#[tauri::command]
pub fn edit_album(id: i64, name: &str, description: &str) -> Result<usize, String> {
    let _ = Album::update_column(id, "name", &name)
        .map_err(|e| format!("Error while editing album with id {}: {}", id, e));

    Album::update_column(id, "description", &description)
        .map_err(|e| format!("Error while editing album with id {}: {}", id, e))
}

/// remove an album
#[tauri::command]
pub async fn remove_album(state: State<'_, IndexCancellation>, id: i64) -> Result<usize, String> {
    let _removal_guard = t_utils::AlbumRemovalGuard::acquire(id);
    state.0.lock().unwrap().insert(id, true);
    t_utils::wait_for_album_scan_end(id).await?;

    let result = {
        let album_sync_lock = t_utils::album_sync_lock(id);
        let _album_sync_guard = album_sync_lock
            .lock()
            .map_err(|_| format!("Album sync lock poisoned: {}", id))?;
        Album::delete_from_db(id)
            .map_err(|e| format!("Error while removing album with id {}: {}", id, e))?
    };
    t_utils::release_album_sync_lock(id);

    let library_id = crate::t_config::load_app_config()
        .map(|c| c.current_library_id)
        .unwrap_or_else(|_| "default".to_string());
    let album_cache_dir = crate::t_config::get_app_cache_dir()
        .map(|dir| dir.join(library_id).join(id.to_string()))
        .map_err(|e| format!("Error while resolving album thumbnail cache path: {}", e))?;
    if album_cache_dir.exists() {
        std::fs::remove_dir_all(&album_cache_dir)
            .map_err(|e| format!("Error while removing album thumbnail cache: {}", e))?;
    }

    Ok(result)
}

#[tauri::command]
pub fn reorder_albums(items: Vec<AlbumDisplayOrder>) -> Result<usize, String> {
    Album::reorder_display_order(items).map_err(|e| format!("Error while reordering albums: {}", e))
}

/// set album cover
#[tauri::command]
pub fn set_album_cover(id: i64, file_id: i64) -> Result<usize, String> {
    Album::update_column(id, "cover_file_id", &file_id)
        .map_err(|e| format!("Error while setting album cover: {}", e))
}

/// index album
#[tauri::command]
pub fn index_album(
    app_handle: tauri::AppHandle,
    state: State<IndexCancellation>,
    album_id: i64,
    thumbnail_size: u32,
    raw_thumbnail_source: String,
    skip_file_path: Option<String>,
    group_raw_jpeg_pairs: bool,
) -> Result<(), String> {
    // Reset cancellation flag
    state.0.lock().unwrap().insert(album_id, false);
    let cancellation_token = state.0.clone();

    tauri::async_runtime::spawn(async move {
        if let Err(e) = t_utils::index_album_worker(
            &app_handle,
            cancellation_token,
            album_id,
            thumbnail_size,
            raw_thumbnail_source == "embedded",
            skip_file_path,
            group_raw_jpeg_pairs,
        )
        .await
        {
            eprintln!("Error indexing album {}: {}", album_id, e);
        }
    });
    Ok(())
}

/// cancel indexing
#[tauri::command]
pub fn cancel_indexing(state: State<IndexCancellation>, album_id: i64) -> Result<(), String> {
    state.0.lock().unwrap().insert(album_id, true);
    Ok(())
}

#[tauri::command]
pub fn get_index_recovery_info() -> Option<crate::t_utils::IndexRecoveryInfo> {
    crate::t_utils::read_index_trace()
}

#[tauri::command]
pub fn clear_index_recovery_info() -> Result<(), String> {
    t_utils::clear_index_trace();
    Ok(())
}

// folder

fn find_renamed_sibling_folder(album_id: i64, folder_path: &str) -> Result<Option<String>, String> {
    let Some(inode) = AFolder::get_inode(album_id, folder_path)? else {
        return Ok(None);
    };
    if inode == 0 {
        return Ok(None);
    }
    let Some(parent) = Path::new(folder_path).parent() else {
        return Ok(None);
    };
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(_) => return Ok(None),
    };

    for entry in entries.flatten() {
        if t_utils::is_fs_entry_hidden(&entry)
            || !entry.file_type().is_ok_and(|file_type| file_type.is_dir())
        {
            continue;
        }
        let path = entry.path().to_string_lossy().to_string();
        if t_utils::FileInfo::new(&path)
            .ok()
            .is_some_and(|info| info.inode as i64 == inode)
        {
            return Ok(Some(path));
        }
    }

    Ok(None)
}

// click to select a sub-folder under an album
#[tauri::command]
pub fn select_folder(
    app_handle: tauri::AppHandle,
    album_id: i64,
    folder_path: &str,
) -> Result<AFolder, String> {
    if !t_utils::directory_accessible(folder_path) {
        if let Some(renamed_path) = find_renamed_sibling_folder(album_id, folder_path)? {
            t_utils::authorize_directory_scope(&app_handle, &renamed_path)
                .map_err(|e| format!("Error while authorizing folder '{}': {}", renamed_path, e))?;
            let inode = t_utils::FileInfo::new(&renamed_path)
                .ok()
                .map(|info| info.inode as i64)
                .filter(|inode| *inode != 0);
            AFolder::migrate_path_by_inode(album_id, &renamed_path, inode)?;
            return AFolder::fetch(&renamed_path)?
                .ok_or_else(|| format!("Renamed folder missing from DB: {}", renamed_path));
        }

        if let Some(folder) = AFolder::fetch(folder_path)? {
            if folder.album_id == album_id {
                return Ok(folder);
            }
        }
    }

    t_utils::authorize_directory_scope(&app_handle, folder_path)
        .map_err(|e| format!("Error while authorizing folder '{}': {}", folder_path, e))?;

    AFolder::add_to_db(album_id, folder_path)
        .map_err(|e| format!("Error while adding folder to DB: {}", e))
}

/// fetch folder and build a FileNode
#[tauri::command]
pub fn fetch_folder(
    path: &str,
    is_recursive: bool,
    sort: i64,
) -> Result<t_utils::FileNode, String> {
    t_utils::FileNode::build_nodes(path, is_recursive, sort)
}

/// count all files in a folder (include all sub-folders)
#[tauri::command]
pub async fn count_folder(path: String) -> Result<(u64, u64, u64, u64, u64, u64, u64), String> {
    tauri::async_runtime::spawn_blocking(move || t_utils::count_folder_files(&path))
        .await
        .map_err(|e| format!("Failed to count folder: {}", e))
}

/// create a new folder
#[tauri::command]
pub fn create_folder(path: &str, folder_name: &str) -> Option<String> {
    let folder_path = t_utils::get_file_path(path, folder_name);
    t_utils::create_new_folder(&folder_path)
}

/// rename a folder
#[tauri::command]
pub fn rename_folder(folder_path: &str, new_folder_name: &str) -> Option<String> {
    let new_folder_path = t_utils::rename_folder(folder_path, new_folder_name);

    match new_folder_path {
        Some(new_path) => {
            if let Err(e) = Album::rename_root_folder(folder_path, &new_path) {
                eprintln!("Error while renaming root folder in DB: {}", e);
                return None;
            }
            // Recipe sidecars traveled with the directory; keep the
            // projection paths aligned (lap-487).
            migrate_develop_projection_folder_logged(Path::new(folder_path), Path::new(&new_path));
            Some(new_path)
        }
        None => None,
    }
}

/// move a folder
#[tauri::command]
pub fn move_folder(
    folder_path: &str,
    new_album_id: i64,
    new_folder_path: &str,
    conflict_policy: &str,
) -> Result<String, String> {
    let old_album_id = AFolder::fetch(folder_path)?.map(|folder| folder.album_id);
    let moved_thumb_keys = if old_album_id.is_some_and(|album_id| album_id != new_album_id) {
        AThumb::get_thumb_keys_in_subtree(folder_path)?
    } else {
        Vec::new()
    };
    let transfer = t_utils::move_folder_with_policy(
        folder_path,
        new_folder_path,
        t_utils::FileConflictPolicy::from_str(conflict_policy),
    )?;
    let new_path = transfer.path.clone();
    let db_result = if conflict_policy == "replace" {
        AFolder::replace_moved_folder(folder_path, new_album_id, &new_path)
    } else {
        AFolder::move_folder(folder_path, new_album_id, &new_path)
    };
    if let Err(error) = db_result {
        let rollback_error = transfer.rollback_move(Path::new(folder_path)).err();
        return Err(match rollback_error {
            Some(rollback_error) => format!(
                "Error while moving folder in DB: {}; rollback also failed: {}",
                error, rollback_error
            ),
            None => format!("Error while moving folder in DB: {}", error),
        });
    }
    // Recipe sidecars traveled with the directory; keep the projection paths
    // aligned (lap-487).
    migrate_develop_projection_folder_logged(Path::new(folder_path), Path::new(&new_path));
    if let Some(old_album_id) = old_album_id {
        AThumb::relocate_for_thumb_keys(&moved_thumb_keys, old_album_id, new_album_id);
    }
    transfer.finalize()
}

/// move a folder outside the library and remove its database records
#[tauri::command]
pub fn move_folder_outside_library(
    folder_path: &str,
    new_folder_path: &str,
    conflict_policy: &str,
) -> Result<String, String> {
    let transfer = t_utils::move_folder_with_policy(
        folder_path,
        new_folder_path,
        t_utils::FileConflictPolicy::from_str(conflict_policy),
    )?;

    if let Err(error) = AFolder::delete_folder(folder_path) {
        let rollback_error = transfer.rollback_move(Path::new(folder_path)).err();
        return Err(match rollback_error {
            Some(rollback_error) => format!(
                "Error while removing folder from DB: {}; rollback also failed: {}",
                error, rollback_error
            ),
            None => format!("Error while removing folder from DB: {}", error),
        });
    }

    // The folder left the library together with its recipe sidecars (lap-487).
    delete_develop_projection_folder_logged(Path::new(folder_path));
    transfer.finalize()
}

/// copy a folder
#[tauri::command]
pub fn copy_folder(
    folder_path: &str,
    new_folder_path: &str,
    new_album_id: i64,
    conflict_policy: &str,
) -> Result<String, String> {
    let transfer = t_utils::copy_folder_with_policy(
        folder_path,
        new_folder_path,
        t_utils::FileConflictPolicy::from_str(conflict_policy),
    )?;
    let new_path = transfer.path.clone();
    // Recipe sidecars were copied verbatim with the directory; each copy gets
    // a NEW asset identity carrying the same initial recipe (lap-487). A fork
    // failure rolls the whole folder copy back instead of leaving duplicated
    // identities behind.
    if let Err(error) = asset_ops::fork_copied_folder_sidecars(Path::new(&new_path)) {
        let rollback_error = transfer.rollback_copy().err();
        return Err(match rollback_error {
            Some(rollback_error) => format!(
                "Failed to fork copied develop recipes: {error}; rollback also failed: {rollback_error}"
            ),
            None => format!("Failed to fork copied develop recipes: {error}"),
        });
    }
    if new_album_id > 0 {
        let db_result = if conflict_policy == "replace" {
            AFolder::replace_copied_folder(new_album_id, &new_path)
        } else {
            AFolder::add_to_db(new_album_id, &new_path)
        };
        if let Err(error) = db_result {
            let rollback_error = transfer.rollback_copy().err();
            return Err(match rollback_error {
                Some(rollback_error) => format!(
                    "Error while adding copied folder to DB: {}; rollback also failed: {}",
                    error, rollback_error
                ),
                None => format!("Error while adding copied folder to DB: {}", error),
            });
        }
    }
    transfer.finalize()
}

/// delete a folder (move to trash)
#[tauri::command]
pub fn delete_folder(folder_path: &str) -> Result<usize, String> {
    // trash the folder
    if t_utils::trash_path(folder_path).is_err() {
        return Ok(0);
    }

    // delete the folder and all children from db
    let deleted = AFolder::delete_folder(folder_path)
        .map_err(|e| format!("Error while deleting folder from DB: {}", e))?;
    // Recipe sidecars were trashed with the folder (lap-487).
    delete_develop_projection_folder_logged(Path::new(folder_path));
    Ok(deleted)
}

/// permanently delete a folder (skip trash)
#[tauri::command]
pub fn delete_folder_permanently(folder_path: &str) -> Result<usize, String> {
    t_utils::delete_folder_permanently(folder_path)?;

    let deleted = AFolder::delete_folder(folder_path)
        .map_err(|e| format!("Error while deleting folder from DB: {}", e))?;
    delete_develop_projection_folder_logged(Path::new(folder_path));
    Ok(deleted)
}

/// reveal a file or folder in the file explorer (or finder)
#[tauri::command]
pub fn reveal_path(path: &str) -> Result<(), String> {
    t_utils::reveal_path(path)
}

/// open an external URL or app-specific deep link
#[tauri::command]
pub fn open_external_url(url: &str) -> Result<(), String> {
    opener::open(url).map_err(|e| e.to_string())
}

/// Set a displayable photo as the desktop wallpaper using the operating
/// system's default layout. A RAW+JPEG pair supplies its JPEG/HEIC companion.
#[tauri::command]
pub async fn set_desktop_wallpaper(
    app_handle: AppHandle,
    file_path: &str,
    companion_path: Option<String>,
) -> Result<(), String> {
    let source_path = companion_path
        .filter(|path| !path.trim().is_empty() && Path::new(path).is_file())
        .unwrap_or_else(|| file_path.to_string());
    if !Path::new(&source_path).is_file() {
        return Err("Wallpaper source file does not exist".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        app_handle
            .run_on_main_thread(move || {
                use objc2::MainThreadMarker;
                use objc2::runtime::AnyObject;
                use objc2_app_kit::{NSScreen, NSWorkspace, NSWorkspaceDesktopImageOptionKey};
                use objc2_foundation::{NSDictionary, NSString, NSURL};

                let result = (|| {
                    let main_thread = MainThreadMarker::new().ok_or_else(|| {
                        "macOS wallpaper updates must run on the main thread".to_string()
                    })?;
                    let screen = NSScreen::mainScreen(main_thread)
                        .ok_or_else(|| "macOS could not find the primary display".to_string())?;
                    let path = NSString::from_str(&source_path);
                    let url = NSURL::fileURLWithPath(&path);
                    let options: objc2::rc::Retained<
                        NSDictionary<NSWorkspaceDesktopImageOptionKey, AnyObject>,
                    > = NSDictionary::new();
                    unsafe {
                        NSWorkspace::sharedWorkspace()
                            .setDesktopImageURL_forScreen_options_error(&url, &screen, &options)
                    }
                    .map_err(|error| {
                        format!(
                            "macOS could not set the desktop wallpaper: {}",
                            error.localizedDescription()
                        )
                    })
                })();
                let _ = sender.send(result);
            })
            .map_err(|error| format!("Failed to schedule macOS wallpaper update: {error}"))?;
        receiver
            .await
            .map_err(|_| "macOS wallpaper update was cancelled".to_string())??;
    }

    #[cfg(target_os = "windows")]
    {
        use windows::Win32::System::Com::{
            CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
            CoUninitialize,
        };
        use windows::Win32::UI::Shell::{DesktopWallpaper, IDesktopWallpaper};
        use windows::core::HSTRING;

        let (sender, receiver) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let result = (|| unsafe {
                // `CoInitializeEx` returns an HRESULT (rather than a Result) in
                // windows 0.61, so convert it before attaching application context.
                CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                    .ok()
                    .map_err(|error| format!("Failed to initialize Windows COM: {error}"))?;
                let update = (|| {
                    let wallpaper: IDesktopWallpaper =
                        CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL).map_err(|error| {
                            format!("Windows desktop wallpaper service is unavailable: {error}")
                        })?;
                    let monitor_count = wallpaper.GetMonitorDevicePathCount().map_err(|error| {
                        format!("Failed to enumerate Windows displays: {error}")
                    })?;
                    let primary_monitor = (0..monitor_count)
                        .find_map(|index| {
                            let monitor = wallpaper.GetMonitorDevicePathAt(index).ok()?;
                            let monitor_id = monitor.to_hstring();
                            CoTaskMemFree(Some(monitor.0 as *const std::ffi::c_void));
                            let rect = wallpaper.GetMonitorRECT(&monitor_id).ok()?;
                            (rect.left <= 0 && rect.right > 0 && rect.top <= 0 && rect.bottom > 0)
                                .then_some(monitor_id)
                        })
                        .ok_or_else(|| "Windows could not find the primary display".to_string())?;
                    let path = HSTRING::from(source_path);
                    wallpaper
                        .SetWallpaper(&primary_monitor, &path)
                        .map_err(|error| {
                            format!("Windows could not set the desktop wallpaper: {error}")
                        })
                })();
                CoUninitialize();
                update
            })();
            let _ = sender.send(result);
        });
        receiver
            .await
            .map_err(|_| "Windows wallpaper update was cancelled".to_string())??;
    }

    #[cfg(target_os = "linux")]
    {
        use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

        const URI_PATH_ENCODE_SET: &AsciiSet = &CONTROLS
            .add(b' ')
            .add(b'"')
            .add(b'#')
            .add(b'%')
            .add(b'<')
            .add(b'>')
            .add(b'?')
            .add(b'[')
            .add(b'\\')
            .add(b']')
            .add(b'^')
            .add(b'`')
            .add(b'{')
            .add(b'|')
            .add(b'}');

        let file_uri = format!(
            "file://{}",
            utf8_percent_encode(&source_path, URI_PATH_ENCODE_SET)
        );
        let set_key = |key: &str| {
            Command::new("gsettings")
                .args(["set", "org.gnome.desktop.background", key, &file_uri])
                .output()
        };
        let output = set_key("picture-uri")
            .map_err(|error| format!("Failed to start GNOME wallpaper service: {error}"))?;
        if !output.status.success() {
            return Err(
                "This Linux desktop environment does not support setting wallpapers from Lap"
                    .to_string(),
            );
        }
        // GNOME 42+ can use a separate dark wallpaper. Older schemas simply
        // reject this key, which is safe to ignore after picture-uri succeeds.
        let _ = set_key("picture-uri-dark");
    }

    Ok(())
}

#[tauri::command]
pub fn get_external_app_display_name(app_path: &str) -> Result<String, String> {
    t_utils::get_external_app_display_name(app_path)
}

/// open a file with a specific external application
#[tauri::command]
pub fn open_file_with_app(file_path: &str, app_path: &str) -> Result<(), String> {
    open_files_with_app(vec![file_path.to_string()], app_path)
}

/// open one or more files with a specific external application
#[tauri::command]
pub fn open_files_with_app(file_paths: Vec<String>, app_path: &str) -> Result<(), String> {
    let file_paths: Vec<String> = file_paths.into_iter().filter(|p| !p.is_empty()).collect();

    if file_paths.is_empty() || app_path.is_empty() {
        return Err("Missing file path or app path".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg("-a")
            .arg(app_path)
            .args(&file_paths)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    #[cfg(not(target_os = "macos"))]
    {
        Command::new(app_path)
            .args(&file_paths)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

// file

/// get query count and sum
#[tauri::command]
pub async fn get_query_count_and_sum(params: QueryParams) -> Result<(i64, i64), String> {
    AFile::get_query_count_and_sum(&params)
        .map_err(|e| format!("Error while getting query files count: {}", e))
}

/// get query time line
#[tauri::command]
pub async fn get_query_time_line(params: QueryParams) -> Result<Vec<ATimeLine>, String> {
    AFile::get_query_time_line(&params)
        .map_err(|e| format!("Error while getting query timeline: {}", e))
}

/// get query file
#[tauri::command]
pub async fn get_query_files(
    params: QueryParams,
    offset: i64,
    limit: i64,
) -> Result<Vec<AFile>, String> {
    AFile::get_query_files(&params, offset, limit)
        .map_err(|e| format!("Error while getting query files: {}", e))
}

/// Get file metadata for a bounded set of IDs without traversing a virtualized query.
#[tauri::command]
pub async fn get_files_by_ids(file_ids: Vec<i64>) -> Result<Vec<AFile>, String> {
    AFile::get_files_by_ids(&file_ids)
        .map_err(|e| format!("Error while getting files by IDs: {}", e))
}

/// Get grouped render rows for a normal query.
/// The result includes group header rows, file item rows, group metadata, and row counts for virtual scrolling.
#[tauri::command]
pub async fn get_grouped_query_rows(
    params: QueryParams,
    offset: i64,
    limit: i64,
) -> Result<GroupedQueryResult, String> {
    AFile::get_grouped_query_rows(&params, offset, limit)
        .map_err(|e| format!("Error while getting grouped query rows: {}", e))
}

/// Get all file ids in one group for a normal query.
/// Used by the group header checkbox so selecting a group is not limited to loaded rows.
#[tauri::command]
pub async fn get_group_file_ids(params: QueryParams, group_id: String) -> Result<Vec<i64>, String> {
    AFile::get_group_file_ids(&params, &group_id)
        .map_err(|e| format!("Error while getting group file ids: {}", e))
}

/// Get a file's index in the flattened grouped query result.
#[tauri::command]
pub async fn get_grouped_file_position(
    params: QueryParams,
    file_id: i64,
) -> Result<Option<i64>, String> {
    AFile::get_grouped_file_position(&params, file_id)
        .map_err(|e| format!("Error while getting grouped file position: {}", e))
}

/// Get all file ids in the current normal query.
/// Used by Select All to support large virtualized result sets without loading every file object.
#[tauri::command]
pub async fn get_query_file_ids(params: QueryParams) -> Result<Vec<i64>, String> {
    AFile::get_query_file_ids(&params)
        .map_err(|e| format!("Error while getting query file ids: {}", e))
}

#[tauri::command]
pub fn get_library_visible_counts(
    small_file_filter: i64,
) -> Result<t_sqlite::LibraryVisibleCounts, String> {
    AFile::get_library_visible_counts(small_file_filter)
        .map_err(|e| format!("Error while getting library visible counts: {}", e))
}

#[tauri::command]
pub async fn get_query_file_position(
    params: QueryParams,
    file_id: i64,
) -> Result<Option<i64>, String> {
    AFile::get_query_file_position(&params, file_id)
        .map_err(|e| format!("Error while getting query file position: {}", e))
}

// collection

#[tauri::command]
pub fn list_collections() -> Result<Vec<ACollection>, String> {
    ACollection::list().map_err(|e| format!("Error while listing collections: {}", e))
}

#[tauri::command]
pub fn get_collection_counts(small_file_filter: i64) -> Result<HashMap<i64, i64>, String> {
    ACollection::get_counts(small_file_filter)
        .map_err(|e| format!("Error while getting collection counts: {}", e))
}

#[tauri::command]
pub fn create_collection(name: &str) -> Result<ACollection, String> {
    ACollection::create(name).map_err(|e| format!("Error while creating collection: {}", e))
}

#[tauri::command]
pub fn rename_collection(id: i64, name: &str) -> Result<usize, String> {
    ACollection::rename(id, name).map_err(|e| format!("Error while renaming collection: {}", e))
}

#[tauri::command]
pub fn delete_collection(id: i64) -> Result<usize, String> {
    ACollection::delete(id).map_err(|e| format!("Error while deleting collection: {}", e))
}

#[tauri::command]
pub fn reorder_collections(items: Vec<ACollectionOrder>) -> Result<usize, String> {
    ACollection::reorder(items).map_err(|e| format!("Error while reordering collections: {}", e))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionAddResult {
    pub added: usize,
    pub skipped: usize,
    pub added_file_ids: Vec<i64>,
    pub skipped_file_ids: Vec<i64>,
}

#[tauri::command]
pub fn add_files_to_collection(
    collection_id: i64,
    file_ids: Vec<i64>,
) -> Result<CollectionAddResult, String> {
    ACollection::add_files(collection_id, file_ids)
        .map(|(added_file_ids, skipped_file_ids)| CollectionAddResult {
            added: added_file_ids.len(),
            skipped: skipped_file_ids.len(),
            added_file_ids,
            skipped_file_ids,
        })
        .map_err(|e| format!("Error while adding files to collection: {}", e))
}

#[tauri::command]
pub fn remove_files_from_collection(
    collection_id: i64,
    file_ids: Vec<i64>,
) -> Result<usize, String> {
    ACollection::remove_files(collection_id, file_ids)
        .map_err(|e| format!("Error while removing files from collection: {}", e))
}

#[tauri::command]
pub fn get_collection_selection_counts(
    file_ids: Vec<i64>,
) -> Result<Vec<ACollectionSelectionCount>, String> {
    ACollection::get_selection_counts(&file_ids)
        .map_err(|e| format!("Error while getting collection selection counts: {}", e))
}

#[tauri::command]
pub fn clear_collection(collection_id: i64) -> Result<usize, String> {
    ACollection::clear(collection_id).map_err(|e| format!("Error while clearing collection: {}", e))
}

#[tauri::command]
pub fn get_collection_file_ids(collection_id: i64) -> Result<Vec<i64>, String> {
    ACollection::file_ids(collection_id)
        .map_err(|e| format!("Error while getting collection file ids: {}", e))
}

#[tauri::command]
pub fn get_file_collections(file_id: i64) -> Result<Vec<AFileCollection>, String> {
    ACollection::for_file(file_id)
        .map_err(|e| format!("Error while getting file collections: {}", e))
}

#[tauri::command]
pub fn get_collection_count_and_sum(
    collection_id: i64,
    params: QueryParams,
) -> Result<(i64, i64), String> {
    AFile::get_collection_count_and_sum(collection_id, &params)
        .map_err(|e| format!("Error while getting collection count: {}", e))
}

#[tauri::command]
pub fn get_collection_files(
    collection_id: i64,
    params: QueryParams,
    offset: i64,
    limit: i64,
) -> Result<Vec<AFile>, String> {
    AFile::get_collection_files(collection_id, &params, offset, limit)
        .map_err(|e| format!("Error while getting collection files: {}", e))
}

#[tauri::command]
pub fn get_collection_grouped_query_rows(
    collection_id: i64,
    params: QueryParams,
    offset: i64,
    limit: i64,
) -> Result<GroupedQueryResult, String> {
    AFile::get_collection_grouped_query_rows(collection_id, &params, offset, limit)
        .map_err(|e| format!("Error while getting grouped collection rows: {}", e))
}

#[tauri::command]
pub fn get_collection_group_file_ids(
    collection_id: i64,
    params: QueryParams,
    group_id: String,
) -> Result<Vec<i64>, String> {
    AFile::get_collection_group_file_ids(collection_id, &params, &group_id)
        .map_err(|e| format!("Error while getting collection group file ids: {}", e))
}

#[tauri::command]
pub fn get_collection_query_file_ids(
    collection_id: i64,
    params: QueryParams,
) -> Result<Vec<i64>, String> {
    AFile::get_collection_query_file_ids(collection_id, &params)
        .map_err(|e| format!("Error while getting collection query file ids: {}", e))
}

#[tauri::command]
pub async fn get_smart_query_count_and_sum(params: SmartQueryParams) -> Result<(i64, i64), String> {
    AFile::get_smart_query_count_and_sum(&params)
        .map_err(|e| format!("Error while getting smart query files count: {}", e))
}

#[tauri::command]
pub async fn get_smart_query_time_line(params: SmartQueryParams) -> Result<Vec<ATimeLine>, String> {
    AFile::get_smart_query_time_line(&params)
        .map_err(|e| format!("Error while getting smart query timeline: {}", e))
}

#[tauri::command]
pub async fn get_smart_query_files(
    params: SmartQueryParams,
    offset: i64,
    limit: i64,
) -> Result<Vec<AFile>, String> {
    AFile::get_smart_query_files(&params, offset, limit)
        .map_err(|e| format!("Error while getting smart query files: {}", e))
}

/// Get grouped render rows for a smart query.
/// The result includes group header rows, file item rows, group metadata, and row counts for virtual scrolling.
#[tauri::command]
pub async fn get_smart_grouped_query_rows(
    params: SmartQueryParams,
    offset: i64,
    limit: i64,
) -> Result<GroupedQueryResult, String> {
    AFile::get_smart_grouped_query_rows(&params, offset, limit)
        .map_err(|e| format!("Error while getting smart grouped query rows: {}", e))
}

/// Get all file ids in one group for a smart query.
/// Used by the group header checkbox so selecting a group is not limited to loaded rows.
#[tauri::command]
pub async fn get_smart_group_file_ids(
    params: SmartQueryParams,
    group_id: String,
) -> Result<Vec<i64>, String> {
    AFile::get_smart_group_file_ids(&params, &group_id)
        .map_err(|e| format!("Error while getting smart group file ids: {}", e))
}

/// Get all file ids in the current smart query.
/// Used by Select All to support large virtualized result sets without loading every file object.
#[tauri::command]
pub async fn get_smart_query_file_ids(params: SmartQueryParams) -> Result<Vec<i64>, String> {
    AFile::get_smart_query_file_ids(&params)
        .map_err(|e| format!("Error while getting smart query file ids: {}", e))
}

#[tauri::command]
pub async fn get_smart_query_file_position(
    params: SmartQueryParams,
    file_id: i64,
) -> Result<Option<i64>, String> {
    AFile::get_smart_query_file_position(&params, file_id)
        .map_err(|e| format!("Error while getting smart query file position: {}", e))
}

/// get all files from the folder
#[tauri::command]
pub fn get_folder_files(
    file_type: i64,
    sort_type: i64,
    sort_order: i64,
    folder_id: i64,
    folder_path: &str,
    from_db_only: Option<bool>,
) -> (Vec<AFile>, u32, u32) {
    t_utils::get_folder_files(
        file_type,
        sort_type,
        sort_order,
        folder_id,
        folder_path,
        from_db_only.unwrap_or(false),
    )
}

/// sync a single folder's mtime and DB records with the filesystem
#[tauri::command]
pub async fn sync_album_folder_mtimes(
    app_handle: tauri::AppHandle,
    album_id: i64,
    folder_id: i64,
    folder_path: String,
    group_raw_jpeg_pairs: bool,
) -> Result<crate::t_utils::FolderMtimeSyncResult, String> {
    let reconcile_folder_path = folder_path.clone();
    let reconciled_folder_for_log = folder_path.clone();
    let sync_app_handle = app_handle.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::t_utils::sync_single_folder(
            &sync_app_handle,
            album_id,
            folder_id,
            &folder_path,
            group_raw_jpeg_pairs,
        )
    })
    .await
    .map_err(|e| format!("folder sync task failed: {}", e))??;

    // External rescan reconciliation (lap-487 / spec A8): re-associate develop
    // recipes with the catalog after external moves/changes. Non-fatal:
    // findings (missing media, uncataloged media, ambiguous matches) are
    // reported through the log so they stay visible.
    let reconcile = tauri::async_runtime::spawn_blocking(
        move || -> Result<lap_lib::develop::asset_operations::AssetReconcileSummary, String> {
            let conn = t_sqlite::open_conn().map_err(|e| e.to_string())?;
            let repo = lap_lib::develop::RecipeRepository::lap_default();
            Ok(lap_lib::develop::asset_operations::reconcile_folder(
                &conn,
                &repo,
                Path::new(&reconcile_folder_path),
            ))
        },
    )
    .await;
    match reconcile {
        Ok(Ok(summary)) => {
            if summary.sidecars_seen > 0
                || !summary.findings.is_empty()
                || !summary.errors.is_empty()
            {
                println!(
                    "develop recipe rescan reconciliation for '{}': {} sidecars, {} projected, {} findings, {} errors",
                    reconciled_folder_for_log,
                    summary.sidecars_seen,
                    summary.projected,
                    summary.findings.len(),
                    summary.errors.len()
                );
            }
            for (path, finding) in &summary.findings {
                eprintln!(
                    "develop recipe rescan finding at {}: {finding:?}",
                    path.display()
                );
            }
            for (path, error) in &summary.errors {
                eprintln!("develop recipe rescan error at {}: {error}", path.display());
            }
        }
        Ok(Err(error)) => {
            eprintln!("develop recipe rescan reconciliation failed: {error}")
        }
        Err(error) => {
            eprintln!("develop recipe rescan reconciliation task failed: {error}")
        }
    }

    // Virtual-copy sidecars (lap-952) reconcile alongside the primary ones.
    let variant_reconcile_folder = reconciled_folder_for_log.clone();
    let variant_reconcile = tauri::async_runtime::spawn_blocking(
        move || -> lap_lib::develop::variants::VariantsReconcileSummary {
            let conn = t_sqlite::open_conn();
            match conn {
                Ok(conn) => {
                    let repo = lap_lib::develop::RecipeRepository::lap_default();
                    lap_lib::develop::variants::reconcile_folder(
                        &conn,
                        &repo,
                        Path::new(&variant_reconcile_folder),
                    )
                }
                Err(e) => {
                    eprintln!("develop virtual-copy reconciliation skipped: {e}");
                    lap_lib::develop::variants::VariantsReconcileSummary::default()
                }
            }
        },
    )
    .await;
    if let Ok(summary) = variant_reconcile {
        if summary.sidecars_seen > 0 || summary.removed_missing > 0 || !summary.errors.is_empty() {
            println!(
                "develop virtual-copy rescan reconciliation for '{}': {} sidecars, {} projected, {} removed, {} errors",
                reconciled_folder_for_log,
                summary.sidecars_seen,
                summary.projected,
                summary.removed_missing,
                summary.errors.len()
            );
        }
        for (path, error) in &summary.errors {
            eprintln!(
                "develop virtual-copy rescan error at {}: {error}",
                path.display()
            );
        }
    }

    if !result.folder_path_migrations.is_empty() {
        let _ = app_handle.emit(
            "album-folder-paths-migrated",
            &result.folder_path_migrations,
        );
    }
    Ok(result)
}

/// Refresh the subfolder tree below a folder without scanning its files.
#[tauri::command]
pub async fn refresh_album_subfolders(
    app_handle: tauri::AppHandle,
    album_id: i64,
    folder_path: String,
) -> Result<(), String> {
    let migrations = tauri::async_runtime::spawn_blocking(move || {
        t_utils::refresh_album_subfolders(album_id, &folder_path)
    })
    .await
    .map_err(|error| format!("Subfolder refresh task failed: {error}"))??;
    if !migrations.is_empty() {
        let _ = app_handle.emit("album-folder-paths-migrated", &migrations);
    }
    Ok(())
}

#[tauri::command]
pub fn is_directory_accessible(path: &str) -> bool {
    t_utils::directory_accessible(path)
}

/// get the thumbnail count of the folder
#[tauri::command]
pub fn get_folder_thumb_count(file_type: i64, folder_id: i64) -> i64 {
    AThumb::get_folder_thumb_count(file_type, folder_id).unwrap_or_default()
}

/// edit an image
#[tauri::command]
pub async fn edit_image(params: t_image::EditParams) -> Result<bool, String> {
    Ok(t_image::edit_image(params).await)
}

/// copy an edited image to clipboard
#[tauri::command]
pub async fn copy_edited_image(params: t_image::EditParams) -> Result<bool, String> {
    Ok(t_image::copy_edited_image_to_clipboard(params).await)
}

/// Copy up to 10 content items to the clipboard (a paired item has two files).
#[tauri::command]
pub async fn copy_images(
    app_handle: tauri::AppHandle,
    file_paths: Vec<String>,
) -> Result<usize, String> {
    t_image::copy_files_to_clipboard(&app_handle, file_paths).await
}

/// rename a file
#[tauri::command]
pub fn rename_file(file_id: i64, file_path: &str, new_name: &str) -> Option<String> {
    let sidecar_rename_plan = build_apple_sidecar_rename_plan(file_id, file_path, new_name).ok()?;
    if !preflight_rename_plan(file_path, new_name, &sidecar_rename_plan) {
        return None;
    }
    let original_db_names = collect_original_rename_db_names(file_id, &sidecar_rename_plan);

    // Develop recipe companions (lap-487): each group member's sidecar is
    // renamed alongside the member so the recipe keeps its identity.
    let primary_member_target = Path::new(file_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(new_name);
    let mut develop_members = vec![(PathBuf::from(file_path), primary_member_target)];
    for plan in &sidecar_rename_plan {
        develop_members.push((plan.old_path.clone(), plan.new_path.clone()));
    }
    let develop_moves = develop_companion_moves(&develop_members);

    let mut renamed_sidecars: Vec<(PathBuf, PathBuf)> = Vec::new();
    for plan in &sidecar_rename_plan {
        if let Err(error) = fs::rename(&plan.old_path, &plan.new_path) {
            eprintln!(
                "Failed to rename sidecar '{}' to '{}': {}",
                plan.old_path.display(),
                plan.new_path.display(),
                error
            );
            rollback_renamed_sidecars(renamed_sidecars);
            return None;
        }
        renamed_sidecars.push((plan.old_path.clone(), plan.new_path.clone()));
    }

    let mut develop_journal = asset_ops::OperationJournal::new();
    if let Err(error) = asset_ops::execute_companion_moves(&mut develop_journal, &develop_moves) {
        eprintln!("Failed to rename develop recipe sidecar: {error}");
        rollback_renamed_sidecars(renamed_sidecars);
        return None;
    }

    match t_utils::rename_file(file_path, new_name) {
        Some(new_file_path) => {
            let mut db_updates = vec![(
                file_id,
                new_name.to_string(),
                Some(t_utils::natural_sort_key(&new_name.to_lowercase())),
            )];
            for plan in &sidecar_rename_plan {
                if let Some(sidecar_file_id) = plan.file_id {
                    db_updates.push((
                        sidecar_file_id,
                        plan.new_name.clone(),
                        Some(t_utils::natural_sort_key(&plan.new_name.to_lowercase())),
                    ));
                }
            }
            if let Err(e) = AFile::batch_update_names(&db_updates) {
                eprintln!("Error while renaming file group in DB: {}", e);
                rollback_develop_journal(develop_journal, "rename_file");
                rollback_rename_changes(
                    file_path,
                    Some(&new_file_path),
                    renamed_sidecars,
                    &original_db_names,
                );
                return None;
            }
            for companion in &develop_moves {
                migrate_develop_projection_logged(&companion.from, &companion.to);
            }
            let _ = develop_journal.commit();
            Some(new_file_path)
        }
        None => {
            rollback_develop_journal(develop_journal, "rename_file");
            rollback_renamed_sidecars(renamed_sidecars);
            None
        }
    }
}

/// move a file to dest folder
#[tauri::command]
pub fn move_file(
    file_id: i64,
    file_path: &str,
    new_folder_id: i64,
    new_folder_path: &str,
    conflict_policy: &str,
) -> Result<String, String> {
    let old_file_info = AFile::get_file_info(file_id).ok().flatten();
    let old_album_id = old_file_info.as_ref().and_then(|file| file.album_id);
    let new_album_id = AFolder::get_by_id(new_folder_id)
        .ok()
        .flatten()
        .map(|folder| folder.album_id);
    let policy = t_utils::FileConflictPolicy::from_str(conflict_policy);
    let (primary_target, sidecar_plans) =
        resolve_group_primary_target(Some(file_id), file_path, new_folder_path, policy)?;
    let mut replaced_file_ids = Vec::new();
    let mut source_file_ids = HashSet::from([file_id]);
    for plan in &sidecar_plans {
        if let Some(component_id) = plan.file_id {
            source_file_ids.insert(component_id);
        }
    }
    if policy == t_utils::FileConflictPolicy::Replace {
        collect_replaced_file_ids_for_targets(
            new_folder_id,
            std::iter::once(&primary_target).chain(sidecar_plans.iter().map(|plan| &plan.new_path)),
            &source_file_ids,
            &mut replaced_file_ids,
        );
    }

    // Develop recipe companions (lap-487): each member's sidecar follows the
    // member to the resolved destination.
    let mut develop_members = vec![(PathBuf::from(file_path), primary_target.clone())];
    for plan in &sidecar_plans {
        develop_members.push((plan.old_path.clone(), plan.new_path.clone()));
    }
    let develop_moves = develop_companion_moves(&develop_members);
    let mut develop_journal = asset_ops::OperationJournal::new();
    if policy == t_utils::FileConflictPolicy::Replace {
        // A replaced destination's recipes are destroyed with the media they
        // belong to; staged through the journal so a failed operation
        // restores them.
        for companion in &develop_moves {
            if companion.to.exists()
                && let Err(error) = develop_journal.remove_file(&companion.to)
            {
                rollback_develop_journal(develop_journal, "move_file");
                return Err(error.to_string());
            }
        }
    }

    let mut component_transfers = Vec::new();
    for plan in &sidecar_plans {
        let source_path = plan.old_path.to_string_lossy().into_owned();
        match t_utils::move_file_to_path_with_policy(&source_path, &plan.new_path, policy) {
            Ok(transfer) => {
                component_transfers.push((plan.file_id, plan.old_path.clone(), transfer))
            }
            Err(error) => {
                rollback_develop_journal(develop_journal, "move_file");
                for (_, original_path, transfer) in component_transfers {
                    let _ = transfer.rollback_move(&original_path);
                }
                return Err(error);
            }
        }
    }

    if let Err(error) = asset_ops::execute_companion_moves(&mut develop_journal, &develop_moves) {
        rollback_develop_journal(develop_journal, "move_file");
        for (_, original_path, transfer) in component_transfers {
            let _ = transfer.rollback_move(&original_path);
        }
        return Err(error.to_string());
    }

    let transfer = match t_utils::move_file_to_path_with_policy(file_path, &primary_target, policy)
    {
        Ok(transfer) => transfer,
        Err(error) => {
            rollback_develop_journal(develop_journal, "move_file");
            for (_, original_path, transfer) in component_transfers {
                let _ = transfer.rollback_move(&original_path);
            }
            return Err(error);
        }
    };

    let component_file_ids = component_transfers
        .iter()
        .filter_map(|(component_id, _, _)| *component_id)
        .collect::<Vec<_>>();
    if let Err(error) = AFile::update_moved_file_group(
        file_id,
        &component_file_ids,
        &replaced_file_ids,
        new_folder_id,
    ) {
        let rollback_error = transfer.rollback_move(Path::new(file_path)).err();
        rollback_develop_journal(develop_journal, "move_file");
        for (_, original_path, transfer) in component_transfers {
            let _ = transfer.rollback_move(&original_path);
        }
        return Err(match rollback_error {
            Some(rollback_error) => format!(
                "Error while moving file group in DB: {}; rollback also failed: {}",
                error, rollback_error
            ),
            None => format!("Error while moving file group in DB: {}", error),
        });
    }

    for companion in &develop_moves {
        migrate_develop_projection_logged(&companion.from, &companion.to);
    }
    let _ = develop_journal.commit();

    if let (Some(old_album_id), Some(new_album_id)) = (old_album_id, new_album_id) {
        let _ = AThumb::relocate_for_file(file_id, old_album_id, new_album_id)
            .map_err(|e| format!("Error while relocating thumbnail cache: {}", e));
    }
    for (component_id, _, transfer) in component_transfers {
        if let Some(component_id) = component_id {
            if let (Some(old_album_id), Some(new_album_id)) = (old_album_id, new_album_id) {
                let _ = AThumb::relocate_for_file(component_id, old_album_id, new_album_id)
                    .map_err(|e| {
                        format!("Error while relocating Live Photo thumbnail cache: {}", e)
                    });
            }
        }
        transfer.finalize()?;
    }
    transfer.finalize()
}

/// move a file outside the library and remove its database record
#[tauri::command]
pub fn move_file_outside_library(
    file_id: i64,
    file_path: &str,
    new_folder_path: &str,
    conflict_policy: &str,
) -> Result<String, String> {
    let policy = t_utils::FileConflictPolicy::from_str(conflict_policy);
    let (primary_target, sidecar_plans) =
        resolve_group_primary_target(Some(file_id), file_path, new_folder_path, policy)?;

    // Develop recipe companions (lap-487): the sidecars travel with the group
    // even though the media leaves the catalog.
    let mut develop_members = vec![(PathBuf::from(file_path), primary_target.clone())];
    for plan in &sidecar_plans {
        develop_members.push((plan.old_path.clone(), plan.new_path.clone()));
    }
    let develop_moves = develop_companion_moves(&develop_members);

    let mut component_transfers = Vec::new();
    for plan in &sidecar_plans {
        let source_path = plan.old_path.to_string_lossy().into_owned();
        match t_utils::move_file_to_path_with_policy(&source_path, &plan.new_path, policy) {
            Ok(transfer) => {
                component_transfers.push((plan.file_id, plan.old_path.clone(), transfer))
            }
            Err(error) => {
                for (_, original_path, transfer) in component_transfers {
                    let _ = transfer.rollback_move(&original_path);
                }
                return Err(error);
            }
        }
    }

    let mut develop_journal = asset_ops::OperationJournal::new();
    if let Err(error) = asset_ops::execute_companion_moves(&mut develop_journal, &develop_moves) {
        rollback_develop_journal(develop_journal, "move_file_outside_library");
        for (_, original_path, transfer) in component_transfers {
            let _ = transfer.rollback_move(&original_path);
        }
        return Err(error.to_string());
    }

    let transfer = match t_utils::move_file_to_path_with_policy(file_path, &primary_target, policy)
    {
        Ok(transfer) => transfer,
        Err(error) => {
            rollback_develop_journal(develop_journal, "move_file_outside_library");
            for (_, original_path, transfer) in component_transfers {
                let _ = transfer.rollback_move(&original_path);
            }
            return Err(error);
        }
    };

    let mut delete_ids = vec![file_id];
    for (component_id, _, _) in &component_transfers {
        if let Some(component_id) = component_id {
            delete_ids.push(*component_id);
        }
    }

    if let Err(error) = AFile::batch_delete(&delete_ids) {
        let rollback_error = transfer.rollback_move(Path::new(file_path)).err();
        rollback_develop_journal(develop_journal, "move_file_outside_library");
        for (_, original_path, transfer) in component_transfers {
            let _ = transfer.rollback_move(&original_path);
        }
        return Err(match rollback_error {
            Some(rollback_error) => format!(
                "Error while removing file from DB: {}; rollback also failed: {}",
                error, rollback_error
            ),
            None => format!("Error while removing file from DB: {}", error),
        });
    }

    // The recipes left the catalog together with their media (lap-487).
    let moved_out_sidecars: Vec<PathBuf> = develop_moves.iter().map(|m| m.from.clone()).collect();
    delete_develop_projection_logged(&moved_out_sidecars);
    let _ = develop_journal.commit();

    for (_, _, transfer) in component_transfers {
        transfer.finalize()?;
    }
    transfer.finalize()
}

/// copy a file to dest folder
#[tauri::command]
pub fn copy_file(
    file_path: &str,
    new_folder_path: &str,
    conflict_policy: &str,
) -> Result<String, String> {
    let policy = t_utils::FileConflictPolicy::from_str(conflict_policy);
    let (primary_target, sidecar_plans) =
        resolve_group_primary_target(None, file_path, new_folder_path, policy)?;

    // Develop recipe companions (lap-487): a copy creates a NEW asset
    // identity carrying the same initial recipe; the original sidecar is
    // never touched.
    let mut develop_members = vec![(PathBuf::from(file_path), primary_target.clone())];
    for plan in &sidecar_plans {
        develop_members.push((plan.old_path.clone(), plan.new_path.clone()));
    }
    let mut replace_journal = asset_ops::OperationJournal::new();
    let mut forked_sidecars: Vec<PathBuf> = Vec::new();

    let mut sidecar_transfers = Vec::new();
    for plan in &sidecar_plans {
        let source_path = plan.old_path.to_string_lossy().into_owned();
        match t_utils::copy_file_to_path_with_policy(&source_path, &plan.new_path, policy) {
            Ok(transfer) => sidecar_transfers.push(transfer),
            Err(error) => {
                rollback_copied_transfers(sidecar_transfers);
                return Err(error);
            }
        }
    }

    for (source_member, copied_member) in &develop_members {
        if policy == t_utils::FileConflictPolicy::Replace {
            // A replaced destination's recipes are destroyed with the media
            // they belong to; staged so a failed copy restores them.
            let destination_sidecar = asset_ops::sidecar_path_for(copied_member);
            if destination_sidecar.exists() {
                if let Err(error) = replace_journal.remove_file(&destination_sidecar) {
                    rollback_develop_journal(replace_journal, "copy_file");
                    for path in &forked_sidecars {
                        let _ = fs::remove_file(path);
                    }
                    rollback_copied_transfers(sidecar_transfers);
                    return Err(error.to_string());
                }
            }
        }
        match asset_ops::fork_sidecar_for_copy(
            source_member,
            copied_member,
            &asset_ops::new_copy_asset_id(),
        ) {
            Ok(Some(forked)) => forked_sidecars.push(forked.sidecar_path),
            Ok(None) => {}
            Err(error) => {
                rollback_develop_journal(replace_journal, "copy_file");
                for path in &forked_sidecars {
                    let _ = fs::remove_file(path);
                }
                rollback_copied_transfers(sidecar_transfers);
                return Err(error.to_string());
            }
        }
    }

    let transfer = match t_utils::copy_file_to_path_with_policy(file_path, &primary_target, policy)
    {
        Ok(transfer) => transfer,
        Err(error) => {
            rollback_develop_journal(replace_journal, "copy_file");
            for path in &forked_sidecars {
                let _ = fs::remove_file(path);
            }
            rollback_copied_transfers(sidecar_transfers);
            return Err(error);
        }
    };
    let _ = replace_journal.commit();

    let copied_file_path = match transfer.finalize() {
        Ok(path) => path,
        Err(error) => {
            for path in &forked_sidecars {
                let _ = fs::remove_file(path);
            }
            rollback_copied_transfers(sidecar_transfers);
            return Err(error);
        }
    };
    for transfer in sidecar_transfers {
        transfer.finalize()?;
    }
    Ok(copied_file_path)
}

/// import a file into a folder preserving the original file name
#[tauri::command]
pub fn import_file(
    file_path: &str,
    folder_id: i64,
    folder_path: &str,
) -> Result<Option<AFile>, String> {
    // Validate the source is a supported type *before* copying.
    t_utils::get_file_type(file_path)
        .ok_or_else(|| format!("Unsupported file type: {}", file_path))?;

    let new_path = t_utils::import_file(file_path, folder_path)
        .ok_or_else(|| format!("Failed to copy file: {}", file_path))?;
    let file_type = t_utils::get_file_type(&new_path).ok_or_else(|| {
        // The renamed file should have a valid extension; if not, remove
        // the orphan so the album folder stays clean.
        let _ = std::fs::remove_file(&new_path);
        format!("Unsupported file type after copy: {}", new_path)
    })?;
    let now = chrono::Utc::now().timestamp_millis();
    let (file, _) = AFile::add_to_db(folder_id, &new_path, file_type, now)?;
    Ok(Some(file))
}

/// Import media from a folder and organize it below the album root by date.
#[tauri::command]
pub fn import_and_organize(
    app_handle: AppHandle,
    state: State<ImportCancellation>,
    album_id: i64,
    source_path: String,
    destination_path: String,
    layout: String,
    completed_paths: Vec<String>,
) -> Result<(), String> {
    {
        let mut import = state
            .0
            .lock()
            .map_err(|_| "Import cancellation state is unavailable")?;
        if import.running {
            return Err("An import is already in progress".to_string());
        }
        import.cancelled = false;
        import.running = true;
    }
    let cancellation = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = t_utils::import_and_organize(
            album_id,
            &source_path,
            &destination_path,
            &layout,
            completed_paths.into_iter().collect(),
            |progress| {
                let _ = app_handle.emit("import-organize-progress", progress);
            },
            || {
                cancellation
                    .lock()
                    .map(|import| import.cancelled)
                    .unwrap_or(true)
            },
        );
        if let Ok(mut import) = cancellation.lock() {
            import.running = false;
        }
        let payload = match result {
            Ok(result) => ImportOrganizeFinished {
                result: Some(result),
                error: None,
            },
            Err(error) => ImportOrganizeFinished {
                result: None,
                error: Some(error),
            },
        };
        let _ = app_handle.emit("import-organize-finished", payload);
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_import_and_organize(state: State<ImportCancellation>) -> Result<(), String> {
    state
        .0
        .lock()
        .map_err(|_| "Import cancellation state is unavailable")?
        .cancelled = true;
    Ok(())
}

/// import an image from a URL into a folder preserving the original file name when possible
#[tauri::command]
pub async fn import_url(
    url: &str,
    folder_id: i64,
    folder_path: String,
) -> Result<Option<AFile>, String> {
    import_url_inner(url, folder_id, folder_path).await
}

/// Import an image from the macOS drag pasteboard (for browser-sourced drags
/// where Tauri cannot provide file paths).
#[tauri::command]
pub async fn import_from_drag(
    folder_id: i64,
    folder_path: String,
) -> Result<Option<AFile>, String> {
    let url = crate::t_pasteboard::get_drag_image_url()
        .ok_or_else(|| "No image URL found in drag pasteboard".to_string())?;
    import_url_inner(&url, folder_id, folder_path).await
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DragPayload {
    pub file_paths: Vec<String>,
    pub url: Option<String>,
}

#[tauri::command]
pub fn get_drag_payload() -> DragPayload {
    DragPayload {
        file_paths: crate::t_pasteboard::get_drag_file_paths(),
        url: crate::t_pasteboard::get_drag_image_url(),
    }
}

async fn import_url_inner(
    url: &str,
    folder_id: i64,
    folder_path: String,
) -> Result<Option<AFile>, String> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("Failed to download image: {}", e))?;

    // Reject HTTP error statuses
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "Server returned {} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        ));
    }

    // Require a supported image content type â€” validate via the shared
    // MIMEâ†’extension table so the response form the importer can name.
    let mime = {
        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| "Response missing Content-Type header".to_string())?;
        let m = ct.split(';').next().unwrap_or(ct).trim().to_string();
        t_utils::image_mime_to_ext(&m).ok_or_else(|| format!("Unsupported image format: {}", m))?;
        m
    };
    let original_name = response
        .headers()
        .get("content-disposition")
        .and_then(|value| value.to_str().ok())
        .and_then(filename_from_content_disposition)
        .or_else(|| filename_from_url(response.url().as_str()))
        .or_else(|| filename_from_url(url));

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;

    let dest_folder = folder_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let new_path = original_name
            .and_then(|name| {
                t_utils::save_downloaded_bytes_with_name(&bytes, &mime, &name, &dest_folder)
            })
            .or_else(|| t_utils::save_bytes_to_folder(&bytes, &mime, &dest_folder))
            .ok_or_else(|| "Failed to save downloaded image".to_string())?;
        let file_type = t_utils::get_file_type(&new_path)
            .ok_or_else(|| format!("Unsupported file type: {}", new_path))?;
        let now = chrono::Utc::now().timestamp_millis();
        let (file, _) = AFile::add_to_db(folder_id, &new_path, file_type, now)?;
        Ok(Some(file))
    })
    .await
    .map_err(|e| format!("Failed to save file: {}", e))?
}

fn filename_from_content_disposition(value: &str) -> Option<String> {
    for part in value.split(';') {
        if let Some((key, raw_value)) = part.trim().split_once('=') {
            if key.trim().eq_ignore_ascii_case("filename*") {
                let raw_value = raw_value.trim().trim_matches('"');
                let encoded = raw_value
                    .split_once("''")
                    .map_or(raw_value, |(_, value)| value);
                return clean_import_filename(&percent_decode_utf8(encoded));
            }
        }
    }

    for part in value.split(';') {
        if let Some((key, raw_value)) = part.trim().split_once('=') {
            if key.trim().eq_ignore_ascii_case("filename") {
                return clean_import_filename(raw_value.trim().trim_matches('"'));
            }
        }
    }

    None
}

fn filename_from_url(url: &str) -> Option<String> {
    let without_query = url.split('?').next().unwrap_or(url);
    let without_fragment = without_query.split('#').next().unwrap_or(without_query);
    let last_segment = without_fragment.trim_end_matches('/').rsplit('/').next()?;
    clean_import_filename(&percent_decode_utf8(last_segment))
}

fn clean_import_filename(name: &str) -> Option<String> {
    let normalized = name.replace('\\', "/");
    let filename = Path::new(&normalized)
        .file_name()
        .and_then(|item| item.to_str())?
        .trim();
    if filename.is_empty() || filename.contains('\0') {
        None
    } else {
        Some(filename.to_string())
    }
}

fn percent_decode_utf8(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                decoded.push(hex);
                index += 3;
                continue;
            }
        }

        decoded.push(bytes[index]);
        index += 1;
    }

    String::from_utf8(decoded).unwrap_or_else(|_| value.to_string())
}

/// Import a file from raw bytes (used by DOM-based drag-drop on Windows
/// where Tauri native file paths are unavailable).
#[tauri::command]
pub async fn import_file_bytes(
    bytes: Vec<u8>,
    name: String,
    folder_id: i64,
    folder_path: String,
) -> Result<Option<AFile>, String> {
    if bytes.is_empty() {
        return Err("Dropped file is empty".to_string());
    }
    let dest_folder = folder_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let new_path = t_utils::save_bytes_with_name(&bytes, &name, &dest_folder)
            .ok_or_else(|| "Failed to save dropped file".to_string())?;
        let file_type = t_utils::get_file_type(&new_path).ok_or_else(|| {
            let _ = std::fs::remove_file(&new_path);
            format!("Unsupported file type: {}", new_path)
        })?;
        let now = chrono::Utc::now().timestamp_millis();
        let (file, _) = AFile::add_to_db(folder_id, &new_path, file_type, now)?;
        Ok(Some(file))
    })
    .await
    .map_err(|e| format!("Failed to save file: {}", e))?
}

fn import_clipboard_file(
    file_path: &Path,
    folder_id: i64,
    folder_path: &str,
) -> Result<AFile, String> {
    let source = file_path
        .to_str()
        .ok_or_else(|| format!("Invalid clipboard file path: {}", file_path.display()))?;
    t_utils::get_file_type(source)
        .ok_or_else(|| format!("Unsupported file type: {}", file_path.display()))?;
    let new_path = t_utils::import_file(source, folder_path)
        .ok_or_else(|| format!("Failed to copy file: {}", file_path.display()))?;
    let file_type = t_utils::get_file_type(&new_path).ok_or_else(|| {
        let _ = std::fs::remove_file(&new_path);
        format!("Unsupported file type after copy: {}", new_path)
    })?;
    let now = chrono::Utc::now().timestamp_millis();
    match AFile::add_to_db(folder_id, &new_path, file_type, now) {
        Ok((file, _)) => Ok(file),
        Err(error) => {
            let _ = std::fs::remove_file(&new_path);
            Err(error)
        }
    }
}

#[tauri::command]
pub async fn has_importable_clipboard(_app_handle: tauri::AppHandle) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Ok(clipboard_data) = crate::t_pasteboard::get_clipboard_import_data(&_app_handle).await
        else {
            return false;
        };

        if clipboard_data.file_paths.iter().any(|path| {
            std::path::Path::new(path).is_file() && t_utils::get_file_type(path).is_some()
        }) {
            return true;
        }

        if clipboard_data.png.is_some() {
            return true;
        }
    }

    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        return false;
    };

    if let Ok(file_paths) = clipboard.get().file_list() {
        if file_paths
            .iter()
            .any(|path| path.is_file() && path.to_str().and_then(t_utils::get_file_type).is_some())
        {
            return true;
        }
    }

    clipboard.get_image().is_ok()
}

/// Import copied image files while preserving their original format and
/// metadata. Fall back to a PNG only when the clipboard contains pixels
/// without a backing file, such as a screenshot.
#[tauri::command]
pub async fn import_clipboard(
    _app_handle: tauri::AppHandle,
    folder_id: i64,
    folder_path: &str,
) -> Result<Vec<AFile>, String> {
    #[cfg(target_os = "linux")]
    {
        let clipboard_data = crate::t_pasteboard::get_clipboard_import_data(&_app_handle).await?;
        if !clipboard_data.file_paths.is_empty() {
            let mut imported = Vec::new();
            for path in clipboard_data
                .file_paths
                .iter()
                .map(std::path::Path::new)
                .filter(|path| path.is_file())
            {
                let supported = path.to_str().and_then(t_utils::get_file_type).is_some();
                if !supported {
                    continue;
                }
                match import_clipboard_file(path, folder_id, folder_path) {
                    Ok(file) => imported.push(file),
                    Err(error) => eprintln!(
                        "Failed to import clipboard file {}: {}",
                        path.display(),
                        error
                    ),
                }
            }
            if !imported.is_empty() {
                return Ok(imported);
            }
        }

        if let Some(png) = clipboard_data.png {
            let new_path = t_utils::save_bytes_to_folder(&png, "image/png", folder_path)
                .ok_or_else(|| "Failed to save clipboard image".to_string())?;
            let file_type = t_utils::get_file_type(&new_path).ok_or_else(|| {
                let _ = std::fs::remove_file(&new_path);
                format!("Unsupported file type: {}", new_path)
            })?;
            let now = chrono::Utc::now().timestamp_millis();
            return match AFile::add_to_db(folder_id, &new_path, file_type, now) {
                Ok((file, _)) => Ok(vec![file]),
                Err(error) => {
                    let _ = std::fs::remove_file(&new_path);
                    Err(error)
                }
            };
        }

        if !clipboard_data.file_paths.is_empty() {
            return Err("Clipboard does not contain supported image files".to_string());
        }
    }

    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("Failed to open clipboard: {}", e))?;
    if let Ok(file_paths) = clipboard.get().file_list() {
        if !file_paths.is_empty() {
            let mut imported = Vec::new();
            for path in file_paths.iter().filter(|path| path.is_file()) {
                let supported = path.to_str().and_then(t_utils::get_file_type).is_some();
                if !supported {
                    continue;
                }
                match import_clipboard_file(path, folder_id, folder_path) {
                    Ok(file) => imported.push(file),
                    Err(error) => eprintln!(
                        "Failed to import clipboard file {}: {}",
                        path.display(),
                        error
                    ),
                }
            }
            if !imported.is_empty() {
                return Ok(imported);
            }
            return Err("Clipboard does not contain supported image files".to_string());
        }
    }

    let clipboard_image = clipboard
        .get_image()
        .map_err(|e| format!("No image found in clipboard: {}", e))?;
    let width = u32::try_from(clipboard_image.width)
        .map_err(|_| "Clipboard image width is too large".to_string())?;
    let height = u32::try_from(clipboard_image.height)
        .map_err(|_| "Clipboard image height is too large".to_string())?;
    let rgba = image::RgbaImage::from_raw(width, height, clipboard_image.bytes.into_owned())
        .ok_or_else(|| "Clipboard image data is invalid".to_string())?;

    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .map_err(|e| format!("Failed to encode clipboard image: {}", e))?;
    let new_path = t_utils::save_bytes_to_folder(bytes.get_ref(), "image/png", folder_path)
        .ok_or_else(|| "Failed to save clipboard image".to_string())?;
    let file_type = t_utils::get_file_type(&new_path).ok_or_else(|| {
        let _ = std::fs::remove_file(&new_path);
        format!("Unsupported file type: {}", new_path)
    })?;
    let now = chrono::Utc::now().timestamp_millis();
    match AFile::add_to_db(folder_id, &new_path, file_type, now) {
        Ok((file, _)) => Ok(vec![file]),
        Err(error) => {
            let _ = std::fs::remove_file(&new_path);
            Err(error)
        }
    }
}

/// delete a file
#[tauri::command]
pub fn delete_file(file_id: i64, file_path: &str) -> Result<BatchDeleteResult, String> {
    delete_file_group(file_id, file_path, false)
}

/// delete a file permanently
#[tauri::command]
pub fn delete_file_permanently(file_id: i64, file_path: &str) -> Result<BatchDeleteResult, String> {
    delete_file_group(file_id, file_path, true)
}

fn delete_file_group(
    file_id: i64,
    file_path: &str,
    permanently: bool,
) -> Result<BatchDeleteResult, String> {
    let component_files = AFile::live_photo_component_files(file_id)?;
    let mut deleted_file_ids = Vec::with_capacity(component_files.len() + 1);
    let mut delete_errors = Vec::new();

    let primary_result = if permanently {
        t_utils::delete_file_permanently(file_path)
    } else {
        t_utils::trash_path(file_path)
    };
    if let Err(error) = primary_result {
        if permanently {
            return Err(error);
        }
        return Ok(BatchDeleteResult {
            failed_count: 1,
            deleted_file_ids: Vec::new(),
            trash_failed_file_ids: vec![file_id],
        });
    }
    deleted_file_ids.push(file_id);

    for component in &component_files {
        if let Some(path) = component.file_path.as_deref() {
            let result = if permanently {
                t_utils::delete_file_permanently(path)
            } else {
                t_utils::trash_path(path)
            };
            match result {
                Ok(_) => {
                    if let Some(id) = component.id {
                        deleted_file_ids.push(id);
                    }
                }
                Err(error) => delete_errors.push(format!(
                    "Failed to delete Live Photo sidecar '{}': {}",
                    path, error
                )),
            }
        }
    }

    if let Err(error) = delete_apple_aae_sidecars(file_path, permanently) {
        delete_errors.push(format!("Failed to delete Apple sidecar: {}", error));
    }

    // Develop recipe companions (lap-487): the sidecar goes to the same
    // trash so an OS restore brings the group back together. A failure keeps
    // the sidecar on disk (no data loss) and is reported explicitly; the
    // fingerprint check at session open prevents any later wrong association.
    let mut trashed_recipe_sidecars: Vec<PathBuf> = Vec::new();
    let mut recipe_members: Vec<&str> = vec![file_path];
    recipe_members.extend(
        component_files
            .iter()
            .filter_map(|c| c.file_path.as_deref()),
    );
    for member in recipe_members {
        let result = if permanently {
            asset_ops::delete_companion_permanently(Path::new(member))
        } else {
            asset_ops::trash_companion(Path::new(member))
        };
        match result {
            Ok(true) => {
                trashed_recipe_sidecars.push(asset_ops::sidecar_path_for(Path::new(member)))
            }
            Ok(false) => {}
            Err(error) => delete_errors.push(format!(
                "Failed to delete develop recipe sidecar: {}",
                error
            )),
        }
    }

    AFile::batch_delete(&deleted_file_ids)
        .map_err(|e| format!("Error while deleting removed files from DB: {}", e))?;

    // The trashed recipes are no longer part of the catalog (lap-487).
    delete_develop_projection_logged(&trashed_recipe_sidecars);

    let failed_count = usize::from(!delete_errors.is_empty());
    for error in delete_errors {
        eprintln!("{}", error);
    }
    Ok(BatchDeleteResult {
        failed_count,
        deleted_file_ids,
        trash_failed_file_ids: Vec::new(),
    })
}

/// delete a file from db
#[tauri::command]
pub fn delete_db_file(file_id: i64) -> Result<usize, String> {
    // delete the file from db
    AFile::delete(file_id).map_err(|e| format!("Error while deleting file from DB: {}", e))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchDeleteFile {
    pub file_id: i64,
    pub file_path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchDeleteResult {
    pub deleted_file_ids: Vec<i64>,
    pub failed_count: usize,
    pub trash_failed_file_ids: Vec<i64>,
}

#[tauri::command]
pub async fn batch_delete_files(
    files: Vec<BatchDeleteFile>,
    permanently: bool,
) -> Result<BatchDeleteResult, String> {
    tauri::async_runtime::spawn_blocking(move || delete_files_grouped(files, permanently))
        .await
        .map_err(|e| format!("Failed to run batch delete: {}", e))?
}

/// Delete files together with their Live Photo and Apple sidecar components.
/// Kept synchronous so other backend workflows can share the same semantics.
pub(crate) fn delete_files_grouped(
    files: Vec<BatchDeleteFile>,
    permanently: bool,
) -> Result<BatchDeleteResult, String> {
    struct DeleteGroup {
        primary_id: i64,
        primary_path: String,
        components: Vec<(i64, String)>,
        aae_sidecars: Vec<String>,
        /// Develop recipe sidecar paths to clean from the projection after a
        /// successful group delete (lap-487).
        recipe_sidecars: Vec<PathBuf>,
    }

    let mut delete_groups = Vec::with_capacity(files.len());
    let mut seen_ids = HashSet::new();
    let mut seen_aae_paths = HashSet::new();
    for file in &files {
        if !seen_ids.insert(file.file_id) {
            continue;
        }
        let mut component_targets = Vec::new();
        if let Ok(components) = AFile::live_photo_component_files(file.file_id) {
            for component in components {
                if let (Some(id), Some(path)) = (component.id, component.file_path) {
                    if seen_ids.insert(id) {
                        component_targets.push((id, path));
                    }
                }
            }
        }
        let mut aae_sidecars = Vec::new();
        for sidecar in apple_aae_sidecar_paths(&file.file_path) {
            let sidecar_path = sidecar.to_string_lossy().into_owned();
            if seen_aae_paths.insert(sidecar_path.to_ascii_lowercase()) {
                aae_sidecars.push(sidecar_path);
            }
        }
        delete_groups.push(DeleteGroup {
            primary_id: file.file_id,
            primary_path: file.file_path.clone(),
            components: component_targets,
            aae_sidecars,
            recipe_sidecars: Vec::new(),
        });
    }

    let mut deleted_file_ids = Vec::new();
    let mut failed_count = 0usize;
    let mut trash_failed_file_ids = Vec::new();
    let mut trashed_recipe_sidecars: Vec<PathBuf> = Vec::new();
    for group in &mut delete_groups {
        let result = if permanently {
            t_utils::delete_file_permanently(&group.primary_path)
        } else {
            t_utils::trash_path(&group.primary_path)
        };
        if result.is_err() {
            failed_count += 1;
            if !permanently {
                trash_failed_file_ids.push(group.primary_id);
            }
            continue;
        }
        deleted_file_ids.push(group.primary_id);
        let mut group_failed = false;

        for (file_id, file_path) in &group.components {
            let result = if permanently {
                t_utils::delete_file_permanently(file_path)
            } else {
                t_utils::trash_path(file_path)
            };
            if result.is_ok() {
                deleted_file_ids.push(*file_id);
            } else {
                group_failed = true;
                eprintln!("Failed to delete Live Photo sidecar: {}", file_path);
            }
        }

        for sidecar_path in &group.aae_sidecars {
            let result = if permanently {
                t_utils::delete_file_permanently(sidecar_path)
            } else {
                t_utils::trash_path(sidecar_path)
            };
            if result.is_err() {
                group_failed = true;
                eprintln!("Failed to delete Apple sidecar: {}", sidecar_path);
            }
        }

        // Develop recipe companions (lap-487): every member's sidecar goes to
        // the same trash so an OS restore brings the group back together.
        let mut recipe_members: Vec<&str> = vec![&group.primary_path];
        recipe_members.extend(group.components.iter().map(|(_, path)| path.as_str()));
        for member in recipe_members {
            let result = if permanently {
                asset_ops::delete_companion_permanently(Path::new(member))
            } else {
                asset_ops::trash_companion(Path::new(member))
            };
            match result {
                Ok(true) => group
                    .recipe_sidecars
                    .push(asset_ops::sidecar_path_for(Path::new(member))),
                Ok(false) => {}
                Err(error) => {
                    group_failed = true;
                    eprintln!("Failed to delete develop recipe sidecar: {}", error);
                }
            }
        }

        if group_failed {
            failed_count += 1;
        }
        trashed_recipe_sidecars.append(&mut group.recipe_sidecars);
    }

    AFile::batch_delete(&deleted_file_ids)
        .map_err(|e| format!("Error while deleting files from DB: {}", e))?;
    // The trashed recipes are no longer part of the catalog (lap-487).
    delete_develop_projection_logged(&trashed_recipe_sidecars);
    Ok(BatchDeleteResult {
        failed_count,
        deleted_file_ids,
        trash_failed_file_ids,
    })
}

/// edit a file's comment
#[tauri::command]
pub fn edit_file_comment(file_id: i64, comment: &str) -> Result<usize, String> {
    AFile::update_column(file_id, "comments", &comment)
        .map_err(|e| format!("Error while editing file comment: {}", e))
}

/// Remove unreferenced thumbnail cache files. When `library_id` is provided,
/// cleans that library's cache; otherwise falls back to the current library.
/// Runs on a blocking thread because it walks the cache directory and issues
/// many small `remove_file` calls â€” heavy IO must not block the main thread
/// (see project convention: async + spawn_blocking for heavy IO commands).
#[tauri::command]
pub async fn clean_unused_thumbnail_cache(
    library_id: Option<String>,
) -> Result<t_sqlite::ThumbnailCacheCleanupResult, String> {
    tauri::async_runtime::spawn_blocking(move || AThumb::clean_unused_cache(library_id.as_deref()))
        .await
        .map_err(|e| format!("Failed to join clean thumbnail cache task: {}", e))?
}

/// get a file's thumb image, if not exist, create a new one.
/// A current developed derivative for the asset's acknowledged commit takes
/// precedence (lap-a58: the catalog prefers developed derivatives).
#[tauri::command]
pub async fn get_file_thumb(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, DevelopAppState>,
    file_id: i64,
    file_path: &str,
    file_type: i64,
    orientation: i32,
    thumbnail_size: u32,
    raw_thumbnail_source: String,
    force_regenerate: bool,
    thumbnail_seek_percent: Option<u8>,
) -> Result<Option<AThumb>, String> {
    if let Some(developed) = developed_thumbnail_override(state.inner(), file_id, thumbnail_size) {
        return Ok(Some(developed));
    }

    if let Some(thumb) = AThumb::get_thumb_if_available(
        file_id,
        file_path,
        thumbnail_size,
        orientation,
        raw_thumbnail_source == "embedded",
        force_regenerate,
    )
    .map_err(|e| format!("Error while getting thumbnail: {}", e))?
    {
        return Ok(Some(thumb));
    }

    let album_id = AFile::get_file_info(file_id)
        .map_err(|e| format!("Error while getting file info for thumbnail: {}", e))?
        .and_then(|file| file.album_id)
        .unwrap_or(0);

    AThumb::schedule_background_generation_for_library(
        app_handle,
        file_id,
        file_path.to_string(),
        file_type,
        orientation,
        thumbnail_size,
        raw_thumbnail_source == "embedded",
        album_id,
        force_regenerate,
        thumbnail_seek_percent,
    );

    Ok(None)
}

/// get a file's thumb image by id, if not exist, create a new one in background.
/// A current developed derivative for the asset's acknowledged commit takes
/// precedence (lap-a58).
#[tauri::command]
pub async fn get_file_thumb_by_id(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, DevelopAppState>,
    file_id: i64,
    thumbnail_size: u32,
    raw_thumbnail_source: String,
    force_regenerate: bool,
) -> Result<Option<AThumb>, String> {
    let Some(file) = AFile::get_file_info(file_id)
        .map_err(|e| format!("Error while getting file info for thumbnail: {}", e))?
    else {
        return Ok(None);
    };

    let Some(file_path) = file.file_path.clone() else {
        return Ok(None);
    };

    if let Some(developed) = developed_thumbnail_override(state.inner(), file_id, thumbnail_size) {
        return Ok(Some(developed));
    }

    let file_type = file.file_type.unwrap_or(0);
    let orientation = file.e_orientation.unwrap_or(1) as i32;

    if let Some(thumb) = AThumb::get_thumb_if_available(
        file_id,
        &file_path,
        thumbnail_size,
        orientation,
        raw_thumbnail_source == "embedded",
        force_regenerate,
    )
    .map_err(|e| format!("Error while getting thumbnail: {}", e))?
    {
        return Ok(Some(thumb));
    }

    AThumb::schedule_background_generation_for_library(
        app_handle,
        file_id,
        file_path,
        file_type,
        orientation,
        thumbnail_size,
        raw_thumbnail_source == "embedded",
        file.album_id.unwrap_or(0),
        force_regenerate,
        None,
    );

    Ok(None)
}

/// get multiple thumbnails in one IPC call; missing thumbnails are generated in background.
/// A current developed derivative takes precedence per file (lap-a58).
#[tauri::command]
pub async fn get_file_thumbs(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, DevelopAppState>,
    files: Vec<ThumbRequest>,
    thumbnail_size: u32,
    raw_thumbnail_source: String,
    force_regenerate: bool,
    trust_cached: bool,
) -> Result<Vec<Option<AThumb>>, String> {
    let mut thumbs = Vec::with_capacity(files.len());
    let file_ids: Vec<i64> = files
        .iter()
        .map(|request| request.file_id)
        .filter(|file_id| *file_id > 0)
        .collect();
    let mut fetched_thumbs = if force_regenerate {
        HashMap::new()
    } else {
        AThumb::fetch_many(&file_ids)
            .map_err(|e| format!("Error while fetching thumbnails: {}", e))?
    };

    for request in files {
        if request.file_id <= 0 {
            thumbs.push(None);
            continue;
        }

        if let Some(developed) =
            developed_thumbnail_override(state.inner(), request.file_id, thumbnail_size)
        {
            thumbs.push(Some(developed));
            continue;
        }

        let mut file_path = request.file_path;
        let mut file_type = request.file_type.unwrap_or(0);
        let mut orientation = request.orientation.unwrap_or(1);
        let mut album_id = request.album_id.unwrap_or(0);

        if file_path.is_none()
            || request.file_type.is_none()
            || request.orientation.is_none()
            || album_id <= 0
        {
            if let Some(file) = AFile::get_file_info(request.file_id)
                .map_err(|e| format!("Error while getting file info for thumbnail: {}", e))?
            {
                if file_path.is_none() {
                    file_path = file.file_path;
                }
                if request.file_type.is_none() {
                    file_type = file.file_type.unwrap_or(0);
                }
                if request.orientation.is_none() {
                    orientation = file.e_orientation.unwrap_or(1) as i32;
                }
                if album_id <= 0 {
                    album_id = file.album_id.unwrap_or(0);
                }
            }
        }

        let Some(file_path) = file_path else {
            thumbs.push(None);
            continue;
        };

        if let Some(fetched_thumb) = fetched_thumbs.remove(&request.file_id) {
            if let Some(thumb) = AThumb::resolve_fetched_thumb_if_available(
                fetched_thumb,
                &file_path,
                thumbnail_size,
                orientation,
                raw_thumbnail_source == "embedded",
                force_regenerate,
                trust_cached,
            )
            .map_err(|e| format!("Error while getting thumbnail: {}", e))?
            {
                thumbs.push(Some(thumb));
                continue;
            }
        }

        AThumb::schedule_background_generation_for_library(
            app_handle.clone(),
            request.file_id,
            file_path,
            file_type,
            orientation,
            thumbnail_size,
            raw_thumbnail_source == "embedded",
            album_id,
            force_regenerate,
            None,
        );

        thumbs.push(None);
    }

    Ok(thumbs)
}

/// get a file's info
#[tauri::command]
pub fn get_file_info(file_id: i64) -> Result<Option<AFile>, String> {
    AFile::get_file_info(file_id).map_err(|e| format!("Error while getting file info: {}", e))
}

/// Extract the embedded MP4 from an Android Motion Photo into the cache and
/// return its path so the frontend can play it through the normal video
/// pipeline (`prepare_video`). Cached by file size + mtime so an edited file
/// invalidates its extracted copy.
#[tauri::command]
pub fn prepare_motion_photo_video(file_id: i64) -> Result<String, String> {
    let file = AFile::get_file_info(file_id)
        .map_err(|e| format!("Error while looking up file: {}", e))?
        .ok_or_else(|| "File not found in catalog".to_string())?;
    let file_path = file
        .file_path
        .as_deref()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| "File has no cataloged path".to_string())?;
    let stored_offset = file
        .motion_photo_offset
        .filter(|o| *o > 0)
        .ok_or_else(|| "File is not a Motion Photo".to_string())?;

    let metadata = fs::metadata(file_path).map_err(|e| format!("Failed to stat file: {}", e))?;
    let mtime_secs = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let library_id = t_config::load_app_config()?.current_library_id;
    let cache_dir = t_config::get_app_cache_dir()?
        .join(library_id)
        .join("motion");
    fs::create_dir_all(&cache_dir).map_err(|e| format!("Failed to create cache dir: {}", e))?;

    let version_prefix = format!("{}-{}-{}-", file_id, metadata.len(), mtime_secs);
    // Fast path: if the stored offset's cache entry already exists, return it
    // without re-detecting (which would read the whole file).
    let stored_cache = cache_dir.join(format!("{}{}.mp4", version_prefix, stored_offset));
    if stored_cache.exists() {
        return Ok(stored_cache.to_string_lossy().into_owned());
    }

    // Cache miss: re-evaluate at playback time so files indexed before an
    // improved detector use the authoritative XMP offset without a rescan.
    let offset = crate::t_motion_photo::detect_motion_photo(Path::new(file_path))
        .map(|offset| offset as i64)
        .unwrap_or(stored_offset);
    let offset_u64 = offset as u64;
    // Validate the offset still points at an MP4 box. An edited/replaced file is
    // no longer a motion photo, and extracting from a stale offset would produce
    // a bogus video.
    if offset_u64 >= metadata.len()
        || !crate::t_motion_photo::is_mp4_at_offset(Path::new(file_path), offset_u64)
    {
        return Err("File is not a Motion Photo".to_string());
    }
    if offset != stored_offset {
        AFile::update_column(file_id, "motion_photo_offset", &offset)
            .map_err(|e| format!("Failed to update Motion Photo offset: {}", e))?;
    }

    let cache_path = cache_dir.join(format!("{}{}.mp4", version_prefix, offset));
    if cache_path.exists() {
        return Ok(cache_path.to_string_lossy().into_owned());
    }
    extract_embedded_mp4(file_path, offset_u64, &cache_path)?;
    evict_stale_motion_videos(&cache_dir, file_id, &cache_path);
    Ok(cache_path.to_string_lossy().into_owned())
}

/// Drop cache entries superseded by `keep` (an updated offset or an earlier
/// version of the same file) so the cache does not grow unbounded.
fn evict_stale_motion_videos(cache_dir: &Path, file_id: i64, keep: &Path) {
    let file_prefix = format!("{}-", file_id);
    let Ok(entries) = fs::read_dir(cache_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let stale = path != keep
            && path.file_name().is_some_and(|n| {
                let n = n.to_string_lossy();
                // Never touch in-progress `.part` temp files of concurrent
                // extractions.
                n.starts_with(&file_prefix) && n.ends_with(".mp4")
            });
        if stale {
            let _ = fs::remove_file(path);
        }
    }
}

fn extract_embedded_mp4(source: &str, offset: u64, dest: &Path) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut src = fs::File::open(source).map_err(|e| format!("Failed to open source: {}", e))?;
    src.seek(SeekFrom::Start(offset))
        .map_err(|e| format!("Failed to seek to video offset: {}", e))?;
    let temp_path = dest.with_extension(format!("mp4-{}.part", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut dst = fs::File::create(&temp_path)
            .map_err(|e| format!("Failed to create cache file: {}", e))?;
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = src
                .read(&mut buf)
                .map_err(|e| format!("Failed to read source: {}", e))?;
            if n == 0 {
                break;
            }
            dst.write_all(&buf[..n])
                .map_err(|e| format!("Failed to write cache file: {}", e))?;
        }
        dst.sync_all()
            .map_err(|e| format!("Failed to finalize cache file: {}", e))?;
        match fs::rename(&temp_path, dest) {
            Ok(()) => Ok(()),
            // Another request may have completed the same cache entry first.
            Err(_) if dest.exists() => {
                let _ = fs::remove_file(&temp_path);
                Ok(())
            }
            Err(e) => Err(format!("Failed to publish cache file: {}", e)),
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

/// update a file's info
#[tauri::command]
pub fn update_file_info(file_id: i64, file_path: &str) -> Result<Option<AFile>, String> {
    let now = chrono::Utc::now().timestamp_millis();
    AFile::update_file_info(file_id, file_path, now)
        .map_err(|e| format!("Error while updating file info: {}", e))
}

/// add or refresh a file in db and return the indexed file info
#[tauri::command]
pub fn add_file_to_db(folder_id: i64, file_path: &str) -> Result<Option<AFile>, String> {
    let file_type = t_utils::get_file_type(file_path)
        .ok_or_else(|| format!("Unsupported file type: {}", file_path))?;
    let now = chrono::Utc::now().timestamp_millis();
    let (file, _) = AFile::add_to_db(folder_id, file_path, file_type, now)?;
    Ok(Some(file))
}

/// check if file exists
#[tauri::command]
pub fn check_file_exists(file_path: &str) -> bool {
    Path::new(file_path).exists()
}

/// set a file's rotate status
#[tauri::command]
pub fn set_file_rotate(file_id: i64, rotate: i32) -> Result<usize, String> {
    AFile::update_column(file_id, "rotate", &rotate)
        .map_err(|e| format!("Error while setting file rotate: {}", e))
}

/// get a file's has_tags status (true or false)
#[tauri::command]
pub fn get_file_has_tags(file_id: i64) -> Result<bool, String> {
    AFile::get_has_tags(file_id)
        .map_err(|e| format!("Error while getting file has_tags status: {}", e))
}

// favorite

/// get all favorite folders
#[tauri::command]
pub fn get_favorite_folders() -> Result<Vec<AFolder>, String> {
    AFolder::get_favorite_folders()
        .map_err(|e| format!("Error while getting favorite folders: {}", e))
}

/// get a folder's favorite status (true or false)
#[tauri::command]
pub fn get_folder_favorite(folder_path: &str) -> Result<bool, String> {
    let is_favorite_opt = AFolder::get_is_favorite(folder_path)
        .map_err(|e| format!("Error while getting folder favorite: {}", e))?;

    match is_favorite_opt {
        Some(val) => Ok(val),
        None => Ok(false), // Default to false if not found
    }
}

/// set a folder's favorite status (true or false)
#[tauri::command]
pub fn set_folder_favorite(folder_id: i64, is_favorite: bool) -> Result<usize, String> {
    AFolder::update_column(folder_id, "is_favorite", &is_favorite)
        .map_err(|e| format!("Error while setting folder favorite: {}", e))
}

/// get a folder's search exclusion status (true or false)
#[tauri::command]
pub fn get_folder_search_excluded(folder_path: &str) -> Result<bool, String> {
    let is_excluded_opt = AFolder::get_is_excluded_from_search(folder_path)
        .map_err(|e| format!("Error while getting folder search exclusion: {}", e))?;

    match is_excluded_opt {
        Some(is_excluded) => Ok(is_excluded),
        None => Ok(false),
    }
}

/// set a folder's search exclusion status (true or false)
#[tauri::command]
pub fn set_folder_search_excluded(
    album_id: i64,
    folder_path: &str,
    is_excluded: bool,
) -> Result<usize, String> {
    let folder = AFolder::add_to_db(album_id, folder_path)
        .map_err(|e| format!("Error while ensuring folder in DB: {}", e))?;
    let folder_id = folder
        .id
        .ok_or_else(|| "Folder was saved without an id".to_string())?;
    AFolder::update_column(folder_id, "is_excluded_from_search", &is_excluded)
        .map_err(|e| format!("Error while setting folder search exclusion: {}", e))
}

/// set a file's favorite status (true or false)
#[tauri::command]
pub fn set_file_favorite(file_id: i64, is_favorite: bool) -> Result<usize, String> {
    AFile::update_column(file_id, "is_favorite", &is_favorite)
        .map_err(|e| format!("Error while setting file favorite: {}", e))
}

/// set a file's rating (0-5)
#[tauri::command]
pub fn set_file_rating(file_id: i64, rating: i32) -> Result<usize, String> {
    let clamped = rating.clamp(0, 5);
    AFile::update_column(file_id, "rating", &clamped)
        .map_err(|e| format!("Error while setting file rating: {}", e))
}

/// Set a file's culling status (0: unreviewed, 1: pick, 2: reject).
#[tauri::command]
pub fn set_file_culling_flag(file_id: i64, culling_flag: i32) -> Result<usize, String> {
    let clamped = culling_flag.clamp(0, 2);
    AFile::update_column(file_id, "culling_flag", &clamped)
        .map_err(|e| format!("Error while setting file culling flag: {}", e))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchFileMetadataUpdate {
    pub file_ids: Vec<i64>,
    pub is_favorite: Option<bool>,
    pub rating: Option<i32>,
    pub culling_flag: Option<i32>,
    pub rotate_delta: Option<i32>,
    pub comment: Option<String>,
}

#[tauri::command]
pub fn batch_update_file_metadata(params: BatchFileMetadataUpdate) -> Result<usize, String> {
    AFile::batch_update_metadata(
        &params.file_ids,
        params.is_favorite,
        params.rating,
        params.culling_flag,
        params.rotate_delta,
        params.comment.as_deref(),
    )
    .map_err(|e| format!("Error while updating file metadata: {}", e))
}

// tag

#[tauri::command]
pub fn get_tag_group_name(id: i64) -> Result<String, String> {
    crate::t_tag_groups::get_name(id)
}

#[tauri::command]
pub fn get_tag_groups(
    small_file_filter: i64,
) -> Result<Vec<crate::t_tag_groups::TagGroup>, String> {
    crate::t_tag_groups::get_all(small_file_filter)
}

#[tauri::command]
pub fn save_tag_group(id: Option<i64>, name: &str) -> Result<i64, String> {
    crate::t_tag_groups::save(id, name)
}

#[tauri::command]
pub fn reorder_tag_groups(ids: Vec<i64>) -> Result<(), String> {
    crate::t_tag_groups::reorder(&ids)
}

#[tauri::command]
pub fn delete_tag_group(id: i64) -> Result<(), String> {
    crate::t_tag_groups::delete(id)
}

#[tauri::command]
pub fn move_tags_to_group(tag_ids: Vec<i64>, group_id: i64) -> Result<(), String> {
    crate::t_tag_groups::move_tags(&tag_ids, group_id)
}

/// get all tags
#[tauri::command]
pub fn get_all_tags(sort: i64, small_file_filter: i64) -> Result<Vec<ATag>, String> {
    ATag::get_all(sort, small_file_filter)
        .map_err(|e| format!("Error while getting all tags: {}", e))
}

#[tauri::command]
pub fn get_tag_counts(small_file_filter: i64) -> Result<HashMap<i64, i64>, String> {
    ATag::get_counts(small_file_filter)
        .map_err(|e| format!("Error while getting tag counts: {}", e))
}

/// get tag name by id
#[tauri::command]
pub fn get_tag_name(tag_id: i64, include_group: Option<bool>) -> Result<String, String> {
    ATag::get_name(tag_id, include_group.unwrap_or(false))
        .map_err(|e| format!("Error while getting tag name: {}", e))
}

/// create a new tag
#[tauri::command]
pub fn create_tag(name: &str, group_id: Option<i64>) -> Result<ATag, String> {
    ATag::add(name, group_id).map_err(|e| format!("Error while creating tag: {}", e))
}

/// rename a tag
#[tauri::command]
pub fn rename_tag(tag_id: i64, new_name: &str) -> Result<usize, String> {
    ATag::rename(tag_id, new_name).map_err(|e| format!("Error while renaming tag: {}", e))
}

/// delete a tag
#[tauri::command]
pub fn delete_tag(tag_id: i64) -> Result<usize, String> {
    ATag::delete(tag_id).map_err(|e| format!("Error while deleting tag: {}", e))
}

/// get all tags for a specific file
#[tauri::command]
pub fn get_tags_for_file(file_id: i64) -> Result<Vec<ATag>, String> {
    ATag::get_tags_for_file(file_id)
        .map_err(|e| format!("Error while getting tags for file: {}", e))
}

/// add a tag to a file
#[tauri::command]
pub fn add_tag_to_file(file_id: i64, tag_id: i64) -> Result<(), String> {
    ATag::add_tag_to_file(file_id, tag_id)
        .map_err(|e| format!("Error while adding tag to file: {}", e))
}

/// remove a tag from a file
#[tauri::command]
pub fn remove_tag_from_file(file_id: i64, tag_id: i64) -> Result<usize, String> {
    ATag::remove_tag_from_file(file_id, tag_id)
        .map_err(|e| format!("Error while removing tag from file: {}", e))
}

#[tauri::command]
pub fn get_tag_selection_counts(file_ids: Vec<i64>) -> Result<Vec<ATagSelectionCount>, String> {
    ATag::get_selection_counts(&file_ids)
        .map_err(|e| format!("Error while getting tag selection counts: {}", e))
}

#[tauri::command]
pub fn apply_tags_to_files(
    file_ids: Vec<i64>,
    add_tag_ids: Vec<i64>,
    remove_tag_ids: Vec<i64>,
) -> Result<Vec<ATagFileState>, String> {
    ATag::apply_to_files(&file_ids, &add_tag_ids, &remove_tag_ids)
        .map_err(|e| format!("Error while applying tags to files: {}", e))
}

// calendar

/// get camera's taken dates
#[tauri::command]
pub fn get_taken_dates(sort: i64, small_file_filter: i64) -> Result<Vec<(String, i64)>, String> {
    AFile::get_taken_dates(sort, small_file_filter)
        .map_err(|e| format!("Error while getting taken dates: {}", e))
}

// camera

/// get a file's camera make and model info
#[tauri::command]
pub fn get_camera_info(sort: i64, small_file_filter: i64) -> Result<Vec<ACamera>, String> {
    ACamera::get_from_db(sort, small_file_filter)
        .map_err(|e| format!("Error while getting camera info: {}", e))
}

/// get a file's lens make and model info
#[tauri::command]
pub fn get_lens_info(sort: i64, small_file_filter: i64) -> Result<Vec<ALens>, String> {
    ALens::get_from_db(sort, small_file_filter)
        .map_err(|e| format!("Error while getting lens info: {}", e))
}

// location

/// get a file's location info
#[tauri::command]
pub fn get_location_info(sort: i64, small_file_filter: i64) -> Result<Vec<ALocation>, String> {
    ALocation::get_from_db(sort, small_file_filter)
        .map_err(|e| format!("Error while getting location info: {}", e))
}

#[tauri::command]
pub fn get_gps_map_points(
    params: t_sqlite::QueryParams,
) -> Result<Vec<t_sqlite::AGpsMapPoint>, String> {
    t_sqlite::AGpsMapPoint::get_map_points_from_db(&params)
        .map_err(|e| format!("Error while getting GPS map points: {}", e))
}

// settings

/// get package info
#[tauri::command]
pub fn get_package_info() -> t_utils::PackageInfo {
    t_utils::PackageInfo::new(GIT_COMMIT_HASH)
}

/// get the build time
#[tauri::command]
pub fn get_build_time() -> u64 {
    BUILD_UNIX_TIME
}

/// get db file info
#[tauri::command]
pub fn get_storage_file_info() -> Result<t_utils::FileInfo, String> {
    // Get the database file path
    let db_file_path = t_storage::get_current_db_path()
        .map_err(|e| format!("Failed to get the database file path: {}", e))?;

    match t_utils::FileInfo::new(&db_file_path) {
        Ok(info) => Ok(info),
        Err(e) => Err(format!("Failed to get the database file size: {}", e)),
    }
}

// image search

/// check ai status
#[tauri::command]
pub fn check_ai_status(state: State<t_ai::AiState>) -> String {
    AFile::check_ai_status(&state)
}

#[tauri::command]
pub fn get_image_search_model_status(
    app_handle: AppHandle,
    state: State<t_ai::AiState>,
) -> t_ai::ImageSearchModelStatus {
    let ai_engine = state.0.lock().unwrap();
    ai_engine.model_status(&app_handle)
}

#[tauri::command]
pub async fn set_image_search_model(
    app_handle: AppHandle,
    state: State<'_, t_ai::AiState>,
    model: i64,
) -> Result<t_ai::ImageSearchModelStatus, String> {
    let mut ai_engine = state.0.lock().unwrap();
    ai_engine.set_text_model(&app_handle, t_ai::ImageSearchTextModel::from_i64(model))?;
    Ok(ai_engine.model_status(&app_handle))
}

#[tauri::command]
pub async fn download_multilingual_image_search_model(app_handle: AppHandle) -> Result<(), String> {
    t_ai::download_multilingual_text_model(app_handle).await
}

#[tauri::command]
pub async fn cancel_multilingual_image_search_model_download(
    app_handle: AppHandle,
) -> Result<(), String> {
    t_ai::cancel_multilingual_text_model_download(app_handle).await
}

/// generate embedding for a file
#[tauri::command]
pub fn generate_embedding(state: State<t_ai::AiState>, file_id: i64) -> Result<String, String> {
    AFile::generate_embedding(&state, file_id)
}

// search similar images
#[tauri::command]
pub async fn search_similar_images(
    state: State<'_, t_ai::AiState>,
    params: ImageSearchParams,
) -> Result<Vec<AFile>, String> {
    AFile::search_similar_images(&state, params)
        .map_err(|e| format!("Error while searching similar images: {}", e))
}

#[tauri::command]
pub fn similar_start_scan(
    app_handle: tauri::AppHandle,
    state: State<t_similar::SimilarState>,
    scope_key: String,
    source_version: i64,
    similarity_threshold: f32,
    params: Option<QueryParams>,
    collection_id: Option<i64>,
    file_ids: Option<Vec<i64>>,
) -> Result<(), String> {
    t_similar::start_scan(
        app_handle,
        state,
        scope_key,
        source_version,
        similarity_threshold,
        params,
        collection_id,
        file_ids,
    )
}

#[tauri::command]
pub fn similar_get_scan_status(
    state: State<t_similar::SimilarState>,
) -> t_similar::SimilarScanStatus {
    t_similar::get_status(state)
}

#[tauri::command]
pub fn similar_cancel_scan(state: State<t_similar::SimilarState>) {
    t_similar::cancel_scan(state)
}

#[tauri::command]
pub fn similar_get_eligible_count(
    params: Option<QueryParams>,
    collection_id: Option<i64>,
    file_ids: Option<Vec<i64>>,
) -> Result<u64, String> {
    t_similar::eligible_count(params, collection_id, file_ids)
}

#[tauri::command]
pub fn similar_list_groups(
    scope_key: String,
    limit: i64,
    offset: i64,
) -> Result<serde_json::Value, String> {
    t_similar::list_groups(&scope_key, limit, offset)
}

#[tauri::command]
pub fn similar_get_overview(scope_key: String) -> Result<serde_json::Value, String> {
    t_similar::get_overview(&scope_key)
}

#[tauri::command]
pub fn similar_get_group(group_id: i64, scope_key: String) -> Result<serde_json::Value, String> {
    t_similar::get_group(group_id, &scope_key)
}

#[tauri::command]
pub fn similar_set_keep(group_id: i64, file_id: i64, scope_key: String) -> Result<(), String> {
    t_similar::set_keep(group_id, file_id, &scope_key)
}

#[tauri::command]
pub fn similar_has_scan(scope_key: String) -> Result<bool, String> {
    t_similar::has_scan(&scope_key)
}

// face recognition

/// index faces for all images in the current library
#[tauri::command]
pub fn index_faces(
    app_handle: tauri::AppHandle,
    state: State<t_face::FaceState>,
    cancel_state: State<t_face::FaceIndexCancellation>,
    status_state: State<t_face::FaceIndexingStatus>,
    progress_state: State<t_face::FaceIndexProgressState>,
    cluster_epsilon: Option<f32>,
) -> Result<(), String> {
    t_face::run_face_indexing(
        app_handle,
        (*state).clone(),
        (*cancel_state).clone(),
        (*status_state).clone(),
        (*progress_state).clone(),
        cluster_epsilon,
    )
}

/// get face indexing stats
#[tauri::command]
pub fn get_face_stats(small_file_filter: i64) -> Result<t_face::FaceStats, String> {
    let (total, processed, unprocessed, faces) = t_sqlite::Face::get_stats_full(small_file_filter)
        .map_err(|e| format!("Error while getting face stats: {}", e))?;

    Ok(t_face::FaceStats {
        total,
        processed,
        unprocessed,
        faces,
    })
}

/// cancel face indexing
#[tauri::command]
pub fn cancel_face_index(state: State<t_face::FaceIndexCancellation>) -> Result<(), String> {
    *state.0.lock().unwrap() = true;

    Ok(())
}

/// reset all faces (delete all faces and persons)
#[tauri::command]
pub fn reset_faces() -> Result<(), String> {
    t_sqlite::Face::reset_all().map_err(|e| format!("Error while resetting faces: {}", e))
}

/// check if face indexing is running, return (is_running, progress)
#[tauri::command]
pub fn is_face_indexing(
    status_state: State<t_face::FaceIndexingStatus>,
    progress_state: State<t_face::FaceIndexProgressState>,
) -> Result<(bool, Option<t_face::FaceIndexProgress>), String> {
    let is_running = *status_state.0.lock().unwrap();
    let progress = if is_running {
        Some(progress_state.0.lock().unwrap().clone())
    } else {
        None
    };
    Ok((is_running, progress))
}

/// get all persons with face counts
#[tauri::command]
pub fn get_persons(sort: i64) -> Result<Vec<Person>, String> {
    Person::get_all(sort).map_err(|e| format!("Error while getting persons: {}", e))
}

/// Get a page of persons with face counts.
#[tauri::command]
pub fn get_persons_page(request: PersonPageRequest) -> Result<PersonPage, String> {
    Person::get_page(&request).map_err(|e| format!("Error while getting persons page: {}", e))
}

/// rename a person
#[tauri::command]
pub fn rename_person(person_id: i64, name: String) -> Result<usize, String> {
    Person::rename(person_id, &name).map_err(|e| format!("Error while renaming person: {}", e))
}

/// delete a person
#[tauri::command]
pub fn delete_person(person_id: i64) -> Result<usize, String> {
    Person::delete(person_id).map_err(|e| format!("Error while deleting person: {}", e))
}

/// get faces for a file
#[tauri::command]
pub fn get_faces_for_file(file_id: i64) -> Result<Vec<t_sqlite::Face>, String> {
    t_sqlite::Face::get_for_file(file_id)
        .map_err(|e| format!("Error while getting faces for file: {}", e))
}

/// Get a single person's face thumbnail (Base64 encoded).
#[tauri::command]
pub fn get_person_thumbnail(person_id: i64) -> Result<Option<String>, String> {
    t_sqlite::Person::get_thumbnail(person_id)
        .map_err(|e| format!("Error while getting person thumbnail: {}", e))
}

// ----------------------------------------------------------------------------
// Deduplication Commands
// ----------------------------------------------------------------------------

#[tauri::command]
pub fn dedup_start_scan(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, crate::t_dedup::DedupState>,
    params: Option<crate::t_sqlite::QueryParams>,
    collection_id: Option<i64>,
    file_ids: Option<Vec<i64>>,
) -> Result<(), String> {
    crate::t_dedup::start_scan(app_handle, state, params, collection_id, file_ids)
}

#[tauri::command]
pub fn dedup_get_scan_status(
    state: tauri::State<'_, crate::t_dedup::DedupState>,
) -> Result<crate::t_dedup::DedupScanStatus, String> {
    let mut status = state.status.lock().unwrap().clone();
    status.is_scanning = state.is_scanning.load(std::sync::atomic::Ordering::SeqCst);
    Ok(status.clone())
}

#[tauri::command]
pub fn dedup_cancel_scan(
    state: tauri::State<'_, crate::t_dedup::DedupState>,
) -> Result<(), String> {
    state
        .cancel_flag
        .store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub fn dedup_list_groups(
    page: u32,
    page_size: u32,
    sort_by: String, // E.g., "size_desc", "count_desc"
    filter: String,  // E.g., "all", "unreviewed"
) -> Result<Vec<crate::t_dedup::DedupGroup>, String> {
    crate::t_dedup::list_groups(page, page_size, &sort_by, &filter)
}

#[tauri::command]
pub fn dedup_get_overview() -> Result<crate::t_dedup::DedupOverview, String> {
    crate::t_dedup::get_overview()
}

#[tauri::command]
pub fn dedup_set_keep(group_id: i64, file_id: i64) -> Result<(), String> {
    crate::t_dedup::set_keep(group_id, file_id)
}

#[tauri::command]
pub async fn dedup_delete(
    request: crate::t_dedup::DedupDeleteRequest,
) -> Result<crate::t_dedup::DedupDeleteResult, String> {
    tauri::async_runtime::spawn_blocking(move || crate::t_dedup::delete(request))
        .await
        .map_err(|e| format!("Failed to run dedup delete: {}", e))?
}

// ----------------------------------------------------------------------------
// Backup / Restore Commands
// ----------------------------------------------------------------------------

#[tauri::command]
pub fn get_db_storage_info() -> Result<Vec<t_storage::DbStorageInfo>, String> {
    t_storage::get_db_storage_info()
}

#[tauri::command]
pub fn backup_databases(
    library_ids: Vec<String>,
    dest_path: String,
) -> Result<t_storage::BackupResult, String> {
    t_storage::backup_databases(&library_ids, &dest_path)
}

#[tauri::command]
pub fn parse_backup_file(path: String) -> Result<t_storage::BackupMetaData, String> {
    t_storage::parse_backup_file(&path)
}

#[tauri::command]
pub fn restore_databases(
    backup_path: String,
    selections: Vec<t_storage::RestoreSelection>,
) -> Result<t_storage::RestoreResult, String> {
    t_storage::restore_databases(&backup_path, &selections)
}

// ----------------------------------------------------------------------------
// Develop session commands (lap-a52 / TASK-302)
//
// Asset-scoped development over the pinned engine's bounded sessions
// (docs/raw-development/spec.md, "Recipe and session contract"). Every command
// carries explicit asset/variant/session/revision identities; previews travel
// as bounded raw-byte handles, never as large base64 JSON payloads. Unedited
// assets keep Lap's LibRaw browsing path untouched; this pipeline decodes with
// the engine's high-precision decoder only.
// ----------------------------------------------------------------------------

/// Managed develop state. The engine session manager (worker pools, bounded
/// queues) is constructed lazily on first use so app startup never pays for
/// it; the GPU context initializes on first render or capability probe.
/// The developed-derivative infrastructure (lap-a58) initializes lazily too.
#[derive(Default)]
pub struct DevelopAppState {
    service: std::sync::OnceLock<std::sync::Arc<DevelopService>>,
    derivatives: std::sync::OnceLock<std::sync::Arc<DevelopDerivativeParts>>,
    resources:
        std::sync::OnceLock<Option<std::sync::Arc<lap_lib::develop::resources::ResourceStore>>>,
}

impl DevelopAppState {
    /// The shared content-addressed resource store (lap-62b; lens profiles
    /// lap-d52). Opened once; the same instance is installed on the preview
    /// renderer so imports are immediately render-verified.
    pub(crate) fn resource_store(
        &self,
    ) -> Option<std::sync::Arc<lap_lib::develop::resources::ResourceStore>> {
        self.resources
            .get_or_init(|| {
                let resource_root = crate::t_config::get_app_data_dir()
                    .map(|dir| dir.join("develop-resources"))
                    .unwrap_or_else(|_| {
                        std::env::temp_dir()
                            .join(format!("lap-develop-resources-{}", std::process::id()))
                    });
                match lap_lib::develop::resources::ResourceStore::open(&resource_root) {
                    Ok(resources) => Some(std::sync::Arc::new(resources)),
                    Err(error) => {
                        eprintln!(
                            "develop resource store unavailable at {}: {error}",
                            resource_root.display()
                        );
                        None
                    }
                }
            })
            .clone()
    }

    fn service(&self) -> std::sync::Arc<DevelopService> {
        std::sync::Arc::clone(self.service.get_or_init(|| {
            let store = std::sync::Arc::new(
                SidecarBackedStore::new(DevelopRecipeRepository::lap_default()).with_conn_factory(
                    std::sync::Arc::new(|| {
                        crate::t_sqlite::open_conn().map(|pooled| {
                            Box::new(pooled)
                                as Box<dyn std::ops::Deref<Target = rusqlite::Connection>>
                        })
                    }),
                ),
            );
            // Content-addressed develop resources (lap-62b): durable data
            // referenced from sidecars, so it lives under the app data
            // directory; if that is unavailable, degrade to a process-local
            // temporary directory (logged) rather than losing imported
            // resources.
            let resources = self.resource_store();
            let gpu = std::sync::Arc::new(
                DevelopGpuPreviewRenderer::new().with_resource_store(resources),
            );
            std::sync::Arc::new(DevelopService::with_gpu_and_sidecar_store(
                DevelopConfig::default(),
                gpu,
                store,
            ))
        }))
    }

    /// The developed-derivative infrastructure (lap-a58): acknowledged-commit
    /// bus, bounded on-disk derivative store and the shared render gate.
    pub(crate) fn derivatives(&self) -> std::sync::Arc<DevelopDerivativeParts> {
        std::sync::Arc::clone(self.derivatives.get_or_init(|| {
            let bus = std::sync::Arc::new(lap_lib::develop::cache::DevelopCommitBus::new());
            // The store is a rebuildable cache: if the library-scoped cache
            // directory is unavailable, degrade to a process-local temporary
            // directory (logged) rather than losing developed thumbnails.
            let root = crate::t_config::get_app_cache_dir()
                .map(|dir| dir.join("developed-derivatives"))
                .unwrap_or_else(|_| {
                    std::env::temp_dir().join("lap-developed-derivatives")
                });
            let store = match lap_lib::develop::cache::DevelopedDerivativeStore::open(
                root,
                std::sync::Arc::clone(&bus),
            ) {
                Ok(store) => std::sync::Arc::new(store),
                Err(error) => {
                    eprintln!(
                        "developed-derivative cache unavailable, falling back to a temporary directory: {error}"
                    );
                    std::sync::Arc::new(
                        lap_lib::develop::cache::DevelopedDerivativeStore::open(
                            std::env::temp_dir().join(format!(
                                "lap-developed-derivatives-{}",
                                std::process::id()
                            )),
                            std::sync::Arc::clone(&bus),
                        )
                        .expect("temporary developed-derivative cache directory"),
                    )
                }
            };
            std::sync::Arc::new(DevelopDerivativeParts {
                bus,
                store,
                gate: std::sync::Arc::new(lap_lib::develop::cache::RenderGate::new(
                    DERIVATIVE_REFRESH_PERMITS,
                )),
                refresh_in_flight: Mutex::new(HashSet::new()),
            })
        }))
    }
}

/// Shared bounded permits for developed-derivative refresh jobs so background
/// thumbnail work respects the same resource limits as interactive sessions
/// and exports (never starves them, never decodes unbounded).
const DERIVATIVE_REFRESH_PERMITS: usize = 2;

/// Longest edge of a thumbnail-tier developed derivative. Bounded decode +
/// render; the served bytes scale down to the requested grid size.
const DERIVATIVE_THUMBNAIL_EDGE: u32 = 1024;

/// Lazily-initialized developed-derivative parts shared by the commit hook,
/// the thumbnail preference lookup and the background refresh jobs.
pub struct DevelopDerivativeParts {
    bus: std::sync::Arc<lap_lib::develop::cache::DevelopCommitBus>,
    store: std::sync::Arc<lap_lib::develop::cache::DevelopedDerivativeStore>,
    gate: std::sync::Arc<lap_lib::develop::cache::RenderGate>,
    refresh_in_flight: Mutex<HashSet<i64>>,
}

type DevelopStamp = lap_lib::develop::cache::DerivativeStamp;

/// Serves the current developed derivative thumbnail for an asset when one
/// exists for its currently acknowledged commit (lap-a58: the catalog prefers
/// developed derivatives). The returned [`AThumb`] is synthetic and
/// read-only: it is never persisted to `athumbs`, whose rows remain the
/// undeveloped cache authority.
fn developed_thumbnail_override(
    state: &DevelopAppState,
    file_id: i64,
    thumbnail_size: u32,
) -> Option<AThumb> {
    let parts = state.derivatives();
    let asset_id = file_id.to_string();
    let stamp = parts.bus.stamp(&asset_id)?;
    let bytes = parts.store.lookup_bytes(&asset_id, &stamp)?;
    use base64::{Engine, engine::general_purpose};
    Some(AThumb {
        id: None,
        file_id,
        error_code: 0,
        thumb_data: None,
        thumb_key: None,
        thumb_mtime: None,
        thumb_size: Some(thumbnail_size as i64),
        updated_at: None,
        thumb_data_base64: Some(general_purpose::STANDARD.encode(bytes)),
    })
}

/// Acknowledged-commit side effects (lap-a58): register the durable stamp,
/// invalidate the affected catalog thumbnail, notify open viewers and the
/// catalog, and schedule the bounded developed-thumbnail refresh.
/// Best-effort invalidation failures are logged, never failed silently.
fn acknowledge_commit_side_effects(
    app_handle: &AppHandle,
    service: std::sync::Arc<DevelopService>,
    state: &DevelopAppState,
    asset_id: &str,
    variant_id: &str,
    source_fingerprint: &str,
    receipt: &lap_lib::develop::sessions::CommitReceiptDto,
) {
    let parts = state.derivatives();
    let stamp = DevelopStamp {
        revision: receipt.revision,
        content_hash: receipt.content_hash.clone().unwrap_or_default(),
        source_fingerprint: source_fingerprint.to_string(),
    };
    parts.bus.acknowledge(asset_id, stamp);

    let Ok(file_id) = asset_id.parse::<i64>() else {
        return;
    };
    if file_id <= 0 {
        return;
    }

    // Invalidate the cached (undeveloped) thumbnail row and cache file so the
    // next request regenerates with the developed preference.
    let thumbnail_invalidated = match AThumb::delete(file_id) {
        Ok(_) => true,
        Err(error) => {
            eprintln!("develop commit: thumbnail invalidation failed for file {file_id}: {error}");
            false
        }
    };

    let album_id = AFile::get_file_info(file_id)
        .ok()
        .flatten()
        .and_then(|file| file.album_id)
        .unwrap_or(0);

    // Explicit viewer notification: open viewers refresh the displayed
    // derivative (MediaViewer listens for this event).
    let _ = app_handle.emit(
        "develop_committed",
        serde_json::json!({
            "fileId": file_id,
            "assetId": asset_id,
            "variantId": variant_id,
            "revision": receipt.revision,
            "contentHash": receipt.content_hash,
            "thumbnailInvalidated": thumbnail_invalidated,
        }),
    );

    // Reuse the existing catalog refresh contract so every open grid/central
    // view repaints the invalidated thumbnails (same payload shape the
    // indexing worker emits).
    if album_id > 0 {
        let _ = app_handle.emit(
            "thumbnail_ready",
            serde_json::json!({
                "album_id": album_id,
                "file_ids": [file_id],
                "invalidate": true,
            }),
        );
    }

    schedule_developed_thumbnail_refresh(
        app_handle.clone(),
        service,
        std::sync::Arc::clone(&parts),
        file_id,
        album_id,
    );
}

/// Schedules the bounded developed-thumbnail refresh: at most one job per
/// asset, each holding one shared render permit. When the job settles, the
/// catalog and viewers are notified through the existing `thumbnail_ready`
/// invalidation contract plus `develop_derivative_ready`.
fn schedule_developed_thumbnail_refresh(
    app_handle: AppHandle,
    service: std::sync::Arc<DevelopService>,
    parts: std::sync::Arc<DevelopDerivativeParts>,
    file_id: i64,
    album_id: i64,
) {
    {
        let mut in_flight = parts
            .refresh_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !in_flight.insert(file_id) {
            return;
        }
    }
    tauri::async_runtime::spawn_blocking(move || {
        let result = render_developed_thumbnail(&service, &parts, file_id);
        parts
            .refresh_in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&file_id);
        match result {
            Ok(true) => {
                if album_id > 0 {
                    let _ = app_handle.emit(
                        "thumbnail_ready",
                        serde_json::json!({
                            "album_id": album_id,
                            "file_ids": [file_id],
                            "invalidate": true,
                        }),
                    );
                }
                let _ = app_handle.emit(
                    "develop_derivative_ready",
                    serde_json::json!({ "fileId": file_id }),
                );
            }
            // Superseded by a newer commit or the gate was busy: nothing to
            // announce; the next commit or catalog refresh retries.
            Ok(false) => {}
            Err(error) => {
                eprintln!("developed thumbnail refresh failed for file {file_id}: {error}")
            }
        }
    });
}

/// Renders the committed recipe at thumbnail tier through a short-lived
/// bounded session and settles it into the derivative store. Returns
/// `Ok(true)` when a current derivative was stored, `Ok(false)` when the job
/// was superseded (newer commit) or the gate was busy, and an explicit error
/// for missing media, GPU failures or IO problems (never a fake success).
fn render_developed_thumbnail(
    service: &DevelopService,
    parts: &DevelopDerivativeParts,
    file_id: i64,
) -> Result<bool, String> {
    let Some(_permit) = parts.gate.try_acquire() else {
        return Ok(false);
    };

    let source_path = develop_asset_source_path(file_id)?;
    let source_bytes = fs::read(&source_path)
        .map_err(|e| format!("failed to read source {}: {e}", source_path.display()))?;
    let source_fingerprint = rapidraw_edit_model::sha256_hex(&source_bytes);
    let repo = DevelopRecipeRepository::lap_default();
    // Identity adoption (lap-487): keep developed thumbnails working after
    // copies, external moves and catalog rebuilds; replaced media is a
    // visible failure instead of a stale derivative.
    lap_lib::develop::asset_operations::adopt_catalog_identity_with_fingerprint(
        &source_path,
        &file_id.to_string(),
        &source_fingerprint,
    )
    .map_err(|e| e.to_string())?;
    let sidecar = repo
        .load_opt(&source_path)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no committed recipe sidecar for {}", source_path.display()))?;
    if sidecar.revision == 0 {
        return Ok(false);
    }

    let asset_id = file_id.to_string();
    let stamp = DevelopStamp {
        revision: sidecar.revision,
        content_hash: sidecar
            .content_hash()
            .map_err(|e| format!("committed recipe hash failed: {e}"))?,
        source_fingerprint: source_fingerprint.clone(),
    };
    // The acknowledged stamp must still match the durable sidecar, or the
    // job is stale before it starts. On a fresh process (empty bus) the
    // durable sidecar seeds the baseline.
    parts.bus.seed_if_absent(&asset_id, stamp.clone());
    if parts.bus.stamp(&asset_id).as_ref() != Some(&stamp) {
        return Ok(false);
    }

    let opened = service
        .open_session(lap_lib::develop::sessions::AssetEditInput {
            asset_id: asset_id.clone(),
            variant_id: sidecar.variant_id.clone(),
            source_path: source_path.clone(),
            source_fingerprint,
            source_bytes,
            sidecar: Some(sidecar.clone()),
        })
        .map_err(develop_error_string)?;

    // Engine-aligned envelope: sidecar revision + 1 (see develop/sessions.rs).
    let mut envelope = sidecar.clone();
    match sidecar.revision.checked_add(1) {
        Some(engine_revision) => envelope.revision = engine_revision,
        None => {
            let _ = service.close_session(opened.session_id);
            return Err("sidecar revision overflow".to_string());
        }
    }

    let outcome = service.render_preview(
        opened.session_id,
        1,
        envelope,
        rapidraw_develop::session::PreviewQuality::Settled,
        DERIVATIVE_THUMBNAIL_EDGE,
    );
    let preview = match outcome {
        Ok(preview) => preview,
        Err(error) => {
            let _ = service.close_session(opened.session_id);
            return Err(develop_error_string(error));
        }
    };
    let frame = match preview {
        lap_lib::develop::sessions::PreviewWait::Completed { ticket } => {
            service.take_preview_frame(&ticket.handle)
        }
        lap_lib::develop::sessions::PreviewWait::Cancelled => {
            let _ = service.close_session(opened.session_id);
            return Ok(false);
        }
        lap_lib::develop::sessions::PreviewWait::Failed { code, message } => {
            let _ = service.close_session(opened.session_id);
            return Err(format!(
                "developed thumbnail render failed ({code}): {message}"
            ));
        }
    };
    let _ = service.close_session(opened.session_id);
    let frame = frame.map_err(develop_error_string)?;

    let mut jpeg_bytes = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, 90);
    let rgba = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba8.clone())
        .ok_or_else(|| "rendered preview does not match its dimensions".to_string())?;
    image::DynamicImage::ImageRgba8(rgba)
        .write_with_encoder(encoder)
        .map_err(|e| format!("developed thumbnail encoding failed: {e}"))?;

    let identity = lap_lib::develop::cache::DerivativeIdentity::from_envelope(
        &sidecar,
        frame.width,
        frame.height,
        lap_lib::develop::cache::QualityTier::Thumbnail,
    )
    .map_err(|e| e.to_string())?;

    match parts.store.settle(lap_lib::develop::cache::SettleJob {
        asset_id,
        stamp,
        identity,
        source_path,
        bytes: jpeg_bytes,
    }) {
        Ok(association) => Ok(association.has_blob),
        // A newer acknowledged commit superseded this delayed job (spec A4):
        // not an error, nothing is stored.
        Err(lap_lib::develop::cache::DerivativeCacheError::StaleContribution { .. }) => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

type DevelopRecipeRepository = lap_lib::develop::recipe_repository::RecipeRepository;
type DevelopGpuPreviewRenderer = lap_lib::develop::sessions::GpuPreviewRenderer;
type DevelopService = lap_lib::develop::sessions::DevelopService;
type DevelopConfig = lap_lib::develop::sessions::DevelopConfig;
type SidecarBackedStore = lap_lib::develop::sessions::SidecarBackedStore;

/// Resolve a catalog asset id to its absolute source path.
fn develop_asset_source_path(asset_id: i64) -> Result<std::path::PathBuf, String> {
    if asset_id <= 0 {
        return Err(format!("invalid asset id {asset_id}"));
    }
    let files = AFile::get_files_by_ids(&[asset_id])?;
    let file = files
        .into_iter()
        .next()
        .ok_or_else(|| format!("asset {asset_id} not found in the catalog"))?;
    file.file_path
        .map(std::path::PathBuf::from)
        .ok_or_else(|| format!("asset {asset_id} has no resolvable source path"))
}

fn develop_error_string(error: lap_lib::develop::sessions::DevelopError) -> String {
    error.to_string()
}

/// Parses the frontend envelope JSON through the shared model's validator so
/// schema violations fail before touching sessions or disk.
fn develop_parse_envelope(
    envelope: serde_json::Value,
) -> Result<rapidraw_edit_model::RecipeEnvelope, String> {
    rapidraw_edit_model::migrate::parse_envelope_value(envelope)
        .map_err(|e| format!("invalid develop envelope: {e}"))
}

fn develop_quality(quality: &str) -> Result<rapidraw_develop::session::PreviewQuality, String> {
    match quality.to_ascii_lowercase().as_str() {
        "interactive" => Ok(rapidraw_develop::session::PreviewQuality::Interactive),
        "settled" => Ok(rapidraw_develop::session::PreviewQuality::Settled),
        other => Err(format!(
            "unknown preview quality '{other}' (expected 'interactive' or 'settled')"
        )),
    }
}

/// Opens an edit session for a catalog asset: resolves the source path,
/// fingerprints the untouched source bytes, loads the committed sidecar, and
/// decodes the original with the shared high-precision pipeline.
#[tauri::command]
pub async fn develop_open_edit_session(
    state: tauri::State<'_, DevelopAppState>,
    asset_id: i64,
    variant_id: String,
) -> Result<lap_lib::develop::sessions::OpenedEditSession, String> {
    let service = state.service();
    tauri::async_runtime::spawn_blocking(move || {
        let source_path = develop_asset_source_path(asset_id)?;
        let source_bytes = fs::read(&source_path)
            .map_err(|e| format!("failed to read source {}: {e}", source_path.display()))?;
        let source_fingerprint = rapidraw_edit_model::sha256_hex(&source_bytes);
        let repo = DevelopRecipeRepository::lap_default();
        // Identity adoption (lap-487): after copies, external moves or a
        // catalog rebuild, a sidecar can carry a stale asset identity. It is
        // re-keyed to the current catalog id ONLY when the source fingerprint
        // still matches; replacement at the same path is a visible failure.
        lap_lib::develop::asset_operations::adopt_catalog_identity_with_fingerprint(
            &source_path,
            &asset_id.to_string(),
            &source_fingerprint,
        )
        .map_err(|e| e.to_string())?;
        let sidecar = repo.load_opt(&source_path).map_err(|e| e.to_string())?;
        service
            .open_session(lap_lib::develop::sessions::AssetEditInput {
                asset_id: asset_id.to_string(),
                variant_id,
                source_path,
                source_fingerprint,
                source_bytes,
                sidecar,
            })
            .map_err(develop_error_string)
    })
    .await
    .map_err(|e| format!("develop session open task failed: {e}"))?
}

/// Renders one preview generation. Coalesced/stale generations resolve as
/// `cancelled`; completed frames are fetched by handle via
/// `develop_take_preview_frame` (raw bytes, bounded transport).
#[tauri::command]
pub async fn develop_render_preview(
    state: tauri::State<'_, DevelopAppState>,
    session_id: u64,
    generation: u64,
    envelope: serde_json::Value,
    quality: Option<String>,
    max_edge: Option<u32>,
) -> Result<lap_lib::develop::sessions::PreviewWait, String> {
    let service = state.service();
    let envelope = develop_parse_envelope(envelope)?;
    let quality = develop_quality(quality.as_deref().unwrap_or("settled"))?;
    let max_edge = max_edge.unwrap_or(1536);
    tauri::async_runtime::spawn_blocking(move || {
        service
            .render_preview(session_id, generation, envelope, quality, max_edge)
            .map_err(develop_error_string)
    })
    .await
    .map_err(|e| format!("develop preview task failed: {e}"))?
}

/// Fetches a rendered preview frame's raw RGBA8 pixels by handle. The frame
/// was rendered at most `develop_render_preview`'s bounded max edge; expired
/// or unknown handles are explicit errors.
#[tauri::command]
pub fn develop_take_preview_frame(
    state: tauri::State<'_, DevelopAppState>,
    handle: String,
) -> Result<tauri::ipc::Response, String> {
    let service = state.service();
    let frame = service
        .take_preview_frame(&handle)
        .map_err(develop_error_string)?;
    Ok(tauri::ipc::Response::new(frame.rgba8))
}

/// Validates and durably persists a recipe with optimistic revision control.
/// A stale commit reports a conflict; the acknowledged revision is durable.
/// After acknowledgment the affected derivatives are invalidated and viewers
/// are notified (lap-a58).
#[tauri::command]
pub async fn develop_commit_recipe(
    state: tauri::State<'_, DevelopAppState>,
    app_handle: tauri::AppHandle,
    session_id: u64,
    expected_revision: u64,
    envelope: serde_json::Value,
) -> Result<lap_lib::develop::sessions::CommitReceiptDto, String> {
    let service = state.service();
    let envelope = develop_parse_envelope(envelope)?;
    let asset_id = envelope.asset_id.clone();
    let variant_id = envelope.variant_id.clone();
    let source_fingerprint = envelope.source_fingerprint.clone();
    let receipt = tauri::async_runtime::spawn_blocking(move || {
        service
            .commit_recipe(session_id, expected_revision, envelope)
            .map_err(develop_error_string)
    })
    .await
    .map_err(|e| format!("develop commit task failed: {e}"))??;

    acknowledge_commit_side_effects(
        &app_handle,
        state.service(),
        state.inner(),
        &asset_id,
        &variant_id,
        &source_fingerprint,
        &receipt,
    );
    Ok(receipt)
}

/// Closes a session: previews are cancelled, pending saves settle first, and
/// buffers plus preview handles are released.
#[tauri::command]
pub async fn develop_close_edit_session(
    state: tauri::State<'_, DevelopAppState>,
    session_id: u64,
) -> Result<lap_lib::develop::sessions::ClosedEditSession, String> {
    let service = state.service();
    tauri::async_runtime::spawn_blocking(move || {
        service
            .close_session(session_id)
            .map_err(develop_error_string)
    })
    .await
    .map_err(|e| format!("develop close task failed: {e}"))?
}

/// Explicit capability report: offscreen GPU status plus the pinned engine
/// identity (spec A7/A11). Never claims a device it did not probe.
#[tauri::command]
pub async fn develop_get_capabilities(
    state: tauri::State<'_, DevelopAppState>,
) -> Result<lap_lib::develop::sessions::CapabilityReport, String> {
    let service = state.service();
    tauri::async_runtime::spawn_blocking(move || service.capabilities())
        .await
        .map_err(|e| format!("develop capabilities task failed: {e}"))
}

// ----------------------------------------------------------------------------
// Lens-correction profiles (lap-d52 / TASK-502). Locally acquired versioned
// resources: imports are content-addressed and carry an explicit,
// user-declared version label; selection resolves the correction coefficients
// through the engine and records full provenance on the recipe. Nothing is
// bundled: distribution terms for lens data remain an unresolved prerequisite
// (docs/raw-development/provenance.json, release hold).
// ----------------------------------------------------------------------------

/// Serializable result of one lens-profile file import.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LensProfileImportDto {
    pub id: String,
    pub digest: String,
    pub size_bytes: u64,
    pub lens_count: usize,
    pub camera_count: usize,
    pub version: String,
    pub newly_stored: bool,
}

/// Serializable summary of a stored lens profile (catalog listing).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LensProfileSummaryDto {
    pub id: String,
    pub lens_count: usize,
    pub camera_count: usize,
}

/// Serializable capability notice from a lens selection.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LensNoticeDto {
    pub kind: String,
    pub detail: String,
}

/// Serializable lens selection: resolved coefficients, visible capability
/// notices and the recorded provenance.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LensSelectionDto {
    pub params: rapidraw_edit_model::LensDistortionParams,
    pub notices: Vec<LensNoticeDto>,
    pub profile: rapidraw_edit_model::LensProfileRef,
}

fn require_resource_store(
    state: &DevelopAppState,
) -> Result<std::sync::Arc<lap_lib::develop::resources::ResourceStore>, String> {
    state.resource_store().ok_or_else(|| {
        "develop resource store is unavailable; lens profiles cannot be imported or verified"
            .to_string()
    })
}

/// Imports lensfun XML files as versioned lens-profile resources. `version`
/// is the user-declared label recorded verbatim in the recipe provenance
/// (`unversioned` when omitted); the backend never invents one.
#[tauri::command]
pub async fn develop_import_lens_profile(
    state: tauri::State<'_, DevelopAppState>,
    paths: Vec<String>,
    version: Option<String>,
) -> Result<Vec<LensProfileImportDto>, String> {
    let store = require_resource_store(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        for path in paths {
            let bytes = std::fs::read(&path)
                .map_err(|error| format!("cannot read lens profile {}: {error}", path))?;
            let imported = store
                .import_lens_profile_bytes(&bytes, None, version.as_deref())
                .map_err(|error| format!("{}: {error}", path))?;
            out.push(LensProfileImportDto {
                id: imported.id.clone(),
                digest: imported.digest.clone(),
                size_bytes: imported.size_bytes,
                lens_count: imported.lens_count,
                camera_count: imported.camera_count,
                version: imported.version_label().to_string(),
                newly_stored: imported.newly_stored,
            });
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("develop lens import task failed: {e}"))?
}

/// Lists every stored object that parses as a lensfun profile database.
#[tauri::command]
pub async fn develop_lens_catalog(
    state: tauri::State<'_, DevelopAppState>,
) -> Result<Vec<LensProfileSummaryDto>, String> {
    let store = require_resource_store(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        let objects = store.root().join("objects");
        let mut stack = vec![objects];
        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let Some(digest) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if digest.len() != 64 {
                    continue;
                }
                let id = lap_lib::develop::resources::lens_resource_id(digest);
                if let Ok(database) = store.lens_database(&id) {
                    out.push(LensProfileSummaryDto {
                        id,
                        lens_count: database.lenses.len(),
                        camera_count: database.cameras.len(),
                    });
                }
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    })
    .await
    .map_err(|e| format!("develop lens catalog task failed: {e}"))?
}

/// Sorted maker list of one stored profile.
#[tauri::command]
pub async fn develop_lens_makers(
    state: tauri::State<'_, DevelopAppState>,
    profile_id: String,
) -> Result<Vec<String>, String> {
    let store = require_resource_store(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        store
            .lens_makers(&profile_id)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|e| format!("develop lens makers task failed: {e}"))?
}

/// Display model names of one maker inside one stored profile.
#[tauri::command]
pub async fn develop_lens_models(
    state: tauri::State<'_, DevelopAppState>,
    profile_id: String,
    maker: String,
) -> Result<Vec<String>, String> {
    let store = require_resource_store(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let database = store
            .lens_database(&profile_id)
            .map_err(|error| error.to_string())?;
        let maker_lenses = rapidraw_develop::lens::lenses_for_maker(&database, &maker);
        let mut models: Vec<String> = maker_lenses
            .iter()
            .map(|lens| lens.get_display_name(&maker_lenses))
            .collect();
        models.sort_unstable();
        models.dedup();
        Ok(models)
    })
    .await
    .map_err(|e| format!("develop lens models task failed: {e}"))?
}

/// Reference fuzzy auto-detection of a lens from camera-reported EXIF names.
#[tauri::command]
pub async fn develop_autodetect_lens(
    state: tauri::State<'_, DevelopAppState>,
    profile_id: String,
    maker: String,
    model: String,
) -> Result<Option<(String, String)>, String> {
    let store = require_resource_store(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        store
            .find_best_lens_match(&profile_id, &maker, &model)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|e| format!("develop lens autodetect task failed: {e}"))?
}

/// Resolves the correction coefficients for a lens selection. The command
/// performs NO recipe write: the frontend applies the returned data through
/// the editor's durable commit path.
#[allow(clippy::too_many_arguments)] // flat argument list is a Tauri IPC requirement
#[tauri::command]
pub async fn develop_select_lens(
    state: tauri::State<'_, DevelopAppState>,
    profile_id: String,
    maker: String,
    model: String,
    version: Option<String>,
    focal_length: f32,
    aperture: Option<f32>,
    distance: Option<f32>,
) -> Result<LensSelectionDto, String> {
    let store = require_resource_store(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut envelope = rapidraw_edit_model::RecipeEnvelope::default();
        let selection = lap_lib::develop::resources::LensSelection {
            profile_id: &profile_id,
            maker: &maker,
            model: &model,
            version: version.as_deref().unwrap_or("unversioned"),
            focal_length,
            aperture,
            distance,
        };
        let (params, notices) = store
            .select_lens_profile(&mut envelope, &selection)
            .map_err(|error| error.to_string())?;
        Ok(LensSelectionDto {
            params,
            notices: notices
                .into_iter()
                .map(|notice| LensNoticeDto {
                    kind: notice.kind.to_string(),
                    detail: notice.detail,
                })
                .collect(),
            profile: envelope
                .recipe
                .lens_profile
                .expect("selection records provenance"),
        })
    })
    .await
    .map_err(|e| format!("develop lens select task failed: {e}"))?
}

/// Explicit one-way `.rrdata` compatibility reader (lap-5c2 / TASK-404;
/// managed continuation of lap-002.4; spec "Persistence and compatibility").
///
/// Validates the source document and returns the converted envelope plus the
/// fidelity report (named limitations for unsupported visible effects,
/// retained original payload keys). This command performs **no writes**: the
/// original rrdata stays byte-identical, and persisting the converted
/// envelope is a separate explicit commit (`develop_commit_recipe`) after the
/// user confirms the reported limitations in the import dialog.
#[tauri::command]
pub async fn develop_import_rrdata(
    rrdata_path: String,
    asset_id: i64,
    variant_id: Option<String>,
    source_fingerprint: String,
    source_width: u32,
    source_height: u32,
) -> Result<lap_lib::develop::rrdata_import::RrdataImportOutcome, String> {
    if asset_id <= 0 {
        return Err("develop rrdata import requires a catalog asset id".to_string());
    }
    if source_width == 0 || source_height == 0 {
        return Err("develop rrdata import requires the decoded original dimensions".to_string());
    }
    if source_fingerprint.len() != 64 {
        return Err(
            "develop rrdata import requires the 64-hex source fingerprint of an opened session"
                .to_string(),
        );
    }
    tauri::async_runtime::spawn_blocking(move || {
        let importer = lap_lib::develop::rrdata_import::RrdataImporter::lap_default();
        let doc = lap_lib::develop::rrdata_import::read_rrdata(std::path::Path::new(&rrdata_path))
            .map_err(|e| e.to_string())?;
        let identity = rapidraw_edit_model::migrate::EnvelopeIdentity {
            engine_version: importer.engine_version().to_string(),
            asset_id: asset_id.to_string(),
            variant_id: variant_id.unwrap_or_else(|| "default".to_string()),
            source_fingerprint,
        };
        importer
            .import_document(
                &doc,
                &identity,
                (u64::from(source_width), u64::from(source_height)),
            )
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("develop rrdata import task failed: {e}"))?
}

// ----------------------------------------------------------------------------
// Durable derivative export commands (lap-7ae / TASK-304)
//
// `export_developed` per docs/raw-development/spec.md: the UI flushes edits
// (develop_commit_recipe) and then requests the derivative at exactly the
// acknowledged committed revision. The backend resolves the durable sidecar
// at that revision, decodes the original at full resolution through the
// shared engine, renders through the bounded export queue, and writes the
// derivative atomically. Source destinations and their aliases are rejected
// before any write; cancellation is cooperative and leaves no partial file.
// ----------------------------------------------------------------------------

/// Active derivative-export jobs keyed by the frontend-supplied export id.
/// Each entry carries the same managed [`DevelopService`] that enqueued the
/// engine job, so `develop_cancel_export` can reach the exact bounded export
/// queue the work runs on.
#[derive(Default)]
pub struct DevelopExportJobs(std::sync::Mutex<std::collections::HashMap<String, DevelopExportJob>>);

struct DevelopExportJob {
    service: std::sync::Arc<DevelopService>,
    cancel_source: rapidraw_develop::CancelSource,
    slot: std::sync::Arc<lap_lib::develop::export::ExportCancelSlot>,
}

impl DevelopExportJobs {
    fn register(
        self: &std::sync::Arc<Self>,
        export_job: &str,
        service: std::sync::Arc<DevelopService>,
    ) -> (
        rapidraw_develop::CancelToken,
        std::sync::Arc<lap_lib::develop::export::ExportCancelSlot>,
        DevelopJobGuard,
    ) {
        let (cancel_source, cancel) = rapidraw_develop::CancelToken::pair();
        let slot = std::sync::Arc::new(lap_lib::develop::export::ExportCancelSlot::new());
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                export_job.to_string(),
                DevelopExportJob {
                    service: std::sync::Arc::clone(&service),
                    cancel_source: cancel_source.clone(),
                    slot: std::sync::Arc::clone(&slot),
                },
            );
        (
            cancel,
            slot,
            DevelopJobGuard {
                registry: std::sync::Arc::clone(self),
                key: export_job.to_string(),
            },
        )
    }

    /// Cooperatively cancels a registered export: the caller-side token ends
    /// any decode/encode phase, and the engine export job (once enqueued) is
    /// cancelled in its own bounded queue. Returns false for unknown or
    /// already-finished ids.
    pub fn cancel(&self, export_job: &str) -> bool {
        let entry = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(export_job)
            .map(|job| {
                (
                    job.cancel_source.clone(),
                    job.slot.engine_job_id(),
                    std::sync::Arc::clone(&job.service),
                )
            });
        match entry {
            Some((cancel_source, engine_job, service)) => {
                cancel_source.cancel();
                if let Some(engine_job) = engine_job {
                    service.cancel_engine_export(engine_job);
                }
                true
            }
            None => false,
        }
    }
}

/// Drops the registry entry when the export command returns (success,
/// failure or cancellation); late cancels of expired ids report false.
struct DevelopJobGuard {
    registry: std::sync::Arc<DevelopExportJobs>,
    key: String,
}

impl Drop for DevelopJobGuard {
    fn drop(&mut self) {
        self.registry
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.key);
    }
}

fn develop_error_string_export(error: lap_lib::develop::export::ExportError) -> String {
    error.to_string()
}

/// Exports the committed recipe revision as a full-resolution derivative.
/// The UI flushes edits first (`develop_commit_recipe`) and passes the
/// acknowledged revision; a durable sidecar at any other revision is a
/// typed revision mismatch, never a silent "latest" export.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn develop_export_developed(
    state: tauri::State<'_, DevelopAppState>,
    export_jobs: tauri::State<'_, std::sync::Arc<DevelopExportJobs>>,
    asset_id: i64,
    variant_id: String,
    revision: u64,
    destination: String,
    format: String,
    quality: Option<u8>,
    max_edge: Option<u32>,
    export_job: String,
) -> Result<lap_lib::develop::export::ExportCompletion, String> {
    if export_job.is_empty() {
        return Err("export_job id is required for cancellation".to_string());
    }
    let format = lap_lib::develop::export::ExportFormat::parse(&format)
        .ok_or_else(|| format!("unknown export format '{format}' (expected 'png' or 'jpeg')"))?;
    let settings = lap_lib::develop::export::ExportSettings {
        destination: std::path::PathBuf::from(&destination),
        format,
        jpeg_quality: quality.unwrap_or(90),
        max_edge,
    };
    let service = state.service();
    let (cancel, slot, guard) = export_jobs.register(&export_job, std::sync::Arc::clone(&service));

    tauri::async_runtime::spawn_blocking(move || {
        let result = (|| {
            let source_path = develop_asset_source_path(asset_id)?;
            let source_bytes = fs::read(&source_path)
                .map_err(|e| format!("failed to read source {}: {e}", source_path.display()))?;
            // Identity adoption (lap-487): exports resolve the committed
            // sidecar by identity, so a copy/external-move asset is re-keyed
            // first; replaced media is refused explicitly.
            lap_lib::develop::asset_operations::adopt_catalog_identity_with_fingerprint(
                &source_path,
                &asset_id.to_string(),
                &rapidraw_edit_model::sha256_hex(&source_bytes),
            )
            .map_err(|e| format!("develop recipe identity check failed: {e}"))?;
            lap_lib::develop::export::export_developed(
                &service,
                lap_lib::develop::export::AssetExportInput {
                    asset_id: asset_id.to_string(),
                    variant_id,
                    source_path,
                    source_bytes,
                    requested_revision: revision,
                },
                settings,
                &cancel,
                &slot,
            )
            .map_err(develop_error_string_export)
        })();
        drop(guard);
        result
    })
    .await
    .map_err(|e| format!("develop export task failed: {e}"))?
}

/// Cooperatively cancels an in-flight or queued derivative export. Reports
/// false for unknown or already-finished export ids.
#[tauri::command]
pub async fn develop_cancel_export(
    export_jobs: tauri::State<'_, std::sync::Arc<DevelopExportJobs>>,
    export_job: String,
) -> Result<bool, String> {
    Ok(export_jobs.cancel(&export_job))
}

// ----------------------------------------------------------------------------
// Virtual copies and bounded batch development (lap-952 / TASK-503).
//
// Virtual copies live in their own `name.ext.lapedit.v-<variantId>.json`
// sidecars with independent per-asset/variant CAS revisions over the same
// immutable source bytes. Batch operations commit or export one explicit
// item at a time with per-item progress events, visible per-item failures
// and cooperative cancellation through the shared export-job registry.
// ----------------------------------------------------------------------------

/// Serializable variant summary for the Develop panel.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantSummaryDto {
    pub variant_id: String,
    pub revision: u64,
    pub is_edited: bool,
    pub is_virtual_copy: bool,
    pub content_hash: String,
    pub sidecar_path: std::path::PathBuf,
    pub exists: bool,
}

impl From<lap_lib::develop::variants::VariantSummary> for VariantSummaryDto {
    fn from(summary: lap_lib::develop::variants::VariantSummary) -> Self {
        Self {
            variant_id: summary.variant_id,
            revision: summary.revision,
            is_edited: summary.is_edited,
            is_virtual_copy: summary.is_virtual_copy,
            content_hash: summary.content_hash,
            sidecar_path: summary.sidecar_path,
            exists: summary.exists,
        }
    }
}

/// Serializable reset receipt (the new durable revision after the reset).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantResetDto {
    pub variant_id: String,
    pub revision: u64,
    pub content_hash: String,
    pub sidecar_path: std::path::PathBuf,
}

/// Lists the asset's develop variants (default first, then virtual copies).
#[tauri::command]
pub async fn develop_list_variants(asset_id: i64) -> Result<Vec<VariantSummaryDto>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let source_path = develop_asset_source_path(asset_id)?;
        let repo = DevelopRecipeRepository::lap_default();
        let list = lap_lib::develop::variants::list_variants(&repo, &source_path)
            .map_err(|e| e.to_string())?;
        Ok(list.into_iter().map(Into::into).collect())
    })
    .await
    .map_err(|e| format!("develop variants task failed: {e}"))?
}

/// Creates a virtual copy of the asset's recipe (`from_variant_id` omitted or
/// "default" copies the primary recipe; a missing asset recipe starts a fresh
/// virtual copy). The source bytes and every other variant are untouched.
#[tauri::command]
pub async fn develop_create_virtual_copy(
    asset_id: i64,
    from_variant_id: Option<String>,
) -> Result<VariantSummaryDto, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let source_path = develop_asset_source_path(asset_id)?;
        let repo = DevelopRecipeRepository::lap_default();
        let conn = t_sqlite::open_conn().map_err(|e| e.to_string())?;
        let envelope = lap_lib::develop::variants::create_virtual_copy(
            &repo,
            Some(&conn),
            &source_path,
            &asset_id.to_string(),
            from_variant_id.as_deref(),
            None,
        )
        .map_err(|e| e.to_string())?;
        Ok(VariantSummaryDto {
            variant_id: envelope.variant_id.clone(),
            revision: envelope.revision,
            is_edited: lap_lib::develop::recipe_repository::is_edited_envelope(&envelope),
            is_virtual_copy: true,
            content_hash: envelope
                .content_hash()
                .map_err(|e| format!("created virtual copy hash failed: {e}"))?,
            sidecar_path: lap_lib::develop::variants::variant_sidecar_path(
                &source_path,
                &envelope.variant_id,
            ),
            exists: true,
        })
    })
    .await
    .map_err(|e| format!("develop create-virtual-copy task failed: {e}"))?
}

/// Clears one variant's recipe/decode/resources under an explicit CAS
/// revision bump. Other variants, resource store objects and the source are
/// untouched.
#[tauri::command]
pub async fn develop_reset_variant(
    asset_id: i64,
    variant_id: String,
    expected_revision: u64,
) -> Result<VariantResetDto, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let source_path = develop_asset_source_path(asset_id)?;
        let repo = DevelopRecipeRepository::lap_default();
        let conn = t_sqlite::open_conn().map_err(|e| e.to_string())?;
        let receipt = lap_lib::develop::variants::reset_variant(
            &repo,
            Some(&conn),
            &source_path,
            &variant_id,
            expected_revision,
        )
        .map_err(|e| e.to_string())?;
        Ok(VariantResetDto {
            variant_id,
            revision: receipt.revision,
            content_hash: receipt.content_hash,
            sidecar_path: receipt.sidecar_path,
        })
    })
    .await
    .map_err(|e| format!("develop reset-variant task failed: {e}"))?
}

/// Deletes one virtual copy: its sidecar, retained previous revision and
/// projection row. The primary variant is protected; every other variant and
/// the shared resource store are untouched.
#[tauri::command]
pub async fn develop_delete_variant(asset_id: i64, variant_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let source_path = develop_asset_source_path(asset_id)?;
        let conn = t_sqlite::open_conn().map_err(|e| e.to_string())?;
        lap_lib::develop::variants::delete_variant(Some(&conn), &source_path, &variant_id)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("develop delete-variant task failed: {e}"))?
}

/// One batch recipe application request from the frontend.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchApplyItemDto {
    pub asset_id: i64,
    pub variant_id: String,
    pub recipe: serde_json::Value,
    pub expected_revision: Option<u64>,
}

/// Applies one recipe to many assets: explicit per-asset CAS commits, one
/// progress event per item, visible per-item failures, cooperative
/// cancellation (`develop_cancel_export` on `batchJob`). Bounded by the
/// module's max-items limit before any write.
#[tauri::command]
pub async fn develop_batch_apply_recipes(
    state: tauri::State<'_, DevelopAppState>,
    export_jobs: tauri::State<'_, std::sync::Arc<DevelopExportJobs>>,
    app_handle: tauri::AppHandle,
    items: Vec<BatchApplyItemDto>,
    batch_job: String,
) -> Result<lap_lib::develop::batch::BatchSummary, String> {
    if batch_job.is_empty() {
        return Err("batch_job id is required for cancellation".to_string());
    }
    let service = state.service();
    let (cancel, _slot, guard) = export_jobs.register(&batch_job, std::sync::Arc::clone(&service));
    let progress_handle = app_handle.clone();
    let progress_job = batch_job.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let result = (|| -> Result<lap_lib::develop::batch::BatchSummary, String> {
            let conn = t_sqlite::open_conn().map_err(|e| e.to_string())?;
            let repo = DevelopRecipeRepository::lap_default();
            let mut batch_items = Vec::with_capacity(items.len());
            for item in &items {
                let source_path = develop_asset_source_path(item.asset_id)?;
                let recipe: rapidraw_edit_model::Recipe =
                    serde_json::from_value(item.recipe.clone()).map_err(|e| {
                        format!("invalid batch recipe for asset {}: {e}", item.asset_id)
                    })?;
                batch_items.push(lap_lib::develop::batch::BatchApplyItem {
                    asset_id: item.asset_id.to_string(),
                    variant_id: item.variant_id.clone(),
                    source_path,
                    recipe,
                    expected_revision: item.expected_revision,
                    fault: None,
                });
            }
            lap_lib::develop::batch::apply_recipes(
                &repo,
                Some(&conn),
                &batch_items,
                &cancel,
                lap_lib::develop::batch::BatchLimits::default(),
                &move |index, total, result| {
                    let _ = progress_handle.emit(
                        "develop_batch_progress",
                        serde_json::json!({
                            "job": progress_job,
                            "index": index,
                            "total": total,
                            "key": result.key,
                            "status": result.status,
                        }),
                    );
                },
            )
            .map_err(|e| e.to_string())
        })();
        drop(guard);
        result
    })
    .await
    .map_err(|e| format!("develop batch apply task failed: {e}"))?
}

/// One batch export request from the frontend.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchExportItemDto {
    pub asset_id: i64,
    pub variant_id: String,
    pub revision: u64,
    pub destination: String,
}

/// Exports many committed recipe revisions as derivatives: sequential items
/// with capped output edges (bounded memory), per-item source hashing and
/// identity checks, per-item failures/cancellation visible in the summary.
/// Failed or skipped items are never counted as exported.
#[tauri::command]
pub async fn develop_batch_export(
    state: tauri::State<'_, DevelopAppState>,
    export_jobs: tauri::State<'_, std::sync::Arc<DevelopExportJobs>>,
    app_handle: tauri::AppHandle,
    items: Vec<BatchExportItemDto>,
    format: String,
    quality: Option<u8>,
    max_edge: Option<u32>,
    batch_job: String,
) -> Result<lap_lib::develop::batch::BatchSummary, String> {
    if batch_job.is_empty() {
        return Err("batch_job id is required for cancellation".to_string());
    }
    let format = lap_lib::develop::export::ExportFormat::parse(&format)
        .ok_or_else(|| format!("unknown export format '{format}' (expected 'png' or 'jpeg')"))?;
    let settings = lap_lib::develop::export::ExportSettings {
        destination: std::path::PathBuf::new(),
        format,
        jpeg_quality: quality.unwrap_or(90),
        max_edge,
    };
    let service = state.service();
    let (cancel, _slot, guard) = export_jobs.register(&batch_job, std::sync::Arc::clone(&service));
    let progress_handle = app_handle.clone();
    let progress_job = batch_job.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let result = (|| -> Result<lap_lib::develop::batch::BatchSummary, String> {
            let mut batch_items = Vec::with_capacity(items.len());
            for item in &items {
                let source_path = develop_asset_source_path(item.asset_id)?;
                // Identity adoption (lap-487): exports resolve the committed
                // sidecar by identity, so a copy/external-move asset is
                // re-keyed first; replaced media is refused explicitly inside
                // the batch item (it re-hashes the source itself).
                if let Ok(source_bytes) = fs::read(&source_path) {
                    lap_lib::develop::asset_operations::adopt_catalog_identity_with_fingerprint(
                        &source_path,
                        &item.asset_id.to_string(),
                        &rapidraw_edit_model::sha256_hex(&source_bytes),
                    )
                    .map_err(|e| format!("develop recipe identity check failed: {e}"))?;
                }
                batch_items.push(lap_lib::develop::batch::BatchExportItem {
                    asset_id: item.asset_id.to_string(),
                    variant_id: item.variant_id.clone(),
                    source_path,
                    requested_revision: item.revision,
                    destination: std::path::PathBuf::from(&item.destination),
                });
            }
            lap_lib::develop::batch::export_developed_batch(
                &service,
                &batch_items,
                settings,
                &cancel,
                lap_lib::develop::batch::BatchExportLimits::default(),
                &move |index, total, result| {
                    let _ = progress_handle.emit(
                        "develop_batch_progress",
                        serde_json::json!({
                            "job": progress_job,
                            "index": index,
                            "total": total,
                            "key": result.key,
                            "status": result.status,
                        }),
                    );
                },
            )
            .map_err(|e| e.to_string())
        })();
        drop(guard);
        result
    })
    .await
    .map_err(|e| format!("develop batch export task failed: {e}"))?
}

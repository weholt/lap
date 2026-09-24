use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const MAX_SIDECAR_BYTES: u64 = 2 * 1024 * 1024;

pub fn bound_sidecar(media_path: &Path) -> Option<PathBuf> {
    let parent = media_path.parent()?;
    let file_name = media_path.file_name()?.to_str()?;
    if let Some(exact) = find_named(parent, &format!("{file_name}.xmp")) {
        if acceptable(&exact) {
            return Some(exact);
        }
    }
    let stem = media_path.file_stem()?.to_str()?;
    let stem_sidecar = find_named(parent, &format!("{stem}.xmp"))?;
    if !acceptable(&stem_sidecar) {
        return None;
    }
    let siblings = media_count_with_stem(parent, stem);
    if siblings <= 1 || is_raw(media_path) {
        Some(stem_sidecar)
    } else {
        None
    }
}

pub fn read_sidecar_xml(media_path: &Path) -> Option<String> {
    let path = bound_sidecar(media_path)?;
    let metadata = fs::metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_SIDECAR_BYTES {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn sidecar_stamp(media_path: &Path) -> Option<String> {
    let path = bound_sidecar(media_path)?;
    let metadata = fs::metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_SIDECAR_BYTES {
        return None;
    }
    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(format!("{}|{modified}|{}", normalize_path(&path), metadata.len()))
}

pub fn xmp_destination_for(source_media: &Path, sidecar: &Path, dest_media: &Path) -> Option<PathBuf> {
    let sidecar_name = sidecar.file_name()?.to_str()?;
    let source_name = source_media.file_name()?.to_str()?;
    let source_stem = source_media.file_stem()?.to_str()?;
    let dest_name = dest_media.file_name()?.to_str()?;
    let dest_stem = dest_media.file_stem()?.to_str()?;
    let sidecar_lower = sidecar_name.to_ascii_lowercase();
    let new_name = if sidecar_lower == format!("{source_name}.xmp").to_ascii_lowercase() {
        format!("{dest_name}.xmp")
    } else if sidecar_lower == format!("{source_stem}.xmp").to_ascii_lowercase() {
        format!("{dest_stem}.xmp")
    } else {
        return None;
    };
    Some(dest_media.parent()?.join(new_name))
}

pub fn plan_xmp_transfer(source_media: &Path, dest_media: &Path) -> Result<Option<(PathBuf, PathBuf)>, String> {
    let Some(old_path) = bound_sidecar(source_media) else {
        return Ok(None);
    };
    let Some(new_path) = xmp_destination_for(source_media, &old_path, dest_media) else {
        return Ok(None);
    };
    if old_path == new_path {
        return Ok(None);
    }
    if new_path.exists() {
        return Err(format!("Destination sidecar already exists: {}", new_path.display()));
    }
    Ok(Some((old_path, new_path)))
}

pub fn copy_bound_xmp_for_import(source_file_path: &str, destination_file_path: &str) -> Result<Vec<PathBuf>, String> {
    let source = Path::new(source_file_path);
    let destination = Path::new(destination_file_path);
    match plan_xmp_transfer(source, destination)? {
        Some((old_path, new_path)) => {
            fs::copy(&old_path, &new_path).map_err(|error| {
                format!("Failed to copy sidecar '{}': {}", old_path.display(), error)
            })?;
            Ok(vec![new_path])
        }
        None => Ok(Vec::new()),
    }
}

fn find_named(directory: &Path, expected: &str) -> Option<PathBuf> {
    let direct = directory.join(expected);
    if direct.is_file() {
        return Some(direct);
    }
    for entry in fs::read_dir(directory).ok()?.flatten() {
        if entry.file_name().to_string_lossy().eq_ignore_ascii_case(expected) && entry.path().is_file() {
            return Some(entry.path());
        }
    }
    None
}

fn acceptable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else { return false };
    metadata.is_file()
        && metadata.len() <= MAX_SIDECAR_BYTES
        && path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| extension.eq_ignore_ascii_case("xmp"))
        && path.parent().is_some()
}

fn media_count_with_stem(directory: &Path, stem: &str) -> usize {
    fs::read_dir(directory)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            let path = entry.path();
            path.is_file()
                && path.file_stem().and_then(|value| value.to_str()).is_some_and(|value| value.eq_ignore_ascii_case(stem))
                && crate::t_utils::get_file_type(&path.to_string_lossy()).is_some()
        })
        .count()
}

fn is_raw(path: &Path) -> bool {
    crate::t_utils::get_file_type(&path.to_string_lossy()) == Some(3)
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lap-sidecar-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn file_metadata_sidecar_exact_wins_and_shared_stem_binds_only_to_raw() {
        let dir = scratch("bind");
        fs::write(dir.join("IMG.CR2"), b"raw").unwrap();
        fs::write(dir.join("IMG.JPG"), b"jpg").unwrap();
        fs::write(dir.join("IMG.xmp"), b"<x:xmpmeta/>").unwrap();
        fs::write(dir.join("IMG.JPG.xmp"), b"<x:xmpmeta>exact</x:xmpmeta>").unwrap();
        fs::write(dir.join("IMG.AAE"), b"aae").unwrap();
        assert_eq!(bound_sidecar(&dir.join("IMG.JPG")).unwrap().file_name().unwrap(), "IMG.JPG.xmp");
        assert_eq!(bound_sidecar(&dir.join("IMG.CR2")).unwrap().file_name().unwrap(), "IMG.xmp");
        assert!(read_sidecar_xml(&dir.join("IMG.JPG")).unwrap().contains("exact"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_metadata_sidecar_lone_jpeg_uses_stem_and_oversized_is_ignored() {
        let dir = scratch("lone");
        let jpeg = dir.join("solo.jpg");
        fs::write(&jpeg, b"jpg").unwrap();
        fs::write(dir.join("solo.xmp"), b"<x:xmpmeta>stem</x:xmpmeta>").unwrap();
        fs::write(dir.join("solo.aae"), b"nope").unwrap();
        assert_eq!(bound_sidecar(&jpeg).unwrap().file_name().unwrap(), "solo.xmp");
        let huge = dir.join("big.jpg");
        fs::write(&huge, b"jpg").unwrap();
        fs::write(dir.join("big.jpg.xmp"), vec![0u8; (MAX_SIDECAR_BYTES + 1) as usize]).unwrap();
        assert!(bound_sidecar(&huge).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_metadata_transfer_renames_exact_and_stem_and_rejects_existing_destination() {
        let dir = scratch("move");
        let raw = dir.join("photo.CR2");
        let jpeg = dir.join("photo.JPG");
        fs::write(&raw, b"raw").unwrap();
        fs::write(&jpeg, b"jpg").unwrap();
        fs::write(dir.join("photo.xmp"), b"<x:xmpmeta/>").unwrap();
        fs::write(dir.join("photo.JPG.xmp"), b"<x:xmpmeta/>").unwrap();
        fs::write(dir.join("photo.aae"), b"aae").unwrap();
        let raw_dest = dir.join("photo(1).CR2");
        let (old, new) = plan_xmp_transfer(&raw, &raw_dest).unwrap().unwrap();
        assert_eq!(old.file_name().unwrap(), "photo.xmp");
        assert_eq!(new.file_name().unwrap(), "photo(1).xmp");
        assert!(plan_xmp_transfer(&jpeg, &dir.join("photo(1).JPG")).unwrap().unwrap().0.ends_with("photo.JPG.xmp"));
        fs::write(dir.join("photo(1).xmp"), b"taken").unwrap();
        assert!(plan_xmp_transfer(&raw, &raw_dest).unwrap_err().contains("already exists"));
        let jpeg_only = dir.join("other.jpg");
        fs::write(&jpeg_only, b"jpg").unwrap();
        fs::write(dir.join("other.aae"), b"aae").unwrap();
        assert!(bound_sidecar(&jpeg_only).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}

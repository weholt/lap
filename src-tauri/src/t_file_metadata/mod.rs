//! IPTC, XMP, and sidecar metadata.
//!
//! Capture readers (EXIF, LibRaw) stay authoritative for exposure and camera
//! fields. Descriptive fields can be read from the file and written back.

mod iptc;
mod packets;
mod sidecar;
mod write;
mod xmp;

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

pub use iptc::parse_iptc;
pub use packets::extract_packets;
pub use sidecar::{
    bound_sidecar, copy_bound_xmp_for_import, read_sidecar_xml, sidecar_stamp,
    xmp_destination_for,
};
pub use write::{apply_metadata_patch, write_descriptive_metadata, MetadataChange};
pub use xmp::parse_xmp;

pub const EMBEDDED_METADATA_VERSION: i64 = 3;
const MAX_KEYWORDS: usize = 64;
const MAX_KEYWORD_CHARS: usize = 128;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileMetadata {
    pub title: Option<String>,
    pub headline: Option<String>,
    pub description: Option<String>,
    pub keywords: Vec<String>,
    pub creator: Option<String>,
    pub copyright: Option<String>,
    pub credit: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub label: Option<String>,
    pub embedded_rating: Option<i32>,
    pub capture_date: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub software: Option<String>,
    pub lens_make: Option<String>,
    pub lens_model: Option<String>,
    pub exposure_time: Option<String>,
    pub f_number: Option<String>,
    pub focal_length: Option<String>,
    pub iso_speed: Option<String>,
    pub exposure_bias: Option<String>,
    pub flash: Option<String>,
    pub gps_latitude: Option<f64>,
    pub gps_longitude: Option<f64>,
    pub gps_altitude: Option<f64>,
}

/// Capture fields already chosen by EXIF or LibRaw. Supplemental metadata may
/// fill only the empty ones. Orientation and pixel size are intentionally absent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CaptureFields {
    pub make: Option<String>,
    pub model: Option<String>,
    pub software: Option<String>,
    pub artist: Option<String>,
    pub copyright: Option<String>,
    pub description: Option<String>,
    pub lens_make: Option<String>,
    pub lens_model: Option<String>,
    pub exposure_bias: Option<String>,
    pub exposure_time: Option<String>,
    pub f_number: Option<String>,
    pub focal_length: Option<String>,
    pub iso_speed: Option<String>,
    pub flash: Option<String>,
    pub date_time: Option<String>,
    pub taken_date: Option<i64>,
    pub file_modified: Option<i64>,
    pub gps_latitude: Option<f64>,
    pub gps_longitude: Option<f64>,
    pub gps_altitude: Option<f64>,
}

pub fn sanitize_rating(value: Option<i32>) -> Option<i32> {
    value.and_then(normalize_rating_number)
}

/// Star ratings are 0 through 5. Windows and some XMP writers store 1, 25, 50, 75, or 99.
pub fn normalize_rating_number(value: i32) -> Option<i32> {
    match value {
        0..=5 => Some(value),
        25 => Some(2),
        50 => Some(3),
        75 => Some(4),
        99 | 100 => Some(5),
        _ => None,
    }
}

pub fn recorded_place(metadata: &FileMetadata) -> Option<String> {
    let parts = [metadata.city.as_deref(), metadata.state.as_deref(), metadata.country.as_deref()]
        .into_iter()
        .filter_map(|part| {
            let part = part?.trim();
            if part.is_empty() { None } else { Some(part.to_string()) }
        })
        .collect::<Vec<_>>();
    if parts.is_empty() { None } else { Some(parts.join(", ")) }
}

pub fn keywords_to_json(keywords: &[String]) -> Option<String> {
    if keywords.is_empty() {
        None
    } else {
        serde_json::to_string(keywords).ok()
    }
}

pub fn keywords_from_json(value: Option<String>) -> Vec<String> {
    value
        .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
        .unwrap_or_default()
}

/// Highest-precedence layer first. Empty strings do not override a later value.
pub fn merge_layers(layers: &[FileMetadata]) -> FileMetadata {
    let mut merged = FileMetadata::default();
    for layer in layers {
        fill_text(&mut merged.title, &layer.title);
        fill_text(&mut merged.headline, &layer.headline);
        fill_text(&mut merged.description, &layer.description);
        fill_text(&mut merged.creator, &layer.creator);
        fill_text(&mut merged.copyright, &layer.copyright);
        fill_text(&mut merged.credit, &layer.credit);
        fill_text(&mut merged.city, &layer.city);
        fill_text(&mut merged.state, &layer.state);
        fill_text(&mut merged.country, &layer.country);
        fill_text(&mut merged.label, &layer.label);
        fill_text(&mut merged.capture_date, &layer.capture_date);
        fill_text(&mut merged.make, &layer.make);
        fill_text(&mut merged.model, &layer.model);
        fill_text(&mut merged.software, &layer.software);
        fill_text(&mut merged.lens_make, &layer.lens_make);
        fill_text(&mut merged.lens_model, &layer.lens_model);
        fill_text(&mut merged.exposure_time, &layer.exposure_time);
        fill_text(&mut merged.f_number, &layer.f_number);
        fill_text(&mut merged.focal_length, &layer.focal_length);
        fill_text(&mut merged.iso_speed, &layer.iso_speed);
        fill_text(&mut merged.exposure_bias, &layer.exposure_bias);
        fill_text(&mut merged.flash, &layer.flash);
        if merged.embedded_rating.is_none() {
            merged.embedded_rating = sanitize_rating(layer.embedded_rating);
        }
        if merged.gps_latitude.is_none() && merged.gps_longitude.is_none() {
            if let (Some(latitude), Some(longitude)) = (layer.gps_latitude, layer.gps_longitude) {
                merged.gps_latitude = Some(latitude);
                merged.gps_longitude = Some(longitude);
                merged.gps_altitude = layer.gps_altitude;
            }
        }
        for keyword in &layer.keywords {
            push_keyword(&mut merged.keywords, keyword);
        }
    }
    merged
}

pub fn fill_capture_gaps(destination: &mut CaptureFields, source: &FileMetadata) {
    fill_text(&mut destination.make, &source.make);
    fill_text(&mut destination.model, &source.model);
    fill_text(&mut destination.software, &source.software);
    fill_text(&mut destination.artist, &source.creator);
    fill_text(&mut destination.copyright, &source.copyright);
    fill_text(&mut destination.description, &source.description);
    fill_text(&mut destination.lens_make, &source.lens_make);
    fill_text(&mut destination.lens_model, &source.lens_model);
    fill_text(&mut destination.exposure_bias, &source.exposure_bias);
    fill_text(&mut destination.exposure_time, &source.exposure_time);
    fill_text(&mut destination.f_number, &source.f_number);
    fill_text(&mut destination.focal_length, &source.focal_length);
    fill_text(&mut destination.iso_speed, &source.iso_speed);
    fill_text(&mut destination.flash, &source.flash);
    if destination.taken_date == destination.file_modified {
        if let Some(capture_date) = source.capture_date.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
            if let Some(timestamp) = crate::t_utils::meta_date_to_timestamp(capture_date) {
                destination.taken_date = Some(timestamp);
                fill_text(&mut destination.date_time, &Some(capture_date.to_string()));
            }
        }
    }
    if destination.gps_latitude.is_none() && destination.gps_longitude.is_none() {
        if let (Some(latitude), Some(longitude)) = (source.gps_latitude, source.gps_longitude) {
            destination.gps_latitude = Some(latitude);
            destination.gps_longitude = Some(longitude);
            destination.gps_altitude = source.gps_altitude;
        }
    }
}

pub fn metadata_refresh_due(
    version: Option<i64>,
    stored_stamp: Option<&str>,
    current_stamp: Option<&str>,
) -> bool {
    version.unwrap_or(0) < EMBEDDED_METADATA_VERSION || stored_stamp != current_stamp
}

/// Read supplemental metadata for an image. Parser failures become empty layers.
pub fn read_supplemental_metadata(path: &Path) -> FileMetadata {
    let packets = extract_packets(path);
    let mut layers = Vec::new();
    if let Some(xml) = read_sidecar_xml(path) {
        layers.push(parse_layer(|| parse_xmp(&xml)));
    }
    if let Some(xml) = packets.xmp {
        layers.push(parse_layer(|| parse_xmp(&xml)));
    }
    if let Some(iptc) = packets.iptc {
        layers.push(parse_layer(|| parse_iptc(&iptc)));
    }
    merge_layers(&layers)
}

fn parse_layer(parse: impl FnOnce() -> FileMetadata) -> FileMetadata {
    catch_unwind(AssertUnwindSafe(parse)).unwrap_or_default()
}

fn fill_text(destination: &mut Option<String>, source: &Option<String>) {
    if destination.as_ref().is_some_and(|value| !value.trim().is_empty()) {
        return;
    }
    if let Some(value) = source.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
        *destination = Some(value.to_string());
    }
}

pub fn assign_keywords(metadata: &mut FileMetadata, keywords: impl IntoIterator<Item = impl AsRef<str>>) {
    metadata.keywords.clear();
    for keyword in keywords {
        push_keyword(&mut metadata.keywords, keyword.as_ref());
    }
}

fn push_keyword(keywords: &mut Vec<String>, raw: &str) {
    if keywords.len() >= MAX_KEYWORDS {
        return;
    }
    let mut cleaned = String::new();
    for character in raw.chars() {
        if !character.is_control() {
            cleaned.push(character);
        }
    }
    let cleaned: String = cleaned.trim().chars().take(MAX_KEYWORD_CHARS).collect();
    if cleaned.is_empty() || keywords.iter().any(|existing| existing == &cleaned) {
        return;
    }
    keywords.push(cleaned);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn layer_with_title(title: &str) -> FileMetadata {
        FileMetadata { title: Some(title.to_string()), ..FileMetadata::default() }
    }

    #[test]
    fn file_metadata_merge_first_nonempty_wins_and_blank_does_not_override() {
        let merged = merge_layers(&[
            FileMetadata { title: Some("  ".to_string()), creator: Some("Sidecar".to_string()), ..FileMetadata::default() },
            layer_with_title("Embedded"),
        ]);
        assert_eq!(merged.title.as_deref(), Some("Embedded"));
        assert_eq!(merged.creator.as_deref(), Some("Sidecar"));
    }

    #[test]
    fn file_metadata_merge_keywords_keep_order_drop_duplicates_and_cap() {
        let mut first = FileMetadata::default();
        first.keywords.push("  Oslo\n".to_string());
        first.keywords.push("Oslo".to_string());
        let mut second = FileMetadata::default();
        for index in 0..70 {
            second.keywords.push(format!("k{index}"));
        }
        let merged = merge_layers(&[first, second]);
        assert_eq!(merged.keywords[0], "Oslo");
        assert_eq!(merged.keywords.len(), 64);
        assert_eq!(merged.keywords[1], "k0");
        assert!(!merged.keywords.contains(&"k63".to_string()));
    }

    #[test]
    fn file_metadata_merge_rating_keeps_zero_and_rejects_out_of_range() {
        let merged = merge_layers(&[
            FileMetadata { embedded_rating: Some(-1), ..FileMetadata::default() },
            FileMetadata { embedded_rating: Some(0), ..FileMetadata::default() },
        ]);
        assert_eq!(merged.embedded_rating, Some(0));
        let too_high = merge_layers(&[FileMetadata { embedded_rating: Some(6), ..FileMetadata::default() }]);
        assert_eq!(too_high.embedded_rating, None);
    }

    #[test]
    fn file_metadata_gaps_keep_existing_make_and_fill_missing_gps() {
        let mut capture = CaptureFields {
            make: Some("Canon".to_string()),
            file_modified: Some(10),
            taken_date: Some(10),
            ..CaptureFields::default()
        };
        let source = FileMetadata {
            make: Some("Nikon".to_string()),
            creator: Some("Ada".to_string()),
            capture_date: Some("2020:01:02 03:04:05".to_string()),
            gps_latitude: Some(59.9),
            gps_longitude: Some(10.7),
            gps_altitude: Some(12.0),
            ..FileMetadata::default()
        };
        fill_capture_gaps(&mut capture, &source);
        assert_eq!(capture.make.as_deref(), Some("Canon"));
        assert_eq!(capture.artist.as_deref(), Some("Ada"));
        assert_eq!(capture.gps_latitude, Some(59.9));
        assert_eq!(capture.gps_longitude, Some(10.7));
        assert!(capture.taken_date.is_some_and(|value| value != 10));
        assert_eq!(capture.date_time.as_deref(), Some("2020:01:02 03:04:05"));
    }

    #[test]
    fn file_metadata_gaps_do_not_replace_a_real_capture_date_or_existing_gps() {
        let mut capture = CaptureFields {
            taken_date: Some(123),
            file_modified: Some(10),
            gps_latitude: Some(1.0),
            gps_longitude: Some(2.0),
            ..CaptureFields::default()
        };
        fill_capture_gaps(&mut capture, &FileMetadata {
            capture_date: Some("2024:05:06 07:08:09".to_string()),
            gps_latitude: Some(9.0),
            gps_longitude: Some(8.0),
            ..FileMetadata::default()
        });
        assert_eq!(capture.taken_date, Some(123));
        assert_eq!(capture.gps_latitude, Some(1.0));
    }

    #[test]
    fn file_metadata_index_sidecar_keywords_override_and_broken_sidecar_keeps_iptc() {
        let dir = std::env::temp_dir().join(format!("lap-meta-index-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let image = dir.join("photo.jpg");
        let iptc = iptc_block();
        let xmp = b"http://ns.adobe.com/xap/1.0/\0<?xpacket begin=\"\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:subject><rdf:Bag><rdf:li>embedded</rdf:li></rdf:Bag></dc:subject></rdf:Description></rdf:RDF></x:xmpmeta>";
        let mut jpeg = vec![0xFF, 0xD8];
        push_segment(&mut jpeg, 0xE1, xmp);
        push_segment(&mut jpeg, 0xED, &iptc);
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        fs::write(&image, &jpeg).unwrap();
        fs::write(dir.join("photo.jpg.xmp"), r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title><rdf:Alt><rdf:li xml:lang="x-default">Sidecar title</rdf:li></rdf:Alt></dc:title><dc:subject><rdf:Bag><rdf:li>sidecar</rdf:li></rdf:Bag></dc:subject></rdf:Description></rdf:RDF></x:xmpmeta>"#).unwrap();

        let metadata = read_supplemental_metadata(&image);
        assert_eq!(metadata.title.as_deref(), Some("Sidecar title"));
        assert_eq!(metadata.keywords, vec!["sidecar".to_string(), "embedded".to_string(), "iptc-key".to_string()]);

        fs::write(dir.join("photo.jpg.xmp"), "<not-xml").unwrap();
        let metadata = read_supplemental_metadata(&image);
        assert_eq!(metadata.keywords, vec!["embedded".to_string(), "iptc-key".to_string()]);
        assert!(metadata.title.is_none());

        fs::remove_file(dir.join("photo.jpg.xmp")).unwrap();
        let metadata = read_supplemental_metadata(&image);
        assert_eq!(metadata.keywords[0], "embedded");
        assert!(metadata.title.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_metadata_index_xmp_gps_fills_and_orientation_is_not_a_field() {
        let dir = std::env::temp_dir().join(format!("lap-meta-gps-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let image = dir.join("scan.png");
        let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:exif="http://ns.adobe.com/exif/1.0/" exif:GPSLatitude="59,54.0N" exif:GPSLongitude="10,45.0E" tiff:Orientation="6" xmlns:tiff="http://ns.adobe.com/tiff/1.0/"/></rdf:RDF></x:xmpmeta>"#;
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut chunk = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
        chunk.extend_from_slice(xml.as_bytes());
        push_png_chunk(&mut png, b"iTXt", &chunk);
        push_png_chunk(&mut png, b"IEND", b"");
        fs::write(&image, png).unwrap();
        let metadata = read_supplemental_metadata(&image);
        assert!(metadata.gps_latitude.is_some_and(|value| (value - 59.9).abs() < 0.01));
        assert!(metadata.gps_longitude.is_some_and(|value| (value - 10.75).abs() < 0.01));
        let mut capture = CaptureFields::default();
        fill_capture_gaps(&mut capture, &metadata);
        assert!(capture.gps_latitude.is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    fn iptc_block() -> Vec<u8> {
        let mut iim = vec![0x1C, 2, 25];
        let keyword = b"iptc-key";
        iim.extend_from_slice(&(keyword.len() as u16).to_be_bytes());
        iim.extend_from_slice(keyword);
        let mut block = b"Photoshop 3.0\0".to_vec();
        block.extend_from_slice(b"8BIM");
        block.extend_from_slice(&0x0404u16.to_be_bytes());
        block.extend_from_slice(&[0, 0]);
        block.extend_from_slice(&(iim.len() as u32).to_be_bytes());
        block.extend_from_slice(&iim);
        block
    }

    fn push_segment(out: &mut Vec<u8>, marker: u8, payload: &[u8]) {
        out.push(0xFF);
        out.push(marker);
        let len = (payload.len() + 2) as u16;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(payload);
    }

    fn push_png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&[0, 0, 0, 0]);
    }

    #[test]
    fn file_metadata_refresh_matching_stamp_is_not_due_and_removal_is() {
        assert!(!metadata_refresh_due(Some(EMBEDDED_METADATA_VERSION), Some("a|1|2"), Some("a|1|2")));
        assert!(metadata_refresh_due(None, Some("a|1|2"), Some("a|1|2")));
        assert!(metadata_refresh_due(Some(1), Some("a|1|2"), None));
        let dir = std::env::temp_dir().join(format!("lap-meta-stamp-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let image = dir.join("only.jpg");
        fs::write(&image, [0xFF, 0xD8, 0xFF, 0xD9]).unwrap();
        let sidecar = dir.join("only.jpg.xmp");
        let mut file = fs::File::create(&sidecar).unwrap();
        write!(file, "<x:xmpmeta></x:xmpmeta>").unwrap();
        let stamp = sidecar_stamp(&image);
        assert!(stamp.is_some());
        fs::remove_file(&sidecar).unwrap();
        assert!(sidecar_stamp(&image).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}

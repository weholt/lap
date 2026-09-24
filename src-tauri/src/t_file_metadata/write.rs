//! Write descriptive metadata back to a photo.
//!
//! JPEG files get an updated XMP packet and an IPTC IIM record, leaving the
//! image data untouched. Other formats, and any photo that already has a
//! sidecar, get an XMP sidecar. An existing sidecar is updated in place so
//! unrelated XMP (such as edit settings) stays put.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Reader;
use quick_xml::Writer;
use serde::Deserialize;

use super::sidecar::bound_sidecar;
use super::{assign_keywords, sanitize_rating, FileMetadata};

const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const XMP_EXTENSION_HEADER: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
const PHOTOSHOP_HEADER: &[u8] = b"Photoshop 3.0\0";
const MAX_SEGMENT_PAYLOAD: usize = 65533;

const OWNED_PROPERTIES: &[&str] = &[
    "title",
    "description",
    "subject",
    "creator",
    "rights",
    "Headline",
    "Credit",
    "City",
    "State",
    "Country",
    "Rating",
    "Label",
    "GPSLatitude",
    "GPSLongitude",
    "GPSAltitude",
];

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MetadataChange {
    pub title: Option<String>,
    pub headline: Option<String>,
    pub description: Option<String>,
    pub creator: Option<String>,
    pub copyright: Option<String>,
    pub credit: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub label: Option<String>,
    pub rating: Option<i32>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub keywords: Option<Vec<String>>,
    pub add_keywords: Vec<String>,
    pub remove_keywords: Vec<String>,
    pub clear: Vec<String>,
}

impl MetadataChange {
    pub fn touches_location(&self) -> bool {
        self.city.is_some()
            || self.state.is_some()
            || self.country.is_some()
            || self.clear.iter().any(|field| {
                matches!(field.as_str(), "city" | "state" | "country" | "location")
            })
    }

    pub fn touches_gps(&self) -> bool {
        self.latitude.is_some()
            || self.longitude.is_some()
            || self.clear.iter().any(|field| field == "gps")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataWriteReport {
    pub sidecar: bool,
    pub embedded_xmp: bool,
    pub embedded_iptc: bool,
}

pub fn apply_metadata_patch(current: &FileMetadata, change: &MetadataChange) -> Result<FileMetadata, String> {
    if change.touches_gps() && !change.clear.iter().any(|field| field == "gps") {
        match (change.latitude, change.longitude) {
            (Some(latitude), Some(longitude))
                if latitude.is_finite()
                    && longitude.is_finite()
                    && (-90.0..=90.0).contains(&latitude)
                    && (-180.0..=180.0).contains(&longitude) => {}
            (None, None) => {}
            _ => return Err("Latitude and longitude must be set together, within range".to_string()),
        }
    }

    let mut updated = current.clone();
    let clear = |name: &str| change.clear.iter().any(|field| field == name);
    set_text(&mut updated.title, &change.title, clear("title"));
    set_text(&mut updated.headline, &change.headline, clear("headline"));
    set_text(&mut updated.description, &change.description, clear("description"));
    set_text(&mut updated.creator, &change.creator, clear("creator"));
    set_text(&mut updated.copyright, &change.copyright, clear("copyright"));
    set_text(&mut updated.credit, &change.credit, clear("credit"));
    set_text(&mut updated.city, &change.city, clear("city") || clear("location"));
    set_text(&mut updated.state, &change.state, clear("state") || clear("location"));
    set_text(&mut updated.country, &change.country, clear("country") || clear("location"));
    set_text(&mut updated.label, &change.label, clear("label"));

    if clear("rating") {
        updated.embedded_rating = None;
    } else if let Some(rating) = change.rating {
        updated.embedded_rating = sanitize_rating(Some(rating));
        if updated.embedded_rating.is_none() {
            return Err("Rating must be from 0 to 5".to_string());
        }
    }

    if clear("gps") {
        updated.gps_latitude = None;
        updated.gps_longitude = None;
        updated.gps_altitude = None;
    } else if let (Some(latitude), Some(longitude)) = (change.latitude, change.longitude) {
        updated.gps_latitude = Some(latitude);
        updated.gps_longitude = Some(longitude);
    }

    let mut keywords = if clear("keywords") {
        Vec::new()
    } else if let Some(keywords) = &change.keywords {
        keywords.clone()
    } else {
        updated.keywords.clone()
    };
    if !clear("keywords") {
        keywords.extend(change.add_keywords.iter().cloned());
    }
    let removed: Vec<String> = change
        .remove_keywords
        .iter()
        .map(|keyword| keyword.trim().to_lowercase())
        .filter(|keyword| !keyword.is_empty())
        .collect();
    keywords.retain(|keyword| !removed.iter().any(|removed| removed == &keyword.to_lowercase()));
    assign_keywords(&mut updated, keywords);
    Ok(updated)
}

fn set_text(slot: &mut Option<String>, incoming: &Option<String>, clear: bool) {
    if clear {
        *slot = None;
        return;
    }
    if let Some(value) = incoming {
        let value = value.trim();
        *slot = if value.is_empty() { None } else { Some(value.to_string()) };
    }
}

pub fn write_descriptive_metadata(path: &Path, metadata: &FileMetadata) -> Result<MetadataWriteReport, String> {
    if !path.is_file() {
        return Err(format!("Photo not found: {}", path.display()));
    }
    let existing_sidecar = bound_sidecar(path);
    let existing_xmp = existing_sidecar
        .as_deref()
        .and_then(|sidecar| fs::read_to_string(sidecar).ok())
        .or_else(|| super::extract_packets(path).xmp);
    let xmp = upsert_xmp(existing_xmp.as_deref(), metadata)?;
    let iim = render_iptc(metadata);

    let mut report = MetadataWriteReport {
        sidecar: false,
        embedded_xmp: false,
        embedded_iptc: false,
    };

    if is_jpeg(path) {
        match embed_jpeg(path, &xmp, &iim) {
            Ok((xmp_written, iptc_written)) => {
                report.embedded_xmp = xmp_written;
                report.embedded_iptc = iptc_written;
            }
            Err(error) => {
                if existing_sidecar.is_none() && xmp.len() + XMP_HEADER.len() > MAX_SEGMENT_PAYLOAD {
                    write_sidecar(path, existing_sidecar.as_deref(), &xmp)?;
                    report.sidecar = true;
                    return Ok(report);
                }
                return Err(error);
            }
        }
    }

    if existing_sidecar.is_some() || !report.embedded_xmp {
        write_sidecar(path, existing_sidecar.as_deref(), &xmp)?;
        report.sidecar = true;
    }
    Ok(report)
}

fn write_sidecar(media: &Path, existing: Option<&Path>, xmp: &str) -> Result<(), String> {
    let destination = match existing {
        Some(path) => path.to_path_buf(),
        None => {
            let name = media
                .file_name()
                .ok_or_else(|| "Photo has no file name".to_string())?
                .to_string_lossy();
            media
                .parent()
                .ok_or_else(|| "Photo has no folder".to_string())?
                .join(format!("{name}.xmp"))
        }
    };
    atomic_write(&destination, xmp.as_bytes())
}

fn upsert_xmp(existing: Option<&str>, metadata: &FileMetadata) -> Result<String, String> {
    let Some(existing) = existing.map(str::trim).filter(|xml| xml.contains("Description")) else {
        return Ok(render_xmp(metadata));
    };
    match rewrite_xmp(existing, metadata) {
        Ok(xml) if xml.contains("Description") => Ok(xml),
        _ => Ok(render_xmp(metadata)),
    }
}

fn rewrite_xmp(existing: &str, metadata: &FileMetadata) -> Result<String, String> {
    let mut reader = Reader::from_str(existing);
    let mut writer = Writer::new(Vec::new());
    let mut buffer = Vec::new();
    let mut skip = 0i32;
    let mut injected = false;
    let mut saw_description = false;
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| format!("Could not read existing XMP: {error}"))?;
        match event {
            Event::Start(_) if skip > 0 => skip += 1,
            Event::End(_) if skip > 0 => skip -= 1,
            Event::Eof => break,
            _ if skip > 0 => {}
            Event::Start(element) if is_owned_element(&element) => skip = 1,
            Event::Empty(element) if is_owned_element(&element) => {}
            Event::Start(element) if local_name(element.name().as_ref()) == "Description" && !injected => {
                saw_description = true;
                writer
                    .write_event(Event::Start(description_start(&element)))
                    .map_err(|error| error.to_string())?;
            }
            Event::Empty(element) if local_name(element.name().as_ref()) == "Description" && !injected => {
                saw_description = true;
                injected = true;
                writer
                    .write_event(Event::Start(description_start(&element)))
                    .map_err(|error| error.to_string())?;
                write_properties(&mut writer, metadata)?;
                let name = String::from_utf8_lossy(element.name().as_ref()).into_owned();
                writer
                    .write_event(Event::End(BytesEnd::new(name)))
                    .map_err(|error| error.to_string())?;
            }
            Event::End(element) if local_name(element.name().as_ref()) == "Description" && !injected => {
                injected = true;
                write_properties(&mut writer, metadata)?;
                writer
                    .write_event(Event::End(element))
                    .map_err(|error| error.to_string())?;
            }
            event => {
                writer.write_event(event).map_err(|error| error.to_string())?;
            }
        }
        buffer.clear();
    }
    if !saw_description {
        return Ok(render_xmp(metadata));
    }
    let bytes = writer.into_inner();
    String::from_utf8(bytes).map_err(|error| format!("XMP was not valid text: {error}"))
}

fn description_start(element: &BytesStart<'_>) -> BytesStart<'static> {
    let mut start = BytesStart::new(String::from_utf8_lossy(element.name().as_ref()).into_owned());
    let mut declared = Vec::new();
    for attribute in element.attributes().flatten() {
        let name = local_name(attribute.key.as_ref());
        if OWNED_PROPERTIES.contains(&name.as_str()) {
            continue;
        }
        declared.push(String::from_utf8_lossy(attribute.key.as_ref()).into_owned());
        let key = attribute.key.as_ref().to_vec();
        let value = attribute.value.as_ref().to_vec();
        start.push_attribute((key.as_slice(), value.as_slice()));
    }
    for (key, value) in [
        ("xmlns:dc", "http://purl.org/dc/elements/1.1/"),
        ("xmlns:photoshop", "http://ns.adobe.com/photoshop/1.0/"),
        ("xmlns:xmp", "http://ns.adobe.com/xap/1.0/"),
        ("xmlns:exif", "http://ns.adobe.com/exif/1.0/"),
    ] {
        if !declared.iter().any(|existing| existing == key) {
            start.push_attribute((key, value));
        }
    }
    start
}

fn is_owned_element(element: &BytesStart<'_>) -> bool {
    OWNED_PROPERTIES.contains(&local_name(element.name().as_ref()).as_str())
}

fn write_properties(writer: &mut Writer<Vec<u8>>, metadata: &FileMetadata) -> Result<(), String> {
    write_alt(writer, "dc:title", metadata.title.as_deref())?;
    write_alt(writer, "dc:description", metadata.description.as_deref())?;
    write_alt(writer, "dc:rights", metadata.copyright.as_deref())?;
    write_bag(writer, "dc:subject", &metadata.keywords)?;
    write_seq(writer, "dc:creator", metadata.creator.as_deref())?;
    write_text(writer, "photoshop:Headline", metadata.headline.as_deref())?;
    write_text(writer, "photoshop:Credit", metadata.credit.as_deref())?;
    write_text(writer, "photoshop:City", metadata.city.as_deref())?;
    write_text(writer, "photoshop:State", metadata.state.as_deref())?;
    write_text(writer, "photoshop:Country", metadata.country.as_deref())?;
    write_text(writer, "xmp:Label", metadata.label.as_deref())?;
    if let Some(rating) = metadata.embedded_rating {
        write_text(writer, "xmp:Rating", Some(&rating.to_string()))?;
    }
    if let Some(latitude) = metadata.gps_latitude {
        write_text(writer, "exif:GPSLatitude", Some(&format!("{latitude:.6}")))?;
    }
    if let Some(longitude) = metadata.gps_longitude {
        write_text(writer, "exif:GPSLongitude", Some(&format!("{longitude:.6}")))?;
    }
    if let Some(altitude) = metadata.gps_altitude {
        write_text(writer, "exif:GPSAltitude", Some(&format!("{altitude:.2}")))?;
    }
    Ok(())
}

fn write_alt(writer: &mut Writer<Vec<u8>>, name: &str, value: Option<&str>) -> Result<(), String> {
    let Some(value) = clean(value) else { return Ok(()) };
    writer
        .write_event(Event::Start(BytesStart::new(name)))
        .map_err(|error| error.to_string())?;
    writer
        .write_event(Event::Start(BytesStart::new("rdf:Alt")))
        .map_err(|error| error.to_string())?;
    let mut item = BytesStart::new("rdf:li");
    item.push_attribute(("xml:lang", "x-default"));
    writer
        .write_event(Event::Start(item))
        .map_err(|error| error.to_string())?;
    writer
        .write_event(Event::Text(BytesText::new(value)))
        .map_err(|error| error.to_string())?;
    for end in ["rdf:li", "rdf:Alt", name] {
        writer
            .write_event(Event::End(BytesEnd::new(end)))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn write_bag(writer: &mut Writer<Vec<u8>>, name: &str, values: &[String]) -> Result<(), String> {
    if values.is_empty() {
        return Ok(());
    }
    writer
        .write_event(Event::Start(BytesStart::new(name)))
        .map_err(|error| error.to_string())?;
    writer
        .write_event(Event::Start(BytesStart::new("rdf:Bag")))
        .map_err(|error| error.to_string())?;
    for value in values {
        writer
            .write_event(Event::Start(BytesStart::new("rdf:li")))
            .map_err(|error| error.to_string())?;
        writer
            .write_event(Event::Text(BytesText::new(value)))
            .map_err(|error| error.to_string())?;
        writer
            .write_event(Event::End(BytesEnd::new("rdf:li")))
            .map_err(|error| error.to_string())?;
    }
    writer
        .write_event(Event::End(BytesEnd::new("rdf:Bag")))
        .map_err(|error| error.to_string())?;
    writer
        .write_event(Event::End(BytesEnd::new(name)))
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn write_seq(writer: &mut Writer<Vec<u8>>, name: &str, value: Option<&str>) -> Result<(), String> {
    let Some(value) = clean(value) else { return Ok(()) };
    writer
        .write_event(Event::Start(BytesStart::new(name)))
        .map_err(|error| error.to_string())?;
    writer
        .write_event(Event::Start(BytesStart::new("rdf:Seq")))
        .map_err(|error| error.to_string())?;
    writer
        .write_event(Event::Start(BytesStart::new("rdf:li")))
        .map_err(|error| error.to_string())?;
    writer
        .write_event(Event::Text(BytesText::new(value)))
        .map_err(|error| error.to_string())?;
    for end in ["rdf:li", "rdf:Seq", name] {
        writer
            .write_event(Event::End(BytesEnd::new(end)))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn write_text(writer: &mut Writer<Vec<u8>>, name: &str, value: Option<&str>) -> Result<(), String> {
    let Some(value) = clean(value) else { return Ok(()) };
    writer
        .write_event(Event::Start(BytesStart::new(name)))
        .map_err(|error| error.to_string())?;
    writer
        .write_event(Event::Text(BytesText::new(value)))
        .map_err(|error| error.to_string())?;
    writer
        .write_event(Event::End(BytesEnd::new(name)))
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn clean(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

pub fn render_xmp(metadata: &FileMetadata) -> String {
    let mut writer = Writer::new(Vec::new());
    writer
        .write_event(Event::Decl(quick_xml::events::BytesDecl::new("1.0", Some("UTF-8"), None)))
        .ok();
    let mut meta = BytesStart::new("x:xmpmeta");
    meta.push_attribute(("xmlns:x", "adobe:ns:meta/"));
    writer.write_event(Event::Start(meta)).ok();
    let mut rdf = BytesStart::new("rdf:RDF");
    rdf.push_attribute(("xmlns:rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"));
    writer.write_event(Event::Start(rdf)).ok();
    let mut description = BytesStart::new("rdf:Description");
    description.push_attribute(("rdf:about", ""));
    description.push_attribute(("xmlns:dc", "http://purl.org/dc/elements/1.1/"));
    description.push_attribute(("xmlns:photoshop", "http://ns.adobe.com/photoshop/1.0/"));
    description.push_attribute(("xmlns:xmp", "http://ns.adobe.com/xap/1.0/"));
    description.push_attribute(("xmlns:exif", "http://ns.adobe.com/exif/1.0/"));
    writer.write_event(Event::Start(description)).ok();
    write_properties(&mut writer, metadata).ok();
    for end in ["rdf:Description", "rdf:RDF", "x:xmpmeta"] {
        writer.write_event(Event::End(BytesEnd::new(end))).ok();
    }
    String::from_utf8(writer.into_inner()).unwrap_or_default()
}

fn render_iptc(metadata: &FileMetadata) -> Vec<u8> {
    let mut iim = dataset(1, 90, &[0x1B, 0x25, 0x47]);
    push_text(&mut iim, 5, metadata.title.as_deref());
    for keyword in &metadata.keywords {
        push_text(&mut iim, 25, Some(keyword));
    }
    push_text(&mut iim, 80, metadata.creator.as_deref());
    push_text(&mut iim, 90, metadata.city.as_deref());
    push_text(&mut iim, 95, metadata.state.as_deref());
    push_text(&mut iim, 101, metadata.country.as_deref());
    push_text(&mut iim, 105, metadata.headline.as_deref());
    push_text(&mut iim, 110, metadata.credit.as_deref());
    push_text(&mut iim, 116, metadata.copyright.as_deref());
    push_text(&mut iim, 120, metadata.description.as_deref());
    iim
}

fn push_text(iim: &mut Vec<u8>, id: u8, value: Option<&str>) {
    let Some(value) = clean(value) else { return };
    let bytes = value.as_bytes();
    let length = bytes.len().min(u16::MAX as usize);
    iim.extend(dataset(2, id, &bytes[..length]));
}

fn dataset(record: u8, id: u8, value: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0x1C, record, id];
    bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
    bytes.extend_from_slice(value);
    bytes
}

fn embed_jpeg(path: &Path, xmp: &str, iim: &[u8]) -> Result<(bool, bool), String> {
    let xmp_payload = xmp_payload(xmp)?;
    let iptc_payload = photoshop_iptc(iim);
    if iptc_payload.len() > MAX_SEGMENT_PAYLOAD {
        return Err("IPTC metadata is too large to store in the photo".to_string());
    }

    let mut input = File::open(path).map_err(|error| format!("Could not open photo: {error}"))?;
    let temp = sibling_temp(path);
    let mut output = File::create(&temp).map_err(|error| format!("Could not prepare photo update: {error}"))?;
    let result = (|| {
        let mut marker = [0u8; 2];
        input
            .read_exact(&mut marker)
            .map_err(|error| format!("Could not read photo: {error}"))?;
        if marker != [0xFF, 0xD8] {
            return Err("Not a JPEG photo".to_string());
        }
        output.write_all(&marker).map_err(|error| error.to_string())?;

        let mut xmp_written = false;
        let mut iptc_written = false;
        loop {
            let kind = read_marker(&mut input)?;
            if kind == 0xDA || kind == 0xD9 {
                if !xmp_written {
                    write_segment(&mut output, 0xE1, &xmp_payload)?;
                    xmp_written = true;
                }
                if !iptc_written {
                    write_segment(&mut output, 0xED, &iptc_payload)?;
                    iptc_written = true;
                }
                output.write_all(&[0xFF, kind]).map_err(|error| error.to_string())?;
                if kind == 0xDA {
                    copy_rest(&mut input, &mut output)?;
                }
                break;
            }
            if kind == 0x01 || (0xD0..=0xD7).contains(&kind) {
                output.write_all(&[0xFF, kind]).map_err(|error| error.to_string())?;
                continue;
            }
            let payload = read_payload(&mut input)?;
            if kind == 0xE1 && (payload.starts_with(XMP_HEADER) || payload.starts_with(XMP_EXTENSION_HEADER)) {
                if !xmp_written && payload.starts_with(XMP_HEADER) {
                    write_segment(&mut output, 0xE1, &xmp_payload)?;
                    xmp_written = true;
                }
                continue;
            }
            if kind == 0xED {
                match rewrite_app13(&payload, iim) {
                    App13Rewrite::Updated(updated) => {
                        write_segment(&mut output, 0xED, &updated)?;
                        iptc_written = true;
                        continue;
                    }
                    App13Rewrite::Keep => {
                        write_segment(&mut output, kind, &payload)?;
                        iptc_written = true;
                        continue;
                    }
                    App13Rewrite::Foreign => {}
                }
            }
            write_segment(&mut output, kind, &payload)?;
        }
        output.flush().map_err(|error| error.to_string())?;
        Ok((xmp_written, iptc_written))
    })();
    drop(output);
    drop(input);
    match result {
        Ok(flags) => {
            replace_file(path, &temp)?;
            Ok(flags)
        }
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

enum App13Rewrite {
    Updated(Vec<u8>),
    Keep,
    Foreign,
}

fn rewrite_app13(payload: &[u8], iim: &[u8]) -> App13Rewrite {
    if !payload.starts_with(PHOTOSHOP_HEADER) {
        return App13Rewrite::Foreign;
    }
    match replace_iptc_resource(payload, iim) {
        Some(updated) if updated.len() <= MAX_SEGMENT_PAYLOAD => App13Rewrite::Updated(updated),
        _ => App13Rewrite::Keep,
    }
}

fn replace_iptc_resource(payload: &[u8], iim: &[u8]) -> Option<Vec<u8>> {
    let mut blocks = &payload[PHOTOSHOP_HEADER.len()..];
    let mut output = PHOTOSHOP_HEADER.to_vec();
    let mut replaced = false;
    while blocks.len() >= 8 && blocks.starts_with(b"8BIM") {
        let id = u16::from_be_bytes([blocks[4], blocks[5]]);
        let name_len = blocks[6] as usize;
        let mut pos = 7 + name_len;
        if (1 + name_len) % 2 == 1 {
            pos += 1;
        }
        if pos + 4 > blocks.len() {
            return None;
        }
        let size = u32::from_be_bytes(blocks[pos..pos + 4].try_into().ok()?) as usize;
        let mut end = pos + 4 + size;
        if size % 2 == 1 {
            end += 1;
        }
        if end > blocks.len() {
            return None;
        }
        if id == 0x0404 {
            output.extend(encode_8bim(0x0404, iim));
            replaced = true;
        } else {
            output.extend_from_slice(&blocks[..end]);
        }
        blocks = &blocks[end..];
    }
    if !blocks.is_empty() {
        return None;
    }
    if !replaced {
        output.extend(encode_8bim(0x0404, iim));
    }
    Some(output)
}

fn encode_8bim(id: u16, data: &[u8]) -> Vec<u8> {
    let mut bytes = b"8BIM".to_vec();
    bytes.extend_from_slice(&id.to_be_bytes());
    bytes.extend_from_slice(&[0, 0]);
    bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
    bytes.extend_from_slice(data);
    if data.len() % 2 == 1 {
        bytes.push(0);
    }
    bytes
}

fn photoshop_iptc(iim: &[u8]) -> Vec<u8> {
    let mut payload = PHOTOSHOP_HEADER.to_vec();
    payload.extend(encode_8bim(0x0404, iim));
    payload
}

fn xmp_payload(xmp: &str) -> Result<Vec<u8>, String> {
    let mut payload = XMP_HEADER.to_vec();
    payload.extend_from_slice(xmp.as_bytes());
    if payload.len() > MAX_SEGMENT_PAYLOAD {
        return Err("Descriptive metadata is too large to embed in this JPEG".to_string());
    }
    Ok(payload)
}

fn read_marker(input: &mut File) -> Result<u8, String> {
    let mut lead = [0u8; 1];
    input
        .read_exact(&mut lead)
        .map_err(|_| "Photo metadata ended unexpectedly".to_string())?;
    if lead[0] != 0xFF {
        return Err("Photo structure could not be updated safely".to_string());
    }
    let mut kind = [0u8; 1];
    input
        .read_exact(&mut kind)
        .map_err(|_| "Photo metadata ended unexpectedly".to_string())?;
    while kind[0] == 0xFF {
        input
            .read_exact(&mut kind)
            .map_err(|_| "Photo metadata ended unexpectedly".to_string())?;
    }
    Ok(kind[0])
}

fn read_payload(input: &mut File) -> Result<Vec<u8>, String> {
    let mut length_bytes = [0u8; 2];
    input
        .read_exact(&mut length_bytes)
        .map_err(|_| "Photo metadata ended unexpectedly".to_string())?;
    let length = u16::from_be_bytes(length_bytes) as usize;
    if length < 2 {
        return Err("Photo structure could not be updated safely".to_string());
    }
    let mut payload = vec![0u8; length - 2];
    input
        .read_exact(&mut payload)
        .map_err(|_| "Photo metadata ended unexpectedly".to_string())?;
    Ok(payload)
}

fn write_segment(output: &mut File, marker: u8, payload: &[u8]) -> Result<(), String> {
    if payload.len() + 2 > 65535 {
        return Err("Metadata segment is too large".to_string());
    }
    let length = (payload.len() + 2) as u16;
    output.write_all(&[0xFF, marker]).map_err(|error| error.to_string())?;
    output
        .write_all(&length.to_be_bytes())
        .map_err(|error| error.to_string())?;
    output.write_all(payload).map_err(|error| error.to_string())?;
    Ok(())
}

fn copy_rest(input: &mut File, output: &mut File) -> Result<(), String> {
    io::copy(input, output).map_err(|error| format!("Could not copy photo data: {error}"))?;
    Ok(())
}

fn is_jpeg(path: &Path) -> bool {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut magic = [0u8; 2];
    file.read_exact(&mut magic).ok() == Some(()) && magic == [0xFF, 0xD8]
}

fn sibling_temp(path: &Path) -> PathBuf {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "photo".to_string());
    path.with_file_name(format!(".{name}.lap-meta-tmp"))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp = sibling_temp(path);
    fs::write(&temp, bytes).map_err(|error| format!("Could not write metadata: {error}"))?;
    replace_file(path, &temp)
}

fn replace_file(destination: &Path, temp: &Path) -> Result<(), String> {
    if !destination.exists() {
        return fs::rename(temp, destination).map_err(|error| {
            let _ = fs::remove_file(temp);
            format!("Could not save metadata: {error}")
        });
    }
    let backup = destination.with_file_name(format!(
        ".{}.lap-meta-bak",
        destination.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    if let Err(error) = fs::rename(destination, &backup) {
        let _ = fs::remove_file(temp);
        return Err(format!("Could not update the photo. It may be open in another program: {error}"));
    }
    if let Err(error) = fs::rename(temp, destination) {
        let _ = fs::rename(&backup, destination);
        return Err(format!("Could not update the photo: {error}"));
    }
    let _ = fs::remove_file(&backup);
    Ok(())
}

fn local_name(name: &[u8]) -> String {
    let name = String::from_utf8_lossy(name);
    if let Some((_, local)) = name.rsplit_once(':') {
        local.to_string()
    } else if let Some(end) = name.rfind('}') {
        name[end + 1..].to_string()
    } else {
        name.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::t_file_metadata::{parse_iptc, parse_xmp, extract_packets};

    fn sample() -> FileMetadata {
        let mut metadata = FileMetadata::default();
        metadata.title = Some("Harbor".to_string());
        metadata.description = Some("Evening & boats".to_string());
        metadata.headline = Some("Oslo harbor".to_string());
        metadata.city = Some("Oslo".to_string());
        metadata.country = Some("Norway".to_string());
        metadata.keywords = vec!["Oslo".to_string(), "Night".to_string()];
        metadata.creator = Some("Ada".to_string());
        metadata.embedded_rating = Some(4);
        metadata.gps_latitude = Some(59.91);
        metadata.gps_longitude = Some(10.75);
        metadata
    }

    #[test]
    fn metadata_xmp_and_iptc_round_trip_descriptive_fields() {
        let metadata = sample();
        let parsed = parse_xmp(&render_xmp(&metadata));
        assert_eq!(parsed.title.as_deref(), Some("Harbor"));
        assert_eq!(parsed.description.as_deref(), Some("Evening & boats"));
        assert_eq!(parsed.keywords, vec!["Oslo".to_string(), "Night".to_string()]);
        assert_eq!(parsed.city.as_deref(), Some("Oslo"));
        assert_eq!(parsed.country.as_deref(), Some("Norway"));
        assert_eq!(parsed.creator.as_deref(), Some("Ada"));
        assert_eq!(parsed.embedded_rating, Some(4));
        assert_eq!(parsed.gps_latitude.map(|value| (value * 100.0).round() as i32), Some(5991));

        let iptc = parse_iptc(&photoshop_iptc(&render_iptc(&metadata)));
        assert_eq!(iptc.title.as_deref(), Some("Harbor"));
        assert_eq!(iptc.keywords, metadata.keywords);
        assert_eq!(iptc.city.as_deref(), Some("Oslo"));
        assert_eq!(iptc.description.as_deref(), Some("Evening & boats"));
    }

    #[test]
    fn metadata_xmp_update_keeps_unrelated_edit_settings() {
        let existing = r#"<?xml version="1.0"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:title><rdf:Alt><rdf:li xml:lang="x-default">Old</rdf:li></rdf:Alt></dc:title>
<crs:Exposure2012>1.2</crs:Exposure2012>
</rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let xml = upsert_xmp(Some(existing), &sample()).unwrap();
        assert!(xml.contains("Exposure2012"));
        assert!(xml.contains("1.2"));
        assert_eq!(parse_xmp(&xml).title.as_deref(), Some("Harbor"));
        assert!(!xml.contains(">Old<"));
    }

    #[test]
    fn metadata_patch_adds_and_clears_keywords() {
        let mut current = sample();
        current.keywords = vec!["Oslo".to_string(), "Keep".to_string()];
        let updated = apply_metadata_patch(
            &current,
            &MetadataChange {
                add_keywords: vec!["Night".to_string()],
                remove_keywords: vec!["oslo".to_string()],
                title: Some("".to_string()),
                ..MetadataChange::default()
            },
        )
        .unwrap();
        assert_eq!(updated.keywords, vec!["Keep".to_string(), "Night".to_string()]);
        assert!(updated.title.is_none());
        assert_eq!(updated.city.as_deref(), Some("Oslo"));
    }

    #[test]
    fn metadata_jpeg_embed_is_readable_and_keeps_image_bytes() {
        let dir = std::env::temp_dir().join(format!("lap-meta-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("harbor.jpg");
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        jpeg.extend_from_slice(b"JFIF\0\x01\x01\x00\x00\x01\x00\x01\x00\x00");
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
        jpeg.extend_from_slice(b"image-bytes");
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        fs::write(&path, &jpeg).unwrap();

        let report = write_descriptive_metadata(&path, &sample()).unwrap();
        assert!(report.embedded_xmp);
        assert!(report.embedded_iptc);
        assert!(!report.sidecar);

        let packets = extract_packets(&path);
        let xmp = parse_xmp(packets.xmp.as_deref().unwrap());
        assert_eq!(xmp.title.as_deref(), Some("Harbor"));
        assert_eq!(xmp.keywords, vec!["Oslo".to_string(), "Night".to_string()]);
        let iptc = parse_iptc(packets.iptc.as_deref().unwrap());
        assert_eq!(iptc.city.as_deref(), Some("Oslo"));
        let written = fs::read(&path).unwrap();
        assert!(written.windows(b"image-bytes".len()).any(|window| window == b"image-bytes"));
        assert!(written.ends_with(&[0xFF, 0xD9]));
        let _ = fs::remove_dir_all(&dir);
    }
}

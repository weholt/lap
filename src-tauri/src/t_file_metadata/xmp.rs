use super::{sanitize_rating, FileMetadata};
use quick_xml::events::Event;
use quick_xml::Reader;

const MAX_PACKET: usize = 2 * 1024 * 1024;

pub fn parse_xmp(xml: &str) -> FileMetadata {
    let xml = xml.trim().trim_start_matches('\u{feff}');
    if xml.is_empty() || xml.len() > MAX_PACKET {
        return FileMetadata::default();
    }
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut records = Vec::new();
    loop {
        let event = match reader.read_event_into(&mut buffer) {
            Ok(event) => event,
            Err(_) => return FileMetadata::default(),
        };
        match event {
            Event::Start(element) => {
                let local = local_name(element.name().as_ref());
                let lang = attribute_local(&element, "lang");
                collect_attributes(&element, &mut records);
                stack.push(Frame { local, lang });
            }
            Event::Empty(element) => {
                collect_attributes(&element, &mut records);
            }
            Event::Text(text) => {
                let Ok(text) = text.unescape() else { continue };
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                if let Some(property) = current_property(&stack) {
                    let lang = stack.last().and_then(|frame| frame.lang.clone());
                    records.push(Record { name: property, lang, value: text.to_string() });
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    assign(records)
}

struct Frame {
    local: String,
    lang: Option<String>,
}

struct Record {
    name: String,
    lang: Option<String>,
    value: String,
}

fn collect_attributes(element: &quick_xml::events::BytesStart<'_>, records: &mut Vec<Record>) {
    for attribute in element.attributes().flatten() {
        let name = local_name(attribute.key.as_ref());
        if !is_property(&name) {
            continue;
        }
        if let Ok(value) = attribute.unescape_value() {
            let value = value.trim();
            if !value.is_empty() {
                records.push(Record { name, lang: None, value: value.to_string() });
            }
        }
    }
}

fn current_property(stack: &[Frame]) -> Option<String> {
    stack.iter().rev().find(|frame| is_property(&frame.local)).map(|frame| frame.local.clone())
}

fn is_property(name: &str) -> bool {
    matches!(
        name,
        "title"
            | "description"
            | "subject"
            | "creator"
            | "rights"
            | "Headline"
            | "Credit"
            | "DateCreated"
            | "City"
            | "State"
            | "Country"
            | "CreateDate"
            | "Rating"
            | "Label"
            | "DateTimeOriginal"
            | "GPSLatitude"
            | "GPSLongitude"
            | "GPSAltitude"
            | "GPSLatitudeRef"
            | "GPSLongitudeRef"
            | "Make"
            | "Model"
            | "Artist"
            | "Copyright"
            | "ImageDescription"
            | "Lens"
            | "LensMake"
            | "LensModel"
    )
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

fn attribute_local(element: &quick_xml::events::BytesStart<'_>, expected: &str) -> Option<String> {
    element.attributes().flatten().find_map(|attribute| {
        let name = local_name(attribute.key.as_ref());
        if name == expected {
            attribute.unescape_value().ok().map(|value| value.into_owned())
        } else {
            None
        }
    })
}

fn assign(records: Vec<Record>) -> FileMetadata {
    let mut metadata = FileMetadata::default();
    metadata.title = pick_alt(&records, "title");
    metadata.description = pick_alt(&records, "description").or_else(|| pick_alt(&records, "ImageDescription"));
    metadata.creator = join_values(&records, "creator").or_else(|| pick_alt(&records, "Artist"));
    metadata.copyright = pick_alt(&records, "rights").or_else(|| pick_alt(&records, "Copyright"));
    metadata.headline = pick_alt(&records, "Headline");
    metadata.credit = pick_alt(&records, "Credit");
    metadata.city = pick_alt(&records, "City");
    metadata.state = pick_alt(&records, "State");
    metadata.country = pick_alt(&records, "Country");
    metadata.label = pick_alt(&records, "Label");
    metadata.make = pick_alt(&records, "Make");
    metadata.model = pick_alt(&records, "Model");
    metadata.lens_make = pick_alt(&records, "LensMake");
    metadata.lens_model = pick_alt(&records, "LensModel").or_else(|| pick_alt(&records, "Lens"));
    metadata.keywords = records.iter().filter(|record| record.name == "subject").map(|record| record.value.clone()).collect();
    metadata.capture_date = pick_alt(&records, "DateTimeOriginal")
        .or_else(|| pick_alt(&records, "DateCreated"))
        .or_else(|| pick_alt(&records, "CreateDate"))
        .and_then(|value| normalize_xmp_date(&value));
    if let Some(rating) = pick_alt(&records, "Rating").and_then(|value| rating_from_text(&value)) {
        metadata.embedded_rating = Some(rating);
    }
    let latitude_ref = pick_alt(&records, "GPSLatitudeRef");
    let longitude_ref = pick_alt(&records, "GPSLongitudeRef");
    metadata.gps_latitude = pick_alt(&records, "GPSLatitude").and_then(|value| parse_coordinate(&value, latitude_ref.as_deref(), true));
    metadata.gps_longitude = pick_alt(&records, "GPSLongitude").and_then(|value| parse_coordinate(&value, longitude_ref.as_deref(), false));
    metadata.gps_altitude = pick_alt(&records, "GPSAltitude").and_then(|value| parse_number(&value)).filter(|value| value.is_finite());
    metadata
}

fn pick_alt(records: &[Record], name: &str) -> Option<String> {
    let mut first = None;
    for record in records.iter().filter(|record| record.name == name) {
        if record.lang.as_deref() == Some("x-default") {
            return Some(record.value.clone());
        }
        if first.is_none() {
            first = Some(record.value.clone());
        }
    }
    first
}

fn join_values(records: &[Record], name: &str) -> Option<String> {
    let values = records.iter().filter(|record| record.name == name).map(|record| record.value.as_str()).collect::<Vec<_>>();
    if values.is_empty() { None } else { Some(values.join(", ")) }
}

fn normalize_xmp_date(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some((date, time)) = raw.split_once('T') {
        let has_zone = raw.ends_with('Z') || raw.ends_with('z') || raw.rfind(['+', '-']).is_some_and(|index| index > 10);
        if has_zone {
            return Some(raw.to_string());
        }
        let date = date.replace('-', ":");
        let time = time.split('.').next().unwrap_or(time);
        let time = if time.len() >= 8 { &time[..8] } else { time };
        return Some(format!("{date} {time}"));
    }
    if raw.len() >= 10 && raw.as_bytes().get(4) == Some(&b'-') {
        return Some(format!("{} 00:00:00", raw[..10].replace('-', ":")));
    }
    Some(raw.to_string())
}

fn parse_coordinate(raw: &str, axis_ref: Option<&str>, latitude: bool) -> Option<f64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let mut hemisphere = axis_ref.unwrap_or("").chars().find(|character| character.is_ascii_alphabetic()).unwrap_or(' ');
    if let Some(letter) = raw.chars().rev().find(|character| character.is_ascii_alphabetic()) {
        hemisphere = letter;
    }
    let cleaned: String = raw.chars().filter(|character| !character.is_ascii_alphabetic()).collect();
    let numbers = cleaned.split([',', ' ']).map(str::trim).filter(|part| !part.is_empty()).filter_map(parse_number).collect::<Vec<_>>();
    let magnitude = match numbers.as_slice() {
        [degrees] => *degrees,
        [degrees, minutes] => degrees + minutes / 60.0,
        [degrees, minutes, seconds, ..] => degrees + minutes / 60.0 + seconds / 3600.0,
        _ => return None,
    };
    let signed = match hemisphere.to_ascii_uppercase() {
        'S' | 'W' => -magnitude.abs(),
        'N' | 'E' => magnitude.abs(),
        _ => magnitude,
    };
    let limit = if latitude { 90.0 } else { 180.0 };
    if signed.is_finite() && signed.abs() <= limit { Some(signed) } else { None }
}

fn rating_from_text(raw: &str) -> Option<i32> {
    let number = parse_number(raw.trim())?;
    if !number.is_finite() {
        return None;
    }
    super::normalize_rating_number(number.round() as i32)
}

fn parse_number(raw: &str) -> Option<f64> {
    let raw = raw.trim();
    if let Some((numerator, denominator)) = raw.split_once('/') {
        let numerator = numerator.trim().parse::<f64>().ok()?;
        let denominator = denominator.trim().parse::<f64>().ok()?;
        if denominator == 0.0 { None } else { Some(numerator / denominator) }
    } else {
        raw.parse::<f64>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_metadata_xmp_reads_alt_bag_decimal_gps_and_rating() {
        let xml = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
        <x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:exif="http://ns.adobe.com/exif/1.0/" xmp:Rating="4" exif:GPSLatitude="37.5" exif:GPSLongitude="-122.25">
          <dc:title><rdf:Alt><rdf:li xml:lang="x-default">Default title</rdf:li><rdf:li xml:lang="fr">Autre</rdf:li></rdf:Alt></dc:title>
          <dc:subject><rdf:Bag><rdf:li>one</rdf:li><rdf:li>two</rdf:li></rdf:Bag></dc:subject>
        </rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let metadata = parse_xmp(xml);
        assert_eq!(metadata.title.as_deref(), Some("Default title"));
        assert_eq!(metadata.keywords, vec!["one".to_string(), "two".to_string()]);
        assert_eq!(metadata.embedded_rating, Some(4));
        assert_eq!(metadata.gps_latitude, Some(37.5));
        assert_eq!(metadata.gps_longitude, Some(-122.25));
    }

    #[test]
    fn file_metadata_xmp_dms_gps_negative_rating_and_dates() {
        let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description
            xmlns:exif="http://ns.adobe.com/exif/1.0/" xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
            exif:GPSLatitude="37,26,31.92N" exif:GPSLongitude="122,15.0W" xmp:Rating="-1" photoshop:DateCreated="2020-01-02T03:04:05" xmp:CreateDate="2019-01-01T00:00:00Z">
            <exif:DateTimeOriginal>2021-05-06T07:08:09</exif:DateTimeOriginal>
        </rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let metadata = parse_xmp(xml);
        assert!(metadata.gps_latitude.is_some_and(|value| (value - 37.4422).abs() < 0.001));
        assert!(metadata.gps_longitude.is_some_and(|value| value < 0.0));
        assert!(metadata.embedded_rating.is_none());
        assert_eq!(metadata.capture_date.as_deref(), Some("2021:05:06 07:08:09"));
    }

    #[test]
    fn file_metadata_xmp_truncated_packet_is_empty_and_lens_survives_develop_settings() {
        assert_eq!(parse_xmp("<x:xmpmeta><rdf:RDF"), FileMetadata::default());
        let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:LensModel="Test Lens" crs:Exposure2012="1.25"/></rdf:RDF></x:xmpmeta>"#;
        let metadata = parse_xmp(xml);
        assert_eq!(metadata.lens_model.as_deref(), Some("Test Lens"));
        assert!(metadata.title.is_none());
        assert!(metadata.exposure_bias.is_none());
    }

    #[test]
    fn file_metadata_xmp_rating_accepts_fraction_and_windows_scale() {
        let fraction = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="5/1"/></rdf:RDF></x:xmpmeta>"#;
        assert_eq!(parse_xmp(fraction).embedded_rating, Some(5));
        let windows = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:MicrosoftPhoto="http://ns.microsoft.com/photo/1.0/" MicrosoftPhoto:Rating="75"/></rdf:RDF></x:xmpmeta>"#;
        assert_eq!(parse_xmp(windows).embedded_rating, Some(4));
    }
}

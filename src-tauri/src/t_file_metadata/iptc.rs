use super::FileMetadata;

const MAX_PAYLOAD: usize = 2 * 1024 * 1024;
const MAX_DATASET: usize = 64 * 1024;

pub fn parse_iptc(payload: &[u8]) -> FileMetadata {
    if payload.is_empty() || payload.len() > MAX_PAYLOAD {
        return FileMetadata::default();
    }
    let iim = extract_8bim_iptc(payload).unwrap_or(payload);
    parse_iim(iim)
}

fn parse_iim(data: &[u8]) -> FileMetadata {
    let mut metadata = FileMetadata::default();
    let mut utf8 = false;
    let mut date = None;
    let mut time = None;
    let mut pos = 0;
    while pos + 5 <= data.len() {
        if data[pos] != 0x1C {
            break;
        }
        let record = data[pos + 1];
        let dataset = data[pos + 2];
        let mut length = u16::from_be_bytes([data[pos + 3], data[pos + 4]]) as usize;
        pos += 5;
        if length & 0x8000 != 0 {
            let octets = length & 0x7FFF;
            if octets == 0 || octets > 4 || pos + octets > data.len() {
                break;
            }
            length = 0;
            for _ in 0..octets {
                length = (length << 8) | data[pos] as usize;
                pos += 1;
            }
        }
        if length > MAX_DATASET || pos + length > data.len() {
            break;
        }
        let value = &data[pos..pos + length];
        pos += length;
        if record == 1 && dataset == 90 && value.windows(3).any(|window| window == [0x1B, 0x25, 0x47]) {
            utf8 = true;
            continue;
        }
        if record != 2 {
            continue;
        }
        let text = decode_text(value, utf8);
        if text.is_empty() && dataset != 25 {
            continue;
        }
        match dataset {
            5 => metadata.title = Some(text),
            25 => metadata.keywords.push(text),
            55 => date = Some(text),
            60 => time = Some(text),
            80 => metadata.creator = Some(text),
            90 => metadata.city = Some(text),
            95 => metadata.state = Some(text),
            101 => metadata.country = Some(text),
            105 => metadata.headline = Some(text),
            110 => metadata.credit = Some(text),
            116 => metadata.copyright = Some(text),
            120 => metadata.description = Some(text),
            _ => {}
        }
    }
    if let Some(date) = date {
        metadata.capture_date = Some(format_iptc_date(&date, time.as_deref()));
    }
    metadata
}

fn decode_text(bytes: &[u8], utf8: bool) -> String {
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    let text = if utf8 {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        bytes.iter().map(|byte| char::from(*byte)).collect()
    };
    text.trim().trim_matches('\0').to_string()
}

fn format_iptc_date(date: &str, time: Option<&str>) -> String {
    let digits: String = date.chars().filter(|character| character.is_ascii_digit()).collect();
    if digits.len() < 8 {
        return date.to_string();
    }
    let formatted_date = format!("{}:{}:{}", &digits[0..4], &digits[4..6], &digits[6..8]);
    let time_digits: String = time.unwrap_or("").chars().filter(|character| character.is_ascii_digit()).collect();
    if time_digits.len() >= 6 {
        format!("{formatted_date} {}:{}:{}", &time_digits[0..2], &time_digits[2..4], &time_digits[4..6])
    } else {
        format!("{formatted_date} 00:00:00")
    }
}

fn extract_8bim_iptc(data: &[u8]) -> Option<&[u8]> {
    let mut index = data.windows(4).position(|window| window == b"8BIM")?;
    while index + 8 <= data.len() && &data[index..index + 4] == b"8BIM" {
        let id = u16::from_be_bytes([data[index + 4], data[index + 5]]);
        let name_len = data[index + 6] as usize;
        let mut pos = index + 7 + name_len;
        if (1 + name_len) % 2 == 1 {
            pos += 1;
        }
        if pos + 4 > data.len() {
            break;
        }
        let size = u32::from_be_bytes(data[pos..pos + 4].try_into().ok()?) as usize;
        pos += 4;
        if size > MAX_PAYLOAD || pos + size > data.len() {
            break;
        }
        if id == 0x0404 {
            return Some(&data[pos..pos + size]);
        }
        pos += size;
        if size % 2 == 1 {
            pos += 1;
        }
        index = pos;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dataset(record: u8, id: u8, value: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0x1C, record, id];
        bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
        bytes.extend_from_slice(value);
        bytes
    }

    #[test]
    fn file_metadata_iptc_reads_utf8_keywords_caption_and_date() {
        let mut payload = dataset(1, 90, &[0x1B, 0x25, 0x47]);
        payload.extend(dataset(2, 25, "Oslo".as_bytes()));
        payload.extend(dataset(2, 25, "Hafen".as_bytes()));
        payload.extend(dataset(2, 105, "Headline".as_bytes()));
        payload.extend(dataset(2, 120, "Caption".as_bytes()));
        payload.extend(dataset(2, 55, b"20200102"));
        payload.extend(dataset(2, 60, b"030405+0000"));
        payload.extend(dataset(2, 80, "Ada".as_bytes()));
        let metadata = parse_iptc(&payload);
        assert_eq!(metadata.keywords, vec!["Oslo".to_string(), "Hafen".to_string()]);
        assert_eq!(metadata.headline.as_deref(), Some("Headline"));
        assert_eq!(metadata.description.as_deref(), Some("Caption"));
        assert_eq!(metadata.capture_date.as_deref(), Some("2020:01:02 03:04:05"));
        assert_eq!(metadata.creator.as_deref(), Some("Ada"));
        assert!(metadata.embedded_rating.is_none());
    }

    #[test]
    fn file_metadata_iptc_latin1_and_truncated_record() {
        let mut payload = dataset(2, 120, &[0xE9]);
        payload.extend_from_slice(&[0x1C, 2, 25, 0x00, 0x10, b'x']);
        let metadata = parse_iptc(&payload);
        assert_eq!(metadata.description.as_deref(), Some("é"));
        assert!(metadata.keywords.is_empty());
    }

    #[test]
    fn file_metadata_iptc_reads_8bim_resource_and_date_without_time() {
        let iim = dataset(2, 5, b"Object");
        let mut date_only = dataset(2, 55, b"19991231");
        let mut wrapped = b"Photoshop 3.0\0".to_vec();
        wrapped.extend_from_slice(b"8BIM");
        wrapped.extend_from_slice(&0x0404u16.to_be_bytes());
        wrapped.extend_from_slice(&[0, 0]);
        let mut iim_all = iim;
        iim_all.append(&mut date_only);
        wrapped.extend_from_slice(&(iim_all.len() as u32).to_be_bytes());
        wrapped.extend_from_slice(&iim_all);
        let metadata = parse_iptc(&wrapped);
        assert_eq!(metadata.title.as_deref(), Some("Object"));
        assert_eq!(metadata.capture_date.as_deref(), Some("1999:12:31 00:00:00"));
    }
}

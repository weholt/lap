use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const MAX_PACKET: usize = 2 * 1024 * 1024;
const SCAN_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct MetadataPackets {
    pub iptc: Option<Vec<u8>>,
    pub xmp: Option<String>,
}

pub fn extract_packets(path: &Path) -> MetadataPackets {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return MetadataPackets::default(),
    };
    let mut header = [0u8; 16];
    let read = file.read(&mut header).unwrap_or(0);
    if file.seek(SeekFrom::Start(0)).is_err() {
        return MetadataPackets::default();
    }
    if read >= 2 && header[0] == 0xFF && header[1] == 0xD8 {
        return extract_jpeg(&mut file);
    }
    if read >= 8 && header.starts_with(b"\x89PNG\r\n\x1a\n") {
        return extract_png(&mut file);
    }
    if read >= 4 && (header.starts_with(b"II*\0") || header.starts_with(b"MM\0*")) {
        let mut packets = extract_tiff(&mut file);
        if packets.xmp.is_none() {
            packets.xmp = scan_xmp_prefix(path);
        }
        return packets;
    }
    if read >= 12 && &header[0..4] == b"RIFF" && &header[8..12] == b"WEBP" {
        return extract_webp(&mut file);
    }
    MetadataPackets { iptc: None, xmp: scan_xmp_prefix(path) }
}

fn extract_jpeg(file: &mut File) -> MetadataPackets {
    let mut marker = [0u8; 2];
    if file.read_exact(&mut marker).is_err() || marker != [0xFF, 0xD8] {
        return MetadataPackets::default();
    }
    let mut packets = MetadataPackets::default();
    loop {
        let mut lead = [0u8; 1];
        if file.read_exact(&mut lead).is_err() {
            break;
        }
        if lead[0] != 0xFF {
            break;
        }
        let mut kind = [0u8; 1];
        if file.read_exact(&mut kind).is_err() {
            break;
        }
        while kind[0] == 0xFF {
            if file.read_exact(&mut kind).is_err() {
                return packets;
            }
        }
        if kind[0] == 0xD8 || kind[0] == 0xD9 || kind[0] == 0xDA || kind[0] == 0x01 || (0xD0..=0xD7).contains(&kind[0]) {
            if kind[0] == 0xD9 || kind[0] == 0xDA {
                break;
            }
            continue;
        }
        let mut length_bytes = [0u8; 2];
        if file.read_exact(&mut length_bytes).is_err() {
            break;
        }
        let length = u16::from_be_bytes(length_bytes) as usize;
        if length < 2 {
            break;
        }
        let payload_len = length - 2;
        if payload_len > MAX_PACKET {
            if file.seek(SeekFrom::Current(payload_len as i64)).is_err() {
                break;
            }
            continue;
        }
        let mut payload = vec![0u8; payload_len];
        if file.read_exact(&mut payload).is_err() {
            break;
        }
        if kind[0] == 0xE1 && payload.starts_with(b"http://ns.adobe.com/xap/1.0/\0") && packets.xmp.is_none() {
            packets.xmp = Some(String::from_utf8_lossy(&payload[29..]).into_owned());
        } else if kind[0] == 0xED && packets.iptc.is_none() {
            packets.iptc = Some(payload);
        }
    }
    packets
}

fn extract_png(file: &mut File) -> MetadataPackets {
    let mut signature = [0u8; 8];
    if file.read_exact(&mut signature).is_err() {
        return MetadataPackets::default();
    }
    let mut packets = MetadataPackets::default();
    loop {
        let mut length_bytes = [0u8; 4];
        if file.read_exact(&mut length_bytes).is_err() {
            break;
        }
        let length = u32::from_be_bytes(length_bytes) as usize;
        let mut kind = [0u8; 4];
        if file.read_exact(&mut kind).is_err() {
            break;
        }
        if length > MAX_PACKET {
            if file.seek(SeekFrom::Current(length as i64 + 4)).is_err() {
                break;
            }
            continue;
        }
        let mut data = vec![0u8; length];
        if file.read_exact(&mut data).is_err() || file.seek(SeekFrom::Current(4)).is_err() {
            break;
        }
        if &kind == b"IEND" {
            break;
        }
        if packets.xmp.is_none() && (&kind == b"iTXt" || &kind == b"zTXt" || &kind == b"tEXt") {
            if let Some(xml) = png_text_xmp(&kind, &data) {
                packets.xmp = Some(xml);
            }
        }
    }
    packets
}

fn png_text_xmp(kind: &[u8; 4], data: &[u8]) -> Option<String> {
    let (keyword, rest) = split_cstr(data)?;
    if keyword != "XML:com.adobe.xmp" {
        return None;
    }
    let text = match kind {
        b"tEXt" => String::from_utf8_lossy(rest).into_owned(),
        b"zTXt" => {
            let rest = rest.get(1..)?;
            inflate(rest)?
        }
        b"iTXt" => {
            let (compression, rest) = rest.split_first()?;
            let (method, rest) = rest.split_first()?;
            let (_, rest) = split_cstr(rest)?;
            let (_, text) = split_cstr(rest)?;
            if *compression == 0 {
                String::from_utf8_lossy(text).into_owned()
            } else if *method == 0 {
                inflate(text)?
            } else {
                return None;
            }
        }
        _ => return None,
    };
    if text.len() > MAX_PACKET { None } else { Some(text) }
}

fn inflate(data: &[u8]) -> Option<String> {
    use flate2::read::ZlibDecoder;
    let mut decoder = ZlibDecoder::new(data);
    let mut output = Vec::new();
    decoder.by_ref().take(MAX_PACKET as u64 + 1).read_to_end(&mut output).ok()?;
    if output.len() > MAX_PACKET { None } else { Some(String::from_utf8_lossy(&output).into_owned()) }
}

fn split_cstr(data: &[u8]) -> Option<(&str, &[u8])> {
    let end = data.iter().position(|byte| *byte == 0)?;
    let text = std::str::from_utf8(&data[..end]).ok()?;
    Some((text, &data[end + 1..]))
}

fn extract_tiff(file: &mut File) -> MetadataPackets {
    let mut header = [0u8; 8];
    if file.read_exact(&mut header).is_err() {
        return MetadataPackets::default();
    }
    let little = header.starts_with(b"II");
    let read_u16 = |bytes: &[u8]| if little { u16::from_le_bytes([bytes[0], bytes[1]]) } else { u16::from_be_bytes([bytes[0], bytes[1]]) };
    let read_u32 = |bytes: &[u8]| if little { u32::from_le_bytes(bytes.try_into().unwrap_or([0; 4])) } else { u32::from_be_bytes(bytes.try_into().unwrap_or([0; 4])) };
    if read_u16(&header[2..4]) != 42 {
        return MetadataPackets::default();
    }
    let ifd_offset = read_u32(&header[4..8]) as u64;
    if file.seek(SeekFrom::Start(ifd_offset)).is_err() {
        return MetadataPackets::default();
    }
    let mut count_bytes = [0u8; 2];
    if file.read_exact(&mut count_bytes).is_err() {
        return MetadataPackets::default();
    }
    let count = read_u16(&count_bytes) as usize;
    if count > 1024 {
        return MetadataPackets::default();
    }
    let mut packets = MetadataPackets::default();
    for _ in 0..count {
        let mut entry = [0u8; 12];
        if file.read_exact(&mut entry).is_err() {
            break;
        }
        let tag = read_u16(&entry[0..2]);
        let kind = read_u16(&entry[2..4]);
        let count = read_u32(&entry[4..8]) as usize;
        let unit = match kind {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            _ => 1,
        };
        let size = count.saturating_mul(unit);
        if size == 0 || size > MAX_PACKET || (tag != 700 && tag != 33723) {
            continue;
        }
        let bytes = if size <= 4 {
            entry[8..8 + size].to_vec()
        } else {
            let offset = read_u32(&entry[8..12]) as u64;
            let current = file.stream_position().unwrap_or(0);
            let Some(bytes) = read_at(file, offset, size) else {
                let _ = file.seek(SeekFrom::Start(current));
                continue;
            };
            let _ = file.seek(SeekFrom::Start(current));
            bytes
        };
        if tag == 700 && packets.xmp.is_none() {
            packets.xmp = Some(String::from_utf8_lossy(&bytes).into_owned());
        } else if tag == 33723 && packets.iptc.is_none() {
            packets.iptc = Some(bytes);
        }
    }
    packets
}

fn extract_webp(file: &mut File) -> MetadataPackets {
    let mut header = [0u8; 12];
    if file.read_exact(&mut header).is_err() || &header[0..4] != b"RIFF" || &header[8..12] != b"WEBP" {
        return MetadataPackets::default();
    }
    let mut packets = MetadataPackets::default();
    loop {
        let mut chunk = [0u8; 8];
        if file.read_exact(&mut chunk).is_err() {
            break;
        }
        let size = u32::from_le_bytes(chunk[4..8].try_into().unwrap_or([0; 4])) as usize;
        if &chunk[0..4] == b"XMP " && size <= MAX_PACKET {
            let mut data = vec![0u8; size];
            if file.read_exact(&mut data).is_err() {
                break;
            }
            packets.xmp = Some(String::from_utf8_lossy(&data).into_owned());
        } else if file.seek(SeekFrom::Current(size as i64)).is_err() {
            break;
        }
        if size % 2 == 1 && file.seek(SeekFrom::Current(1)).is_err() {
            break;
        }
        if packets.xmp.is_some() {
            break;
        }
    }
    packets
}

fn read_at(file: &mut File, offset: u64, size: usize) -> Option<Vec<u8>> {
    if size > MAX_PACKET {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buffer = vec![0u8; size];
    file.read_exact(&mut buffer).ok()?;
    Some(buffer)
}

fn scan_xmp_prefix(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut buffer = vec![0u8; SCAN_LIMIT];
    let read = file.read(&mut buffer).ok()?;
    find_xmp_packet(&buffer[..read])
}

fn find_xmp_packet(data: &[u8]) -> Option<String> {
    let start = find_slice(data, b"<x:xmpmeta").or_else(|| find_slice(data, b"<?xpacket begin"))?;
    let end_tag: &[u8] = if data[start..].starts_with(b"<?xpacket") { b"<?xpacket end" } else { b"</x:xmpmeta>" };
    let end_relative = find_slice(&data[start..], end_tag)?;
    let mut end = start + end_relative + end_tag.len();
    if let Some(close) = data.get(end..).and_then(|tail| tail.iter().position(|byte| *byte == b'>')) {
        end += close + 1;
    }
    if end - start > MAX_PACKET {
        return None;
    }
    Some(String::from_utf8_lossy(&data[start..end]).into_owned())
}

fn find_slice(data: &[u8], needle: &[u8]) -> Option<usize> {
    data.windows(needle.len()).position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xFF, marker];
        bytes.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn file_metadata_packets_jpeg_collects_xmp_and_app13_not_exif() {
        let dir = std::env::temp_dir().join(format!("lap-packets-jpeg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.jpg");
        let mut bytes = vec![0xFF, 0xD8];
        bytes.extend(segment(0xE1, b"Exif\0\0exif-body"));
        let mut xmp = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        xmp.extend_from_slice(b"<x:xmpmeta>hello</x:xmpmeta>");
        bytes.extend(segment(0xE1, &xmp));
        bytes.extend(segment(0xED, b"Photoshop 3.0\08BIM"));
        bytes.extend_from_slice(&[0xFF, 0xD9]);
        std::fs::write(&path, &bytes).unwrap();
        let packets = extract_packets(&path);
        assert!(packets.xmp.unwrap().contains("hello"));
        assert!(packets.iptc.unwrap().starts_with(b"Photoshop 3.0"));
        let exif_only = dir.join("exif.jpg");
        let mut only = vec![0xFF, 0xD8];
        only.extend(segment(0xE1, b"Exif\0\0body"));
        only.extend_from_slice(&[0xFF, 0xD9]);
        std::fs::write(&exif_only, only).unwrap();
        let packets = extract_packets(&exif_only);
        assert!(packets.xmp.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_metadata_packets_png_tiff_and_scan_cap() {
        let dir = std::env::temp_dir().join(format!("lap-packets-more-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut text = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
        text.extend_from_slice(b"<x:xmpmeta>png</x:xmpmeta>");
        bytes.extend_from_slice(&(text.len() as u32).to_be_bytes());
        bytes.extend_from_slice(b"iTXt");
        bytes.extend_from_slice(&text);
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        std::fs::write(&png, bytes).unwrap();
        assert!(extract_packets(&png).xmp.unwrap().contains("png"));

        let tiff = dir.join("a.tif");
        let xml = b"<x:xmpmeta>tiff</x:xmpmeta>";
        let mut data = Vec::new();
        data.extend_from_slice(b"II*\0");
        data.extend_from_slice(&8u32.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&700u16.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&(xml.len() as u32).to_le_bytes());
        data.extend_from_slice(&26u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(xml);
        std::fs::write(&tiff, data).unwrap();
        assert!(extract_packets(&tiff).xmp.unwrap().contains("tiff"));

        let late = dir.join("late.bin");
        let mut file = std::fs::File::create(&late).unwrap();
        file.write_all(&vec![0u8; SCAN_LIMIT + 16]).unwrap();
        file.write_all(b"<x:xmpmeta>late</x:xmpmeta>").unwrap();
        assert!(extract_packets(&late).xmp.is_none());
        assert!(extract_packets(dir.join("missing.bin").as_path()).xmp.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

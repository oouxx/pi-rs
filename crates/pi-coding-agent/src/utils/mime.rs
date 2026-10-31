//! MIME type detection for images.
//!
//! Mirrors packages/coding-agent/src/utils/mime.ts

/// Number of bytes to read for MIME type sniffing.
pub const IMAGE_TYPE_SNIFF_BYTES: usize = 4100;

const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// Detect the MIME type of an image from its raw bytes.
pub fn detect_supported_image_mime_type(buffer: &[u8]) -> Option<&'static str> {
    // JPEG: starts with FF D8 FF. Reject JPEG 2000 (FF D8 FF F7).
    if buffer.len() >= 3 && buffer[0] == 0xFF && buffer[1] == 0xD8 && buffer[2] == 0xFF {
        if buffer.get(3) == Some(&0xF7) {
            return None;
        }
        return Some("image/jpeg");
    }

    if buffer.starts_with(&PNG_SIGNATURE) {
        return if is_png(buffer) && !is_animated_png(buffer) {
            Some("image/png")
        } else {
            None
        };
    }

    // GIF: "GIF87a" or "GIF89a" (require the full 6-byte header so plain text
    // starting with "GIF" is not misclassified as an image).
    if buffer.len() >= 6 && (&buffer[..6] == b"GIF87a" || &buffer[..6] == b"GIF89a") {
        return Some("image/gif");
    }

    // WebP: "RIFF" .... "WEBP"
    if starts_with_ascii(buffer, 0, b"RIFF") && starts_with_ascii(buffer, 8, b"WEBP") {
        return Some("image/webp");
    }

    // BMP: "BM" plus a valid DIB header.
    if starts_with_ascii(buffer, 0, b"BM") && is_bmp(buffer) {
        return Some("image/bmp");
    }

    None
}

/// Detect the MIME type of an image file by reading from the filesystem.
pub fn detect_supported_image_mime_type_from_file(path: &str) -> Result<Option<String>, String> {
    let mut buffer = vec![0u8; IMAGE_TYPE_SNIFF_BYTES];
    let mut file = std::fs::File::open(path).map_err(|e| format!("Failed to open {path}: {e}"))?;

    use std::io::Read;
    let n = file
        .read(&mut buffer)
        .map_err(|e| format!("Failed to read {path}: {e}"))?;

    buffer.truncate(n);
    Ok(detect_supported_image_mime_type(&buffer).map(|s| s.to_string()))
}

fn is_png(buffer: &[u8]) -> bool {
    buffer.len() >= 16
        && read_u32_be(buffer, PNG_SIGNATURE.len()) == 13
        && starts_with_ascii(buffer, 12, b"IHDR")
}

fn is_animated_png(buffer: &[u8]) -> bool {
    let len = buffer.len() as u64;
    let mut offset = PNG_SIGNATURE.len() as u64;
    while offset + 8 <= len {
        let chunk_length = u64::from(read_u32_be(buffer, offset as usize));
        let chunk_type_offset = offset + 4;
        if starts_with_ascii(buffer, chunk_type_offset as usize, b"acTL") {
            return true;
        }
        if starts_with_ascii(buffer, chunk_type_offset as usize, b"IDAT") {
            return false;
        }

        let next_offset = offset + 8 + chunk_length + 4;
        if next_offset <= offset || next_offset > len {
            return false;
        }
        offset = next_offset;
    }
    false
}

fn is_bmp(buffer: &[u8]) -> bool {
    if buffer.len() < 26 {
        return false;
    }

    let declared_file_size = u64::from(read_u32_le(buffer, 2));
    let pixel_data_offset = u64::from(read_u32_le(buffer, 10));
    let dib_header_size = u64::from(read_u32_le(buffer, 14));
    if declared_file_size != 0 && declared_file_size < 26 {
        return false;
    }
    if pixel_data_offset < 14 + dib_header_size {
        return false;
    }
    if declared_file_size != 0 && pixel_data_offset >= declared_file_size {
        return false;
    }

    let (color_planes, bits_per_pixel) = if dib_header_size == 12 {
        (read_u16_le(buffer, 22), read_u16_le(buffer, 24))
    } else if (40..=124).contains(&dib_header_size) {
        if buffer.len() < 30 {
            return false;
        }
        (read_u16_le(buffer, 26), read_u16_le(buffer, 28))
    } else {
        return false;
    };

    color_planes == 1 && matches!(bits_per_pixel, 1 | 4 | 8 | 16 | 24 | 32)
}

fn read_u16_le(buffer: &[u8], offset: usize) -> u16 {
    u16::from(buffer.get(offset).copied().unwrap_or(0))
        | (u16::from(buffer.get(offset + 1).copied().unwrap_or(0)) << 8)
}

fn read_u32_be(buffer: &[u8], offset: usize) -> u32 {
    (u32::from(buffer.get(offset).copied().unwrap_or(0)) << 24)
        | (u32::from(buffer.get(offset + 1).copied().unwrap_or(0)) << 16)
        | (u32::from(buffer.get(offset + 2).copied().unwrap_or(0)) << 8)
        | u32::from(buffer.get(offset + 3).copied().unwrap_or(0))
}

fn read_u32_le(buffer: &[u8], offset: usize) -> u32 {
    u32::from(buffer.get(offset).copied().unwrap_or(0))
        | (u32::from(buffer.get(offset + 1).copied().unwrap_or(0)) << 8)
        | (u32::from(buffer.get(offset + 2).copied().unwrap_or(0)) << 16)
        | (u32::from(buffer.get(offset + 3).copied().unwrap_or(0)) << 24)
}

fn starts_with_ascii(buffer: &[u8], offset: usize, text: &[u8]) -> bool {
    buffer
        .get(offset..offset + text.len())
        .is_some_and(|slice| slice == text)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn png_with_idat() -> Vec<u8> {
        let mut png = PNG_SIGNATURE.to_vec();
        // IHDR chunk: length 13, type "IHDR", 13 bytes of payload, CRC.
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0u8; 13]);
        png.extend_from_slice(&[0u8; 4]);
        // IDAT chunk (empty payload) so is_animated_png stops scanning.
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(b"IDAT");
        png.extend_from_slice(&[0u8; 4]);
        png
    }

    #[test]
    fn test_detect_png() {
        assert_eq!(
            detect_supported_image_mime_type(&png_with_idat()),
            Some("image/png")
        );
    }

    #[test]
    fn test_png_signature_alone_is_not_enough() {
        // TS `isPng` also validates the IHDR chunk; a bare signature must not match.
        assert_eq!(detect_supported_image_mime_type(&PNG_SIGNATURE), None);
    }

    #[test]
    fn test_detect_animated_png_rejected() {
        let mut png = PNG_SIGNATURE.to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0u8; 13]);
        png.extend_from_slice(&[0u8; 4]);
        // acTL chunk before IDAT marks an APNG.
        png.extend_from_slice(&8u32.to_be_bytes());
        png.extend_from_slice(b"acTL");
        png.extend_from_slice(&[0u8; 12]);
        assert_eq!(detect_supported_image_mime_type(&png), None);
    }

    #[test]
    fn test_detect_jpeg() {
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0];
        assert_eq!(detect_supported_image_mime_type(&jpeg), Some("image/jpeg"));
    }

    #[test]
    fn test_detect_jpeg2000_rejected() {
        let jpeg2000 = vec![0xFF, 0xD8, 0xFF, 0xF7];
        assert_eq!(detect_supported_image_mime_type(&jpeg2000), None);
    }

    #[test]
    fn test_detect_gif() {
        let gif = b"GIF89a".to_vec();
        assert_eq!(detect_supported_image_mime_type(&gif), Some("image/gif"));
        assert_eq!(
            detect_supported_image_mime_type(b"GIF87a"),
            Some("image/gif")
        );
    }

    #[test]
    fn test_text_starting_with_gif_is_not_an_image() {
        // TS #9755: a text file whose first bytes are "GIF" must not be treated
        // as an image.
        assert_eq!(detect_supported_image_mime_type(b"GIF is a format"), None);
        assert_eq!(detect_supported_image_mime_type(b"GIf89a"), None);
    }

    #[test]
    fn test_detect_webp() {
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&0u32.to_le_bytes());
        webp.extend_from_slice(b"WEBP");
        assert_eq!(detect_supported_image_mime_type(&webp), Some("image/webp"));
    }

    #[test]
    fn test_detect_bmp_requires_valid_dib_header() {
        // A `BM` prefix alone is not a BMP per TS `isBmp`.
        assert_eq!(detect_supported_image_mime_type(b"BMnot-a-bmp"), None);
    }

    #[test]
    fn test_detect_valid_bmp() {
        let mut bmp = vec![0u8; 54];
        bmp[0] = b'B';
        bmp[1] = b'M';
        // declared file size (must be greater than the pixel data offset)
        bmp[2..6].copy_from_slice(&58u32.to_le_bytes());
        // pixel data offset
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        // BITMAPINFOHEADER
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        // planes = 1, bits per pixel = 24
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
        assert_eq!(detect_supported_image_mime_type(&bmp), Some("image/bmp"));
    }

    #[test]
    fn test_detect_unknown() {
        let unknown = b"not an image".to_vec();
        assert_eq!(detect_supported_image_mime_type(&unknown), None);
    }
}

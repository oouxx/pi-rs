//! MIME type detection for images and files.
//!
//! Mirrors packages/coding-agent/src/utils/mime.ts

/// Number of bytes to read for MIME type sniffing.
pub const IMAGE_TYPE_SNIFF_BYTES: usize = 4100;

/// Detect the MIME type of an image from its raw bytes.
pub fn detect_supported_image_mime_type(buffer: &[u8]) -> Option<&'static str> {
    // JPEG: starts with FF D8 FF
    if buffer.len() >= 3 && buffer[0] == 0xFF && buffer[1] == 0xD8 && buffer[2] == 0xFF {
        // Check for JPEG 2000 (FF D8 FF F7) which is not supported
        if buffer.len() >= 4 && buffer[3] == 0xF7 {
            return None;
        }
        return Some("image/jpeg");
    }

    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if buffer.len() >= 8 {
        let png_sig: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        if buffer[..8] == png_sig {
            return Some("image/png");
        }
    }

    // GIF: "GIF87a" or "GIF89a"
    if buffer.len() >= 6 && &buffer[..3] == b"GIF" {
        return Some("image/gif");
    }

    // WebP: "RIFF" .... "WEBP"
    if buffer.len() >= 12 && &buffer[..4] == b"RIFF" && &buffer[8..12] == b"WEBP" {
        return Some("image/webp");
    }

    // BMP: "BM"
    if buffer.len() >= 2 && &buffer[..2] == b"BM" {
        return Some("image/bmp");
    }

    None
}

/// Detect the MIME type of an image file by reading from the filesystem.
pub fn detect_supported_image_mime_type_from_file(path: &str) -> Result<Option<String>, String> {
    let mut buffer = vec![0u8; IMAGE_TYPE_SNIFF_BYTES];
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to open {path}: {e}"))?;

    use std::io::Read;
    let n = file.read(&mut buffer)
        .map_err(|e| format!("Failed to read {path}: {e}"))?;

    buffer.truncate(n);
    Ok(detect_supported_image_mime_type(&buffer).map(|s| s.to_string()))
}

/// Detect MIME type from file extension.
pub fn detect_mime_type_from_extension(path: &str) -> Option<&'static str> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        "ico" => Some("image/x-icon"),
        "pdf" => Some("application/pdf"),
        "json" => Some("application/json"),
        "txt" | "md" => Some("text/plain"),
        "html" | "htm" => Some("text/html"),
        "css" => Some("text/css"),
        "js" | "mjs" => Some("application/javascript"),
        "ts" | "tsx" => Some("application/typescript"),
        "rs" | "go" | "py" | "rb" | "java" | "c" | "cpp" | "h" | "hpp" => Some("text/plain"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_png() {
        // PNG signature bytes
        let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(detect_supported_image_mime_type(&png), Some("image/png"));
    }

    #[test]
    fn test_detect_jpeg() {
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0];
        assert_eq!(detect_supported_image_mime_type(&jpeg), Some("image/jpeg"));
    }

    #[test]
    fn test_detect_gif() {
        let gif = b"GIF89a".to_vec();
        assert_eq!(detect_supported_image_mime_type(&gif), Some("image/gif"));
    }

    #[test]
    fn test_detect_unknown() {
        let unknown = b"not an image".to_vec();
        assert_eq!(detect_supported_image_mime_type(&unknown), None);
    }

    #[test]
    fn test_detect_from_extension() {
        assert_eq!(detect_mime_type_from_extension("photo.jpg"), Some("image/jpeg"));
        assert_eq!(detect_mime_type_from_extension("image.png"), Some("image/png"));
        assert_eq!(detect_mime_type_from_extension("doc.pdf"), Some("application/pdf"));
        assert_eq!(detect_mime_type_from_extension("main.rs"), Some("text/plain"));
        assert_eq!(detect_mime_type_from_extension("style.css"), Some("text/css"));
    }
}

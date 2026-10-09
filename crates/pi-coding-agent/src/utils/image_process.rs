//! Image normalization + resize for the tool/attachment pipeline.
//!
//! Port of `packages/coding-agent/src/utils/image-process.ts` and
//! `image-resize-core.ts`. The TS original decodes/resizes with Photon
//! (Rust/WASM) inside a worker thread; Rust uses the `image` crate directly
//! (no worker needed — decoding is already native).
//!
//! Semantics kept identical:
//! - Supported inline formats: PNG / JPEG / GIF / WebP. Anything else
//!   (e.g. BMP) is converted to PNG.
//! - Default limits: 2000x2000 px, 4.5MB base64 payload (headroom under
//!   Anthropic's 5MB limit), JPEG quality 80.
//! - Images already within all limits pass through untouched; otherwise they
//!   are resized, encoded as PNG and JPEG (quality 80/85/70/55/40) and the
//!   first candidate under the byte limit wins; if none fit, dimensions are
//!   scaled down 25% at a time until 1x1.
//! - A dimension note is emitted when the image was resized so the model can
//!   map coordinates back to the original.

use std::io::Cursor;

use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::DynamicImage;

/// TS `ImageResizeOptions` defaults (`image-resize-core.ts`).
const MAX_WIDTH: u32 = 2000;
const MAX_HEIGHT: u32 = 2000;
/// 4.5MB of base64 payload — headroom below Anthropic's 5MB limit.
const MAX_BYTES: usize = (4.5 * 1024.0 * 1024.0) as usize;
const JPEG_QUALITY: u8 = 80;
/// TS `qualitySteps = [...new Set([jpegQuality, 85, 70, 55, 40])]`.
const QUALITY_STEPS: [u8; 5] = [JPEG_QUALITY, 85, 70, 55, 40];

/// A processed image ready to be attached as an inline content block.
#[derive(Debug)]
pub struct ProcessedImage {
    /// Base64-encoded image bytes.
    pub data: String,
    pub mime_type: String,
    /// Human-readable notes (conversion / dimension mapping), TS `hints`.
    pub hints: Vec<String>,
}

struct ResizedImage {
    data: String,
    mime_type: String,
    original_width: u32,
    original_height: u32,
    width: u32,
    height: u32,
    was_resized: bool,
}

/// TS `baseMimeType`.
fn base_mime(mime: &str) -> String {
    mime.split(';').next().unwrap_or(mime).trim().to_lowercase()
}

/// TS `normalizeSupportedImageMimeType`.
fn normalize_supported_mime(mime: &str) -> Option<&'static str> {
    match base_mime(mime).as_str() {
        "image/png" => Some("image/png"),
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        _ => None,
    }
}

fn encode_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// TS `Math.ceil(byteLength / 3) * 4` — base64 payload size without encoding.
fn base64_size(bytes_len: usize) -> usize {
    bytes_len.div_ceil(3) * 4
}

fn encode_png(img: &DynamicImage) -> Option<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    img.write_to(&mut cursor, image::ImageFormat::Png).ok()?;
    Some(cursor.into_inner())
}

fn encode_jpeg(img: &DynamicImage, quality: u8) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut cursor = Cursor::new(&mut out);
    let mut encoder = JpegEncoder::new_with_quality(&mut cursor, quality);
    encoder.encode_image(img).ok()?;
    drop(encoder);
    Some(out)
}

/// TS `convertImageBytesToPng`.
fn convert_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    encode_png(&img)
}

/// TS `conversionHint`.
fn conversion_hint(from: Option<&str>, to: &str) -> Option<String> {
    let from = from?;
    if from == to {
        return None;
    }
    Some(format!("[Image converted from {from} to {to}.]"))
}

/// TS `formatDimensionNote`.
fn format_dimension_note(result: &ResizedImage) -> Option<String> {
    if !result.was_resized {
        return None;
    }
    let scale = f64::from(result.original_width) / f64::from(result.width);
    Some(format!(
        "[Image: original {}x{}, displayed at {}x{}. Multiply coordinates by {scale:.2} to map to original image.]",
        result.original_width, result.original_height, result.width, result.height
    ))
}

/// TS `resizeImageInProcess`: returns `None` when the image cannot be decoded
/// or cannot be brought under `MAX_BYTES`.
fn resize_image(bytes: &[u8], mime_type: &str) -> Option<ResizedImage> {
    let input_base64_size = base64_size(bytes.len());
    let img = image::load_from_memory(bytes).ok()?;
    let original_width = img.width();
    let original_height = img.height();

    // Already within every limit: pass the original bytes through.
    if original_width <= MAX_WIDTH && original_height <= MAX_HEIGHT && input_base64_size < MAX_BYTES {
        // TS `mimeType.split("/")[1] ?? "png"`.
        let format = base_mime(mime_type);
        let format = format.strip_prefix("image/").filter(|s| !s.is_empty()).unwrap_or("png").to_string();
        let mime = if mime_type.is_empty() {
            format!("image/{format}")
        } else {
            mime_type.to_string()
        };
        return Some(ResizedImage {
            data: encode_base64(bytes),
            mime_type: mime,
            original_width,
            original_height,
            width: original_width,
            height: original_height,
            was_resized: false,
        });
    }

    // Initial target dimensions respecting the max bounds (preserve aspect).
    let mut target_width = original_width;
    let mut target_height = original_height;
    if target_width > MAX_WIDTH {
        target_height = ((u64::from(target_height) * u64::from(MAX_WIDTH)) as f64 / f64::from(target_width)).round() as u32;
        target_width = MAX_WIDTH;
    }
    if target_height > MAX_HEIGHT {
        target_width = ((u64::from(target_width) * u64::from(MAX_HEIGHT)) as f64 / f64::from(target_height)).round() as u32;
        target_height = MAX_HEIGHT;
    }

    let mut current_width = target_width.max(1);
    let mut current_height = target_height.max(1);
    loop {
        let resized = img.resize_exact(current_width, current_height, FilterType::Lanczos3);

        // Try PNG first, then JPEG at each quality step; first under limit wins.
        if let Some(png) = encode_png(&resized) {
            if base64_size(png.len()) < MAX_BYTES {
                return Some(ResizedImage {
                    data: encode_base64(&png),
                    mime_type: "image/png".to_string(),
                    original_width,
                    original_height,
                    width: current_width,
                    height: current_height,
                    was_resized: true,
                });
            }
        }
        for quality in QUALITY_STEPS {
            if let Some(jpeg) = encode_jpeg(&resized, quality) {
                if base64_size(jpeg.len()) < MAX_BYTES {
                    return Some(ResizedImage {
                        data: encode_base64(&jpeg),
                        mime_type: "image/jpeg".to_string(),
                        original_width,
                        original_height,
                        width: current_width,
                        height: current_height,
                        was_resized: true,
                    });
                }
            }
        }

        if current_width == 1 && current_height == 1 {
            break;
        }
        let next_width = if current_width == 1 { 1 } else { ((f64::from(current_width) * 0.75).floor() as u32).max(1) };
        let next_height = if current_height == 1 { 1 } else { ((f64::from(current_height) * 0.75).floor() as u32).max(1) };
        if next_width == current_width && next_height == current_height {
            break;
        }
        current_width = next_width;
        current_height = next_height;
    }

    None
}

/// TS `processImage`: normalize the MIME type (converting unsupported formats
/// to PNG), optionally resize to inline limits, and base64-encode.
///
/// Returns `Err` with the TS "omitted" message when the image cannot be
/// converted or resized below the byte limit.
pub fn process_image(bytes: &[u8], mime_type: &str, auto_resize: bool) -> Result<ProcessedImage, String> {
    // Normalize to a supported inline format (or convert to PNG).
    let (normalized_bytes, normalized_mime, converted_from) =
        if let Some(m) = normalize_supported_mime(mime_type) {
            (bytes.to_vec(), m.to_string(), None)
        } else {
            let png = convert_to_png(bytes).ok_or_else(|| {
                "[Image omitted: could not be converted to a supported inline image format.]".to_string()
            })?;
            (png, "image/png".to_string(), Some(base_mime(mime_type)))
        };

    if auto_resize {
        let resized = resize_image(&normalized_bytes, &normalized_mime).ok_or_else(|| {
            "[Image omitted: could not be resized below the inline image size limit.]".to_string()
        })?;
        let mut hints = Vec::new();
        if let Some(hint) = conversion_hint(converted_from.as_deref(), &resized.mime_type) {
            hints.push(hint);
        }
        if let Some(note) = format_dimension_note(&resized) {
            hints.push(note);
        }
        Ok(ProcessedImage {
            data: resized.data,
            mime_type: resized.mime_type,
            hints,
        })
    } else {
        let mut hints = Vec::new();
        if let Some(hint) = conversion_hint(converted_from.as_deref(), &normalized_mime) {
            hints.push(hint);
        }
        Ok(ProcessedImage {
            data: encode_base64(&normalized_bytes),
            mime_type: normalized_mime,
            hints,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// Build a solid-color PNG of the given size for tests.
    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(width, height, image::Rgb([10, 20, 30]));
        encode_png(&DynamicImage::ImageRgb8(img)).expect("encode png")
    }

    fn decode_dimensions(b64: &str) -> (u32, u32) {
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).expect("b64");
        let img = image::load_from_memory(&bytes).expect("decode");
        (img.width(), img.height())
    }

    #[test]
    fn small_image_passes_through_untouched() {
        let bytes = png_bytes(8, 8);
        let out = process_image(&bytes, "image/png", true).expect("ok");
        assert_eq!(out.mime_type, "image/png");
        assert!(out.hints.is_empty(), "no resize/conversion hints");
        // Original bytes, not re-encoded.
        assert_eq!(out.data, encode_base64(&bytes));
    }

    #[test]
    fn oversized_image_is_resized_with_dimension_note() {
        let bytes = png_bytes(3000, 1500);
        let out = process_image(&bytes, "image/png", true).expect("ok");
        let (w, h) = decode_dimensions(&out.data);
        assert_eq!((w, h), (2000, 1000), "scaled to max width, aspect preserved");
        assert_eq!(out.mime_type, "image/png", "small enough as PNG");
        assert_eq!(out.hints.len(), 1);
        assert!(out.hints[0].contains("original 3000x1500"), "got: {:?}", out.hints);
        assert!(out.hints[0].contains("displayed at 2000x1000"), "got: {:?}", out.hints);
    }

    #[test]
    fn unsupported_format_converted_to_png_with_hint() {
        // BMP is detected by mime but not a supported inline format.
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]));
        let mut bmp = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(img)
            .write_to(&mut bmp, image::ImageFormat::Bmp)
            .expect("bmp");
        let out = process_image(bmp.get_ref(), "image/bmp", true).expect("ok");
        assert_eq!(out.mime_type, "image/png");
        assert_eq!(out.hints.len(), 1);
        assert_eq!(out.hints[0], "[Image converted from image/bmp to image/png.]");
    }

    #[test]
    fn non_resize_path_keeps_original_mime_and_size() {
        let bytes = png_bytes(3000, 1500);
        let out = process_image(&bytes, "image/png", false).expect("ok");
        assert_eq!(out.mime_type, "image/png");
        assert!(out.hints.is_empty());
        // No resize → original bytes base64.
        assert_eq!(out.data, encode_base64(&bytes));
    }

    #[test]
    fn undecodable_bytes_report_omitted_message() {
        // Unsupported mime + garbage bytes → conversion failure.
        let err = process_image(b"not an image", "image/bmp", true).unwrap_err();
        assert_eq!(
            err,
            "[Image omitted: could not be converted to a supported inline image format.]"
        );
    }
}

//! Process `@file` CLI arguments into text content and image attachments.
//!
//! Mirrors packages/coding-agent/src/cli/file-processor.ts

use std::io::Read;
use std::path::Path;

use colored::*;

use crate::core::tools::path_utils::resolve_read_path;
use crate::utils::mime::detect_supported_image_mime_type_from_file;

/// Result of processing file arguments.
#[derive(Debug, Clone, Default)]
pub struct ProcessedFiles {
    pub text: String,
    pub images: Vec<ImageAttachment>,
}

/// An image attachment for an agent message.
#[derive(Debug, Clone)]
pub struct ImageAttachment {
    pub data: String,
    pub mime_type: String,
}

/// Options for file processing.
#[derive(Debug, Clone)]
pub struct ProcessFileOptions {
    pub auto_resize_images: bool,
}

impl Default for ProcessFileOptions {
    fn default() -> Self {
        ProcessFileOptions {
            auto_resize_images: true,
        }
    }
}

/// Process `@file` arguments into text content and image attachments.
///
/// Each file argument is resolved relative to `cwd`. Text files have their
/// content wrapped in `<file>` tags. Image files are detected via magic bytes
/// and returned as image attachments with a text reference.
pub fn process_file_arguments(
    file_args: &[String],
    cwd: &str,
) -> ProcessedFiles {
    let mut text = String::new();
    let mut images = Vec::new();

    for file_arg in file_args {
        let arg = file_arg.trim();

        // Strip leading @ if present (used for @file syntax)
        let file_path = if arg.starts_with('@') {
            &arg[1..]
        } else {
            arg
        };

        let resolved = resolve_read_path(file_path, cwd);
        let absolute_path = match std::fs::canonicalize(&resolved) {
            Ok(p) => p,
            Err(_) => {
                eprintln!("{} File not found: {}", "Error:".red().bold(), resolved.display());
                std::process::exit(1);
            }
        };

        // Check if file exists and is not empty
        let metadata = match std::fs::metadata(&absolute_path) {
            Ok(m) => m,
            Err(_) => {
                eprintln!("{} File not found: {}", "Error:".red().bold(), absolute_path.display());
                std::process::exit(1);
            }
        };

        if metadata.len() == 0 {
            continue;
        }

        let path_str = absolute_path.to_string_lossy().to_string();

        // Detect if it's an image file
        match detect_supported_image_mime_type_from_file(&path_str) {
            Ok(Some(mime_type)) => {
                // Read and encode image
                match read_image_file(&path_str) {
                    Ok(data) => {
                        images.push(ImageAttachment {
                            data: data.clone(),
                            mime_type: mime_type.clone(),
                        });
                        text.push_str(&format!("<file name=\"{path_str}\"></file>\n"));
                    }
                    Err(e) => {
                        text.push_str(&format!("<file name=\"{path_str}\">{e}</file>\n"));
                    }
                }
            }
            Ok(None) => {
                // Text file
                match read_text_file(&path_str) {
                    Ok(content) => {
                        text.push_str(&format!("<file name=\"{path_str}\">\n{content}\n</file>\n"));
                    }
                    Err(e) => {
                        eprintln!("{} Could not read file {}: {}", "Error:".red().bold(), path_str, e);
                        std::process::exit(1);
                    }
                }
            }
            Err(e) => {
                eprintln!("{} Error reading file {}: {}", "Error:".red().bold(), path_str, e);
                std::process::exit(1);
            }
        }
    }

    ProcessedFiles { text, images }
}

/// Read a text file's contents as UTF-8.
fn read_text_file(path: &str) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to open: {e}"))?;
    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|e| format!("Failed to read: {e}"))?;
    Ok(content)
}

/// Read an image file and base64-encode its contents.
fn read_image_file(path: &str) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to open image: {e}"))?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .map_err(|e| format!("Failed to read image: {e}"))?;

    Ok(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &buffer))
}

/// Process @file references embedded in a message string.
pub fn extract_file_refs(message: &str, cwd: &str) -> ProcessedFiles {
    let mut file_args = Vec::new();

    for word in message.split_whitespace() {
        if word.starts_with('@') && word.len() > 1 {
            file_args.push(word.to_string());
        }
    }

    process_file_arguments(&file_args, cwd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_process_text_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello world").unwrap();

        let result = process_file_arguments(
            &[file_path.to_string_lossy().to_string()],
            dir.path().to_str().unwrap(),
        );

        assert!(result.text.contains("<file name="));
        assert!(result.text.contains("hello world"));
        assert!(result.text.contains("</file>"));
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_process_empty_file_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("empty.txt");
        fs::write(&file_path, "").unwrap();

        let result = process_file_arguments(
            &[file_path.to_string_lossy().to_string()],
            dir.path().to_str().unwrap(),
        );

        assert!(result.text.is_empty());
    }

    #[test]
    fn test_process_with_at_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("data.txt");
        fs::write(&file_path, "content").unwrap();

        let result = process_file_arguments(
            &[format!("@{}", file_path.to_string_lossy())],
            dir.path().to_str().unwrap(),
        );

        assert!(result.text.contains("content"));
    }

    #[test]
    fn test_extract_file_refs() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("notes.txt");
        fs::write(&file_path, "notes content").unwrap();

        let message = format!("read @{} and summarize", file_path.to_string_lossy());
        let result = extract_file_refs(&message, dir.path().to_str().unwrap());

        assert!(result.text.contains("notes content"));
    }

    #[test]
    fn test_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "file a").unwrap();
        fs::write(dir.path().join("b.txt"), "file b").unwrap();

        let a = dir.path().join("a.txt").to_string_lossy().to_string();
        let b = dir.path().join("b.txt").to_string_lossy().to_string();

        let result = process_file_arguments(&[a, b], dir.path().to_str().unwrap());
        assert!(result.text.contains("file a"));
        assert!(result.text.contains("file b"));
    }

    #[test]
    fn test_extract_no_refs() {
        let result = extract_file_refs("hello world without refs", "/tmp");
        assert!(result.text.is_empty());
        assert!(result.images.is_empty());
    }
}

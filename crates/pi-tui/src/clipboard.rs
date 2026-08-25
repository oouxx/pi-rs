//! System clipboard access — Rust port of the TS original
//! (`packages/coding-agent/src/utils/clipboard.ts` + `clipboard-native.ts`).
//!
//! Write (`copy_to_clipboard`): platform clipboard tools (`pbcopy` on macOS,
//! `clip` on Windows, `termux-clipboard-set` / `wl-copy` / `xclip` / `xsel`
//! on Linux) with an OSC 52 escape-sequence fallback (needed for SSH/remote
//! sessions where no local display tool exists). Read (`read_clipboard_text`):
//! `pbpaste` / `Get-Clipboard` / `wl-paste` / `xclip` / `xsel`.
//!
//! Clipboard errors are silent for reads (TS "Silently ignore clipboard
//! errors") and surfaced as `Err` for writes.

use std::io::Write;
use std::process::{Command, Stdio};

/// TS `MAX_OSC52_ENCODED_LENGTH` — OSC 52 payloads beyond this are dropped
/// (terminals desynchronize on huge pastes).
const MAX_OSC52_ENCODED_LENGTH: usize = 100_000;

/// True on remote sessions (TS `isRemoteSession`): OSC 52 is the only
/// clipboard transport that works through SSH/mosh.
fn is_remote_session() -> bool {
    std::env::var("SSH_CONNECTION").is_ok()
        || std::env::var("SSH_CLIENT").is_ok()
        || std::env::var("MOSH_CONNECTION").is_ok()
}

/// Write `text` to a command's stdin and wait for a clean exit.
fn pipe_to(program: &str, args: &[&str], text: &str) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn {program}: {e}"))?;
    let mut stdin = child.stdin.take().ok_or_else(|| format!("{program}: no stdin"))?;
    let _ = stdin.write_all(text.as_bytes());
    drop(stdin);
    let status = child.wait().map_err(|e| format!("wait {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

/// Run a command capturing stdout as UTF-8 (clipboard reads).
fn read_from(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

// ── Linux ──────────────────────────────────────────────────────────────────

/// X11 copy with `xclip`, falling back to `xsel` (TS `copyToX11Clipboard`).
#[cfg(target_os = "linux")]
fn copy_x11(text: &str) -> bool {
    pipe_to("xclip", &["-selection", "clipboard"], text).is_ok()
        || pipe_to("xsel", &["--clipboard", "--input"], text).is_ok()
}

#[cfg(target_os = "linux")]
fn linux_copy(text: &str) -> bool {
    if std::env::var("TERMUX_VERSION").is_ok() && pipe_to("termux-clipboard-set", &[], text).is_ok() {
        return true;
    }
    if std::env::var("WAYLAND_DISPLAY").is_ok() && pipe_to("wl-copy", &[], text).is_ok() {
        return true;
    }
    if std::env::var("DISPLAY").is_ok() && copy_x11(text) {
        return true;
    }
    // No DISPLAY/WAYLAND_DISPLAY env: try xclip/xsel anyway (some setups
    // hardcode the display in XAUTHORITY).
    copy_x11(text)
}

#[cfg(target_os = "linux")]
fn linux_read() -> Option<String> {
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        if let Some(text) = read_from("wl-paste", &["--no-newline", "--type", "text"]) {
            return Some(text);
        }
    }
    read_from("xclip", &["-selection", "clipboard", "-o"])
        .or_else(|| read_from("xsel", &["--clipboard", "--output"]))
}

// ── OSC 52 ─────────────────────────────────────────────────────────────────

/// Base64 (RFC 4648) — small encoder so pi-tui needs no extra dependency.
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(n >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[n as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Emit an OSC 52 clipboard write (`ESC ] 52 ; c ; base64 ST`). Returns
/// false when the payload is too large (TS drops it above 100k encoded
/// chars).
fn emit_osc52(text: &str) -> bool {
    let encoded = base64_encode(text.as_bytes());
    if encoded.len() > MAX_OSC52_ENCODED_LENGTH {
        return false;
    }
    let mut stdout = std::io::stdout().lock();
    let _ = write!(stdout, "\u{1b}]52;c;{encoded}\u{07}");
    let _ = stdout.flush();
    true
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Copy `text` to the system clipboard (TS `copyToClipboard`).
///
/// Platform tools first; OSC 52 is emitted when the session is remote or no
/// tool succeeded. Returns an error only when every transport failed.
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    if is_remote_session() && emit_osc52(text) {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    let copied = pipe_to("pbcopy", &[], text).is_ok();
    #[cfg(target_os = "windows")]
    let copied = pipe_to("clip", &[], text).is_ok();
    #[cfg(target_os = "linux")]
    let copied = linux_copy(text);
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    let copied = false;

    if copied || emit_osc52(text) {
        Ok(())
    } else {
        Err("Failed to copy to clipboard (no clipboard tool and OSC 52 unavailable)".into())
    }
}

/// Read plain text from the system clipboard (TS `readClipboardText`).
/// Returns `None` when unavailable (no tool / permission denied).
pub fn read_clipboard_text() -> Option<String> {
    #[cfg(target_os = "macos")]
    let read = read_from("pbpaste", &[]);
    #[cfg(target_os = "windows")]
    let read = read_from("powershell.exe", &["-NoProfile", "-Command", "Get-Clipboard -Raw"]);
    #[cfg(target_os = "linux")]
    let read = linux_read();
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    let read = None;
    read
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4648 vectors — the OSC 52 payload must encode exactly like the TS
    /// `Buffer.from(text).toString("base64")`.
    #[test]
    fn base64_encodes_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode("你好".as_bytes()), "5L2g5aW9");
    }

    /// OSC 52 payloads beyond 100k encoded chars are refused (TS
    /// `MAX_OSC52_ENCODED_LENGTH`).
    #[test]
    fn osc52_refuses_oversized_payload() {
        let big = "x".repeat(80_000);
        assert!(!emit_osc52(&big), "80k ASCII → ~107k base64 > 100k limit");
        let small = "x".repeat(70_000);
        assert!(emit_osc52(&small), "70k ASCII fits");
    }
}

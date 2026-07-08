use std::process::Command;

/// Open a URL or file in the system's default browser/handler.
///
/// Best-effort: errors are silently ignored.
/// Never invokes a shell (uses `Command::new` directly).
pub fn open_browser(target: &str) {
    let (program, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("open", &[target])
    } else if cfg!(target_os = "windows") {
        ("rundll32", &["url.dll,FileProtocolHandler", target])
    } else {
        ("xdg-open", &[target])
    };

    // Spawn and forget: detach from the child process.
    // Do NOT call .kill() — that would terminate the opener before
    // it can communicate with the system browser/handler.
    let _ = Command::new(program).args(args).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    // No tests — open_browser is a best-effort function that spawns
    // a system process; testing it would open actual browser windows.
}

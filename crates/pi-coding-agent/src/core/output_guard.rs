use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

static STDOUT_TAKEN_OVER: AtomicBool = AtomicBool::new(false);

struct OutputGuardState {
    original_stdout: Box<dyn Write + Send>,
    raw_stdout: Option<std::fs::File>,
}

impl OutputGuardState {
    fn raw_stdout() -> Option<std::fs::File> {
        #[cfg(unix)]
        {
            std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/stdout")
                .ok()
        }
        #[cfg(not(unix))]
        {
            std::fs::OpenOptions::new().write(true).open("CONOUT$").ok()
        }
    }
}

pub struct OutputGuard {
    state: Option<OutputGuardState>,
}

impl OutputGuard {
    pub fn new() -> Self {
        OutputGuard { state: None }
    }

    pub fn take_over_stdout(&mut self) {
        if STDOUT_TAKEN_OVER.load(Ordering::SeqCst) {
            return;
        }

        let original_stdout: Box<dyn Write + Send> = Box::new(io::stdout());
        let raw_stdout = OutputGuardState::raw_stdout();

        // Redirect stdout to stderr
        // In Rust, we can't easily replace the global stdout write function like in JS.
        // Instead, we provide a mechanism through `write_to_stdout_or_stderr`.
        // Actual process-level stdout redirection would require raw fd manipulation.
        // For practical purposes, we set the flag and provide helper functions.

        self.state = Some(OutputGuardState {
            original_stdout,
            raw_stdout,
        });
        STDOUT_TAKEN_OVER.store(true, Ordering::SeqCst);
    }

    pub fn restore_stdout(&mut self) {
        if !STDOUT_TAKEN_OVER.load(Ordering::SeqCst) {
            return;
        }
        self.state = None;
        STDOUT_TAKEN_OVER.store(false, Ordering::SeqCst);
    }

    pub fn is_taken_over(&self) -> bool {
        STDOUT_TAKEN_OVER.load(Ordering::SeqCst)
    }

    pub fn write_raw_stdout(&self, text: &str) -> io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        if let Some(ref state) = self.state {
            if let Some(ref file) = state.raw_stdout {
                let mut file = file.try_clone()?;
                write!(file, "{}", text)?;
                file.flush()?;
            }
        }
        Ok(())
    }

    pub fn flush_raw_stdout(&self) -> io::Result<()> {
        if let Some(ref state) = self.state {
            if let Some(ref file) = state.raw_stdout {
                let mut file = file.try_clone()?;
                file.flush()?;
            }
        }
        Ok(())
    }
}

pub fn is_stdout_taken_over() -> bool {
    STDOUT_TAKEN_OVER.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state() {
        let guard = OutputGuard::new();
        assert!(!guard.is_taken_over());
    }

    #[test]
    fn test_take_over_and_restore() {
        let mut guard = OutputGuard::new();
        guard.take_over_stdout();
        assert!(guard.is_taken_over());
        guard.restore_stdout();
        assert!(!guard.is_taken_over());
    }

    #[test]
    fn test_write_raw_stdout_empty() {
        let mut guard = OutputGuard::new();
        guard.take_over_stdout();
        // Should not panic
        let _ = guard.write_raw_stdout("");
        guard.restore_stdout();
    }
}

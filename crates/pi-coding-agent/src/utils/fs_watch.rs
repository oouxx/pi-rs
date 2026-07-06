//! File system watching utilities.
//!
//! Mirrors packages/coding-agent/src/utils/fs-watch.ts

use std::path::Path;
use std::thread;
use std::time::Duration;

/// Default retry delay for file watcher errors.
pub const FS_WATCH_RETRY_DELAY_MS: u64 = 5000;

/// A file system watcher that polls a path for changes.
pub struct FsWatcher {
    _path: String,
    stop_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl FsWatcher {
    /// Create a new file watcher for the given path.
    /// `on_change` is called when the file changes.
    /// `on_error` is called when an error occurs.
    pub fn watch(
        path: &str,
        on_change: Box<dyn Fn() + Send + 'static>,
    ) -> Result<Self, String> {
        let file_path = path.to_string();
        let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop_flag.clone();
        let watch_path = file_path.clone();

        if !Path::new(&file_path).exists() {
            return Err(format!("Path does not exist: {file_path}"));
        }

        let handle = thread::spawn(move || {
            let mut last_modified = std::fs::metadata(&watch_path)
                .and_then(|m| m.modified())
                .ok();

            while !flag.load(std::sync::atomic::Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(1000));

                if !Path::new(&watch_path).exists() {
                    continue;
                }

                let current = std::fs::metadata(&watch_path)
                    .and_then(|m| m.modified())
                    .ok();

                match (last_modified.as_ref(), current.as_ref()) {
                    (Some(last), Some(curr)) if *curr > *last => {
                        last_modified = current;
                        on_change();
                    }
                    (None, Some(_)) => {
                        last_modified = current;
                        on_change();
                    }
                    _ => {}
                }
            }
        });

        Ok(FsWatcher {
            _path: file_path,
            stop_flag,
            handle: Some(handle),
        })
    }

    /// Close the watcher.
    pub fn close(&mut self) {
        self.stop_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for FsWatcher {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watch_nonexistent() {
        let result = FsWatcher::watch(
            "/nonexistent/path",
            Box::new(|| {}),
        );
        assert!(result.is_err());
    }
}

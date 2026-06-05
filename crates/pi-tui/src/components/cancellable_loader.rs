use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::keybindings::get_keybindings;
use crate::tui::Component;

use super::loader::{Loader, LoaderIndicatorOptions};

pub struct CancellableLoader {
    loader: Loader,
    cancelled: Arc<AtomicBool>,
    on_abort: Option<Box<dyn Fn() + Send + Sync>>,
}

impl CancellableLoader {
    pub fn new(
        request_render: Option<Box<dyn Fn() + Send + Sync>>,
        spinner_color_fn: Box<dyn Fn(&str) -> String + Send + Sync>,
        message_color_fn: Box<dyn Fn(&str) -> String + Send + Sync>,
        message: impl Into<String>,
        indicator: Option<LoaderIndicatorOptions>,
    ) -> Self {
        Self {
            loader: Loader::new(request_render, spinner_color_fn, message_color_fn, message, indicator),
            cancelled: Arc::new(AtomicBool::new(false)),
            on_abort: None,
        }
    }

    pub fn signal(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }

    pub fn aborted(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn set_on_abort(&mut self, cb: Option<Box<dyn Fn() + Send + Sync>>) {
        self.on_abort = cb;
    }

    pub fn dispose(&mut self) {
        self.loader.stop();
    }

    pub fn start(&mut self) {
        self.loader.start();
    }

    pub fn stop(&mut self) {
        self.loader.stop();
    }

    pub fn set_message(&mut self, message: impl Into<String>) {
        self.loader.set_message(message);
    }

    pub fn advance_frame(&mut self) {
        self.loader.advance_frame();
    }
}

impl Component for CancellableLoader {
    fn render(&self, width: u16) -> Vec<String> {
        self.loader.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        let kb = get_keybindings();
        if kb.matches(data, "cancel") {
            self.cancelled.store(true, Ordering::SeqCst);
            if let Some(ref cb) = self.on_abort {
                cb();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cancellable_loader_creation() {
        let cl = CancellableLoader::new(
            None,
            Box::new(|s| s.to_string()),
            Box::new(|s| s.to_string()),
            "Working...",
            None,
        );
        assert!(!cl.aborted());
    }

    #[test]
    fn test_cancellable_loader_abort() {
        let mut cl = CancellableLoader::new(
            None,
            Box::new(|s| s.to_string()),
            Box::new(|s| s.to_string()),
            "Working...",
            None,
        );
        cl.handle_input("\x1b");
        assert!(cl.aborted());
    }

    #[test]
    fn test_cancellable_loader_signal() {
        let cl = CancellableLoader::new(
            None,
            Box::new(|s| s.to_string()),
            Box::new(|s| s.to_string()),
            "Working...",
            None,
        );
        let signal = cl.signal();
        assert!(!signal.load(Ordering::SeqCst));
    }

    #[test]
    fn test_cancellable_loader_render() {
        let cl = CancellableLoader::new(
            None,
            Box::new(|s| s.to_string()),
            Box::new(|s| s.to_string()),
            "Working...",
            None,
        );
        let lines = cl.render(80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_cancellable_loader_dispose() {
        let mut cl = CancellableLoader::new(
            None,
            Box::new(|s| s.to_string()),
            Box::new(|s| s.to_string()),
            "Working...",
            None,
        );
        cl.dispose();
        // After dispose, render still works
        let lines = cl.render(80);
        assert!(!lines.is_empty());
    }
}

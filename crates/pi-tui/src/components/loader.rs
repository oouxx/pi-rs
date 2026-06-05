use std::time::Instant;

use crate::tui::Component;
use crate::utils::wrap_text_with_ansi;

pub const DEFAULT_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
pub const DEFAULT_INTERVAL_MS: u64 = 80;

pub struct LoaderIndicatorOptions {
    pub frames: Option<Vec<String>>,
    pub interval_ms: Option<u64>,
}

impl Default for LoaderIndicatorOptions {
    fn default() -> Self {
        Self {
            frames: None,
            interval_ms: None,
        }
    }
}

pub struct Loader {
    frames: Vec<String>,
    interval_ms: u64,
    current_frame: usize,
    last_update: Instant,
    message: String,
    display_text: String,
    padding_y: u16,
    render_indicator_verbatim: bool,
    spinner_color_fn: Box<dyn Fn(&str) -> String + Send + Sync>,
    message_color_fn: Box<dyn Fn(&str) -> String + Send + Sync>,
    request_render: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Loader {
    pub fn new(
        request_render: Option<Box<dyn Fn() + Send + Sync>>,
        spinner_color_fn: Box<dyn Fn(&str) -> String + Send + Sync>,
        message_color_fn: Box<dyn Fn(&str) -> String + Send + Sync>,
        message: impl Into<String>,
        indicator: Option<LoaderIndicatorOptions>,
    ) -> Self {
        let mut slf = Self {
            frames: DEFAULT_FRAMES.iter().map(|s| s.to_string()).collect(),
            interval_ms: DEFAULT_INTERVAL_MS,
            current_frame: 0,
            last_update: Instant::now(),
            message: message.into(),
            display_text: String::new(),
            padding_y: 1,
            render_indicator_verbatim: false,
            spinner_color_fn,
            message_color_fn,
            request_render,
        };
        slf.set_indicator(indicator);
        slf.update_display();
        slf
    }

    pub fn start(&mut self) {
        self.last_update = Instant::now();
        self.update_display();
    }

    pub fn stop(&mut self) {}

    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.update_display();
    }

    pub fn set_indicator(&mut self, indicator: Option<LoaderIndicatorOptions>) {
        self.render_indicator_verbatim = indicator.is_some();
        if let Some(ind) = indicator {
            if let Some(frames) = ind.frames {
                if frames.is_empty() {
                    self.frames.clear();
                } else {
                    self.frames = frames;
                }
            }
            if let Some(ms) = ind.interval_ms {
                if ms > 0 {
                    self.interval_ms = ms;
                }
            }
        } else {
            self.frames = DEFAULT_FRAMES.iter().map(|s| s.to_string()).collect();
            self.interval_ms = DEFAULT_INTERVAL_MS;
        }
        self.current_frame = 0;
        self.start();
    }

    pub fn advance_frame(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_update);
        if elapsed.as_millis() as u64 >= self.interval_ms && self.frames.len() > 1 {
            self.current_frame = (self.current_frame + 1) % self.frames.len();
            self.last_update = now;
            self.update_display();
        }
    }

    fn update_display(&mut self) {
        let frame = self.frames.get(self.current_frame).cloned().unwrap_or_default();
        let rendered_frame = if self.render_indicator_verbatim {
            frame.clone()
        } else {
            (self.spinner_color_fn)(&frame)
        };
        let indicator = if frame.is_empty() {
            String::new()
        } else {
            format!("{} ", rendered_frame)
        };
        self.display_text = format!("{}{}", indicator, (self.message_color_fn)(&self.message));
        if let Some(ref cb) = self.request_render {
            cb();
        }
    }
}

impl Component for Loader {
    fn render(&self, width: u16) -> Vec<String> {
        let mut result = Vec::new();
        for _ in 0..self.padding_y {
            result.push(" ".repeat(width as usize));
        }
        for line in wrap_text_with_ansi(&self.display_text, width as usize) {
            result.push(line);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loader_creation() {
        let loader = Loader::new(
            None,
            Box::new(|s| format!("\x1b[36m{}\x1b[0m", s)),
            Box::new(|s| format!("\x1b[2m{}\x1b[0m", s)),
            "Loading...",
            None,
        );
        let lines = loader.render(80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_loader_render_multi_line() {
        let loader = Loader::new(
            None,
            Box::new(|s| s.to_string()),
            Box::new(|s| s.to_string()),
            "Loading...",
            None,
        );
        let lines = loader.render(80);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), 80);
        assert!(lines[1].contains("Loading"));
    }

    #[test]
    fn test_loader_set_message() {
        let mut loader = Loader::new(
            None,
            Box::new(|s| s.to_string()),
            Box::new(|s| s.to_string()),
            "Loading...",
            None,
        );
        loader.set_message("Processing...");
        let lines = loader.render(80);
        assert!(lines[1].contains("Processing"));
    }

    #[test]
    fn test_loader_advance_frame() {
        let mut loader = Loader::new(
            None,
            Box::new(|s| s.to_string()),
            Box::new(|s| s.to_string()),
            "test",
            None,
        );
        let frame0 = loader.current_frame;
        // Force advance by manipulating last_update
        loader.last_update = Instant::now() - std::time::Duration::from_millis(200);
        loader.advance_frame();
        assert_ne!(loader.current_frame, frame0);
    }

    #[test]
    fn test_loader_no_advance_before_interval() {
        let mut loader = Loader::new(
            None,
            Box::new(|s| s.to_string()),
            Box::new(|s| s.to_string()),
            "test",
            None,
        );
        let frame0 = loader.current_frame;
        loader.advance_frame();
        assert_eq!(loader.current_frame, frame0);
    }

    #[test]
    fn test_loader_empty_frames() {
        let mut loader = Loader::new(
            None,
            Box::new(|s| s.to_string()),
            Box::new(|s| s.to_string()),
            "no spinner",
            Some(LoaderIndicatorOptions {
                frames: Some(vec![]),
                interval_ms: None,
            }),
        );
        let lines = loader.render(80);
        assert!(lines[1].contains("no spinner"));
        assert!(!lines[1].contains("⠋"));
    }

    #[test]
    fn test_loader_width_zero() {
        let loader = Loader::new(
            None,
            Box::new(|s| s.to_string()),
            Box::new(|s| s.to_string()),
            "test",
            None,
        );
        let lines = loader.render(0);
        assert!(!lines.is_empty());
    }
}

use std::fmt::Write;

use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Color;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{Image, Resize};

use crate::tui::Component;

/// Terminal image component using ratatui-image for rendering.
///
/// Supports all terminal graphics protocols detected by `ratatui-image`:
/// Kitty, iTerm2, Sixel, and Unicode Halfblocks.
pub struct ImageComponent {
    protocol: Protocol,
    cell_size: (u16, u16),
}

impl ImageComponent {
    /// Create a new ImageComponent from a `DynamicImage` using the given `Picker`.
    ///
    /// The `picker` handles terminal capability detection and font size querying.
    /// The image will be fit to the available cell area, maintaining aspect ratio.
    pub fn from_picker(picker: &Picker, img: DynamicImage) -> Result<Self, String> {
        let font_size = picker.font_size();
        let img_w = img.width();
        let img_h = img.height();
        let cell_w = font_size.width as u32;
        let cell_h = font_size.height as u32;

        let cols = (img_w + cell_w - 1).div_ceil(cell_w).max(1) as u16;
        let rows = (img_h + cell_h - 1).div_ceil(cell_h).max(1) as u16;

        let size = ratatui::layout::Size::new(cols, rows);
        let protocol = picker
            .new_protocol(img, size, Resize::Fit(None))
            .map_err(|e| format!("Protocol creation failed: {}", e))?;

        Ok(Self {
            protocol,
            cell_size: (cols, rows),
        })
    }

    /// Load an image from a file path and create an ImageComponent.
    pub fn from_path(picker: &Picker, path: &str) -> Result<Self, String> {
        let img = image::ImageReader::open(path)
            .map_err(|e| format!("Cannot open image: {}", e))?
            .decode()
            .map_err(|e| format!("Cannot decode image: {}", e))?;
        Self::from_picker(picker, img)
    }

    /// Create an ImageComponent from raw RGBA pixel data.
    pub fn from_rgba(
        picker: &Picker,
        data: &[u8],
        img_width: u32,
        img_height: u32,
    ) -> Result<Self, String> {
        let img = DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(img_width, img_height, data.to_vec())
                .ok_or("Invalid image dimensions")?,
        );
        Self::from_picker(picker, img)
    }

    /// Get the size of the rendered image in terminal cells.
    pub fn cell_size(&self) -> (u16, u16) {
        self.cell_size
    }

    fn is_halfblocks(&self) -> bool {
        matches!(self.protocol, Protocol::Halfblocks(_))
    }

    fn render_to_lines(&self, area: Rect) -> Vec<String> {
        if area.width == 0 || area.height == 0 {
            return vec![String::new()];
        }

        let mut buf = Buffer::empty(area);
        Image::new(&self.protocol).render(area, &mut buf);

        if self.is_halfblocks() {
            render_halfblocks(&buf, area)
        } else {
            render_escape_protocol(&buf, area)
        }
    }
}

impl Component for ImageComponent {
    fn render(&self, width: u16) -> Vec<String> {
        let area = Rect::new(0, 0, self.cell_size.0.min(width), self.cell_size.1);
        self.render_to_lines(area)
    }
}

fn render_halfblocks(buf: &Buffer, area: Rect) -> Vec<String> {
    let mut lines = Vec::with_capacity(area.height as usize);
    for y in 0..area.height {
        let mut line = String::new();
        for x in 0..area.width {
            let cell = &buf[(x, y)];
            let ch = cell.symbol().chars().next().unwrap_or(' ');
            let fg_ansi = color_to_ansi_fg(cell.fg);
            let bg_ansi = color_to_ansi_bg(cell.bg);
            let _ = write!(line, "{}{}{}\x1b[0m", fg_ansi, bg_ansi, ch);
        }
        lines.push(line);
    }
    lines
}

fn render_escape_protocol(buf: &Buffer, area: Rect) -> Vec<String> {
    let escape = buf[(0, 0)].symbol().to_string();
    let pad_width = area.width as usize;

    let mut lines = Vec::with_capacity(area.height as usize);
    if !escape.is_empty() && escape != " " {
        let mut first = escape;
        for _ in first.len()..pad_width {
            first.push(' ');
        }
        lines.push(first);
    } else {
        lines.push(" ".repeat(pad_width));
    }
    for _ in 1..area.height {
        lines.push(String::new());
    }
    lines
}

fn color_to_ansi_fg(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("\x1b[38;2;{};{};{}m", r, g, b),
        Color::Reset | Color::White => "\x1b[39m".to_string(),
        Color::Black => "\x1b[30m".to_string(),
        Color::Red => "\x1b[31m".to_string(),
        Color::Green => "\x1b[32m".to_string(),
        Color::Yellow => "\x1b[33m".to_string(),
        Color::Blue => "\x1b[34m".to_string(),
        Color::Magenta => "\x1b[35m".to_string(),
        Color::Cyan => "\x1b[36m".to_string(),
        Color::DarkGray => "\x1b[90m".to_string(),
        Color::LightRed => "\x1b[91m".to_string(),
        Color::LightGreen => "\x1b[92m".to_string(),
        Color::LightYellow => "\x1b[93m".to_string(),
        Color::LightBlue => "\x1b[94m".to_string(),
        Color::LightMagenta => "\x1b[95m".to_string(),
        Color::LightCyan => "\x1b[96m".to_string(),
        _ => String::new(),
    }
}

fn color_to_ansi_bg(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("\x1b[48;2;{};{};{}m", r, g, b),
        Color::Reset | Color::White => "\x1b[49m".to_string(),
        Color::Black => "\x1b[40m".to_string(),
        Color::Red => "\x1b[41m".to_string(),
        Color::Green => "\x1b[42m".to_string(),
        Color::Yellow => "\x1b[43m".to_string(),
        Color::Blue => "\x1b[44m".to_string(),
        Color::Magenta => "\x1b[45m".to_string(),
        Color::Cyan => "\x1b[46m".to_string(),
        Color::DarkGray => "\x1b[100m".to_string(),
        Color::LightRed => "\x1b[101m".to_string(),
        Color::LightGreen => "\x1b[102m".to_string(),
        Color::LightYellow => "\x1b[103m".to_string(),
        Color::LightBlue => "\x1b[104m".to_string(),
        Color::LightMagenta => "\x1b[105m".to_string(),
        Color::LightCyan => "\x1b[106m".to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui_image::picker::Picker;

    fn test_picker() -> Picker {
        Picker::halfblocks()
    }

    #[test]
    fn test_image_component_creation() {
        let picker = test_picker();
        let img = DynamicImage::new_rgba8(100, 50);
        let comp = ImageComponent::from_picker(&picker, img).unwrap();
        let (cols, rows) = comp.cell_size();
        assert!(cols > 0);
        assert!(rows > 0);
    }

    #[test]
    fn test_image_render_returns_lines() {
        let picker = test_picker();
        let img = DynamicImage::new_rgba8(100, 50);
        let comp = ImageComponent::from_picker(&picker, img).unwrap();
        let lines = comp.render(80);
        assert!(!lines.is_empty());
        assert_eq!(lines.len(), comp.cell_size.1 as usize);
    }

    #[test]
    fn test_image_render_width_clamp() {
        let picker = test_picker();
        let img = DynamicImage::new_rgba8(400, 100);
        let comp = ImageComponent::from_picker(&picker, img).unwrap();
        let lines = comp.render(5);
        assert!(!lines.is_empty());
        for line in &lines {
            assert!(line.len() <= 100);
        }
    }

    #[test]
    fn test_is_halfblocks() {
        let picker = test_picker();
        let img = DynamicImage::new_rgba8(100, 50);
        let comp = ImageComponent::from_picker(&picker, img).unwrap();
        assert!(comp.is_halfblocks());
    }

    #[test]
    fn test_render_halfblocks_content() {
        let picker = test_picker();
        let mut img = DynamicImage::new_rgba8(16, 16);
        for pixel in img.as_mut_rgba8().unwrap().pixels_mut() {
            *pixel = image::Rgba([255u8, 255u8, 255u8, 255u8]);
        }
        let comp = ImageComponent::from_picker(&picker, img).unwrap();
        let lines = comp.render(80);
        assert!(!lines.is_empty());
        for line in &lines {
            assert!(line.contains("\x1b["));
        }
    }

    #[test]
    fn test_color_to_ansi() {
        assert_eq!(
            color_to_ansi_fg(Color::Rgb(10, 20, 30)),
            "\x1b[38;2;10;20;30m"
        );
        assert_eq!(
            color_to_ansi_bg(Color::Rgb(200, 100, 50)),
            "\x1b[48;2;200;100;50m"
        );
        assert_eq!(color_to_ansi_fg(Color::Red), "\x1b[31m");
        assert_eq!(color_to_ansi_bg(Color::Blue), "\x1b[44m");
    }

    #[test]
    fn test_empty_image_renders_empty() {
        let picker = test_picker();
        let img = DynamicImage::new_rgba8(1, 1);
        let comp = ImageComponent::from_picker(&picker, img).unwrap();
        let lines = comp.render(0);
        assert_eq!(lines.len(), 1);
    }
}

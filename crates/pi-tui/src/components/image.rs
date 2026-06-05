use image::DynamicImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{Image, Resize};

use crate::tui::Component;

pub struct ImageComponent {
    protocol: Protocol,
    cell_size: (u16, u16),
}

impl ImageComponent {
    pub fn from_picker(picker: &Picker, img: DynamicImage) -> Result<Self, String> {
        let font_size = picker.font_size();
        let img_w = img.width();
        let img_h = img.height();
        let cell_w = font_size.0 as u32;
        let cell_h = font_size.1 as u32;

        let cols = (img_w + cell_w - 1).div_ceil(cell_w).max(1) as u16;
        let rows = (img_h + cell_h - 1).div_ceil(cell_h).max(1) as u16;

        let rect = Rect::new(0, 0, cols, rows);
        let protocol = picker
            .new_protocol(img, rect, Resize::Fit(None))
            .map_err(|e| format!("Protocol creation failed: {}", e))?;

        Ok(Self {
            protocol,
            cell_size: (cols, rows),
        })
    }

    pub fn from_path(picker: &Picker, path: &str) -> Result<Self, String> {
        let img = image::ImageReader::open(path)
            .map_err(|e| format!("Cannot open image: {}", e))?
            .decode()
            .map_err(|e| format!("Cannot decode image: {}", e))?;
        Self::from_picker(picker, img)
    }

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

    pub fn cell_size(&self) -> (u16, u16) {
        self.cell_size
    }

    fn is_halfblocks(&self) -> bool {
        matches!(self.protocol, Protocol::Halfblocks(_))
    }

    fn render_to_lines(&self, area: Rect) -> Vec<Line<'static>> {
        if area.width == 0 || area.height == 0 {
            return vec![Line::from(vec![])];
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
    fn render(&self, width: u16) -> Vec<Line<'static>> {
        let area = Rect::new(0, 0, self.cell_size.0.min(width), self.cell_size.1);
        self.render_to_lines(area)
    }
}

fn render_halfblocks(buf: &Buffer, area: Rect) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(area.height as usize);
    for y in 0..area.height {
        let mut spans = Vec::with_capacity(area.width as usize);
        for x in 0..area.width {
            let cell = &buf[(x, y)];
            let ch = cell.symbol().chars().next().unwrap_or(' ');
            let style = Style::default().fg(cell.fg).bg(cell.bg);
            spans.push(Span::styled(ch.to_string(), style));
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn render_escape_protocol(buf: &Buffer, area: Rect) -> Vec<Line<'static>> {
    let escape = buf[(0, 0)].symbol().to_string();
    let mut lines = Vec::new();
    if !escape.is_empty() && escape != " " {
        lines.push(Line::from(Span::raw(escape)));
    } else {
        lines.push(Line::from(Span::raw(" ".repeat(area.width as usize))));
    }
    for _ in 1..area.height {
        lines.push(Line::from(vec![]));
    }
    lines
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
            assert!(line.to_string().len() <= 100);
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
            let text = line.to_string();
            assert!(!text.is_empty());
        }
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

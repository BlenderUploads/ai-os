//! Surfaces and drawing primitives.
//!
//! A `Surface` is a plain 32-bit XRGB pixel buffer with a clip rectangle.
//! The screen's back buffer is one; so is every window's contents. All drawing
//! goes through here, which is why windows can be composited without any of
//! the widget code knowing where it will end up.

use alloc::vec;
use alloc::vec::Vec;

use super::font::{self, Weight, GLYPH_HEIGHT, GLYPH_WIDTH};
use super::palette::{self, Color};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const EMPTY: Rect = Rect::new(0, 0, 0, 0);

    #[inline]
    pub const fn right(&self) -> i32 {
        self.x + self.w
    }
    #[inline]
    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    #[inline]
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.right() && py < self.bottom()
    }

    pub fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        Rect::new(x, y, (right - x).max(0), (bottom - y).max(0))
    }

    pub fn inset(&self, by: i32) -> Rect {
        Rect::new(self.x + by, self.y + by, self.w - 2 * by, self.h - 2 * by)
    }

    pub fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
}

pub struct Surface {
    pub pixels: Vec<Color>,
    pub width: usize,
    pub height: usize,
    clip: Rect,
}

impl Surface {
    /// A zero-sized surface, usable in `const` context so statics can hold one
    /// before the framebuffer exists.
    pub const fn empty() -> Self {
        Self {
            pixels: Vec::new(),
            width: 0,
            height: 0,
            clip: Rect::EMPTY,
        }
    }

    pub fn new(width: usize, height: usize) -> Self {
        Self {
            pixels: vec![0; width * height],
            width,
            height,
            clip: Rect::new(0, 0, width as i32, height as i32),
        }
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }

    pub fn clip(&self) -> Rect {
        self.clip
    }

    /// Restrict drawing to `rect` (intersected with the surface). Returns the
    /// previous clip so callers can restore it.
    pub fn set_clip(&mut self, rect: Rect) -> Rect {
        let previous = self.clip;
        self.clip = rect.intersect(&self.bounds());
        previous
    }

    pub fn reset_clip(&mut self) {
        self.clip = self.bounds();
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        if width == self.width && height == self.height {
            return;
        }
        self.pixels = vec![0; width * height];
        self.width = width;
        self.height = height;
        self.reset_clip();
    }

    #[inline]
    pub fn pixel(&mut self, x: i32, y: i32, color: Color) {
        if self.clip.contains(x, y) {
            self.pixels[y as usize * self.width + x as usize] = color;
        }
    }

    #[inline]
    pub fn pixel_blend(&mut self, x: i32, y: i32, color: Color, alpha: u8) {
        if self.clip.contains(x, y) {
            let index = y as usize * self.width + x as usize;
            self.pixels[index] = palette::blend(self.pixels[index], color, alpha);
        }
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> Color {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            self.pixels[y as usize * self.width + x as usize]
        } else {
            0
        }
    }

    pub fn clear(&mut self, color: Color) {
        self.pixels.fill(color);
    }

    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        let area = rect.intersect(&self.clip);
        if area.is_empty() {
            return;
        }
        for y in area.y..area.bottom() {
            let start = y as usize * self.width + area.x as usize;
            self.pixels[start..start + area.w as usize].fill(color);
        }
    }

    pub fn fill_rect_blend(&mut self, rect: Rect, color: Color, alpha: u8) {
        let area = rect.intersect(&self.clip);
        if area.is_empty() {
            return;
        }
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let index = y as usize * self.width + x as usize;
                self.pixels[index] = palette::blend(self.pixels[index], color, alpha);
            }
        }
    }

    /// One-pixel outline just inside `rect`.
    pub fn stroke_rect(&mut self, rect: Rect, color: Color) {
        if rect.is_empty() {
            return;
        }
        self.fill_rect(Rect::new(rect.x, rect.y, rect.w, 1), color);
        self.fill_rect(Rect::new(rect.x, rect.bottom() - 1, rect.w, 1), color);
        self.fill_rect(Rect::new(rect.x, rect.y, 1, rect.h), color);
        self.fill_rect(Rect::new(rect.right() - 1, rect.y, 1, rect.h), color);
    }

    /// Vertical gradient. Cheap and used for the desktop and title bars.
    pub fn gradient_v(&mut self, rect: Rect, top: Color, bottom: Color) {
        let area = rect.intersect(&self.clip);
        if area.is_empty() {
            return;
        }
        for y in area.y..area.bottom() {
            let t = if rect.h > 1 {
                ((y - rect.y) * 255 / (rect.h - 1)).clamp(0, 255) as u8
            } else {
                0
            };
            let color = palette::mix(top, bottom, t);
            let start = y as usize * self.width + area.x as usize;
            self.pixels[start..start + area.w as usize].fill(color);
        }
    }

    /// A rectangle with the corner pixels knocked out — enough to suggest a
    /// rounded panel at this resolution without the cost of real curves.
    pub fn panel(&mut self, rect: Rect, fill: Color, border: Color) {
        if rect.w < 3 || rect.h < 3 {
            self.fill_rect(rect, fill);
            return;
        }
        self.fill_rect(Rect::new(rect.x + 1, rect.y, rect.w - 2, rect.h), fill);
        self.fill_rect(Rect::new(rect.x, rect.y + 1, 1, rect.h - 2), fill);
        self.fill_rect(Rect::new(rect.right() - 1, rect.y + 1, 1, rect.h - 2), fill);

        self.fill_rect(Rect::new(rect.x + 1, rect.y, rect.w - 2, 1), border);
        self.fill_rect(
            Rect::new(rect.x + 1, rect.bottom() - 1, rect.w - 2, 1),
            border,
        );
        self.fill_rect(Rect::new(rect.x, rect.y + 1, 1, rect.h - 2), border);
        self.fill_rect(
            Rect::new(rect.right() - 1, rect.y + 1, 1, rect.h - 2),
            border,
        );
    }

    pub fn hline(&mut self, x: i32, y: i32, len: i32, color: Color) {
        self.fill_rect(Rect::new(x, y, len, 1), color);
    }

    pub fn vline(&mut self, x: i32, y: i32, len: i32, color: Color) {
        self.fill_rect(Rect::new(x, y, 1, len), color);
    }

    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
        // Bresenham.
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut error = dx + dy;
        let (mut x, mut y) = (x0, y0);
        loop {
            self.pixel(x, y, color);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * error;
            if e2 >= dy {
                error += dy;
                x += sx;
            }
            if e2 <= dx {
                error += dx;
                y += sy;
            }
        }
    }

    pub fn disc(&mut self, cx: i32, cy: i32, radius: i32, color: Color) {
        let r2 = radius * radius;
        for y in -radius..=radius {
            for x in -radius..=radius {
                if x * x + y * y <= r2 {
                    self.pixel(cx + x, cy + y, color);
                }
            }
        }
    }

    pub fn circle(&mut self, cx: i32, cy: i32, radius: i32, color: Color) {
        let r2 = radius * radius;
        let inner = (radius - 1) * (radius - 1);
        for y in -radius..=radius {
            for x in -radius..=radius {
                let d = x * x + y * y;
                if d <= r2 && d > inner {
                    self.pixel(cx + x, cy + y, color);
                }
            }
        }
    }

    // --- text ---

    /// Draw one glyph. `scale` multiplies both axes.
    pub fn glyph(&mut self, code: u8, x: i32, y: i32, color: Color, weight: Weight, scale: i32) {
        for row in 0..GLYPH_HEIGHT {
            let bits = font::scanline(code, row, weight);
            if bits == 0 {
                continue;
            }
            for column in 0..GLYPH_WIDTH {
                if bits & (0x80 >> column) == 0 {
                    continue;
                }
                let px = x + column as i32 * scale;
                let py = y + row as i32 * scale;
                if scale == 1 {
                    self.pixel(px, py, color);
                } else {
                    self.fill_rect(Rect::new(px, py, scale, scale), color);
                }
            }
        }
    }

    pub fn text(&mut self, text: &str, x: i32, y: i32, color: Color) -> i32 {
        self.text_ex(text, x, y, color, Weight::Regular, 1)
    }

    pub fn text_bold(&mut self, text: &str, x: i32, y: i32, color: Color) -> i32 {
        self.text_ex(text, x, y, color, Weight::Bold, 1)
    }

    /// Returns the x coordinate just past the last glyph.
    pub fn text_ex(
        &mut self,
        text: &str,
        x: i32,
        y: i32,
        color: Color,
        weight: Weight,
        scale: i32,
    ) -> i32 {
        let mut cursor = x;
        let advance = GLYPH_WIDTH as i32 * scale;
        for ch in text.chars() {
            // Skip glyphs entirely off the clip to keep long lines cheap.
            if cursor + advance > self.clip.x && cursor < self.clip.right() {
                self.glyph(font::code_of(ch), cursor, y, color, weight, scale);
            }
            cursor += advance;
        }
        cursor
    }

    /// Text with a soft halo underneath, the way phosphor bleeds.
    pub fn text_glow(&mut self, text: &str, x: i32, y: i32, color: Color, glow: Color) {
        for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
            let mut cursor = x + dx;
            for ch in text.chars() {
                self.glyph_blend(font::code_of(ch), cursor, y + dy, glow, 90);
                cursor += GLYPH_WIDTH as i32;
            }
        }
        self.text(text, x, y, color);
    }

    fn glyph_blend(&mut self, code: u8, x: i32, y: i32, color: Color, alpha: u8) {
        for row in 0..GLYPH_HEIGHT {
            let bits = font::scanline(code, row, Weight::Regular);
            if bits == 0 {
                continue;
            }
            for column in 0..GLYPH_WIDTH {
                if bits & (0x80 >> column) != 0 {
                    self.pixel_blend(x + column as i32, y + row as i32, color, alpha);
                }
            }
        }
    }

    pub fn text_centered(&mut self, text: &str, cx: i32, y: i32, color: Color, weight: Weight) {
        let width = font::text_width(text, 1) as i32;
        self.text_ex(text, cx - width / 2, y, color, weight, 1);
    }

    // --- composition ---

    /// Copy `source` onto this surface at (x, y).
    pub fn blit(&mut self, source: &Surface, x: i32, y: i32) {
        let target = Rect::new(x, y, source.width as i32, source.height as i32);
        let area = target.intersect(&self.clip);
        if area.is_empty() {
            return;
        }
        for row in area.y..area.bottom() {
            let src_y = (row - y) as usize;
            let src_x = (area.x - x) as usize;
            let src_start = src_y * source.width + src_x;
            let dst_start = row as usize * self.width + area.x as usize;
            let len = area.w as usize;
            self.pixels[dst_start..dst_start + len]
                .copy_from_slice(&source.pixels[src_start..src_start + len]);
        }
    }

    /// Copy `source`, treating `key` as transparent. Used for the cursor.
    pub fn blit_keyed(&mut self, source: &Surface, x: i32, y: i32, key: Color) {
        let target = Rect::new(x, y, source.width as i32, source.height as i32);
        let area = target.intersect(&self.clip);
        if area.is_empty() {
            return;
        }
        for row in area.y..area.bottom() {
            for column in area.x..area.right() {
                let pixel =
                    source.pixels[(row - y) as usize * source.width + (column - x) as usize];
                if pixel != key {
                    self.pixels[row as usize * self.width + column as usize] = pixel;
                }
            }
        }
    }

    /// Darken a region — used for window shadows.
    pub fn shade(&mut self, rect: Rect, factor: u8) {
        let area = rect.intersect(&self.clip);
        if area.is_empty() {
            return;
        }
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let index = y as usize * self.width + x as usize;
                self.pixels[index] = palette::scale(self.pixels[index], factor);
            }
        }
    }

    /// Scroll the contents of `rect` up by `lines` pixels, filling the gap.
    pub fn scroll_up(&mut self, rect: Rect, lines: i32, fill: Color) {
        let area = rect.intersect(&self.bounds());
        if area.is_empty() || lines <= 0 {
            return;
        }
        if lines >= area.h {
            self.fill_rect(area, fill);
            return;
        }
        for y in area.y..area.bottom() - lines {
            let src = (y + lines) as usize * self.width + area.x as usize;
            let dst = y as usize * self.width + area.x as usize;
            let len = area.w as usize;
            self.pixels.copy_within(src..src + len, dst);
        }
        self.fill_rect(
            Rect::new(area.x, area.bottom() - lines, area.w, lines),
            fill,
        );
    }
}

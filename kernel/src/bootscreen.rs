//! The boot screen.
//!
//! Mirrors the serial boot log onto the framebuffer with HALCYON's wordmark
//! above it, one line per subsystem as it comes up.

use alloc::string::String;
use alloc::vec::Vec;

use crate::gfx::crt;
use crate::gfx::draw::{Rect, Surface};
use crate::gfx::fb;
use crate::gfx::font::{glyph, Weight, GLYPH_HEIGHT};
use crate::gfx::palette;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Info,
    Warn,
    Fail,
}

impl Status {
    fn color(self) -> palette::Color {
        match self {
            Status::Ok => palette::OK,
            Status::Info => palette::CYAN,
            Status::Warn => palette::WARN,
            Status::Fail => palette::ERROR,
        }
    }

    fn mark(self) -> u8 {
        match self {
            Status::Ok => glyph::BULLET,
            Status::Info => glyph::ARROW_RIGHT,
            Status::Warn => b'!',
            Status::Fail => b'x',
        }
    }
}

pub struct BootScreen {
    lines: Vec<(Status, String)>,
    max_lines: usize,
}

impl BootScreen {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            max_lines: 18,
        }
    }

    /// Add a line and redraw. Also echoes to the serial log.
    pub fn step(&mut self, status: Status, text: String) {
        crate::serial_println!(
            "[boot] {} {}",
            match status {
                Status::Ok => "ok  ",
                Status::Info => "..  ",
                Status::Warn => "warn",
                Status::Fail => "FAIL",
            },
            text
        );
        self.lines.push((status, text));
        while self.lines.len() > self.max_lines {
            self.lines.remove(0);
        }
        self.render();
    }

    pub fn render(&self) {
        if !fb::is_ready() {
            return;
        }
        fb::with_screen(|surface| self.draw(surface));
    }

    fn draw(&self, surface: &mut Surface) {
        surface.reset_clip();
        crt::draw_backdrop(surface);

        let width = surface.width as i32;
        let height = surface.height as i32;
        let cx = width / 2;

        crt::draw_wordmark(surface, cx, 56, 4);
        surface.text_centered(
            "A FROM-SCRATCH OPERATING SYSTEM",
            cx,
            120,
            palette::CYAN_DIM,
            Weight::Regular,
        );

        // Console panel, sized to the lines actually present so it grows with
        // the boot rather than sitting mostly empty.
        let rows = self.lines.len().max(3) as i32;
        let panel_height = (rows * (GLYPH_HEIGHT as i32 + 2) + 24).min(height - 240);
        let panel = Rect::new(cx - 340, 160, 680, panel_height);
        surface.panel(panel, palette::PANEL, palette::BORDER);
        surface.fill_rect(
            Rect::new(panel.x + 1, panel.y + 1, panel.w - 2, 1),
            palette::BORDER_BRIGHT,
        );

        let mut y = panel.y + 12;
        let x = panel.x + 14;
        for (status, text) in &self.lines {
            if y + GLYPH_HEIGHT as i32 > panel.bottom() - 10 {
                break;
            }
            surface.glyph(status.mark(), x, y, status.color(), Weight::Regular, 1);
            surface.text(text, x + 16, y, palette::TEXT);
            y += GLYPH_HEIGHT as i32 + 2;
        }

        crt::draw_palette_strip(surface, Rect::new(cx - 160, height - 78, 320, 6));
        surface.text_centered(
            "everything on this screen was drawn by code in this repository",
            cx,
            height - 56,
            palette::TEXT_FAINT,
            Weight::Regular,
        );

        crt::apply_scanlines(surface);
        crt::apply_vignette(surface);
    }
}

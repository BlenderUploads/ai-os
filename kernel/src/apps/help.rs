//! The manual. Reads the pages shipped in the initrd.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::fs;
use crate::gfx::draw::{Rect, Surface};
use crate::gfx::font::Weight;
use crate::gfx::palette::{self, Color};
use crate::input::{Key, KeyEvent};
use crate::ui::theme::CELL_H;
use crate::ui::window::{App, AppResponse, WindowMouse};

const SIDEBAR: i32 = 150;

pub struct Help {
    pages: Vec<(String, String)>,
    selected: usize,
    scroll: usize,
    dirty: bool,
}

impl Help {
    pub fn new() -> Self {
        let mut pages = Vec::new();
        let filesystem = fs::FS.lock();
        for (path, _, _) in filesystem.list_dir("/help") {
            if let Some(file) = filesystem.read(&path) {
                if let Ok(text) = core::str::from_utf8(file.bytes()) {
                    let title = path
                        .rsplit('/')
                        .next()
                        .unwrap_or(&path)
                        .trim_end_matches(".txt")
                        .to_string();
                    pages.push((title, text.to_string()));
                }
            }
        }
        drop(filesystem);

        if pages.is_empty() {
            pages.push((
                "missing".to_string(),
                "The manual pages were not found in the initrd.\n\n\
                 They should be at /help/*.txt."
                    .to_string(),
            ));
        }

        Self {
            pages,
            selected: 0,
            scroll: 0,
            dirty: true,
        }
    }

    fn body(&self) -> Vec<&str> {
        self.pages
            .get(self.selected)
            .map(|(_, text)| text.lines().collect())
            .unwrap_or_default()
    }
}

/// Headings are underlined with `=`, which reads fine as plain text and gives
/// the renderer something to colour.
fn line_style(line: &str, next: Option<&&str>) -> (Color, Weight) {
    if next.map(|n| n.starts_with("===")).unwrap_or(false) {
        (palette::AMBER, Weight::Bold)
    } else if line.starts_with("===") {
        (palette::AMBER_DIM, Weight::Regular)
    } else if line.starts_with("  ") {
        (palette::CYAN_DIM, Weight::Regular)
    } else {
        (palette::TEXT, Weight::Regular)
    }
}

impl App for Help {
    fn draw(&mut self, surface: &mut Surface, _focused: bool) {
        surface.clear(palette::PANEL);
        let width = surface.width as i32;
        let height = surface.height as i32;

        surface.fill_rect(
            Rect::new(0, 0, SIDEBAR, height),
            palette::rgb(0x0A, 0x0E, 0x1A),
        );
        surface.vline(SIDEBAR, 0, height, palette::BORDER);
        surface.text_ex("MANUAL", 10, 8, palette::CYAN, Weight::Bold, 1);
        surface.hline(0, 28, SIDEBAR, palette::BORDER);

        let mut y = 34;
        for (index, (title, _)) in self.pages.iter().enumerate() {
            let active = index == self.selected;
            if active {
                surface.fill_rect(
                    Rect::new(0, y - 2, SIDEBAR, CELL_H + 2),
                    palette::rgb(0x1E, 0x26, 0x3E),
                );
                surface.fill_rect(Rect::new(0, y - 2, 2, CELL_H + 2), palette::AMBER);
            }
            surface.text(
                title,
                10,
                y,
                if active {
                    palette::AMBER
                } else {
                    palette::TEXT_DIM
                },
            );
            y += CELL_H;
        }

        let body = self.body();
        let visible = ((height - 16) / CELL_H).max(1) as usize;
        let previous = surface.set_clip(Rect::new(SIDEBAR + 1, 0, width - SIDEBAR - 1, height));
        let mut y = 8;
        for (offset, line) in body.iter().skip(self.scroll).take(visible).enumerate() {
            let next = body.get(self.scroll + offset + 1);
            let (colour, weight) = line_style(line, next);
            surface.text_ex(line, SIDEBAR + 12, y, colour, weight, 1);
            y += CELL_H;
        }
        surface.set_clip(previous);

        if body.len() > visible {
            surface.text(
                &format!("line {} of {}", self.scroll + 1, body.len()),
                SIDEBAR + 12,
                height - CELL_H - 2,
                palette::TEXT_FAINT,
            );
        }

        self.dirty = false;
    }

    fn on_key(&mut self, event: &KeyEvent, _response: &mut AppResponse) {
        if !event.pressed {
            return;
        }
        self.dirty = true;
        let length = self.body().len();
        match event.key {
            Key::Down => self.scroll = (self.scroll + 1).min(length.saturating_sub(1)),
            Key::Up => self.scroll = self.scroll.saturating_sub(1),
            Key::PageDown => self.scroll = (self.scroll + 12).min(length.saturating_sub(1)),
            Key::PageUp => self.scroll = self.scroll.saturating_sub(12),
            Key::Home => self.scroll = 0,
            Key::End => self.scroll = length.saturating_sub(1),
            Key::Tab => {
                self.selected = (self.selected + 1) % self.pages.len();
                self.scroll = 0;
            }
            _ => {}
        }
    }

    fn on_mouse(&mut self, event: &WindowMouse, _response: &mut AppResponse) {
        if !event.pressed || event.x >= SIDEBAR || event.y < 34 {
            return;
        }
        let index = ((event.y - 34) / CELL_H) as usize;
        if index < self.pages.len() {
            self.selected = index;
            self.scroll = 0;
            self.dirty = true;
        }
    }

    fn dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn min_size(&self) -> (i32, i32) {
        (520, 300)
    }
}

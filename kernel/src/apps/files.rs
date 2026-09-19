//! File browser with a preview pane.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::fs;
use crate::gfx::draw::{Rect, Surface};
use crate::gfx::font::{glyph, Weight};
use crate::gfx::palette;
use crate::input::{Key, KeyEvent};
use crate::ui::theme::{CELL_H, CELL_W};
use crate::ui::window::{App, AppResponse, WindowMouse};

pub struct Files {
    entries: Vec<(String, usize, bool)>,
    selected: usize,
    preview: Vec<String>,
    dirty: bool,
    last_refresh: u64,
}

impl Files {
    pub fn new() -> Self {
        let mut browser = Self {
            entries: Vec::new(),
            selected: 0,
            preview: Vec::new(),
            dirty: true,
            last_refresh: 0,
        };
        browser.refresh();
        browser
    }

    fn refresh(&mut self) {
        let filesystem = fs::FS.lock();
        let entries = filesystem.list();
        drop(filesystem);
        // Only disturb the selection if the listing actually changed.
        if entries != self.entries {
            self.entries = entries;
            self.selected = self.selected.min(self.entries.len().saturating_sub(1));
            self.load_preview();
            self.dirty = true;
        }
    }

    fn load_preview(&mut self) {
        self.preview.clear();
        let Some((path, _, _)) = self.entries.get(self.selected) else {
            return;
        };
        let filesystem = fs::FS.lock();
        let Some(file) = filesystem.read(path) else {
            return;
        };
        match core::str::from_utf8(&file.data) {
            Ok(text) => {
                for line in text.lines().take(200) {
                    self.preview.push(line.to_string());
                }
            }
            Err(_) => {
                // Hex dump the first few rows of a binary file.
                for chunk in file.data.chunks(16).take(24) {
                    let mut row = String::new();
                    for byte in chunk {
                        row.push_str(&format!("{:02x} ", byte));
                    }
                    self.preview.push(row);
                }
            }
        }
    }

    fn select(&mut self, index: usize) {
        if index < self.entries.len() {
            self.selected = index;
            self.load_preview();
            self.dirty = true;
        }
    }
}

impl App for Files {
    fn draw(&mut self, surface: &mut Surface, _focused: bool) {
        surface.clear(palette::PANEL);
        let width = surface.width as i32;
        let height = surface.height as i32;
        let split = (width * 2 / 5).clamp(140, 320);

        // Listing.
        surface.fill_rect(
            Rect::new(0, 0, split, height),
            palette::rgb(0x0A, 0x0E, 0x1A),
        );
        surface.vline(split, 0, height, palette::BORDER);

        surface.text_ex("FILES", 8, 6, palette::CYAN, Weight::Bold, 1);
        surface.hline(0, 24, split, palette::BORDER);

        let mut y = 30;
        for (index, (path, size, from_initrd)) in self.entries.iter().enumerate() {
            if y > height - CELL_H {
                break;
            }
            if index == self.selected {
                surface.fill_rect(
                    Rect::new(0, y - 2, split, CELL_H + 2),
                    palette::rgb(0x1E, 0x26, 0x3E),
                );
                surface.fill_rect(Rect::new(0, y - 2, 2, CELL_H + 2), palette::AMBER);
            }
            let color = if index == self.selected {
                palette::AMBER
            } else if *from_initrd {
                palette::TEXT_DIM
            } else {
                palette::TEXT
            };
            surface.glyph(
                if *from_initrd {
                    glyph::CHIP
                } else {
                    glyph::BULLET
                },
                6,
                y,
                color,
                Weight::Regular,
                1,
            );
            // Trim from the left so the filename stays visible.
            let available = ((split - 24 - 6 * CELL_W) / CELL_W).max(4) as usize;
            let name: String = if path.chars().count() > available {
                path.chars().skip(path.chars().count() - available).collect()
            } else {
                path.clone()
            };
            surface.text(&name, 22, y, color);
            surface.text(
                &format!("{:>5}", size),
                split - 6 * CELL_W,
                y,
                palette::TEXT_FAINT,
            );
            y += CELL_H;
        }

        // Preview.
        let header = self
            .entries
            .get(self.selected)
            .map(|(path, size, initrd)| {
                format!(
                    "{}  ({} bytes{})",
                    path,
                    size,
                    if *initrd { ", from initrd" } else { "" }
                )
            })
            .unwrap_or_else(|| "no files".to_string());
        surface.text_ex(&header, split + 8, 6, palette::AMBER, Weight::Bold, 1);
        surface.hline(split, 24, width - split, palette::BORDER);

        let previous = surface.set_clip(Rect::new(split + 1, 26, width - split - 1, height - 26));
        let mut y = 30;
        for line in &self.preview {
            if y > height - CELL_H {
                break;
            }
            surface.text(line, split + 8, y, palette::TEXT);
            y += CELL_H;
        }
        surface.set_clip(previous);

        surface.text(
            "enter: open in editor   up/down: select",
            split + 8,
            height - CELL_H - 4,
            palette::TEXT_FAINT,
        );

        self.dirty = false;
    }

    fn on_key(&mut self, event: &KeyEvent, response: &mut AppResponse) {
        if !event.pressed {
            return;
        }
        match event.key {
            Key::Up => {
                if self.selected > 0 {
                    self.select(self.selected - 1);
                }
            }
            Key::Down => self.select(self.selected + 1),
            Key::Home => self.select(0),
            Key::End => self.select(self.entries.len().saturating_sub(1)),
            Key::Enter => {
                if let Some((path, _, _)) = self.entries.get(self.selected) {
                    // The editor opens whatever EDITOR_TARGET names.
                    super::set_editor_target(path);
                    response.launch = Some("editor".to_string());
                }
            }
            _ => {}
        }
    }

    fn on_mouse(&mut self, event: &WindowMouse, _response: &mut AppResponse) {
        if !event.pressed {
            return;
        }
        if event.y >= 30 {
            let index = ((event.y - 30) / CELL_H) as usize;
            self.select(index);
        }
    }

    fn tick(&mut self, now_ms: u64, _response: &mut AppResponse) {
        // Pick up files created elsewhere, but not on every frame.
        if now_ms.saturating_sub(self.last_refresh) > 700 {
            self.last_refresh = now_ms;
            self.refresh();
        }
    }

    fn dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn min_size(&self) -> (i32, i32) {
        (460, 260)
    }
}

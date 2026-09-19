//! A small text editor over the RAM filesystem.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::fs;
use crate::gfx::draw::{Rect, Surface};
use crate::gfx::font::Weight;
use crate::gfx::palette;
use crate::input::{Key, KeyEvent};
use crate::ui::theme::{CELL_H, CELL_W};
use crate::ui::window::{App, AppResponse, WindowMouse};

const GUTTER: i32 = 5; // digits reserved for line numbers

pub struct Editor {
    path: String,
    lines: Vec<String>,
    row: usize,
    column: usize,
    scroll: usize,
    modified: bool,
    message: String,
    dirty: bool,
    blink: bool,
    last_blink: u64,
}

impl Editor {
    pub fn new() -> Self {
        let mut editor = Self {
            path: String::from("/home/notes.txt"),
            lines: alloc::vec![String::new()],
            row: 0,
            column: 0,
            scroll: 0,
            modified: false,
            message: String::new(),
            dirty: true,
            blink: true,
            last_blink: 0,
        };
        editor.load(&editor.path.clone());
        editor
    }

    pub fn open(path: &str) -> Self {
        let mut editor = Self::new();
        editor.load(path);
        editor
    }

    fn load(&mut self, path: &str) {
        let filesystem = fs::FS.lock();
        match filesystem.read(path) {
            Some(file) => {
                let text = core::str::from_utf8(&file.data).unwrap_or("<binary file>");
                self.lines = text.lines().map(|line| line.to_string()).collect();
                if self.lines.is_empty() {
                    self.lines.push(String::new());
                }
                self.message = format!("opened {} ({} lines)", path, self.lines.len());
            }
            None => {
                self.lines = alloc::vec![String::new()];
                self.message = format!("{} is new", path);
            }
        }
        self.path = path.to_string();
        self.row = 0;
        self.column = 0;
        self.scroll = 0;
        self.modified = false;
        self.dirty = true;
    }

    fn save(&mut self) {
        let mut data = Vec::new();
        for line in &self.lines {
            data.extend_from_slice(line.as_bytes());
            data.push(b'\n');
        }
        let length = data.len();
        let mut filesystem = fs::FS.lock();
        match filesystem.write(&self.path, data) {
            Ok(()) => {
                self.modified = false;
                self.message = format!("saved {} ({} bytes)", self.path, length);
            }
            Err(error) => self.message = format!("save failed: {}", error),
        }
    }

    fn current_len(&self) -> usize {
        self.lines[self.row].chars().count()
    }

    fn byte_offset(&self, row: usize, column: usize) -> usize {
        self.lines[row]
            .char_indices()
            .nth(column)
            .map(|(offset, _)| offset)
            .unwrap_or(self.lines[row].len())
    }

    fn visible_rows(&self, surface: &Surface) -> usize {
        (((surface.height as i32 - 24) / CELL_H).max(1)) as usize
    }

    fn scroll_to_cursor(&mut self, rows: usize) {
        if self.row < self.scroll {
            self.scroll = self.row;
        } else if self.row >= self.scroll + rows {
            self.scroll = self.row - rows + 1;
        }
    }
}

impl App for Editor {
    fn draw(&mut self, surface: &mut Surface, focused: bool) {
        surface.clear(palette::rgb(0x07, 0x0A, 0x14));
        let width = surface.width as i32;
        let rows = self.visible_rows(surface);

        // Gutter.
        surface.fill_rect(
            Rect::new(0, 0, GUTTER * CELL_W + 4, surface.height as i32),
            palette::rgb(0x0B, 0x10, 0x1C),
        );

        let mut y = 4;
        for index in self.scroll..(self.scroll + rows).min(self.lines.len()) {
            let number = format!("{:>4} ", index + 1);
            surface.text(
                &number,
                2,
                y,
                if index == self.row {
                    palette::AMBER_DIM
                } else {
                    palette::TEXT_FAINT
                },
            );
            if index == self.row {
                surface.fill_rect_blend(
                    Rect::new(GUTTER * CELL_W + 4, y - 1, width, CELL_H),
                    palette::AMBER,
                    14,
                );
            }
            surface.text(
                &self.lines[index],
                GUTTER * CELL_W + 6,
                y,
                palette::TEXT_BRIGHT,
            );
            y += CELL_H;
        }

        // Caret.
        if focused && self.blink && self.row >= self.scroll && self.row < self.scroll + rows {
            let caret_x = GUTTER * CELL_W + 6 + self.column as i32 * CELL_W;
            let caret_y = 4 + (self.row - self.scroll) as i32 * CELL_H;
            surface.fill_rect(
                Rect::new(caret_x, caret_y, CELL_W, CELL_H - 1),
                palette::AMBER,
            );
            if let Some(ch) = self.lines[self.row].chars().nth(self.column) {
                let mut buffer = [0u8; 4];
                surface.text(
                    ch.encode_utf8(&mut buffer),
                    caret_x,
                    caret_y,
                    palette::rgb(0x07, 0x0A, 0x14),
                );
            }
        }

        // Status bar.
        let bar = Rect::new(0, surface.height as i32 - 20, width, 20);
        surface.fill_rect(bar, palette::rgb(0x12, 0x18, 0x28));
        surface.hline(0, bar.y, width, palette::BORDER);
        let status = format!(
            "{}{}  {}:{}",
            self.path,
            if self.modified { " *" } else { "" },
            self.row + 1,
            self.column + 1
        );
        surface.text_ex(&status, 6, bar.y + 3, palette::AMBER, Weight::Bold, 1);
        let hint = "ctrl+s save   ctrl+o reload";
        surface.text(
            hint,
            width - (hint.len() as i32 + 1) * CELL_W,
            bar.y + 3,
            palette::TEXT_FAINT,
        );
        if !self.message.is_empty() {
            surface.text(&self.message, 6, bar.y - CELL_H - 2, palette::CYAN_DIM);
        }

        self.dirty = false;
    }

    fn on_key(&mut self, event: &KeyEvent, _response: &mut AppResponse) {
        if !event.pressed {
            return;
        }
        self.dirty = true;
        self.blink = true;
        self.last_blink = crate::arch::pit::ticks();

        match event.key {
            Key::Char => {
                let Some(ch) = event.character else { return };
                if event.modifiers.ctrl {
                    match ch {
                        's' => self.save(),
                        'o' => {
                            let path = self.path.clone();
                            self.load(&path);
                        }
                        _ => {}
                    }
                    return;
                }
                let offset = self.byte_offset(self.row, self.column);
                self.lines[self.row].insert(offset, ch);
                self.column += 1;
                self.modified = true;
            }
            Key::Enter => {
                let offset = self.byte_offset(self.row, self.column);
                let tail = self.lines[self.row].split_off(offset);
                self.lines.insert(self.row + 1, tail);
                self.row += 1;
                self.column = 0;
                self.modified = true;
            }
            Key::Backspace => {
                if self.column > 0 {
                    self.column -= 1;
                    let offset = self.byte_offset(self.row, self.column);
                    self.lines[self.row].remove(offset);
                    self.modified = true;
                } else if self.row > 0 {
                    // Join with the previous line.
                    let current = self.lines.remove(self.row);
                    self.row -= 1;
                    self.column = self.current_len();
                    self.lines[self.row].push_str(&current);
                    self.modified = true;
                }
            }
            Key::Delete => {
                let offset = self.byte_offset(self.row, self.column);
                if offset < self.lines[self.row].len() {
                    self.lines[self.row].remove(offset);
                    self.modified = true;
                } else if self.row + 1 < self.lines.len() {
                    let next = self.lines.remove(self.row + 1);
                    self.lines[self.row].push_str(&next);
                    self.modified = true;
                }
            }
            Key::Left => {
                if self.column > 0 {
                    self.column -= 1;
                } else if self.row > 0 {
                    self.row -= 1;
                    self.column = self.current_len();
                }
            }
            Key::Right => {
                if self.column < self.current_len() {
                    self.column += 1;
                } else if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.column = 0;
                }
            }
            Key::Up => {
                if self.row > 0 {
                    self.row -= 1;
                    self.column = self.column.min(self.current_len());
                }
            }
            Key::Down => {
                if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.column = self.column.min(self.current_len());
                }
            }
            Key::Home => self.column = 0,
            Key::End => self.column = self.current_len(),
            Key::Tab => {
                let offset = self.byte_offset(self.row, self.column);
                self.lines[self.row].insert_str(offset, "    ");
                self.column += 4;
                self.modified = true;
            }
            Key::PageUp => {
                self.row = self.row.saturating_sub(10);
                self.column = self.column.min(self.current_len());
            }
            Key::PageDown => {
                self.row = (self.row + 10).min(self.lines.len() - 1);
                self.column = self.column.min(self.current_len());
            }
            _ => {}
        }

        // 24 rows is a safe lower bound for the visible height here; the exact
        // figure is recomputed on the next draw.
        self.scroll_to_cursor(20);
    }

    fn on_mouse(&mut self, event: &WindowMouse, _response: &mut AppResponse) {
        if !event.pressed {
            return;
        }
        let row = ((event.y - 4) / CELL_H).max(0) as usize + self.scroll;
        if row < self.lines.len() {
            self.row = row;
            let column = ((event.x - GUTTER * CELL_W - 6) / CELL_W).max(0) as usize;
            self.column = column.min(self.current_len());
            self.dirty = true;
        }
    }

    fn tick(&mut self, _now_ms: u64, _response: &mut AppResponse) {
        let ticks = crate::arch::pit::ticks();
        if ticks.saturating_sub(self.last_blink) > 500 {
            self.last_blink = ticks;
            self.blink = !self.blink;
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
        (420, 240)
    }
}

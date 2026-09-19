//! The terminal window: scrollback, line editing, and hsh underneath.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::arch::pit;
use crate::gfx::draw::{Rect, Surface};
use crate::gfx::font::Weight;
use crate::gfx::palette;
use crate::input::{Key, KeyEvent};
use crate::ui::theme::{CELL_H, CELL_W};
use crate::ui::window::{App, AppResponse, WindowMouse};

use super::shell::{Line, Shell, ShellEffects};

const MAX_SCROLLBACK: usize = 600;
const PROMPT: &str = "halcyon> ";

pub struct Terminal {
    shell: Shell,
    lines: Vec<Line>,
    input: String,
    /// Caret position within `input`, counted in characters.
    caret: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    scroll: usize,
    dirty: bool,
    blink_on: bool,
    last_blink: u64,
}

impl Terminal {
    pub fn new() -> Self {
        let shell = Shell::new();
        let lines = shell.banner();
        Self {
            shell,
            lines,
            input: String::new(),
            caret: 0,
            history: Vec::new(),
            history_index: None,
            scroll: 0,
            dirty: true,
            blink_on: true,
            last_blink: 0,
        }
    }

    fn push(&mut self, line: Line) {
        self.lines.push(line);
        while self.lines.len() > MAX_SCROLLBACK {
            self.lines.remove(0);
        }
    }

    fn submit(&mut self, response: &mut AppResponse) {
        let input = core::mem::take(&mut self.input);
        self.caret = 0;
        self.history_index = None;
        self.scroll = 0;

        self.push(Line::new(
            alloc::format!("{}{}", PROMPT, input),
            palette::AMBER,
        ));

        if !input.trim().is_empty() {
            // Avoid stacking identical consecutive entries in the history.
            if self
                .history
                .last()
                .map(|last| last != &input)
                .unwrap_or(true)
            {
                self.history.push(input.clone());
            }
        }

        let mut effects = ShellEffects::default();
        let produced = self.shell.execute(&input, &mut effects);
        for line in produced {
            self.push(line);
        }

        if effects.clear {
            self.lines.clear();
        }
        if let Some(name) = effects.launch {
            response.launch = Some(name);
        }
        if effects.close {
            response.close = true;
        }
    }

    fn recall(&mut self, backwards: bool) {
        if self.history.is_empty() {
            return;
        }
        let index = match (self.history_index, backwards) {
            (None, true) => self.history.len() - 1,
            (None, false) => return,
            (Some(current), true) => current.saturating_sub(1),
            (Some(current), false) => {
                if current + 1 >= self.history.len() {
                    self.history_index = None;
                    self.input.clear();
                    self.caret = 0;
                    return;
                }
                current + 1
            }
        };
        self.history_index = Some(index);
        self.input = self.history[index].clone();
        self.caret = self.input.chars().count();
    }

    /// Byte offset of the caret, which is tracked in characters.
    fn caret_byte(&self) -> usize {
        self.input
            .char_indices()
            .nth(self.caret)
            .map(|(offset, _)| offset)
            .unwrap_or(self.input.len())
    }

    fn visible_rows(&self, surface: &Surface) -> usize {
        ((surface.height as i32 - 8) / CELL_H).max(1) as usize - 1
    }
}

impl App for Terminal {
    fn draw(&mut self, surface: &mut Surface, focused: bool) {
        surface.clear(palette::rgb(0x06, 0x09, 0x12));

        let rows = self.visible_rows(surface);
        let total = self.lines.len();
        // `scroll` counts rows back from the bottom.
        let end = total.saturating_sub(self.scroll);
        let start = end.saturating_sub(rows);

        let mut y = 4;
        for line in &self.lines[start..end] {
            surface.text(&line.text, 6, y, line.color);
            y += CELL_H;
        }

        // Prompt line, pinned to the bottom.
        let prompt_y = surface.height as i32 - CELL_H - 4;
        surface.fill_rect(
            Rect::new(0, prompt_y - 3, surface.width as i32, CELL_H + 6),
            palette::rgb(0x0B, 0x10, 0x1E),
        );
        surface.hline(0, prompt_y - 3, surface.width as i32, palette::BORDER);

        let mut x = surface.text_ex(PROMPT, 6, prompt_y, palette::AMBER, Weight::Bold, 1);
        x = surface.text(&self.input, x, prompt_y, palette::TEXT_BRIGHT);
        let _ = x;

        if focused && self.blink_on {
            let caret_x = 6 + (PROMPT.chars().count() + self.caret) as i32 * CELL_W;
            surface.fill_rect(
                Rect::new(caret_x, prompt_y, CELL_W, CELL_H - 1),
                palette::AMBER,
            );
            // Draw the character under the caret in reverse video.
            if let Some(ch) = self.input.chars().nth(self.caret) {
                let mut buffer = [0u8; 4];
                surface.text(
                    ch.encode_utf8(&mut buffer),
                    caret_x,
                    prompt_y,
                    palette::rgb(0x06, 0x09, 0x12),
                );
            }
        }

        // Scroll indicator.
        if self.scroll > 0 {
            let label = alloc::format!("-- {} lines below --", self.scroll);
            surface.text_ex(
                &label,
                surface.width as i32 - (label.len() as i32 + 1) * CELL_W,
                prompt_y - CELL_H - 6,
                palette::WARN,
                Weight::Regular,
                1,
            );
        }

        self.dirty = false;
    }

    fn on_key(&mut self, event: &KeyEvent, response: &mut AppResponse) {
        if !event.pressed {
            return;
        }
        self.dirty = true;
        self.blink_on = true;
        self.last_blink = pit::ticks();

        match event.key {
            Key::Enter => self.submit(response),
            Key::Backspace => {
                if self.caret > 0 {
                    self.caret -= 1;
                    let offset = self.caret_byte();
                    self.input.remove(offset);
                }
            }
            Key::Delete => {
                let offset = self.caret_byte();
                if offset < self.input.len() {
                    self.input.remove(offset);
                }
            }
            Key::Left => self.caret = self.caret.saturating_sub(1),
            Key::Right => self.caret = (self.caret + 1).min(self.input.chars().count()),
            Key::Home => self.caret = 0,
            Key::End => self.caret = self.input.chars().count(),
            Key::Up => self.recall(true),
            Key::Down => self.recall(false),
            Key::PageUp => {
                self.scroll = (self.scroll + 8).min(self.lines.len());
            }
            Key::PageDown => self.scroll = self.scroll.saturating_sub(8),
            Key::Escape => {
                self.input.clear();
                self.caret = 0;
            }
            Key::Tab => {
                // Complete against the names ORACLE knows.
                let prefix: String = self.input.chars().take(self.caret).collect();
                let word = prefix
                    .rsplit(|c: char| c.is_whitespace() || c == '(')
                    .next()
                    .unwrap_or("");
                if !word.is_empty() {
                    let names = self.shell.interpreter.global.names();
                    let matches: Vec<&String> =
                        names.iter().filter(|name| name.starts_with(word)).collect();
                    if matches.len() == 1 {
                        let completion = &matches[0][word.len()..];
                        let offset = self.caret_byte();
                        self.input.insert_str(offset, completion);
                        self.caret += completion.chars().count();
                    } else if matches.len() > 1 {
                        let joined: Vec<String> = matches
                            .iter()
                            .take(24)
                            .map(|name| name.to_string())
                            .collect();
                        self.push(Line::new(joined.join("  "), palette::TEXT_DIM));
                    }
                }
            }
            Key::Char => {
                if let Some(ch) = event.character {
                    if event.modifiers.ctrl {
                        match ch {
                            'l' => self.lines.clear(),
                            'c' => {
                                self.input.clear();
                                self.caret = 0;
                                self.push(Line::new("^C", palette::TEXT_DIM));
                            }
                            'a' => self.caret = 0,
                            'e' => self.caret = self.input.chars().count(),
                            'u' => {
                                self.input.clear();
                                self.caret = 0;
                            }
                            _ => {}
                        }
                        return;
                    }
                    let offset = self.caret_byte();
                    self.input.insert(offset, ch);
                    self.caret += 1;
                }
            }
            _ => {}
        }
    }

    fn on_mouse(&mut self, event: &WindowMouse, _response: &mut AppResponse) {
        let _ = event;
    }

    fn tick(&mut self, _now_ms: u64, _response: &mut AppResponse) {
        let ticks = pit::ticks();
        if ticks.saturating_sub(self.last_blink) > 500 {
            self.last_blink = ticks;
            self.blink_on = !self.blink_on;
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
        (420, 220)
    }
}

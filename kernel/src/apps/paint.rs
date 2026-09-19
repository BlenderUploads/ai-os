//! A drawing pad — mostly here to prove the mouse works properly.

use crate::gfx::draw::{Rect, Surface};
use crate::gfx::font::Weight;
use crate::gfx::palette::{self, Color};
use crate::input::{Key, KeyEvent};
use crate::ui::window::{App, AppResponse, WindowMouse};

const PALETTE: [Color; 10] = [
    palette::AMBER,
    palette::CYAN,
    palette::OK,
    palette::WARN,
    palette::ERROR,
    palette::VIOLET,
    palette::TEXT_BRIGHT,
    palette::TEXT_DIM,
    palette::rgb(0x5A, 0x9B, 0xFF),
    palette::rgb(0x0A, 0x0E, 0x18),
];

const TOOLBAR: i32 = 26;

pub struct Paint {
    canvas: Surface,
    color: usize,
    brush: i32,
    last: Option<(i32, i32)>,
    dirty: bool,
}

impl Paint {
    pub fn new() -> Self {
        let mut canvas = Surface::new(1, 1);
        canvas.clear(palette::rgb(0x0A, 0x0E, 0x18));
        Self {
            canvas,
            color: 0,
            brush: 3,
            last: None,
            dirty: true,
        }
    }

    fn ensure_canvas(&mut self, width: usize, height: usize) {
        if self.canvas.width != width || self.canvas.height != height {
            // Keep what has been drawn when the window grows.
            let mut replacement = Surface::new(width, height);
            replacement.clear(palette::rgb(0x0A, 0x0E, 0x18));
            replacement.blit(&self.canvas, 0, 0);
            self.canvas = replacement;
            self.dirty = true;
        }
    }

    fn stroke(&mut self, x: i32, y: i32) {
        let color = PALETTE[self.color];
        match self.last {
            // Interpolate, or fast mouse movement leaves gaps.
            Some((px, py)) => {
                let steps = (px - x).abs().max((py - y).abs()).max(1);
                for step in 0..=steps {
                    let ix = px + (x - px) * step / steps;
                    let iy = py + (y - py) * step / steps;
                    self.canvas.disc(ix, iy, self.brush, color);
                }
            }
            None => self.canvas.disc(x, y, self.brush, color),
        }
        self.last = Some((x, y));
        self.dirty = true;
    }
}

impl App for Paint {
    fn draw(&mut self, surface: &mut Surface, _focused: bool) {
        let width = surface.width as i32;
        let height = surface.height as i32;
        self.ensure_canvas(width.max(1) as usize, (height - TOOLBAR).max(1) as usize);

        surface.clear(palette::PANEL);
        surface.blit(&self.canvas, 0, TOOLBAR);

        // Toolbar.
        surface.fill_rect(
            Rect::new(0, 0, width, TOOLBAR),
            palette::rgb(0x12, 0x18, 0x28),
        );
        surface.hline(0, TOOLBAR - 1, width, palette::BORDER);

        for (index, color) in PALETTE.iter().enumerate() {
            let swatch = Rect::new(6 + index as i32 * 22, 5, 18, 16);
            surface.fill_rect(swatch, *color);
            surface.stroke_rect(
                swatch,
                if index == self.color {
                    palette::TEXT_BRIGHT
                } else {
                    palette::BORDER
                },
            );
        }

        let info = alloc::format!("brush {}  [ ] size   c clear", self.brush);
        surface.text_ex(
            &info,
            6 + PALETTE.len() as i32 * 22 + 12,
            5,
            palette::TEXT_DIM,
            Weight::Regular,
            1,
        );

        self.dirty = false;
    }

    fn on_mouse(&mut self, event: &WindowMouse, _response: &mut AppResponse) {
        if event.released {
            self.last = None;
            return;
        }
        if event.y < TOOLBAR {
            if event.pressed {
                let index = ((event.x - 6) / 22) as usize;
                if index < PALETTE.len() {
                    self.color = index;
                    self.dirty = true;
                }
            }
            return;
        }
        if event.left {
            if event.pressed {
                self.last = None;
            }
            self.stroke(event.x, event.y - TOOLBAR);
        } else {
            self.last = None;
        }
    }

    fn on_key(&mut self, event: &KeyEvent, _response: &mut AppResponse) {
        if !event.pressed {
            return;
        }
        if let Some(ch) = event.character {
            match ch {
                'c' => {
                    self.canvas.clear(palette::rgb(0x0A, 0x0E, 0x18));
                    self.dirty = true;
                }
                '[' => {
                    self.brush = (self.brush - 1).max(1);
                    self.dirty = true;
                }
                ']' => {
                    self.brush = (self.brush + 1).min(24);
                    self.dirty = true;
                }
                digit if digit.is_ascii_digit() => {
                    let index = digit.to_digit(10).unwrap_or(0) as usize;
                    if index < PALETTE.len() {
                        self.color = index;
                        self.dirty = true;
                    }
                }
                _ => {}
            }
        }
        if event.key == Key::Escape {
            self.last = None;
        }
    }

    fn dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn min_size(&self) -> (i32, i32) {
        (320, 220)
    }
}

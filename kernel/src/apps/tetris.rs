//! Tetris.

use alloc::format;
use alloc::vec::Vec;

use crate::arch::cpu;
use crate::drivers::speaker;
use crate::gfx::draw::{Rect, Surface};
use crate::gfx::font::Weight;
use crate::gfx::palette::{self, Color};
use crate::input::{Key, KeyEvent};
use crate::ui::window::{App, AppResponse, WindowMouse};

const COLUMNS: i32 = 10;
const ROWS: i32 = 20;
const HEADER: i32 = 24;

/// The seven pieces, each as four rotations of four (x, y) cells.
/// Written out rather than rotated at runtime: it is less code overall and
/// gives the classic wall-kick-free shapes people expect.
const PIECES: [[[(i32, i32); 4]; 4]; 7] = [
    // I
    [
        [(0, 1), (1, 1), (2, 1), (3, 1)],
        [(2, 0), (2, 1), (2, 2), (2, 3)],
        [(0, 2), (1, 2), (2, 2), (3, 2)],
        [(1, 0), (1, 1), (1, 2), (1, 3)],
    ],
    // J
    [
        [(0, 0), (0, 1), (1, 1), (2, 1)],
        [(1, 0), (2, 0), (1, 1), (1, 2)],
        [(0, 1), (1, 1), (2, 1), (2, 2)],
        [(1, 0), (1, 1), (0, 2), (1, 2)],
    ],
    // L
    [
        [(2, 0), (0, 1), (1, 1), (2, 1)],
        [(1, 0), (1, 1), (1, 2), (2, 2)],
        [(0, 1), (1, 1), (2, 1), (0, 2)],
        [(0, 0), (1, 0), (1, 1), (1, 2)],
    ],
    // O
    [
        [(1, 0), (2, 0), (1, 1), (2, 1)],
        [(1, 0), (2, 0), (1, 1), (2, 1)],
        [(1, 0), (2, 0), (1, 1), (2, 1)],
        [(1, 0), (2, 0), (1, 1), (2, 1)],
    ],
    // S
    [
        [(1, 0), (2, 0), (0, 1), (1, 1)],
        [(1, 0), (1, 1), (2, 1), (2, 2)],
        [(1, 1), (2, 1), (0, 2), (1, 2)],
        [(0, 0), (0, 1), (1, 1), (1, 2)],
    ],
    // T
    [
        [(1, 0), (0, 1), (1, 1), (2, 1)],
        [(1, 0), (1, 1), (2, 1), (1, 2)],
        [(0, 1), (1, 1), (2, 1), (1, 2)],
        [(1, 0), (0, 1), (1, 1), (1, 2)],
    ],
    // Z
    [
        [(0, 0), (1, 0), (1, 1), (2, 1)],
        [(2, 0), (1, 1), (2, 1), (1, 2)],
        [(0, 1), (1, 1), (1, 2), (2, 2)],
        [(1, 0), (0, 1), (1, 1), (0, 2)],
    ],
];

fn piece_colour(kind: usize) -> Color {
    match kind {
        0 => palette::CYAN,
        1 => palette::rgb(0x5A, 0x9B, 0xFF),
        2 => palette::rgb(0xFF, 0x9A, 0x3C),
        3 => palette::WARN,
        4 => palette::OK,
        5 => palette::VIOLET,
        _ => palette::ERROR,
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

pub struct Tetris {
    /// 0 is empty; otherwise the piece index plus one.
    board: Vec<u8>,
    kind: usize,
    next_kind: usize,
    rotation: usize,
    x: i32,
    y: i32,
    score: u32,
    lines: u32,
    level: u32,
    alive: bool,
    started: bool,
    paused: bool,
    last_step: u64,
    rng: Rng,
    dirty: bool,
}

impl Tetris {
    pub fn new() -> Self {
        let mut rng = Rng(cpu::rdtsc() | 1);
        let kind = (rng.next() % 7) as usize;
        let next_kind = (rng.next() % 7) as usize;
        let mut game = Self {
            board: alloc::vec![0u8; (COLUMNS * ROWS) as usize],
            kind,
            next_kind,
            rotation: 0,
            x: COLUMNS / 2 - 2,
            y: 0,
            score: 0,
            lines: 0,
            level: 1,
            alive: true,
            started: false,
            paused: false,
            last_step: 0,
            rng,
            dirty: true,
        };
        game.spawn();
        game
    }

    fn cells(&self, kind: usize, rotation: usize, x: i32, y: i32) -> [(i32, i32); 4] {
        let mut out = [(0, 0); 4];
        for (index, (cx, cy)) in PIECES[kind][rotation % 4].iter().enumerate() {
            out[index] = (x + cx, y + cy);
        }
        out
    }

    fn fits(&self, kind: usize, rotation: usize, x: i32, y: i32) -> bool {
        for (cx, cy) in self.cells(kind, rotation, x, y) {
            if cx < 0 || cx >= COLUMNS || cy >= ROWS {
                return false;
            }
            // Above the top is allowed while a piece is entering.
            if cy >= 0 && self.board[(cy * COLUMNS + cx) as usize] != 0 {
                return false;
            }
        }
        true
    }

    fn spawn(&mut self) {
        self.kind = self.next_kind;
        self.next_kind = (self.rng.next() % 7) as usize;
        self.rotation = 0;
        self.x = COLUMNS / 2 - 2;
        self.y = -1;
        if !self.fits(self.kind, self.rotation, self.x, self.y) {
            self.alive = false;
            speaker::error_tone();
        }
    }

    fn lock(&mut self) {
        for (cx, cy) in self.cells(self.kind, self.rotation, self.x, self.y) {
            if cy >= 0 && cy < ROWS && cx >= 0 && cx < COLUMNS {
                self.board[(cy * COLUMNS + cx) as usize] = self.kind as u8 + 1;
            }
        }
        self.clear_lines();
        self.spawn();
    }

    fn clear_lines(&mut self) {
        let mut cleared = 0;
        let mut row = ROWS - 1;
        while row >= 0 {
            let full =
                (0..COLUMNS).all(|column| self.board[(row * COLUMNS + column) as usize] != 0);
            if full {
                cleared += 1;
                // Shift everything above down by one and re-test this row.
                for above in (1..=row).rev() {
                    for column in 0..COLUMNS {
                        self.board[(above * COLUMNS + column) as usize] =
                            self.board[((above - 1) * COLUMNS + column) as usize];
                    }
                }
                for column in 0..COLUMNS {
                    self.board[column as usize] = 0;
                }
            } else {
                row -= 1;
            }
        }
        if cleared > 0 {
            // The classic scoring curve: clearing four at once is worth far
            // more than four singles.
            self.score += match cleared {
                1 => 100,
                2 => 300,
                3 => 500,
                _ => 800,
            } * self.level;
            self.lines += cleared;
            self.level = 1 + self.lines / 10;
            speaker::beep(700 + cleared * 200, 25);
        }
    }

    fn step_interval(&self) -> u64 {
        (600u64)
            .saturating_sub((self.level as u64 - 1) * 55)
            .max(80)
    }

    fn drop_one(&mut self) {
        if self.fits(self.kind, self.rotation, self.x, self.y + 1) {
            self.y += 1;
        } else {
            self.lock();
        }
        self.dirty = true;
    }

    fn hard_drop(&mut self) {
        while self.fits(self.kind, self.rotation, self.x, self.y + 1) {
            self.y += 1;
            self.score += 2;
        }
        self.lock();
        self.dirty = true;
    }

    fn reset(&mut self) {
        self.board.iter_mut().for_each(|cell| *cell = 0);
        self.score = 0;
        self.lines = 0;
        self.level = 1;
        self.alive = true;
        self.started = false;
        self.paused = false;
        self.spawn();
        self.dirty = true;
    }
}

impl App for Tetris {
    fn draw(&mut self, surface: &mut Surface, _focused: bool) {
        surface.clear(palette::rgb(0x06, 0x09, 0x12));
        let width = surface.width as i32;
        let height = surface.height as i32;

        surface.fill_rect(
            Rect::new(0, 0, width, HEADER),
            palette::rgb(0x12, 0x18, 0x28),
        );
        surface.hline(0, HEADER - 1, width, palette::BORDER);
        surface.text_ex(
            &format!("score {}", self.score),
            8,
            4,
            palette::AMBER,
            Weight::Bold,
            1,
        );
        surface.text(
            &format!("lines {}   level {}", self.lines, self.level),
            8 + 15 * 8,
            4,
            palette::TEXT_DIM,
        );

        let cell = ((width - 90) / COLUMNS)
            .min((height - HEADER - 12) / ROWS)
            .max(4);
        let board_w = cell * COLUMNS;
        let board_h = cell * ROWS;
        let ox = 8;
        let oy = HEADER + (height - HEADER - board_h) / 2;

        surface.fill_rect(
            Rect::new(ox, oy, board_w, board_h),
            palette::rgb(0x08, 0x0C, 0x16),
        );
        surface.stroke_rect(Rect::new(ox, oy, board_w, board_h), palette::BORDER);

        let block = |surface: &mut Surface, x: i32, y: i32, colour: Color| {
            let rect = Rect::new(ox + x * cell + 1, oy + y * cell + 1, cell - 2, cell - 2);
            surface.fill_rect(rect, colour);
            // A lighter top edge gives the blocks a little relief.
            surface.hline(rect.x, rect.y, rect.w, palette::scale(colour, 200));
        };

        for row in 0..ROWS {
            for column in 0..COLUMNS {
                let value = self.board[(row * COLUMNS + column) as usize];
                if value != 0 {
                    block(surface, column, row, piece_colour(value as usize - 1));
                }
            }
        }

        if self.alive {
            // Landing shadow, so the drop position is obvious.
            let mut ghost_y = self.y;
            while self.fits(self.kind, self.rotation, self.x, ghost_y + 1) {
                ghost_y += 1;
            }
            for (cx, cy) in self.cells(self.kind, self.rotation, self.x, ghost_y) {
                if cy >= 0 {
                    surface.stroke_rect(
                        Rect::new(ox + cx * cell + 1, oy + cy * cell + 1, cell - 2, cell - 2),
                        palette::scale(piece_colour(self.kind), 90),
                    );
                }
            }
            for (cx, cy) in self.cells(self.kind, self.rotation, self.x, self.y) {
                if cy >= 0 {
                    block(surface, cx, cy, piece_colour(self.kind));
                }
            }
        }

        // Next piece.
        let panel_x = ox + board_w + 10;
        surface.text("next", panel_x, oy + 4, palette::TEXT_DIM);
        for (cx, cy) in PIECES[self.next_kind][0] {
            let rect = Rect::new(
                panel_x + cx * (cell - 2) + 2,
                oy + 22 + cy * (cell - 2),
                cell - 4,
                cell - 4,
            );
            surface.fill_rect(rect, piece_colour(self.next_kind));
        }

        surface.text("arrows move", panel_x, oy + 100, palette::TEXT_FAINT);
        surface.text("up rotates", panel_x, oy + 116, palette::TEXT_FAINT);
        surface.text("space drops", panel_x, oy + 132, palette::TEXT_FAINT);
        surface.text("p pauses", panel_x, oy + 148, palette::TEXT_FAINT);

        if !self.started || self.paused || !self.alive {
            surface.fill_rect_blend(
                Rect::new(ox, oy + board_h / 2 - 26, board_w, 52),
                palette::BLACK,
                200,
            );
            let (line, colour) = if !self.alive {
                ("GAME OVER", palette::ERROR)
            } else if self.paused {
                ("PAUSED", palette::WARN)
            } else {
                ("press a key", palette::TEXT_BRIGHT)
            };
            surface.text_centered(
                line,
                ox + board_w / 2,
                oy + board_h / 2 - 14,
                colour,
                Weight::Bold,
            );
            if !self.alive {
                surface.text_centered(
                    "r restarts",
                    ox + board_w / 2,
                    oy + board_h / 2 + 4,
                    palette::TEXT_DIM,
                    Weight::Regular,
                );
            }
        }

        self.dirty = false;
    }

    fn on_key(&mut self, event: &KeyEvent, _response: &mut AppResponse) {
        if !event.pressed {
            return;
        }
        self.dirty = true;

        if event.character == Some('r') {
            self.reset();
            return;
        }
        if !self.alive {
            return;
        }
        if event.character == Some('p') {
            self.paused = !self.paused;
            return;
        }
        self.started = true;
        if self.paused {
            return;
        }

        match event.key {
            Key::Left => {
                if self.fits(self.kind, self.rotation, self.x - 1, self.y) {
                    self.x -= 1;
                }
            }
            Key::Right => {
                if self.fits(self.kind, self.rotation, self.x + 1, self.y) {
                    self.x += 1;
                }
            }
            Key::Down => self.drop_one(),
            Key::Up => {
                let rotated = (self.rotation + 1) % 4;
                // Try in place, then nudged either way -- a poor man's wall kick.
                for offset in [0, -1, 1, -2, 2] {
                    if self.fits(self.kind, rotated, self.x + offset, self.y) {
                        self.rotation = rotated;
                        self.x += offset;
                        break;
                    }
                }
            }
            Key::Char if event.character == Some(' ') => self.hard_drop(),
            _ => {}
        }
    }

    fn on_mouse(&mut self, _event: &WindowMouse, _response: &mut AppResponse) {}

    fn tick(&mut self, now_ms: u64, _response: &mut AppResponse) {
        if !self.started || self.paused || !self.alive {
            return;
        }
        if now_ms.saturating_sub(self.last_step) >= self.step_interval() {
            self.last_step = now_ms;
            self.drop_one();
        }
    }

    fn dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn min_size(&self) -> (i32, i32) {
        (330, 420)
    }
}

//! Snake. Every operating system needs one.

use alloc::collections::VecDeque;
use alloc::format;

use crate::arch::cpu;
use crate::drivers::speaker;
use crate::gfx::draw::{Rect, Surface};
use crate::gfx::font::Weight;
use crate::gfx::palette;
use crate::input::{Key, KeyEvent};
use crate::ui::window::{App, AppResponse, WindowMouse};

const GRID_W: i32 = 32;
const GRID_H: i32 = 22;
const HEADER: i32 = 22;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    fn delta(self) -> (i32, i32) {
        match self {
            Direction::Up => (0, -1),
            Direction::Down => (0, 1),
            Direction::Left => (-1, 0),
            Direction::Right => (1, 0),
        }
    }

    fn opposes(self, other: Direction) -> bool {
        matches!(
            (self, other),
            (Direction::Up, Direction::Down)
                | (Direction::Down, Direction::Up)
                | (Direction::Left, Direction::Right)
                | (Direction::Right, Direction::Left)
        )
    }
}

/// xorshift64*, seeded from the timestamp counter. Good enough to put food in
/// an unpredictable place.
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        Rng(cpu::rdtsc() | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, limit: i32) -> i32 {
        (self.next() % limit.max(1) as u64) as i32
    }
}

pub struct Snake {
    body: VecDeque<(i32, i32)>,
    direction: Direction,
    queued: Option<Direction>,
    food: (i32, i32),
    score: u32,
    best: u32,
    alive: bool,
    started: bool,
    last_step: u64,
    step_ms: u64,
    rng: Rng,
    dirty: bool,
}

impl Snake {
    pub fn new() -> Self {
        let mut game = Self {
            body: VecDeque::new(),
            direction: Direction::Right,
            queued: None,
            food: (0, 0),
            score: 0,
            best: 0,
            alive: true,
            started: false,
            last_step: 0,
            step_ms: 110,
            rng: Rng::new(),
            dirty: true,
        };
        game.reset();
        game
    }

    fn reset(&mut self) {
        self.body.clear();
        for x in 0..4 {
            self.body.push_front((4 + x, GRID_H / 2));
        }
        self.direction = Direction::Right;
        self.queued = None;
        self.score = 0;
        self.alive = true;
        self.started = false;
        self.step_ms = 110;
        self.place_food();
        self.dirty = true;
    }

    fn place_food(&mut self) {
        // Retry rather than scanning: the board is never full enough for this
        // to matter, and it keeps the code honest.
        for _ in 0..500 {
            let candidate = (self.rng.below(GRID_W), self.rng.below(GRID_H));
            if !self.body.contains(&candidate) {
                self.food = candidate;
                return;
            }
        }
        self.food = (0, 0);
    }

    fn step(&mut self) {
        if !self.alive || !self.started {
            return;
        }
        if let Some(queued) = self.queued.take() {
            if !queued.opposes(self.direction) {
                self.direction = queued;
            }
        }

        let (dx, dy) = self.direction.delta();
        let &(hx, hy) = self.body.front().unwrap();
        let head = (hx + dx, hy + dy);

        if head.0 < 0 || head.1 < 0 || head.0 >= GRID_W || head.1 >= GRID_H {
            self.die();
            return;
        }
        // The tail moves out of the way this tick, so it is not a collision.
        let tail = *self.body.back().unwrap();
        if self.body.iter().any(|segment| *segment == head) && head != tail {
            self.die();
            return;
        }

        self.body.push_front(head);
        if head == self.food {
            self.score += 1;
            self.best = self.best.max(self.score);
            self.step_ms = self.step_ms.saturating_sub(2).max(45);
            self.place_food();
            speaker::beep(1200, 8);
        } else {
            self.body.pop_back();
        }
        self.dirty = true;
    }

    fn die(&mut self) {
        self.alive = false;
        self.dirty = true;
        speaker::error_tone();
    }
}

impl App for Snake {
    fn draw(&mut self, surface: &mut Surface, _focused: bool) {
        surface.clear(palette::rgb(0x06, 0x09, 0x12));
        let width = surface.width as i32;
        let height = surface.height as i32;

        // Header.
        surface.fill_rect(
            Rect::new(0, 0, width, HEADER),
            palette::rgb(0x12, 0x18, 0x28),
        );
        surface.hline(0, HEADER - 1, width, palette::BORDER);
        surface.text_ex(
            &format!("score {}", self.score),
            8,
            3,
            palette::AMBER,
            Weight::Bold,
            1,
        );
        surface.text(
            &format!("best {}", self.best),
            8 + 14 * 8,
            3,
            palette::TEXT_DIM,
        );

        // Board, centred and square-celled.
        let cell = ((width / GRID_W).min((height - HEADER) / GRID_H)).max(3);
        let board_w = cell * GRID_W;
        let board_h = cell * GRID_H;
        let ox = (width - board_w) / 2;
        let oy = HEADER + (height - HEADER - board_h) / 2;

        surface.fill_rect(
            Rect::new(ox, oy, board_w, board_h),
            palette::rgb(0x08, 0x0C, 0x16),
        );
        surface.stroke_rect(Rect::new(ox, oy, board_w, board_h), palette::BORDER);

        // Food, pulsing.
        surface.fill_rect(
            Rect::new(
                ox + self.food.0 * cell + 1,
                oy + self.food.1 * cell + 1,
                cell - 2,
                cell - 2,
            ),
            palette::ERROR,
        );

        for (index, (x, y)) in self.body.iter().enumerate() {
            let color = if index == 0 {
                palette::AMBER
            } else {
                // Fade along the body.
                let shade = 220u32.saturating_sub(index as u32 * 4).max(90) as u8;
                palette::scale(palette::OK, shade)
            };
            surface.fill_rect(
                Rect::new(ox + x * cell + 1, oy + y * cell + 1, cell - 2, cell - 2),
                color,
            );
        }

        if !self.started {
            surface.fill_rect_blend(
                Rect::new(ox, oy + board_h / 2 - 22, board_w, 44),
                palette::BLACK,
                190,
            );
            surface.text_centered(
                "arrow keys to start",
                ox + board_w / 2,
                oy + board_h / 2 - 8,
                palette::TEXT_BRIGHT,
                Weight::Bold,
            );
        } else if !self.alive {
            surface.fill_rect_blend(
                Rect::new(ox, oy + board_h / 2 - 28, board_w, 56),
                palette::BLACK,
                200,
            );
            surface.text_centered(
                "GAME OVER",
                ox + board_w / 2,
                oy + board_h / 2 - 16,
                palette::ERROR,
                Weight::Bold,
            );
            surface.text_centered(
                "press r to play again",
                ox + board_w / 2,
                oy + board_h / 2 + 4,
                palette::TEXT_DIM,
                Weight::Regular,
            );
        }

        self.dirty = false;
    }

    fn on_key(&mut self, event: &KeyEvent, _response: &mut AppResponse) {
        if !event.pressed {
            return;
        }
        let direction = match event.key {
            Key::Up => Some(Direction::Up),
            Key::Down => Some(Direction::Down),
            Key::Left => Some(Direction::Left),
            Key::Right => Some(Direction::Right),
            _ => None,
        };
        if let Some(direction) = direction {
            self.started = true;
            self.queued = Some(direction);
            self.dirty = true;
            return;
        }
        if event.character == Some('r') || (event.key == Key::Enter && !self.alive) {
            self.reset();
        }
    }

    fn on_mouse(&mut self, _event: &WindowMouse, _response: &mut AppResponse) {}

    fn tick(&mut self, now_ms: u64, _response: &mut AppResponse) {
        if now_ms.saturating_sub(self.last_step) >= self.step_ms {
            self.last_step = now_ms;
            self.step();
        }
    }

    fn dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn min_size(&self) -> (i32, i32) {
        (320, 260)
    }
}

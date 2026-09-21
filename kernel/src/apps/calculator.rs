//! Calculator.
//!
//! Arithmetic is done in fixed point with four decimal places, because the
//! kernel is built soft-float and a calculator has no business dragging the
//! FPU into the compositor thread.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::gfx::draw::{Rect, Surface};
use crate::gfx::font::Weight;
use crate::gfx::palette::{self, Color};
use crate::input::{Key, KeyEvent};
use crate::ui::window::{App, AppResponse, WindowMouse};

/// Four decimal places, so 1.2345 is stored as 12345.
const SCALE: i64 = 10_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operator {
    None,
    Add,
    Subtract,
    Multiply,
    Divide,
}

impl Operator {
    fn symbol(self) -> &'static str {
        match self {
            Operator::None => "",
            Operator::Add => "+",
            Operator::Subtract => "-",
            Operator::Multiply => "*",
            Operator::Divide => "/",
        }
    }
}

fn render(value: i64) -> String {
    let negative = value < 0;
    let magnitude = value.unsigned_abs();
    let whole = magnitude / SCALE as u64;
    let fraction = magnitude % SCALE as u64;
    let mut text = if fraction == 0 {
        format!("{}", whole)
    } else {
        let mut digits = format!("{:04}", fraction);
        while digits.ends_with('0') {
            digits.pop();
        }
        format!("{}.{}", whole, digits)
    };
    if negative {
        text.insert(0, '-');
    }
    text
}

struct Button {
    label: &'static str,
    rect: Rect,
    accent: bool,
}

pub struct Calculator {
    /// The number being typed, as text, so digits after a decimal point behave.
    entry: String,
    accumulator: i64,
    pending: Operator,
    fresh: bool,
    history: Vec<String>,
    error: Option<&'static str>,
    buttons: Vec<Button>,
    dirty: bool,
    pressed: Option<usize>,
}

const LAYOUT: [[&str; 4]; 5] = [
    ["C", "+/-", "%", "/"],
    ["7", "8", "9", "*"],
    ["4", "5", "6", "-"],
    ["1", "2", "3", "+"],
    ["0", ".", "<", "="],
];

impl Calculator {
    pub fn new() -> Self {
        Self {
            entry: String::from("0"),
            accumulator: 0,
            pending: Operator::None,
            fresh: true,
            history: Vec::new(),
            error: None,
            buttons: Vec::new(),
            dirty: true,
            pressed: None,
        }
    }

    fn value(&self) -> i64 {
        let text = self.entry.trim();
        let negative = text.starts_with('-');
        let text = text.trim_start_matches('-');
        let mut parts = text.splitn(2, '.');
        let whole: i64 = parts.next().unwrap_or("0").parse().unwrap_or(0);
        let fraction = parts.next().unwrap_or("");
        let mut scaled = 0i64;
        let mut place = SCALE / 10;
        for ch in fraction.chars().take(4) {
            scaled += (ch as i64 - '0' as i64) * place;
            place /= 10;
        }
        let value = whole * SCALE + scaled;
        if negative {
            -value
        } else {
            value
        }
    }

    fn apply(&mut self, operator: Operator) {
        let right = self.value();
        if self.pending == Operator::None {
            self.accumulator = right;
        } else {
            let left = self.accumulator;
            let result = match self.pending {
                Operator::Add => Some(left + right),
                Operator::Subtract => Some(left - right),
                // Both operands carry the scale, so a product has it twice.
                Operator::Multiply => Some(left * right / SCALE),
                Operator::Divide => {
                    if right == 0 {
                        None
                    } else {
                        Some(left * SCALE / right)
                    }
                }
                Operator::None => Some(right),
            };
            match result {
                Some(value) => {
                    self.history.push(format!(
                        "{} {} {} = {}",
                        render(left),
                        self.pending.symbol(),
                        render(right),
                        render(value)
                    ));
                    while self.history.len() > 6 {
                        self.history.remove(0);
                    }
                    self.accumulator = value;
                }
                None => {
                    self.error = Some("division by zero");
                    self.accumulator = 0;
                }
            }
        }
        self.entry = render(self.accumulator);
        self.pending = operator;
        self.fresh = true;
        self.dirty = true;
    }

    fn digit(&mut self, ch: char) {
        if self.fresh || self.entry == "0" {
            self.entry.clear();
            self.fresh = false;
        }
        if self.entry.chars().filter(|c| c.is_ascii_digit()).count() >= 12 {
            return;
        }
        self.entry.push(ch);
        self.error = None;
        self.dirty = true;
    }

    fn press(&mut self, label: &str) {
        match label {
            "C" => {
                self.entry = String::from("0");
                self.accumulator = 0;
                self.pending = Operator::None;
                self.fresh = true;
                self.error = None;
            }
            "<" => {
                self.entry.pop();
                if self.entry.is_empty() || self.entry == "-" {
                    self.entry = String::from("0");
                    self.fresh = true;
                }
            }
            "+/-" => {
                if self.entry.starts_with('-') {
                    self.entry.remove(0);
                } else if self.entry != "0" {
                    self.entry.insert(0, '-');
                }
            }
            "%" => {
                let value = self.value() / 100;
                self.entry = render(value);
                self.fresh = true;
            }
            "." => {
                if self.fresh {
                    self.entry = String::from("0");
                    self.fresh = false;
                }
                if !self.entry.contains('.') {
                    self.entry.push('.');
                }
            }
            "+" => self.apply(Operator::Add),
            "-" => self.apply(Operator::Subtract),
            "*" => self.apply(Operator::Multiply),
            "/" => self.apply(Operator::Divide),
            "=" => {
                self.apply(Operator::None);
                self.pending = Operator::None;
            }
            digit if digit.len() == 1 && digit.chars().next().unwrap().is_ascii_digit() => {
                self.digit(digit.chars().next().unwrap())
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn layout(&mut self, width: i32, height: i32) {
        let top = 96;
        let gap = 4;
        let columns = 4;
        let rows = LAYOUT.len() as i32;
        let cell_w = (width - gap * (columns + 1)) / columns;
        let cell_h = (height - top - gap * (rows + 1)).max(16) / rows;

        self.buttons.clear();
        for (row_index, row) in LAYOUT.iter().enumerate() {
            for (column_index, label) in row.iter().enumerate() {
                let rect = Rect::new(
                    gap + column_index as i32 * (cell_w + gap),
                    top + gap + row_index as i32 * (cell_h + gap),
                    cell_w,
                    cell_h,
                );
                let accent = matches!(*label, "+" | "-" | "*" | "/" | "=");
                self.buttons.push(Button {
                    label,
                    rect,
                    accent,
                });
            }
        }
    }
}

impl App for Calculator {
    fn draw(&mut self, surface: &mut Surface, _focused: bool) {
        let width = surface.width as i32;
        let height = surface.height as i32;
        self.layout(width, height);

        surface.clear(palette::PANEL);

        // Display.
        let display = Rect::new(6, 6, width - 12, 84);
        surface.fill_rect(display, palette::rgb(0x06, 0x0A, 0x14));
        surface.stroke_rect(display, palette::BORDER);

        for (index, line) in self.history.iter().rev().take(3).enumerate() {
            surface.text(
                line,
                display.x + 8,
                display.y + 6 + index as i32 * 14,
                palette::TEXT_FAINT,
            );
        }

        let shown = self.error.unwrap_or(&self.entry);
        let colour: Color = if self.error.is_some() {
            palette::ERROR
        } else {
            palette::AMBER
        };
        let text_width = shown.chars().count() as i32 * 8 * 2;
        surface.text_ex(
            shown,
            display.right() - text_width - 10,
            display.bottom() - 26,
            colour,
            Weight::Bold,
            2,
        );
        if self.pending != Operator::None {
            surface.text_ex(
                self.pending.symbol(),
                display.x + 8,
                display.bottom() - 26,
                palette::CYAN,
                Weight::Bold,
                2,
            );
        }

        for (index, button) in self.buttons.iter().enumerate() {
            let held = self.pressed == Some(index);
            let fill = if held {
                palette::rgb(0x34, 0x40, 0x66)
            } else if button.accent {
                palette::rgb(0x22, 0x2B, 0x46)
            } else {
                palette::rgb(0x16, 0x1D, 0x30)
            };
            surface.fill_rect(button.rect, fill);
            surface.stroke_rect(
                button.rect,
                if button.accent {
                    palette::AMBER_DIM
                } else {
                    palette::BORDER
                },
            );
            let label_width = button.label.chars().count() as i32 * 8;
            surface.text_ex(
                button.label,
                button.rect.x + (button.rect.w - label_width) / 2,
                button.rect.y + (button.rect.h - 16) / 2,
                if button.accent {
                    palette::AMBER
                } else {
                    palette::TEXT_BRIGHT
                },
                Weight::Bold,
                1,
            );
        }

        self.dirty = false;
    }

    fn on_key(&mut self, event: &KeyEvent, _response: &mut AppResponse) {
        if !event.pressed {
            return;
        }
        match event.key {
            Key::Enter => self.press("="),
            Key::Backspace => self.press("<"),
            Key::Escape | Key::Delete => self.press("C"),
            Key::Char => {
                if let Some(ch) = event.character {
                    match ch {
                        '0'..='9' => self.digit(ch),
                        '.' | ',' => self.press("."),
                        '+' => self.press("+"),
                        '-' => self.press("-"),
                        '*' | 'x' => self.press("*"),
                        '/' => self.press("/"),
                        '=' => self.press("="),
                        'c' => self.press("C"),
                        '%' => self.press("%"),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn on_mouse(&mut self, event: &WindowMouse, _response: &mut AppResponse) {
        if event.released {
            self.pressed = None;
            self.dirty = true;
            return;
        }
        if !event.pressed {
            return;
        }
        // Collected first so the borrow ends before `press` takes &mut self.
        let hit = self
            .buttons
            .iter()
            .position(|button| button.rect.contains(event.x, event.y));
        if let Some(index) = hit {
            let label = self.buttons[index].label;
            self.pressed = Some(index);
            self.press(label);
        }
    }

    fn dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn min_size(&self) -> (i32, i32) {
        (260, 340)
    }
}

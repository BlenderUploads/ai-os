//! The CRT look, the boot splash, and the fault screen.
//!
//! The scanline and vignette passes run once per frame over the finished back
//! buffer. They are deliberately cheap: a multiply per pixel on alternate rows
//! and a precomputed edge falloff, no per-pixel branching worth speaking of.

use crate::arch::interrupts::TrapFrame;

use super::draw::{Rect, Surface};
use super::font::{glyph, Weight, GLYPH_HEIGHT};
use super::palette::{self, Color};

/// Darken every other row so the display reads as a tube rather than an LCD.
///
/// This runs over the whole screen on every composited frame, so it avoids
/// `palette::scale` and its divide: subtracting `c >> 2` per channel is a
/// shift, a mask and a subtract, and darkens by a flat 25%. The mask keeps
/// each channel's borrow from bleeding into the one below it.
pub fn apply_scanlines(surface: &mut Surface) {
    let bounds = surface.bounds();
    apply_scanlines_rect(surface, bounds);
}

/// Scanlines over one region, for partial redraws.
///
/// The darkened rows are chosen by absolute y, not by position within the
/// rectangle, so a region redrawn on its own lines up with everything around
/// it instead of shifting the stripe pattern.
pub fn apply_scanlines_rect(surface: &mut Surface, rect: Rect) {
    let area = rect.intersect(&surface.bounds());
    if area.is_empty() {
        return;
    }
    let width = surface.width;
    let first = if area.y % 2 == 0 { area.y + 1 } else { area.y };
    let mut y = first;
    while y < area.bottom() {
        let start = y as usize * width + area.x as usize;
        let row = &mut surface.pixels[start..start + area.w as usize];
        for pixel in row.iter_mut() {
            let value = *pixel;
            *pixel = value - ((value >> 2) & 0x003F_3F3F);
        }
        y += 2;
    }
}

/// Darken the corners. Approximated with a separable falloff so it costs one
/// multiply per pixel in the affected band rather than a distance per pixel.
pub fn apply_vignette(surface: &mut Surface) {
    let width = surface.width as i32;
    let height = surface.height as i32;
    let margin_x = width / 6;
    let margin_y = height / 6;

    for y in 0..height {
        let fy = if y < margin_y {
            180 + (y * 75 / margin_y)
        } else if y >= height - margin_y {
            180 + ((height - 1 - y) * 75 / margin_y)
        } else {
            255
        };
        for x in 0..width {
            let fx = if x < margin_x {
                180 + (x * 75 / margin_x)
            } else if x >= width - margin_x {
                180 + ((width - 1 - x) * 75 / margin_x)
            } else {
                255
            };
            if fx == 255 && fy == 255 {
                continue;
            }
            let factor = (fx * fy / 255).clamp(0, 255) as u8;
            let index = y as usize * surface.width + x as usize;
            surface.pixels[index] = palette::scale(surface.pixels[index], factor);
        }
    }
}

/// The HALCYON wordmark, drawn large.
pub fn draw_wordmark(surface: &mut Surface, cx: i32, y: i32, scale: i32) {
    let text = "HALCYON";
    let width = text.len() as i32 * 8 * scale;
    let x = cx - width / 2;

    // A soft amber bloom underneath the letters.
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1)] {
        let mut cursor = x + dx * scale;
        for ch in text.chars() {
            surface.glyph(
                ch as u8,
                cursor,
                y + dy * scale,
                palette::AMBER_GLOW,
                Weight::Bold,
                scale,
            );
            cursor += 8 * scale;
        }
    }
    surface.text_ex(text, x, y, palette::AMBER, Weight::Bold, scale);
}

/// Full-screen desktop backdrop: gradient, a faint grid, and a vignette.
pub fn draw_backdrop(surface: &mut Surface) {
    let bounds = surface.bounds();
    surface.gradient_v(bounds, palette::DESKTOP_TOP, palette::DESKTOP_BOTTOM);

    // A faint engineering grid, brighter every fifth line.
    let step = 32;
    let mut x = 0;
    while x < bounds.w {
        let strong = (x / step) % 5 == 0;
        surface.fill_rect_blend(
            Rect::new(x, 0, 1, bounds.h),
            palette::CYAN,
            if strong { 14 } else { 7 },
        );
        x += step;
    }
    let mut y = 0;
    while y < bounds.h {
        let strong = (y / step) % 5 == 0;
        surface.fill_rect_blend(
            Rect::new(0, y, bounds.w, 1),
            palette::CYAN,
            if strong { 14 } else { 7 },
        );
        y += step;
    }
}

/// The boot splash, shown while subsystems come up.
pub struct Splash {
    pub lines: usize,
}

impl Splash {
    pub fn new() -> Self {
        Self { lines: 0 }
    }

    pub fn draw_frame(&self, surface: &mut Surface) {
        surface.clear(palette::VOID);
        draw_backdrop(surface);
        let cx = surface.width as i32 / 2;
        draw_wordmark(surface, cx, surface.height as i32 / 2 - 90, 4);
        surface.text_centered(
            "A FROM-SCRATCH OPERATING SYSTEM",
            cx,
            surface.height as i32 / 2 - 40,
            palette::CYAN_DIM,
            Weight::Regular,
        );
    }
}

/// Terminal fault screen. Allocation-free and lock-free by construction: it
/// runs when the machine is already broken.
pub fn draw_fault_screen(name: &str, trap: &TrapFrame, cr2: Option<u64>) {
    let framebuffer = &super::fb::FRAMEBUFFER;
    // The faulting code may have been holding this lock.
    unsafe { framebuffer.force_unlock() };
    let framebuffer = unsafe { framebuffer.get_unchecked() };
    let surface = &mut framebuffer.back;
    if surface.width == 0 {
        return;
    }
    surface.reset_clip();

    surface.clear(palette::rgb(0x18, 0x03, 0x06));
    let bounds = surface.bounds();
    surface.gradient_v(
        bounds,
        palette::rgb(0x24, 0x05, 0x09),
        palette::rgb(0x0A, 0x01, 0x03),
    );

    let margin = 48;
    let panel = Rect::new(margin, margin, bounds.w - margin * 2, bounds.h - margin * 2);
    surface.panel(panel, palette::rgb(0x12, 0x02, 0x05), palette::ERROR);

    let mut y = panel.y + 24;
    let x = panel.x + 24;

    surface.text_ex("HALCYON HAS STOPPED", x, y, palette::ERROR, Weight::Bold, 2);
    y += 44;
    surface.hline(x, y, panel.w - 48, palette::rgb(0x55, 0x12, 0x18));
    y += 16;

    let put = |surface: &mut Surface, label: &str, value: &str, y: &mut i32| {
        surface.text(label, x, *y, palette::rgb(0xB0, 0x60, 0x68));
        surface.text_bold(value, x + 15 * 8, *y, palette::rgb(0xFF, 0xC8, 0xCC));
        *y += GLYPH_HEIGHT as i32 + 2;
    };

    let mut buffer = HexBuffer::new();
    put(surface, "exception", name, &mut y);
    put(surface, "vector", buffer.dec(trap.vector), &mut y);
    put(surface, "error code", buffer.hex(trap.error_code), &mut y);
    put(surface, "rip", buffer.hex(trap.rip), &mut y);
    put(surface, "rsp", buffer.hex(trap.rsp), &mut y);
    put(surface, "rflags", buffer.hex(trap.rflags), &mut y);
    if let Some(address) = cr2 {
        put(surface, "address (cr2)", buffer.hex(address), &mut y);
    }

    y += 12;
    surface.text("registers", x, y, palette::rgb(0xB0, 0x60, 0x68));
    y += GLYPH_HEIGHT as i32 + 4;

    let registers: [(&str, u64); 8] = [
        ("rax", trap.rax),
        ("rbx", trap.rbx),
        ("rcx", trap.rcx),
        ("rdx", trap.rdx),
        ("rsi", trap.rsi),
        ("rdi", trap.rdi),
        ("rbp", trap.rbp),
        ("r8 ", trap.r8),
    ];
    for pair in registers.chunks(2) {
        let mut column = x;
        for (label, value) in pair {
            surface.text(label, column, y, palette::rgb(0x9A, 0x50, 0x58));
            surface.text(
                buffer.hex(*value),
                column + 4 * 8,
                y,
                palette::rgb(0xE8, 0xA8, 0xB0),
            );
            column += 28 * 8;
        }
        y += GLYPH_HEIGHT as i32 + 2;
    }

    y = panel.bottom() - 40;
    surface.glyph(glyph::BULLET, x, y, palette::ERROR, Weight::Regular, 1);
    surface.text(
        "The machine is halted. Nothing was written to any disk.",
        x + 16,
        y,
        palette::rgb(0xC0, 0x80, 0x88),
    );

    apply_scanlines(surface);
    framebuffer.present();
}

/// Formats integers without allocating, for the fault path.
pub struct HexBuffer {
    storage: [u8; 24],
}

impl HexBuffer {
    pub const fn new() -> Self {
        Self { storage: [0; 24] }
    }

    pub fn hex(&mut self, value: u64) -> &str {
        self.storage = [0; 24];
        self.storage[0] = b'0';
        self.storage[1] = b'x';
        for index in 0..16 {
            let nibble = ((value >> ((15 - index) * 4)) & 0xF) as u8;
            self.storage[2 + index] = if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + nibble - 10
            };
        }
        core::str::from_utf8(&self.storage[..18]).unwrap_or("?")
    }

    pub fn dec(&mut self, value: u64) -> &str {
        self.storage = [0; 24];
        if value == 0 {
            self.storage[0] = b'0';
            return core::str::from_utf8(&self.storage[..1]).unwrap_or("?");
        }
        let mut digits = [0u8; 20];
        let mut count = 0;
        let mut remaining = value;
        while remaining > 0 {
            digits[count] = b'0' + (remaining % 10) as u8;
            remaining /= 10;
            count += 1;
        }
        for index in 0..count {
            self.storage[index] = digits[count - 1 - index];
        }
        core::str::from_utf8(&self.storage[..count]).unwrap_or("?")
    }
}

/// Convenience for drawing a colour swatch strip — used by the boot self-test.
pub fn draw_palette_strip(surface: &mut Surface, rect: Rect) {
    let colors: [Color; 8] = [
        palette::AMBER,
        palette::CYAN,
        palette::OK,
        palette::WARN,
        palette::ERROR,
        palette::VIOLET,
        palette::TEXT,
        palette::TEXT_DIM,
    ];
    let cell = rect.w / colors.len() as i32;
    for (index, color) in colors.iter().enumerate() {
        surface.fill_rect(
            Rect::new(rect.x + index as i32 * cell, rect.y, cell - 2, rect.h),
            *color,
        );
    }
}

//! "About HALCYON" — what this thing is.

use crate::gfx::crt;
use crate::gfx::draw::{Rect, Surface};
use crate::gfx::font::{glyph, Weight};
use crate::gfx::palette;
use crate::ui::window::App;

pub struct About {
    dirty: bool,
    frame: u64,
}

impl About {
    pub fn new() -> Self {
        Self {
            dirty: true,
            frame: 0,
        }
    }
}

const FACTS: &[(&str, &str)] = &[
    ("kernel", "written from scratch, x86_64 long mode"),
    ("language", "Rust on stable, no_std, zero crates"),
    ("bootloader", "GRUB via Multiboot2 (BIOS + UEFI)"),
    ("memory", "bitmap frames, 4-level paging, own heap"),
    ("scheduling", "pre-emptive round-robin kernel threads"),
    ("graphics", "own font, own compositor, own everything"),
    ("shell", "hsh, with ORACLE as its interpreter"),
];

impl App for About {
    fn draw(&mut self, surface: &mut Surface, _focused: bool) {
        let width = surface.width as i32;
        surface.clear(palette::PANEL);

        let header = Rect::new(0, 0, width, 84);
        surface.gradient_v(
            header,
            palette::rgb(0x10, 0x16, 0x28),
            palette::rgb(0x0A, 0x0E, 0x1A),
        );
        crt::draw_wordmark(surface, width / 2, 20, 3);
        surface.text_centered(
            "a from-scratch operating system",
            width / 2,
            60,
            palette::CYAN_DIM,
            Weight::Regular,
        );
        surface.hline(0, 84, width, palette::BORDER);

        let mut y = 98;
        for (label, value) in FACTS {
            surface.glyph(
                glyph::BULLET,
                12,
                y,
                palette::AMBER_DIM,
                Weight::Regular,
                1,
            );
            surface.text(label, 28, y, palette::CYAN);
            surface.text(value, 28 + 12 * 8, y, palette::TEXT);
            y += 18;
        }

        y += 10;
        surface.hline(12, y, width - 24, palette::BORDER);
        y += 10;
        surface.text(
            "There is no Linux under here. The bootstrap that put this",
            12,
            y,
            palette::TEXT_DIM,
        );
        y += 16;
        surface.text(
            "CPU into long mode, the page tables, the allocator, the",
            12,
            y,
            palette::TEXT_DIM,
        );
        y += 16;
        surface.text(
            "letterforms you are reading -- all of it lives in one repo.",
            12,
            y,
            palette::TEXT_DIM,
        );

        // A slowly sweeping phosphor line, so the window is visibly alive.
        let sweep = ((self.frame / 2) % (width as u64 + 80)) as i32 - 40;
        surface.fill_rect_blend(
            Rect::new(sweep, 84, 40, 1),
            palette::AMBER,
            140,
        );

        self.dirty = false;
    }

    fn tick(&mut self, _now_ms: u64, _response: &mut crate::ui::window::AppResponse) {
        self.frame += 1;
        self.dirty = true;
    }

    fn dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn min_size(&self) -> (i32, i32) {
        (480, 300)
    }
}

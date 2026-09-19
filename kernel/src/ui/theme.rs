//! Window metrics and shared chrome drawing.

use crate::gfx::draw::{Rect, Surface};
use crate::gfx::font::{Weight, GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::gfx::palette::{self, Color};

pub const TITLE_HEIGHT: i32 = 22;
pub const BORDER: i32 = 1;
pub const TASKBAR_HEIGHT: i32 = 28;
pub const BUTTON_SIZE: i32 = 14;
pub const SHADOW: i32 = 4;

pub const CELL_W: i32 = GLYPH_WIDTH as i32;
pub const CELL_H: i32 = GLYPH_HEIGHT as i32;

/// The frame a window of the given content size occupies.
pub fn frame_for(content: Rect) -> Rect {
    Rect::new(
        content.x - BORDER,
        content.y - TITLE_HEIGHT,
        content.w + BORDER * 2,
        content.h + TITLE_HEIGHT + BORDER,
    )
}

/// Where a window's content sits inside its frame.
pub fn content_for(frame: Rect) -> Rect {
    Rect::new(
        frame.x + BORDER,
        frame.y + TITLE_HEIGHT,
        frame.w - BORDER * 2,
        frame.h - TITLE_HEIGHT - BORDER,
    )
}

pub fn title_bar(frame: Rect) -> Rect {
    Rect::new(frame.x, frame.y, frame.w, TITLE_HEIGHT)
}

pub fn close_button(frame: Rect) -> Rect {
    Rect::new(
        frame.right() - BUTTON_SIZE - 5,
        frame.y + (TITLE_HEIGHT - BUTTON_SIZE) / 2,
        BUTTON_SIZE,
        BUTTON_SIZE,
    )
}

pub fn minimise_button(frame: Rect) -> Rect {
    let close = close_button(frame);
    Rect::new(close.x - BUTTON_SIZE - 4, close.y, BUTTON_SIZE, BUTTON_SIZE)
}

/// Bottom-right grab handle for resizing.
pub fn resize_grip(frame: Rect) -> Rect {
    Rect::new(frame.right() - 12, frame.bottom() - 12, 12, 12)
}

pub fn draw_shadow(surface: &mut Surface, frame: Rect) {
    // Two offset bands rather than a real blur — cheap and reads correctly at
    // this resolution.
    surface.fill_rect_blend(
        Rect::new(frame.x + SHADOW, frame.y + SHADOW, frame.w, frame.h),
        palette::BLACK,
        110,
    );
    surface.fill_rect_blend(
        Rect::new(frame.x + SHADOW / 2, frame.y + SHADOW / 2, frame.w, frame.h),
        palette::BLACK,
        70,
    );
}

pub fn draw_frame(surface: &mut Surface, frame: Rect, title: &str, focused: bool, icon: u8) {
    let border = if focused {
        palette::AMBER
    } else {
        palette::BORDER
    };
    let bar_top = if focused {
        palette::rgb(0x23, 0x2C, 0x48)
    } else {
        palette::rgb(0x14, 0x1A, 0x2B)
    };
    let bar_bottom = if focused {
        palette::rgb(0x16, 0x1C, 0x30)
    } else {
        palette::rgb(0x0E, 0x13, 0x20)
    };

    // Body and border.
    surface.fill_rect(frame, palette::PANEL);
    surface.stroke_rect(frame, border);

    // Title bar.
    let bar = title_bar(frame).inset(BORDER);
    surface.gradient_v(bar, bar_top, bar_bottom);
    surface.hline(bar.x, bar.bottom(), bar.w, border);

    let text_color = if focused {
        palette::AMBER
    } else {
        palette::TEXT_DIM
    };
    surface.glyph(icon, bar.x + 6, bar.y + 3, text_color, Weight::Regular, 1);
    surface.text_ex(
        title,
        bar.x + 6 + CELL_W + 4,
        bar.y + 3,
        text_color,
        if focused { Weight::Bold } else { Weight::Regular },
        1,
    );

    draw_button(surface, minimise_button(frame), b'_', focused);
    draw_button(surface, close_button(frame), b'x', focused);
}

fn draw_button(surface: &mut Surface, rect: Rect, label: u8, focused: bool) {
    let fill = if focused {
        palette::rgb(0x2C, 0x36, 0x55)
    } else {
        palette::rgb(0x18, 0x1E, 0x30)
    };
    surface.fill_rect(rect, fill);
    surface.stroke_rect(rect, palette::BORDER);
    let color = if focused {
        palette::TEXT_BRIGHT
    } else {
        palette::TEXT_DIM
    };
    surface.glyph(
        label,
        rect.x + (rect.w - CELL_W) / 2,
        rect.y + (rect.h - CELL_H) / 2,
        color,
        Weight::Regular,
        1,
    );
}

/// A sunken inner area, used by most app content.
pub fn draw_well(surface: &mut Surface, rect: Rect, fill: Color) {
    surface.fill_rect(rect, fill);
    surface.stroke_rect(rect, palette::PANEL_SUNKEN);
}

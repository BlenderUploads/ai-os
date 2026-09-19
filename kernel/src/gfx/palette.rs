//! HALCYON's colour scheme.
//!
//! Amber and cyan phosphor over a deep blue-black, the way a good terminal
//! looked before anyone thought to make them white.

pub type Color = u32;

#[inline]
pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

#[inline]
pub const fn red_of(c: Color) -> u8 {
    (c >> 16) as u8
}
#[inline]
pub const fn green_of(c: Color) -> u8 {
    (c >> 8) as u8
}
#[inline]
pub const fn blue_of(c: Color) -> u8 {
    c as u8
}

/// Blend `top` over `bottom`. `alpha` is 0..=255.
#[inline]
pub fn blend(bottom: Color, top: Color, alpha: u8) -> Color {
    if alpha == 255 {
        return top;
    }
    if alpha == 0 {
        return bottom;
    }
    let a = alpha as u32;
    let inv = 255 - a;
    let r = (red_of(top) as u32 * a + red_of(bottom) as u32 * inv) / 255;
    let g = (green_of(top) as u32 * a + green_of(bottom) as u32 * inv) / 255;
    let b = (blue_of(top) as u32 * a + blue_of(bottom) as u32 * inv) / 255;
    rgb(r as u8, g as u8, b as u8)
}

/// Scale a colour's brightness. `factor` is 0..=255 where 255 is unchanged.
#[inline]
pub fn scale(color: Color, factor: u8) -> Color {
    let f = factor as u32;
    rgb(
        ((red_of(color) as u32 * f) / 255) as u8,
        ((green_of(color) as u32 * f) / 255) as u8,
        ((blue_of(color) as u32 * f) / 255) as u8,
    )
}

/// Linear interpolation between two colours, `t` in 0..=255.
#[inline]
pub fn mix(a: Color, b: Color, t: u8) -> Color {
    blend(a, b, t)
}

// --- surfaces ---
pub const VOID: Color = rgb(0x03, 0x05, 0x0A);
pub const DESKTOP_TOP: Color = rgb(0x0A, 0x10, 0x22);
pub const DESKTOP_BOTTOM: Color = rgb(0x04, 0x06, 0x0F);
pub const PANEL: Color = rgb(0x0D, 0x12, 0x20);
pub const PANEL_RAISED: Color = rgb(0x16, 0x1D, 0x31);
pub const PANEL_SUNKEN: Color = rgb(0x07, 0x0A, 0x14);
pub const BORDER: Color = rgb(0x2A, 0x33, 0x50);
pub const BORDER_BRIGHT: Color = rgb(0x44, 0x53, 0x7E);

// --- phosphor ---
pub const AMBER: Color = rgb(0xFF, 0xB0, 0x24);
pub const AMBER_DIM: Color = rgb(0x8A, 0x5E, 0x10);
pub const AMBER_GLOW: Color = rgb(0x4A, 0x30, 0x08);
pub const CYAN: Color = rgb(0x3D, 0xE0, 0xD0);
pub const CYAN_DIM: Color = rgb(0x1C, 0x72, 0x6C);

// --- text ---
pub const TEXT: Color = rgb(0xC8, 0xD4, 0xE8);
pub const TEXT_BRIGHT: Color = rgb(0xEC, 0xF3, 0xFF);
pub const TEXT_DIM: Color = rgb(0x6D, 0x7C, 0x99);
pub const TEXT_FAINT: Color = rgb(0x3E, 0x49, 0x60);

// --- status ---
pub const OK: Color = rgb(0x5C, 0xDE, 0x8A);
pub const WARN: Color = rgb(0xFF, 0xC5, 0x4D);
pub const ERROR: Color = rgb(0xFF, 0x5C, 0x6A);
pub const VIOLET: Color = rgb(0xB1, 0x8C, 0xFF);

pub const BLACK: Color = rgb(0, 0, 0);
pub const WHITE: Color = rgb(0xFF, 0xFF, 0xFF);

/// The eight-colour ramp the terminal and the Lisp printer use.
pub const ANSI: [Color; 8] = [
    rgb(0x1A, 0x20, 0x33), // black / shadow
    ERROR,
    OK,
    AMBER,
    rgb(0x5A, 0x9B, 0xFF), // blue
    VIOLET,
    CYAN,
    TEXT_BRIGHT,
];

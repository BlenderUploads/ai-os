//! Text rendering over the framebuffer.

pub use super::font_data::{FONT, GLYPH_HEIGHT, GLYPH_WIDTH};

/// Glyph codes for the drawing characters `mkfont.py` puts in the control range.
pub mod glyph {
    pub const LOGO: u8 = 0x01;
    pub const CHIP: u8 = 0x02;
    pub const DITHER: u8 = 0x03;
    pub const BLOCK: u8 = 0x04;
    pub const HALF_BLOCK: u8 = 0x05;
    pub const BULLET: u8 = 0x06;
    pub const ARROW_RIGHT: u8 = 0x07;
    pub const ARROW_LEFT: u8 = 0x08;
    pub const ARROW_UP: u8 = 0x0E;
    pub const ARROW_DOWN: u8 = 0x0F;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Regular,
    /// Synthesised by smearing each scanline one pixel to the right, which is
    /// how bitmap terminals have always faked bold.
    Bold,
}

#[inline]
pub fn scanline(code: u8, row: usize, weight: Weight) -> u8 {
    let bits = FONT[code as usize][row];
    match weight {
        Weight::Regular => bits,
        Weight::Bold => bits | (bits >> 1),
    }
}

/// Width in pixels of `text` at the given scale.
#[inline]
pub fn text_width(text: &str, scale: usize) -> usize {
    text.chars().count() * GLYPH_WIDTH * scale
}

/// Map a `char` onto the font's 256-entry table.
///
/// The font is ASCII plus a few drawing glyphs, so the typographic characters
/// that turn up in ordinary Rust string literals are folded onto their ASCII
/// equivalents rather than rendered as missing-glyph boxes.
#[inline]
pub fn code_of(ch: char) -> u8 {
    match ch {
        '\u{2014}' | '\u{2013}' | '\u{2212}' => b'-', // em dash, en dash, minus
        '\u{2018}' | '\u{2019}' => b'\'',             // curly single quotes
        '\u{201C}' | '\u{201D}' => b'"',              // curly double quotes
        '\u{2026}' => b'.',                           // ellipsis
        '\u{00A0}' => b' ',                           // non-breaking space
        '\u{2022}' => glyph::BULLET,
        '\u{2192}' => glyph::ARROW_RIGHT,
        '\u{2190}' => glyph::ARROW_LEFT,
        '\u{2191}' => glyph::ARROW_UP,
        '\u{2193}' => glyph::ARROW_DOWN,
        '\u{2588}' => glyph::BLOCK,
        '\u{2592}' => glyph::HALF_BLOCK,
        _ => {
            let value = ch as u32;
            if value < 256 {
                value as u8
            } else {
                // Anything else renders as the missing-glyph box, which is
                // deliberately visible.
                0x7F
            }
        }
    }
}

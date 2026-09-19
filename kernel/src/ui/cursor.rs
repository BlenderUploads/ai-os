//! The pointer, drawn rather than stored as an image.

use crate::gfx::draw::Surface;
use crate::gfx::palette;

/// Classic arrow, one row per line: '#' outline, '.' fill, ' ' transparent.
const ARROW: [&str; 17] = [
    "#         ",
    "##        ",
    "#.#       ",
    "#..#      ",
    "#...#     ",
    "#....#    ",
    "#.....#   ",
    "#......#  ",
    "#.......# ",
    "#........#",
    "#.....####",
    "#..#..#   ",
    "#.# #..#  ",
    "##  #..#  ",
    "#    #..# ",
    "     #..# ",
    "      ##  ",
];

pub const WIDTH: i32 = 10;
pub const HEIGHT: i32 = 17;

pub fn draw(surface: &mut Surface, x: i32, y: i32, pressed: bool) {
    let fill = if pressed {
        palette::AMBER
    } else {
        palette::TEXT_BRIGHT
    };
    // A faint drop shadow so the pointer stays visible over bright content.
    for (row, line) in ARROW.iter().enumerate() {
        for (column, pixel) in line.bytes().enumerate() {
            if pixel != b' ' {
                surface.pixel_blend(
                    x + column as i32 + 1,
                    y + row as i32 + 1,
                    palette::BLACK,
                    90,
                );
            }
        }
    }
    for (row, line) in ARROW.iter().enumerate() {
        for (column, pixel) in line.bytes().enumerate() {
            let color = match pixel {
                b'#' => palette::BLACK,
                b'.' => fill,
                _ => continue,
            };
            surface.pixel(x + column as i32, y + row as i32, color);
        }
    }
}

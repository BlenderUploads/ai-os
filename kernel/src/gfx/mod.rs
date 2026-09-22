//! Graphics: framebuffer, surfaces, font, and the CRT presentation pass.

pub mod crt;
pub mod draw;
pub mod fb;
pub mod font;
mod font_data;
pub mod modeset;
pub mod palette;

/// Change the screen resolution.
///
/// Two halves: the adapter is told to switch, then the framebuffer is retuned
/// to match what it actually did. The compositor notices the new dimensions on
/// its next frame and rebuilds itself around them.
pub fn set_resolution(width: u32, height: u32) -> Result<(), &'static str> {
    let (width, height, pitch) = modeset::set_mode(width, height)?;
    fb::retune(width as usize, height as usize, pitch as usize);
    Ok(())
}

//! The hardware framebuffer and its back buffer.
//!
//! Everything draws into a RAM back buffer; `present` pushes it to the real
//! framebuffer in one pass. That keeps tearing down and, more importantly,
//! means the compositor never does a read-modify-write against uncached video
//! memory, which is ruinously slow on real hardware.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::boot::{FramebufferInfo, FramebufferKind};
use crate::sync::SpinLock;

use super::draw::Surface;
use super::palette::Color;

static READY: AtomicBool = AtomicBool::new(false);

pub struct Framebuffer {
    hardware: *mut u8,
    pitch: usize,
    bytes_per_pixel: usize,
    pub width: usize,
    pub height: usize,
    red_shift: u8,
    green_shift: u8,
    blue_shift: u8,
    /// True when the hardware layout is exactly 0x00RRGGBB, so presenting is a
    /// straight row copy.
    fast_path: bool,
    pub back: Surface,
    pub frames_presented: u64,
}

unsafe impl Send for Framebuffer {}

impl Framebuffer {
    const fn placeholder() -> Self {
        Self {
            hardware: core::ptr::null_mut(),
            pitch: 0,
            bytes_per_pixel: 0,
            width: 0,
            height: 0,
            red_shift: 16,
            green_shift: 8,
            blue_shift: 0,
            fast_path: false,
            back: Surface::empty(),
            frames_presented: 0,
        }
    }

    /// Push the back buffer to the screen.
    pub fn present(&mut self) {
        if self.hardware.is_null() {
            return;
        }
        self.frames_presented += 1;

        if self.fast_path {
            // When the scanline pitch is exactly the visible width there is no
            // padding between rows, so the whole framebuffer is one contiguous
            // run and a single copy beats 768 separate ones.
            if self.pitch == self.width * 4 {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        self.back.pixels.as_ptr(),
                        self.hardware as *mut u32,
                        self.width * self.height,
                    );
                }
                return;
            }
            for y in 0..self.height {
                unsafe {
                    let dst = self.hardware.add(y * self.pitch) as *mut u32;
                    let src = self.back.pixels.as_ptr().add(y * self.width);
                    core::ptr::copy_nonoverlapping(src, dst, self.width);
                }
            }
            return;
        }

        for y in 0..self.height {
            for x in 0..self.width {
                let color = self.back.pixels[y * self.width + x];
                let encoded = self.encode(color);
                unsafe {
                    let dst = self.hardware.add(y * self.pitch + x * self.bytes_per_pixel);
                    match self.bytes_per_pixel {
                        4 => (dst as *mut u32).write_volatile(encoded),
                        3 => {
                            dst.write_volatile(encoded as u8);
                            dst.add(1).write_volatile((encoded >> 8) as u8);
                            dst.add(2).write_volatile((encoded >> 16) as u8);
                        }
                        2 => (dst as *mut u16).write_volatile(encoded as u16),
                        _ => {}
                    }
                }
            }
        }
    }

    /// Push one rectangle of the back buffer to the screen.
    pub fn present_rect(&mut self, rect: super::draw::Rect) {
        if self.hardware.is_null() {
            return;
        }
        let area = rect.intersect(&super::draw::Rect::new(
            0,
            0,
            self.width as i32,
            self.height as i32,
        ));
        if area.is_empty() {
            return;
        }
        self.frames_presented += 1;

        if self.fast_path {
            for y in area.y..area.bottom() {
                unsafe {
                    let dst = self
                        .hardware
                        .add(y as usize * self.pitch + area.x as usize * 4)
                        as *mut u32;
                    let src = self
                        .back
                        .pixels
                        .as_ptr()
                        .add(y as usize * self.width + area.x as usize);
                    core::ptr::copy_nonoverlapping(src, dst, area.w as usize);
                }
            }
            return;
        }

        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let color = self.back.pixels[y as usize * self.width + x as usize];
                let encoded = self.encode(color);
                unsafe {
                    let dst = self
                        .hardware
                        .add(y as usize * self.pitch + x as usize * self.bytes_per_pixel);
                    match self.bytes_per_pixel {
                        4 => (dst as *mut u32).write_volatile(encoded),
                        3 => {
                            dst.write_volatile(encoded as u8);
                            dst.add(1).write_volatile((encoded >> 8) as u8);
                            dst.add(2).write_volatile((encoded >> 16) as u8);
                        }
                        2 => (dst as *mut u16).write_volatile(encoded as u16),
                        _ => {}
                    }
                }
            }
        }
    }

    /// Repack an XRGB colour into whatever layout the hardware wants.
    #[inline]
    fn encode(&self, color: Color) -> u32 {
        let r = (color >> 16) & 0xFF;
        let g = (color >> 8) & 0xFF;
        let b = color & 0xFF;
        (r << self.red_shift) | (g << self.green_shift) | (b << self.blue_shift)
    }

    pub fn describe(&self) -> (usize, usize, usize, bool) {
        (
            self.width,
            self.height,
            self.bytes_per_pixel * 8,
            self.fast_path,
        )
    }
}

pub static FRAMEBUFFER: SpinLock<Framebuffer> = SpinLock::new(Framebuffer::placeholder());

#[derive(Debug)]
pub enum InitError {
    NoFramebuffer,
    TextMode,
    Unsupported,
}

/// Adopt the framebuffer the bootloader handed us, mapped at `virt`.
pub fn init(info: &FramebufferInfo, virt: u64) -> Result<(), InitError> {
    if virt == 0 || info.addr == 0 {
        return Err(InitError::NoFramebuffer);
    }
    if info.kind == FramebufferKind::EgaText {
        return Err(InitError::TextMode);
    }
    if info.kind != FramebufferKind::Rgb || info.bpp < 15 {
        return Err(InitError::Unsupported);
    }

    let bytes_per_pixel = (info.bpp as usize + 7) / 8;
    let width = info.width as usize;
    let height = info.height as usize;

    let fast_path = info.bpp == 32
        && info.red_shift == 16
        && info.green_shift == 8
        && info.blue_shift == 0
        && info.red_size == 8
        && info.green_size == 8
        && info.blue_size == 8;

    let mut framebuffer = FRAMEBUFFER.lock();
    framebuffer.hardware = virt as *mut u8;
    framebuffer.pitch = info.pitch as usize;
    framebuffer.bytes_per_pixel = bytes_per_pixel;
    framebuffer.width = width;
    framebuffer.height = height;
    framebuffer.red_shift = info.red_shift;
    framebuffer.green_shift = info.green_shift;
    framebuffer.blue_shift = info.blue_shift;
    framebuffer.fast_path = fast_path;
    framebuffer.back = Surface::new(width, height);
    drop(framebuffer);

    READY.store(true, Ordering::Release);
    Ok(())
}

/// Adopt a new geometry on the same physical framebuffer.
///
/// Only the resolution changes; the adapter's linear framebuffer stays where
/// it is, so the mapping set up at boot is still the right one and only the
/// numbers the compositor works from need replacing. The caller is responsible
/// for having checked the new mode fits inside the mapped window.
pub fn retune(width: usize, height: usize, pitch: usize) {
    let mut framebuffer = FRAMEBUFFER.lock();
    if framebuffer.hardware.is_null() {
        return;
    }
    framebuffer.width = width;
    framebuffer.height = height;
    framebuffer.pitch = pitch;
    framebuffer.bytes_per_pixel = 4;
    // Mode setting always asks for 0x00RRGGBB, and refuses anything else.
    framebuffer.red_shift = 16;
    framebuffer.green_shift = 8;
    framebuffer.blue_shift = 0;
    framebuffer.fast_path = true;
    framebuffer.back = Surface::new(width, height);
    drop(framebuffer);
    // Whatever was on screen belongs to the old mode; blank it so a partial
    // repaint cannot leave stripes of the previous resolution behind.
    FRAMEBUFFER.lock().present();
}

#[inline]
pub fn is_ready() -> bool {
    READY.load(Ordering::Acquire)
}

pub fn dimensions() -> (usize, usize) {
    let framebuffer = FRAMEBUFFER.lock();
    (framebuffer.width, framebuffer.height)
}

/// Draw with the back buffer, then push it to the screen.
pub fn with_screen<F: FnOnce(&mut Surface)>(f: F) {
    let mut framebuffer = FRAMEBUFFER.lock();
    f(&mut framebuffer.back);
    framebuffer.present();
}

/// Draw without presenting — for callers that batch several passes.
pub fn with_back<F: FnOnce(&mut Surface)>(f: F) {
    let mut framebuffer = FRAMEBUFFER.lock();
    f(&mut framebuffer.back);
}

pub fn present() {
    FRAMEBUFFER.lock().present();
}

/// Push only part of the back buffer. Used by the compositor's damage
/// tracking, which is what makes moving the pointer nearly free.
pub fn present_rect(rect: super::draw::Rect) {
    FRAMEBUFFER.lock().present_rect(rect);
}

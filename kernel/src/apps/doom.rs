//! DOOM.
//!
//! The engine is the unmodified doomgeneric fork of id Software's release,
//! vendored under `doom/src` and compiled freestanding against the small C
//! library in `doom/libc.c` and the sound module in `doom/sound.c`. Everything
//! below is the other side of that bridge: the `hal_*` hooks those two call
//! into, the six `DG_*` functions doomgeneric expects a platform to supply,
//! and the window that shows the result.
//!
//! The game runs in its own kernel thread. `doomgeneric_Tick` blocks
//! internally, so driving it from the compositor would stall every other
//! window on a slow frame. That split is also why pointer movement is queued
//! here rather than posted straight into the engine: `D_PostEvent` belongs to
//! the game thread, and the compositor is a different one.

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::pit;
use crate::fs;
use crate::gfx::draw::{Rect, Surface};
use crate::gfx::font::Weight;
use crate::gfx::palette;
use crate::input::{Key, KeyEvent};
use crate::sync::SpinLock;
use crate::task;
use crate::ui::window::{App, AppResponse, WindowMouse};

/// doomgeneric's fixed output size.
pub const DOOM_WIDTH: usize = 640;
pub const DOOM_HEIGHT: usize = 400;

/// Where the IWAD is expected to live in HALCYON's filesystem.
const IWAD_PATH: &str = "/doom.wad";

extern "C" {
    fn doomgeneric_Create(argc: i32, argv: *mut *mut u8);
    fn doomgeneric_Tick();
    fn D_PostEvent(event: *mut DoomEvent);
    static mut DG_ScreenBuffer: *mut u32;
}

/// doomgeneric has no mouse hook, so pointer movement goes in through the
/// engine's own event queue. This must match `event_t` in `doom/src/d_event.h`.
#[repr(C)]
struct DoomEvent {
    kind: i32,
    data1: i32,
    data2: i32,
    data3: i32,
    data4: i32,
}

/// `ev_mouse`, the third entry of `evtype_t`.
const EV_MOUSE: i32 = 2;

struct Shared {
    /// The most recent completed frame, copied out of DOOM's own buffer.
    frame: Vec<u32>,
    frame_ready: bool,
    frames: u64,
    /// (pressed, DOOM key code)
    keys: VecDeque<(bool, u8)>,
    /// (button bitfield, accumulated horizontal movement). Posting these from
    /// the compositor thread would race the engine's event queue, so they wait
    /// here and the game thread drains them between frames.
    mouse: VecDeque<(i32, i32)>,
    title: String,
    failure: Option<String>,
    started: bool,
}

impl Shared {
    const fn new() -> Self {
        Self {
            frame: Vec::new(),
            frame_ready: false,
            frames: 0,
            keys: VecDeque::new(),
            mouse: VecDeque::new(),
            title: String::new(),
            failure: None,
            started: false,
        }
    }
}

static SHARED: SpinLock<Shared> = SpinLock::new(Shared::new());
static THREAD_RUNNING: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------
// Hooks called by doom/libc.c
// ---------------------------------------------------------------------

/// Allocation. The C side keeps its own size header, because Rust's allocator
/// wants the layout back at free time and C's `free` does not supply one.
#[no_mangle]
pub extern "C" fn hal_alloc(size: usize, align: usize) -> *mut u8 {
    let Ok(layout) = Layout::from_size_align(size.max(1), align.max(16)) else {
        return core::ptr::null_mut();
    };
    unsafe { crate::mm::heap::ALLOCATOR.alloc(layout) }
}

#[no_mangle]
pub extern "C" fn hal_release(pointer: *mut u8, size: usize, align: usize) {
    if pointer.is_null() {
        return;
    }
    let Ok(layout) = Layout::from_size_align(size.max(1), align.max(16)) else {
        return;
    };
    unsafe { crate::mm::heap::ALLOCATOR.dealloc(pointer, layout) }
}

#[no_mangle]
pub extern "C" fn hal_log(text: *const u8, length: usize) {
    if text.is_null() || length == 0 {
        return;
    }
    let bytes = unsafe { core::slice::from_raw_parts(text, length) };
    if let Ok(text) = core::str::from_utf8(bytes) {
        crate::serial_print!("{}", text);
    }
}

unsafe fn c_str(pointer: *const u8) -> String {
    if pointer.is_null() {
        return String::new();
    }
    let mut length = 0;
    while *pointer.add(length) != 0 && length < 4096 {
        length += 1;
    }
    String::from_utf8_lossy(core::slice::from_raw_parts(pointer, length)).into_owned()
}

/// Hand DOOM a pointer straight into the file's bytes.
///
/// The IWAD is close to 30 MB; copying it on every open would be absurd, and
/// the filesystem's storage is stable for the machine's lifetime.
#[no_mangle]
pub extern "C" fn hal_file_open(path: *const u8, length: *mut usize) -> *const u8 {
    let path = unsafe { c_str(path) };
    let filesystem = fs::FS.lock();
    match filesystem.read(&path) {
        Some(file) => {
            let bytes = file.bytes();
            unsafe { *length = bytes.len() };
            bytes.as_ptr()
        }
        None => {
            unsafe { *length = 0 };
            core::ptr::null()
        }
    }
}

#[no_mangle]
pub extern "C" fn hal_file_store(path: *const u8, data: *const u8, length: usize) -> i32 {
    let path = unsafe { c_str(path) };
    let bytes = if data.is_null() || length == 0 {
        Vec::new()
    } else {
        unsafe { core::slice::from_raw_parts(data, length) }.to_vec()
    };
    let mut filesystem = fs::FS.lock();
    match filesystem.write(&path, bytes) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

#[no_mangle]
pub extern "C" fn hal_file_exists(path: *const u8) -> i32 {
    let path = unsafe { c_str(path) };
    let filesystem = fs::FS.lock();
    if filesystem.exists(&path) {
        1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn hal_ticks_ms() -> u32 {
    pit::uptime_ms() as u32
}

/// The rate the mixer in `doom/sound.c` must produce, or zero when this
/// machine has no audio device at all — which is how that file decides whether
/// to register a sound module in the first place.
#[no_mangle]
pub extern "C" fn hal_audio_rate() -> u32 {
    if crate::audio::present() {
        crate::audio::SAMPLE_RATE
    } else {
        0
    }
}

/// Stereo frames already queued. The mixer tops this up to its own target
/// rather than filling the ring, which is what keeps the latency down.
#[no_mangle]
pub extern "C" fn hal_audio_queued() -> u32 {
    crate::audio::queued_frames() as u32
}

/// Queue interleaved stereo frames; returns how many were accepted.
#[no_mangle]
pub extern "C" fn hal_audio_write(frames: *const i16, count: u32) -> u32 {
    if frames.is_null() || count == 0 {
        return 0;
    }
    let samples = unsafe { core::slice::from_raw_parts(frames, count as usize * 2) };
    crate::audio::write(samples) as u32
}

/// DOOM's `I_Error` path ends here. It must not return, and it must not take
/// the rest of the machine down: the message is handed to the window and the
/// game thread retires.
#[no_mangle]
pub extern "C" fn hal_panic(message: *const u8) -> ! {
    let message = unsafe { c_str(message) };
    crate::serial_println!("[doom] {}", message);
    SHARED.lock().failure = Some(message);
    THREAD_RUNNING.store(false, Ordering::Release);
    task::exit_current()
}

// ---------------------------------------------------------------------
// The six functions doomgeneric asks a platform to implement
// ---------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn DG_Init() {
    let mut shared = SHARED.lock();
    shared.frame = vec![0u32; DOOM_WIDTH * DOOM_HEIGHT];
    shared.started = true;
}

#[no_mangle]
pub extern "C" fn DG_DrawFrame() {
    let source = unsafe { DG_ScreenBuffer };
    if source.is_null() {
        return;
    }
    let pixels = unsafe { core::slice::from_raw_parts(source, DOOM_WIDTH * DOOM_HEIGHT) };
    let mut shared = SHARED.lock();
    if shared.frame.len() != pixels.len() {
        shared.frame = vec![0u32; pixels.len()];
    }
    shared.frame.copy_from_slice(pixels);
    shared.frame_ready = true;
    shared.frames += 1;
}

#[no_mangle]
pub extern "C" fn DG_SleepMs(ms: u32) {
    task::sleep_ms(ms as u64);
}

#[no_mangle]
pub extern "C" fn DG_GetTicksMs() -> u32 {
    pit::uptime_ms() as u32
}

#[no_mangle]
pub extern "C" fn DG_GetKey(pressed: *mut i32, key: *mut u8) -> i32 {
    let mut shared = SHARED.lock();
    match shared.keys.pop_front() {
        Some((down, code)) => {
            unsafe {
                *pressed = if down { 1 } else { 0 };
                *key = code;
            }
            1
        }
        None => 0,
    }
}

#[no_mangle]
pub extern "C" fn DG_SetWindowTitle(title: *const u8) {
    let title = unsafe { c_str(title) };
    SHARED.lock().title = title;
}

// ---------------------------------------------------------------------
// The game thread
// ---------------------------------------------------------------------

extern "C" fn doom_thread(_: u64) {
    // argv has to outlive the call, and DOOM keeps the pointers.
    let mut program = b"halcyon-doom\0".to_vec();
    let mut flag = b"-iwad\0".to_vec();
    let mut path = format!("{}\0", IWAD_PATH).into_bytes();
    let mut argv: Vec<*mut u8> = vec![
        program.as_mut_ptr(),
        flag.as_mut_ptr(),
        path.as_mut_ptr(),
        core::ptr::null_mut(),
    ];
    core::mem::forget(program);
    core::mem::forget(flag);
    core::mem::forget(path);

    crate::serial_println!("[doom] starting, iwad {}", IWAD_PATH);
    unsafe {
        doomgeneric_Create(3, argv.as_mut_ptr());
    }
    core::mem::forget(argv);

    crate::serial_println!("[doom] DOOM-RUNNING");
    while THREAD_RUNNING.load(Ordering::Acquire) {
        drain_mouse();
        unsafe { doomgeneric_Tick() };
    }
    task::exit_current()
}

/// Hand the queued pointer movement to the engine. Called only from the game
/// thread, which is the only thread allowed near `D_PostEvent`.
fn drain_mouse() {
    loop {
        let Some((buttons, dx)) = SHARED.lock().mouse.pop_front() else {
            break;
        };
        let mut event = DoomEvent {
            kind: EV_MOUSE,
            data1: buttons,
            data2: dx,
            // Vanilla DOOM walks forward and back on the mouse's Y axis, since
            // it has no vertical aiming to spend it on. That surprises anyone
            // who has used a mouse since 1993, so it is left at rest.
            data3: 0,
            data4: 0,
        };
        unsafe { D_PostEvent(&mut event) };
    }
}

/// Queue one pointer event, folding pure movement into the pending entry so a
/// fast sweep cannot flood the queue while a frame is being drawn.
fn push_mouse(buttons: i32, dx: i32) {
    let mut shared = SHARED.lock();
    if let Some(last) = shared.mouse.back_mut() {
        if last.0 == buttons {
            last.1 += dx;
            return;
        }
    }
    if shared.mouse.len() < 64 {
        shared.mouse.push_back((buttons, dx));
    }
}

/// Translate a HALCYON key event into DOOM's own key numbering.
fn doom_key(event: &KeyEvent) -> Option<u8> {
    let code = match event.key {
        Key::Left => 0xAC,
        Key::Right => 0xAE,
        Key::Up => 0xAD,
        Key::Down => 0xAF,
        Key::Enter => 13,
        Key::Escape => 27,
        Key::Tab => 9,
        // DOOM's defaults: ctrl fires, alt strafes, shift runs.
        Key::Control => 0xA3,
        Key::Alt => 0xB8,
        Key::Shift => 0xB6,
        Key::Backspace => 0x7F,
        Key::F1 => 0x80 + 0x3B,
        Key::F2 => 0x80 + 0x3C,
        Key::F3 => 0x80 + 0x3D,
        Key::F4 => 0x80 + 0x3E,
        Key::F5 => 0x80 + 0x3F,
        Key::F6 => 0x80 + 0x40,
        Key::F7 => 0x80 + 0x41,
        Key::F8 => 0x80 + 0x42,
        Key::F9 => 0x80 + 0x43,
        Key::F10 => 0x80 + 0x44,
        Key::F12 => 0x80 + 0x58,
        Key::Home => 0x80 + 0x47,
        Key::End => 0x80 + 0x4F,
        Key::PageUp => 0x80 + 0x49,
        Key::PageDown => 0x80 + 0x51,
        Key::Insert => 0x80 + 0x52,
        Key::Delete => 0x80 + 0x53,
        Key::Char => {
            let character = event.character?;
            return Some(match character {
                // Space is use, ctrl is fire; both also have their own keys.
                ' ' => 0xA2,
                other => (other as u32 & 0xFF) as u8,
            });
        }
        _ => return None,
    };
    Some(code as u8)
}

pub struct Doom {
    dirty: bool,
    last_frame: u64,
    /// Scratch copy so the surface blit does not hold the shared lock.
    scratch: Vec<u32>,
    missing_wad: bool,
    /// True while this window owns the pointer and is being sent raw deltas.
    grabbed: bool,
    /// Show the pointer-capture hint until this many milliseconds of uptime.
    hint_until: u64,
}

impl Doom {
    pub fn new() -> Self {
        let missing_wad = !fs::FS.lock().exists(IWAD_PATH);
        if !missing_wad && !THREAD_RUNNING.swap(true, Ordering::AcqRel) {
            task::spawn_with_stack("doom", doom_thread, 0, 512 * 1024);
        }
        Self {
            dirty: true,
            last_frame: 0,
            scratch: Vec::new(),
            missing_wad,
            grabbed: false,
            hint_until: pit::uptime_ms() + 8000,
        }
    }

    fn draw_missing_wad(&self, surface: &mut Surface) {
        surface.clear(palette::rgb(0x0A, 0x04, 0x04));
        let width = surface.width as i32;
        surface.text_centered("NO IWAD FOUND", width / 2, 40, palette::ERROR, Weight::Bold);
        let lines = [
            "DOOM needs a WAD file, and none was found at /doom.wad.",
            "",
            "Build an ISO that includes one with:",
            "    make doom-iso",
            "",
            "That fetches Freedoom, which is BSD-licensed and freely",
            "redistributable. Your own doom1.wad or doom.wad works too:",
            "drop it in as doom.wad before building.",
        ];
        let mut y = 76;
        for line in lines {
            surface.text(line, 20, y, palette::TEXT_DIM);
            y += 18;
        }
    }
}

impl App for Doom {
    fn draw(&mut self, surface: &mut Surface, _focused: bool) {
        if self.missing_wad {
            self.draw_missing_wad(surface);
            self.dirty = false;
            return;
        }

        let (ready, failure) = {
            let mut shared = SHARED.lock();
            let failure = shared.failure.clone();
            let ready = if shared.frame_ready && shared.frames != self.last_frame {
                self.last_frame = shared.frames;
                if self.scratch.len() != shared.frame.len() {
                    self.scratch = vec![0u32; shared.frame.len()];
                }
                self.scratch.copy_from_slice(&shared.frame);
                shared.frame_ready = false;
                true
            } else {
                !self.scratch.is_empty()
            };
            (ready, failure)
        };

        surface.clear(palette::BLACK);

        if let Some(message) = failure {
            surface.text_centered(
                "DOOM STOPPED",
                surface.width as i32 / 2,
                30,
                palette::ERROR,
                Weight::Bold,
            );
            surface.text(&message, 16, 60, palette::TEXT);
            self.dirty = false;
            return;
        }

        if !ready {
            surface.text_centered(
                "loading DOOM...",
                surface.width as i32 / 2,
                surface.height as i32 / 2,
                palette::AMBER,
                Weight::Bold,
            );
            return;
        }

        // Integer-scale to fit, so the pixels stay square and sharp.
        let width = surface.width as i32;
        let height = surface.height as i32;
        let scale = (width / DOOM_WIDTH as i32)
            .min(height / DOOM_HEIGHT as i32)
            .max(1);
        let drawn_w = DOOM_WIDTH as i32 * scale;
        let drawn_h = DOOM_HEIGHT as i32 * scale;
        let origin_x = (width - drawn_w) / 2;
        let origin_y = (height - drawn_h) / 2;

        if scale == 1 {
            for row in 0..DOOM_HEIGHT as i32 {
                let target_y = origin_y + row;
                if target_y < 0 || target_y >= height {
                    continue;
                }
                for column in 0..DOOM_WIDTH as i32 {
                    let target_x = origin_x + column;
                    if target_x < 0 || target_x >= width {
                        continue;
                    }
                    let pixel = self.scratch[row as usize * DOOM_WIDTH + column as usize];
                    surface.pixel(target_x, target_y, pixel & 0x00FF_FFFF);
                }
            }
        } else {
            for row in 0..DOOM_HEIGHT as i32 {
                for column in 0..DOOM_WIDTH as i32 {
                    let pixel = self.scratch[row as usize * DOOM_WIDTH + column as usize];
                    surface.fill_rect(
                        Rect::new(
                            origin_x + column * scale,
                            origin_y + row * scale,
                            scale,
                            scale,
                        ),
                        pixel & 0x00FF_FFFF,
                    );
                }
            }
        }

        // The mouse is worth mentioning, but not forever: the hint shows for a
        // few seconds when the window opens and again whenever the pointer is
        // handed back, then gets out of the way.
        if !self.grabbed && pit::uptime_ms() < self.hint_until {
            let strip = Rect::new(0, height - 20, width, 20);
            surface.fill_rect(strip, palette::rgb(0x0A, 0x0E, 0x18));
            surface.hline(0, height - 20, width, palette::BORDER);
            surface.text_centered(
                "click to aim with the mouse  —  ctrl+G gives it back",
                width / 2,
                height - 15,
                palette::AMBER_DIM,
                Weight::Regular,
            );
        }
    }

    fn on_key(&mut self, event: &KeyEvent, response: &mut AppResponse) {
        if self.missing_wad {
            return;
        }
        // F11 belongs to the window manager, not to DOOM.
        if event.key == Key::F11 {
            return;
        }
        if let Some(code) = doom_key(event) {
            let mut shared = SHARED.lock();
            // Bound the queue: a key held during a long frame must not grow it
            // without limit.
            if shared.keys.len() < 64 {
                shared.keys.push_back((event.pressed, code));
            }
        }
        let _ = response;
    }

    fn on_mouse(&mut self, event: &WindowMouse, response: &mut AppResponse) {
        if self.missing_wad {
            return;
        }
        if !self.grabbed {
            // A click asks the compositor for the pointer. Until it says yes,
            // the cursor belongs to the desktop and DOOM sees nothing.
            if event.pressed && event.left {
                response.grab_pointer = Some(true);
            }
            return;
        }
        // Bit 0 fire, bit 1 strafe, the same order DOOM reads them in.
        let buttons = event.left as i32 | ((event.right as i32) << 1);
        push_mouse(buttons, event.dx);
    }

    fn on_pointer_grab(&mut self, grabbed: bool) {
        self.grabbed = grabbed;
        self.dirty = true;
        self.hint_until = if grabbed { 0 } else { pit::uptime_ms() + 5000 };
        if !grabbed {
            // Releasing mid-sweep would otherwise leave the player turning.
            SHARED.lock().mouse.clear();
        }
    }

    fn tick(&mut self, _now_ms: u64, response: &mut AppResponse) {
        let shared = SHARED.lock();
        if shared.frames != self.last_frame || shared.failure.is_some() {
            self.dirty = true;
        }
        if !shared.title.is_empty() {
            response.retitle = Some(shared.title.clone());
        }
    }

    fn dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn min_size(&self) -> (i32, i32) {
        (DOOM_WIDTH as i32, DOOM_HEIGHT as i32)
    }
}

/// Is a playable IWAD present?
pub fn wad_present() -> bool {
    fs::FS.lock().exists(IWAD_PATH)
}

pub fn status() -> String {
    let shared = SHARED.lock();
    if let Some(failure) = &shared.failure {
        return format!("doom: stopped ({})", failure);
    }
    if !shared.started {
        return "doom: not started".to_string();
    }
    format!("doom: {} frames rendered", shared.frames)
}

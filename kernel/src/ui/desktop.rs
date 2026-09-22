//! The compositor and window manager.
//!
//! One thread owns the whole desktop: it drains the input queues, updates
//! window state, repaints any window whose app is dirty, and composites the
//! screen. Windows are kept bottom-to-top in `windows`, so the last entry is
//! the one on top and hit testing walks the list backwards.
//!
//! Composition is skipped entirely when nothing has changed. That is what
//! keeps an idle desktop nearly free: without it, a machine showing a static
//! screen still burned a full-screen blit, an alpha pass per window shadow and
//! a 3 MB copy to video memory thirty times a second.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::arch::{cpu, pit};
use crate::drivers::rtc;
use crate::gfx::crt;
use crate::gfx::draw::{Rect, Surface};
use crate::gfx::fb;
use crate::gfx::font::{glyph, Weight};
use crate::gfx::palette;
use crate::input::{self, Key, KeyEvent};

use super::cursor;
use super::menu::{self, Action, Menu};
use super::theme::{self, Edges};
use super::window::{App, AppResponse, Window, WindowMouse};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    None,
    Move {
        id: u64,
        dx: i32,
        dy: i32,
    },
    Resize {
        id: u64,
        edges: Edges,
        origin: Rect,
        grab_x: i32,
        grab_y: i32,
    },
}

/// Where a drag would snap the window if released now.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Snap {
    None,
    Maximise,
    Left,
    Right,
}

/// One entry in the launcher.
pub struct AppEntry {
    pub name: &'static str,
    pub title: &'static str,
    pub icon: u8,
    pub width: i32,
    pub height: i32,
    pub build: fn() -> Box<dyn App>,
}

/// The regions that changed since the last composite.
///
/// Everything in the paint path already honours the surface clip, so a partial
/// redraw is the same code with a smaller clip — and presenting only those
/// rows is what takes pointer movement from a 3 MB copy to a few kilobytes.
struct Damage {
    rects: Vec<Rect>,
    full: bool,
}

/// Past this many regions it is cheaper to merge them than to run the paint
/// pipeline again.
const MAX_DAMAGE_RECTS: usize = 4;

/// The screen every app's default window size was chosen against.
const REFERENCE_WIDTH: i32 = 1024;
const REFERENCE_HEIGHT: i32 = 740;

impl Damage {
    fn new() -> Self {
        Self {
            rects: Vec::new(),
            full: true,
        }
    }

    fn is_empty(&self) -> bool {
        !self.full && self.rects.is_empty()
    }

    fn mark_full(&mut self) {
        self.full = true;
        self.rects.clear();
    }

    fn add(&mut self, rect: Rect) {
        if self.full || rect.is_empty() {
            return;
        }
        // A pixel of slack absorbs the shadow and border bleed around a region.
        let rect = rect.inset(-2);
        for existing in self.rects.iter_mut() {
            if existing.intersect(&rect).is_empty() {
                continue;
            }
            *existing = union(*existing, rect);
            return;
        }
        self.rects.push(rect);
        if self.rects.len() > MAX_DAMAGE_RECTS {
            let merged = self
                .rects
                .iter()
                .copied()
                .reduce(union)
                .unwrap_or(Rect::EMPTY);
            self.rects.clear();
            self.rects.push(merged);
        }
    }

    fn clear(&mut self) {
        self.full = false;
        self.rects.clear();
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    Rect::new(
        x,
        y,
        a.right().max(b.right()) - x,
        a.bottom().max(b.bottom()) - y,
    )
}

/// Rolling frame-time history, so the taskbar can show an honest rate.
struct FrameClock {
    last_tsc: u64,
    /// Nanoseconds-ish per frame, smoothed.
    smoothed_us: u64,
    tsc_per_us: u64,
    composed: u64,
    skipped: u64,
}

impl FrameClock {
    fn new() -> Self {
        Self {
            last_tsc: 0,
            smoothed_us: 0,
            tsc_per_us: 0,
            composed: 0,
            skipped: 0,
        }
    }

    /// Calibrate the timestamp counter against the PIT, which we already trust.
    fn calibrate(&mut self) {
        let start_tick = pit::ticks();
        while pit::ticks() == start_tick {
            core::hint::spin_loop();
        }
        let start = cpu::rdtsc();
        let target = pit::ticks() + 50;
        while pit::ticks() < target {
            core::hint::spin_loop();
        }
        let elapsed = cpu::rdtsc() - start;
        // 50 ticks at 1 kHz is 50_000 microseconds.
        self.tsc_per_us = (elapsed / 50_000).max(1);
    }

    fn begin(&mut self) {
        self.last_tsc = cpu::rdtsc();
    }

    fn end(&mut self) {
        if self.tsc_per_us == 0 {
            return;
        }
        let elapsed = cpu::rdtsc().saturating_sub(self.last_tsc) / self.tsc_per_us;
        self.smoothed_us = if self.smoothed_us == 0 {
            elapsed
        } else {
            // Exponential moving average; cheap and steady enough to read.
            (self.smoothed_us * 7 + elapsed) / 8
        };
        self.composed += 1;
    }

    fn frames_per_second(&self) -> u64 {
        if self.smoothed_us == 0 {
            0
        } else {
            1_000_000 / self.smoothed_us
        }
    }

    fn millis(&self) -> u64 {
        self.smoothed_us / 1000
    }
}

pub struct Desktop {
    windows: Vec<Window>,
    next_id: u64,
    focused: Option<u64>,
    cursor_x: i32,
    cursor_y: i32,
    previous_cursor: (i32, i32),
    left_down: bool,
    right_down: bool,
    drag: Drag,
    snap: Snap,
    backdrop: Surface,
    width: i32,
    height: i32,
    registry: Vec<AppEntry>,
    menu: Option<Menu>,
    /// Cascade offset for the next window opened.
    spawn_offset: i32,
    pub frames: u64,
    status: String,
    status_until: u64,
    /// Regions that changed since the last composite.
    damage: Damage,
    /// Last wall-clock second painted into the taskbar.
    last_second: u8,
    clock: FrameClock,
    show_fps: bool,
    last_click: u64,
    last_click_pos: (i32, i32),
    /// The window holding the pointer, if any. While something holds it the
    /// cursor freezes, stops being drawn, and raw deltas go to that window.
    grab: Option<u64>,
}

impl Desktop {
    pub fn new(registry: Vec<AppEntry>) -> Self {
        let (width, height) = fb::dimensions();
        let (width, height) = (width as i32, height as i32);

        // The backdrop is static, so it is drawn once and blitted each frame
        // rather than regenerated. The vignette is baked in here for the same
        // reason: it is far too expensive to run per frame.
        let mut backdrop = Surface::new(width as usize, height as usize);
        crt::draw_backdrop(&mut backdrop);
        crt::apply_vignette(&mut backdrop);

        let mut clock = FrameClock::new();
        clock.calibrate();

        Self {
            windows: Vec::new(),
            next_id: 1,
            focused: None,
            cursor_x: width / 2,
            cursor_y: height / 2,
            previous_cursor: (width / 2, height / 2),
            left_down: false,
            right_down: false,
            drag: Drag::None,
            snap: Snap::None,
            backdrop,
            width,
            height,
            registry,
            menu: None,
            spawn_offset: 0,
            frames: 0,
            status: String::new(),
            status_until: 0,
            damage: Damage::new(),
            last_second: 0xFF,
            clock,
            show_fps: false,
            last_click: 0,
            last_click_pos: (0, 0),
            grab: None,
        }
    }

    pub fn set_status(&mut self, text: &str, ms: u64) {
        self.status = text.to_string();
        self.status_until = pit::ticks() + ms;
        self.damage.mark_full();
    }

    /// The area windows may occupy — everything above the taskbar.
    fn work_area(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height - theme::TASKBAR_HEIGHT)
    }

    /// Rebuild around a screen that changed size underneath us.
    ///
    /// The Settings app can change the resolution mid-frame, so rather than
    /// plumbing an event through every layer the compositor simply asks the
    /// framebuffer its size once a frame and reacts when the answer changes.
    /// Cheap, and it also covers anything else that ever sets a mode.
    fn follow_resolution(&mut self) {
        let (width, height) = fb::dimensions();
        let (width, height) = (width as i32, height as i32);
        if (width, height) == (self.width, self.height) || width == 0 || height == 0 {
            return;
        }
        crate::serial_println!(
            "[ui  ] RESOLUTION-CHANGED {}x{} -> {}x{}",
            self.width,
            self.height,
            width,
            height
        );
        self.width = width;
        self.height = height;

        self.backdrop = Surface::new(width as usize, height as usize);
        crt::draw_backdrop(&mut self.backdrop);
        crt::apply_vignette(&mut self.backdrop);

        // Drag state and menus refer to the old geometry, and a window that
        // was off to the right of a wider screen would be unreachable.
        self.drag = Drag::None;
        self.snap = Snap::None;
        self.menu = None;
        let work = self.work_area();
        for window in self.windows.iter_mut() {
            if window.fullscreen {
                window.frame = Rect::new(0, 0, width, height);
            } else if window.is_maximised() {
                // `restore` is where un-maximising puts it back, so it is
                // clamped rather than replaced.
                if let Some(previous) = window.restore {
                    let w = previous.w.min(width);
                    let h = previous.h.min(work.h);
                    window.restore = Some(Rect::new(
                        previous.x.clamp(0, (width - w).max(0)),
                        previous.y.clamp(0, (work.h - h).max(0)),
                        w,
                        h,
                    ));
                }
                window.frame = work;
            } else {
                let w = window.frame.w.min(width);
                let h = window.frame.h.min(work.h);
                window.frame = Rect::new(
                    window.frame.x.clamp(0, (width - w).max(0)),
                    window.frame.y.clamp(0, (work.h - h).max(0)),
                    w,
                    h,
                );
            }
            window.sync_content_size();
            window.needs_paint = true;
        }

        self.cursor_x = self.cursor_x.clamp(0, width - 1);
        self.cursor_y = self.cursor_y.clamp(0, height - 1);
        self.previous_cursor = (self.cursor_x, self.cursor_y);
        self.last_second = 0xFF;
        self.damage.mark_full();
    }

    fn screen(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    // --- window management ---

    pub fn open(&mut self, name: &str) -> Option<u64> {
        let entry = self.registry.iter().find(|entry| entry.name == name)?;
        let (title, icon, width, height, build) = (
            entry.title,
            entry.icon,
            entry.width,
            entry.height,
            entry.build,
        );

        let offset = self.spawn_offset;
        self.spawn_offset = (self.spawn_offset + 26) % 160;

        let work = self.work_area();
        // The registry's sizes were chosen against 1024x768. On a bigger
        // screen they leave windows huddled in one corner, so they grow with
        // it — but only so far, because a terminal four times the size is not
        // four times as useful.
        let scale = (work.w * 8 / REFERENCE_WIDTH)
            .min(work.h * 8 / REFERENCE_HEIGHT)
            .clamp(8, 14);
        let frame_width = (width * scale / 8).min(work.w - 40);
        let frame_height = (height * scale / 8).min(work.h - 40);
        // Cascade from a proportional origin too, so the stack sits in the
        // upper-left third of the desktop rather than pinned to the corner.
        let origin_x = (work.w / 12).clamp(40, 200);
        let origin_y = (work.h / 12).clamp(40, 160);
        let x = (origin_x + offset).min(work.w - frame_width - 8).max(0);
        let y = (origin_y + offset).min(work.h - frame_height - 8).max(0);

        // Built before the frame is settled, because an app may have an
        // opinion about its own size that the registry cannot express.
        let app = build();
        let (frame_width, frame_height) = match app.preferred_size((work.w, work.h)) {
            Some((wanted_w, wanted_h)) => (wanted_w.min(work.w), wanted_h.min(work.h)),
            None => (frame_width, frame_height),
        };
        let x = x.min(work.w - frame_width - 8).max(0);
        let y = y.min(work.h - frame_height - 8).max(0);

        let id = self.next_id;
        self.next_id += 1;
        let window = Window::new(
            id,
            title,
            icon,
            Rect::new(x, y, frame_width, frame_height),
            app,
        );
        self.windows.push(window);
        self.focused = Some(id);
        self.damage.mark_full();
        Some(id)
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.windows.iter().position(|window| window.id == id)
    }

    fn raise(&mut self, id: u64) {
        if let Some(index) = self.index_of(id) {
            if index != self.windows.len() - 1 {
                let window = self.windows.remove(index);
                self.windows.push(window);
            }
            self.focused = Some(id);
            self.damage.mark_full();
        }
    }

    fn close(&mut self, id: u64) {
        if let Some(index) = self.index_of(id) {
            self.windows.remove(index);
        }
        if self.focused == Some(id) {
            self.focused = self
                .windows
                .iter()
                .rev()
                .find(|window| !window.minimised)
                .map(|window| window.id);
        }
        self.damage.mark_full();
    }

    fn minimise(&mut self, id: u64) {
        if let Some(index) = self.index_of(id) {
            self.windows[index].minimised = true;
        }
        if self.focused == Some(id) {
            self.focused = self
                .windows
                .iter()
                .rev()
                .find(|window| !window.minimised)
                .map(|window| window.id);
        }
        self.damage.mark_full();
    }

    fn place(&mut self, id: u64, rect: Rect, remember: bool) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        if remember && self.windows[index].restore.is_none() {
            self.windows[index].restore = Some(self.windows[index].frame);
        }
        self.windows[index].frame = rect;
        self.windows[index].sync_content_size();
        self.damage.mark_full();
    }

    fn toggle_maximise(&mut self, id: u64) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        match self.windows[index].restore.take() {
            Some(previous) => {
                self.windows[index].frame = previous;
                self.windows[index].sync_content_size();
            }
            None => {
                let work = self.work_area();
                self.place(id, work, true);
            }
        }
        self.damage.mark_full();
    }

    fn set_fullscreen(&mut self, id: u64, on: bool) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        if self.windows[index].fullscreen == on {
            return;
        }
        if on {
            if self.windows[index].restore.is_none() {
                self.windows[index].restore = Some(self.windows[index].frame);
            }
            self.windows[index].fullscreen = true;
            self.windows[index].frame = self.screen();
        } else {
            self.windows[index].fullscreen = false;
            if let Some(previous) = self.windows[index].restore.take() {
                self.windows[index].frame = previous;
            }
        }
        self.windows[index].sync_content_size();
        self.raise(id);
        self.damage.mark_full();
    }

    fn snap_to(&mut self, id: u64, snap: Snap) {
        let work = self.work_area();
        let rect = match snap {
            Snap::None => return,
            Snap::Maximise => work,
            Snap::Left => Rect::new(work.x, work.y, work.w / 2, work.h),
            Snap::Right => Rect::new(work.x + work.w / 2, work.y, work.w - work.w / 2, work.h),
        };
        self.place(id, rect, true);
    }

    fn tile_all(&mut self) {
        let visible: Vec<u64> = self
            .windows
            .iter()
            .filter(|window| !window.minimised)
            .map(|window| window.id)
            .collect();
        if visible.is_empty() {
            return;
        }
        let work = self.work_area();
        // Squarish grid: as many columns as rows, give or take.
        let columns = {
            let mut c = 1;
            while c * c < visible.len() {
                c += 1;
            }
            c
        };
        let rows = visible.len().div_ceil(columns);
        for (index, id) in visible.iter().enumerate() {
            let column = (index % columns) as i32;
            let row = (index / columns) as i32;
            let cell_w = work.w / columns as i32;
            let cell_h = work.h / rows as i32;
            let rect = Rect::new(
                work.x + column * cell_w,
                work.y + row * cell_h,
                cell_w - 2,
                cell_h - 2,
            );
            if let Some(window_index) = self.index_of(*id) {
                self.windows[window_index].restore = None;
            }
            self.place(*id, rect, false);
        }
    }

    fn cascade_all(&mut self) {
        let visible: Vec<u64> = self
            .windows
            .iter()
            .filter(|window| !window.minimised)
            .map(|window| window.id)
            .collect();
        let work = self.work_area();
        for (index, id) in visible.iter().enumerate() {
            let offset = index as i32 * 26;
            let Some(window_index) = self.index_of(*id) else {
                continue;
            };
            let frame = self.windows[window_index].frame;
            let width = frame.w.min(work.w - offset - 20).max(200);
            let height = frame.h.min(work.h - offset - 20).max(140);
            self.windows[window_index].restore = None;
            self.place(
                *id,
                Rect::new(20 + offset, 20 + offset, width, height),
                false,
            );
        }
    }

    fn focus_next(&mut self, backwards: bool) {
        let visible: Vec<u64> = self
            .windows
            .iter()
            .filter(|window| !window.minimised)
            .map(|window| window.id)
            .collect();
        if visible.is_empty() {
            return;
        }
        let current = self.focused.unwrap_or(0);
        let position = visible.iter().position(|&id| id == current);
        let next = match position {
            Some(index) => {
                if backwards {
                    visible[(index + visible.len() - 1) % visible.len()]
                } else {
                    visible[(index + 1) % visible.len()]
                }
            }
            None => visible[0],
        };
        self.raise(next);
    }

    /// Topmost non-minimised window containing the point.
    fn window_at(&self, x: i32, y: i32) -> Option<u64> {
        self.windows
            .iter()
            .rev()
            .find(|window| !window.minimised && window.frame.contains(x, y))
            .map(|window| window.id)
    }

    /// Topmost window whose resize band contains the point.
    fn resize_target(&self, x: i32, y: i32) -> Option<(u64, Edges)> {
        for window in self.windows.iter().rev() {
            if window.minimised || window.fullscreen || window.is_maximised() {
                continue;
            }
            let edges = theme::resize_edges(window.frame, x, y);
            if edges.any() {
                return Some((window.id, edges));
            }
            // A window body under the pointer blocks anything beneath it.
            if window.frame.contains(x, y) {
                return None;
            }
        }
        None
    }

    // --- event handling ---

    pub fn handle_input(&mut self) {
        self.validate_grab();
        while let Some(event) = input::pop_key() {
            self.on_key(&event);
        }
        while let Some(event) = input::pop_mouse() {
            self.on_mouse(event);
        }
    }

    // --- pointer capture ---

    /// Hand the pointer to a window. The cursor stops where it is and stops
    /// being drawn, so the window has to be worth it.
    fn grab_pointer(&mut self, id: u64) {
        if self.grab == Some(id) {
            return;
        }
        self.release_pointer();
        let Some(index) = self.index_of(id) else {
            return;
        };
        self.grab = Some(id);
        self.drag = Drag::None;
        self.menu = None;
        self.windows[index].app.on_pointer_grab(true);
        self.set_status("pointer captured — ctrl+G releases it", 4000);
        crate::serial_println!("[ui  ] POINTER-GRABBED by window {}", id);
        self.damage.mark_full();
    }

    fn release_pointer(&mut self) {
        let Some(id) = self.grab.take() else {
            return;
        };
        if let Some(index) = self.index_of(id) {
            self.windows[index].app.on_pointer_grab(false);
        }
        crate::serial_println!("[ui  ] POINTER-RELEASED by window {}", id);
        self.damage.mark_full();
    }

    /// Give the pointer back the moment the window that holds it stops being
    /// the one in front, so a capture can never outlive its window.
    fn validate_grab(&mut self) {
        let Some(id) = self.grab else { return };
        let stale = match self.index_of(id) {
            Some(index) => self.windows[index].minimised || self.focused != Some(id),
            None => true,
        };
        if stale {
            self.release_pointer();
        }
    }

    /// Deliver a raw mouse packet to the window holding the pointer.
    fn forward_grabbed(&mut self, id: u64, event: &input::MouseEvent) {
        let Some(index) = self.index_of(id) else {
            self.release_pointer();
            return;
        };
        if event.left_changed {
            self.left_down = event.left;
        }
        if event.right_changed {
            self.right_down = event.right;
        }
        let content = self.windows[index].content_rect();
        let mouse = WindowMouse {
            // Frozen, but an app that also wants a position should still get a
            // sensible one rather than a stale screen coordinate.
            x: self.cursor_x - content.x,
            y: self.cursor_y - content.y,
            dx: event.dx,
            dy: event.dy,
            left: event.left,
            right: event.right,
            pressed: (event.left_changed && event.left) || (event.right_changed && event.right),
            released: (event.left_changed && !event.left) || (event.right_changed && !event.right),
            moved: event.dx != 0 || event.dy != 0,
            wheel: 0,
        };
        let mut response = AppResponse::default();
        self.windows[index].app.on_mouse(&mouse, &mut response);
        self.apply(id, response);
    }

    fn on_key(&mut self, event: &KeyEvent) {
        if !event.pressed {
            // A release is never a shortcut, so it skips all of the below and
            // goes straight to whoever has the keyboard. Without this a game
            // sees every key go down and none of them come back up.
            let target = self
                .windows
                .iter()
                .find(|window| window.fullscreen)
                .map(|window| window.id)
                .or(self.focused);
            if let Some(id) = target {
                self.deliver_key(id, event);
            }
            return;
        }
        self.damage.mark_full();

        // The escape hatch from pointer capture, above everything else: a
        // window that has the mouse must never be able to keep it.
        if self.grab.is_some() && event.modifiers.ctrl && event.character == Some('g') {
            self.release_pointer();
            self.set_status("pointer released", 2500);
            return;
        }

        // A fullscreen window gets everything except the escape hatch.
        let fullscreen_id = self
            .windows
            .iter()
            .find(|window| window.fullscreen)
            .map(|window| window.id);
        if let Some(id) = fullscreen_id {
            if event.key == Key::F11 || (event.key == Key::Escape && event.modifiers.shift) {
                self.set_fullscreen(id, false);
                return;
            }
            self.deliver_key(id, event);
            return;
        }

        if self.menu.is_some() && event.key == Key::Escape {
            self.menu = None;
            return;
        }

        if event.modifiers.alt && event.key == Key::Tab {
            self.focus_next(event.modifiers.shift);
            return;
        }

        if let Some(id) = self.focused {
            if event.modifiers.ctrl {
                if let Some(character) = event.character {
                    match character {
                        'w' => {
                            self.close(id);
                            return;
                        }
                        'm' => {
                            self.minimise(id);
                            return;
                        }
                        _ => {}
                    }
                }
            }
            // Window placement on the arrow keys, the way every desktop does it.
            if event.modifiers.alt {
                match event.key {
                    Key::Up => {
                        self.snap_to(id, Snap::Maximise);
                        return;
                    }
                    Key::Left => {
                        self.snap_to(id, Snap::Left);
                        return;
                    }
                    Key::Right => {
                        self.snap_to(id, Snap::Right);
                        return;
                    }
                    Key::Down => {
                        if self.index_of(id).map(|i| self.windows[i].is_maximised()) == Some(true) {
                            self.toggle_maximise(id);
                        } else {
                            self.minimise(id);
                        }
                        return;
                    }
                    _ => {}
                }
            }
            if event.key == Key::F11 {
                self.set_fullscreen(id, true);
                return;
            }
        }

        let launch_index = match event.key {
            Key::F1 => Some(0),
            Key::F2 => Some(1),
            Key::F3 => Some(2),
            Key::F4 => Some(3),
            Key::F5 => Some(4),
            Key::F6 => Some(5),
            Key::F7 => Some(6),
            Key::F8 => Some(7),
            Key::F9 => Some(8),
            Key::F10 => Some(9),
            _ => None,
        };
        if let Some(index) = launch_index {
            if let Some(entry) = self.registry.get(index) {
                let name = entry.name;
                self.menu = None;
                self.open(name);
            }
            return;
        }

        let Some(id) = self.focused else { return };
        self.deliver_key(id, event);
    }

    fn deliver_key(&mut self, id: u64, event: &KeyEvent) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let mut response = AppResponse::default();
        self.windows[index].app.on_key(event, &mut response);
        self.apply(id, response);
    }

    fn on_mouse(&mut self, event: input::MouseEvent) {
        if let Some(id) = self.grab {
            self.forward_grabbed(id, &event);
            return;
        }

        let previous = (self.cursor_x, self.cursor_y);
        self.cursor_x = (self.cursor_x + event.dx).clamp(0, self.width - 1);
        self.cursor_y = (self.cursor_y + event.dy).clamp(0, self.height - 1);
        if (self.cursor_x, self.cursor_y) != previous {
            // Only the pointer moved: damage where it was and where it is.
            self.damage.add(cursor::bounds(previous.0, previous.1));
            self.damage
                .add(cursor::bounds(self.cursor_x, self.cursor_y));
        }
        let (x, y) = (self.cursor_x, self.cursor_y);

        if event.right_changed {
            self.right_down = event.right;
            if event.right {
                self.on_right_press(x, y);
            }
            return;
        }

        if event.left_changed {
            self.left_down = event.left;
            if event.left {
                self.on_press(x, y);
            } else {
                self.on_release(x, y);
            }
            return;
        }

        // A drag in progress owns the pointer.
        match self.drag {
            Drag::Move { id, dx, dy } => {
                self.drag_move(id, x, y, dx, dy);
                return;
            }
            Drag::Resize {
                id,
                edges,
                origin,
                grab_x,
                grab_y,
            } => {
                self.drag_resize(id, edges, origin, x - grab_x, y - grab_y);
                return;
            }
            Drag::None => {}
        }

        if let Some(menu) = &mut self.menu {
            if menu.hover(x, y) {
                self.damage.mark_full();
            }
            return;
        }

        if event.dx != 0 || event.dy != 0 {
            self.forward_mouse(x, y, false, false, true);
        }
    }

    fn drag_move(&mut self, id: u64, x: i32, y: i32, dx: i32, dy: i32) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        // Dragging a maximised window restores it under the pointer.
        if self.windows[index].is_maximised() {
            if let Some(previous) = self.windows[index].restore.take() {
                self.windows[index].frame = Rect::new(
                    x - previous.w / 2,
                    y - theme::TITLE_HEIGHT / 2,
                    previous.w,
                    previous.h,
                );
                self.windows[index].sync_content_size();
                self.drag = Drag::Move {
                    id,
                    dx: previous.w / 2,
                    dy: theme::TITLE_HEIGHT / 2,
                };
            }
            self.damage.mark_full();
            return;
        }

        let frame = self.windows[index].frame;
        let new_x = (x - dx).clamp(-frame.w + 80, self.width - 80);
        let new_y = (y - dy).clamp(0, self.height - theme::TASKBAR_HEIGHT - 8);
        self.windows[index].frame = Rect::new(new_x, new_y, frame.w, frame.h);

        // Edge proximity previews a snap, applied on release.
        let snap = if y <= 2 {
            Snap::Maximise
        } else if x <= 2 {
            Snap::Left
        } else if x >= self.width - 3 {
            Snap::Right
        } else {
            Snap::None
        };
        if snap != self.snap {
            self.snap = snap;
        }
        self.damage.mark_full();
    }

    fn drag_resize(&mut self, id: u64, edges: Edges, origin: Rect, dx: i32, dy: i32) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let (min_w, min_h) = self.windows[index].app.min_size();
        let min_w = min_w.max(160);
        let min_h = min_h.max(theme::TITLE_HEIGHT + 40);

        let mut rect = origin;
        if edges.left {
            // Moving the left edge right shrinks the window; clamp so it never
            // inverts or slides past its own right edge.
            let shift = dx.min(origin.w - min_w);
            rect.x = origin.x + shift;
            rect.w = origin.w - shift;
        }
        if edges.right {
            rect.w = (origin.w + dx).max(min_w);
        }
        if edges.top {
            let shift = dy.min(origin.h - min_h);
            rect.y = (origin.y + shift).max(0);
            rect.h = origin.h - (rect.y - origin.y);
        }
        if edges.bottom {
            rect.h = (origin.h + dy).max(min_h);
        }

        rect.w = rect.w.clamp(min_w, self.width * 2);
        rect.h = rect.h.clamp(min_h, self.height * 2);

        self.windows[index].frame = rect;
        self.windows[index].sync_content_size();
        self.damage.mark_full();
    }

    fn on_press(&mut self, x: i32, y: i32) {
        self.damage.mark_full();

        // Menus consume the click that lands in them, and close otherwise.
        if let Some(menu) = &self.menu {
            let chosen = menu.item_at(x, y);
            let inside = menu.rect.contains(x, y);
            if let Some(index) = chosen {
                let action = menu.items[index].action.clone();
                self.menu = None;
                self.run_action(action);
                return;
            }
            self.menu = None;
            if inside {
                return;
            }
        }

        // Double click on a title bar toggles maximise.
        let now = pit::ticks();
        let double = now.saturating_sub(self.last_click) < 400
            && (x - self.last_click_pos.0).abs() < 6
            && (y - self.last_click_pos.1).abs() < 6;
        self.last_click = now;
        self.last_click_pos = (x, y);

        if y >= self.height - theme::TASKBAR_HEIGHT {
            self.on_taskbar_press(x, y);
            return;
        }

        // Resize bands are checked before the window body, since they overlap.
        if let Some((id, edges)) = self.resize_target(x, y) {
            self.raise(id);
            if let Some(index) = self.index_of(id) {
                self.drag = Drag::Resize {
                    id,
                    edges,
                    origin: self.windows[index].frame,
                    grab_x: x,
                    grab_y: y,
                };
            }
            return;
        }

        let Some(id) = self.window_at(x, y) else {
            // Empty desktop: clear focus so keystrokes do not go somewhere
            // invisible.
            self.focused = None;
            return;
        };
        self.raise(id);
        let Some(index) = self.index_of(id) else {
            return;
        };
        let frame = self.windows[index].frame;

        if self.windows[index].fullscreen {
            self.forward_mouse(x, y, true, false, false);
            return;
        }

        if theme::close_button(frame).contains(x, y) {
            self.close(id);
            return;
        }
        if theme::maximise_button(frame).contains(x, y) {
            self.toggle_maximise(id);
            return;
        }
        if theme::minimise_button(frame).contains(x, y) {
            self.minimise(id);
            return;
        }
        if theme::title_bar(frame).contains(x, y) {
            if double {
                self.toggle_maximise(id);
                return;
            }
            self.drag = Drag::Move {
                id,
                dx: x - frame.x,
                dy: y - frame.y,
            };
            return;
        }

        self.forward_mouse(x, y, true, false, false);
    }

    fn on_release(&mut self, x: i32, y: i32) {
        self.damage.mark_full();
        if let Drag::Move { id, .. } = self.drag {
            let snap = self.snap;
            self.snap = Snap::None;
            self.drag = Drag::None;
            if snap != Snap::None {
                self.snap_to(id, snap);
                return;
            }
        }
        self.drag = Drag::None;
        self.snap = Snap::None;
        self.forward_mouse(x, y, false, true, false);
    }

    fn on_right_press(&mut self, x: i32, y: i32) {
        self.damage.mark_full();
        self.menu = None;

        if y >= self.height - theme::TASKBAR_HEIGHT {
            self.menu = Some(Menu::new(self.taskbar_menu(), x, y, self.screen()));
            return;
        }

        if let Some(id) = self.window_at(x, y) {
            let Some(index) = self.index_of(id) else {
                return;
            };
            let frame = self.windows[index].frame;
            self.raise(id);
            if theme::title_bar(frame).contains(x, y) {
                let items = self.window_menu(id);
                self.menu = Some(Menu::new(items, x, y, self.screen()));
                return;
            }
            // Inside the content: let the app have it, and fall back to the
            // window menu only if it does nothing with it.
            self.forward_mouse(x, y, true, false, false);
            return;
        }

        self.menu = Some(Menu::new(self.desktop_menu(), x, y, self.screen()));
    }

    fn forward_mouse(&mut self, x: i32, y: i32, pressed: bool, released: bool, moved: bool) {
        let Some(id) = self.window_at(x, y) else {
            return;
        };
        let Some(index) = self.index_of(id) else {
            return;
        };
        let content = self.windows[index].content_rect();
        if !content.contains(x, y) && !released {
            return;
        }
        let event = WindowMouse {
            x: x - content.x,
            y: y - content.y,
            dx: 0,
            dy: 0,
            left: self.left_down,
            right: self.right_down,
            pressed,
            released,
            moved,
            wheel: 0,
        };
        let mut response = AppResponse::default();
        self.windows[index].app.on_mouse(&event, &mut response);
        self.apply(id, response);
    }

    fn apply(&mut self, id: u64, response: AppResponse) {
        if let Some(title) = response.retitle {
            if let Some(index) = self.index_of(id) {
                self.windows[index].title = title;
                self.damage.mark_full();
            }
        }
        if let Some(name) = response.launch {
            self.open(&name);
        }
        if let Some(on) = response.fullscreen {
            self.set_fullscreen(id, on);
        }
        if let Some(on) = response.grab_pointer {
            if on {
                self.grab_pointer(id);
            } else if self.grab == Some(id) {
                self.release_pointer();
            }
        }
        if response.close {
            self.close(id);
        }
    }

    // --- menus ---

    fn launcher_items(&self) -> Vec<menu::Item> {
        self.registry
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                menu::Item::new(entry.title, Action::Launch(entry.name))
                    .with_icon(entry.icon)
                    .with_hint(&format!("F{}", index + 1))
            })
            .collect()
    }

    fn taskbar_menu(&self) -> Vec<menu::Item> {
        let mut items = self.launcher_items();
        items.push(menu::Item::separator());
        items.push(menu::Item::new("Tile windows", Action::TileAll));
        items.push(menu::Item::new("Cascade windows", Action::CascadeAll));
        items.push(menu::Item::new("Minimise all", Action::MinimiseAll));
        items.push(menu::Item::separator());
        items.push(
            menu::Item::new("Scanlines", Action::ToggleScanlines).with_hint(
                if crt::scanlines_enabled() {
                    "on"
                } else {
                    "off"
                },
            ),
        );
        items.push(
            menu::Item::new("Frame rate", Action::ToggleFps).with_hint(if self.show_fps {
                "on"
            } else {
                "off"
            }),
        );
        items.push(menu::Item::separator());
        items.push(menu::Item::new("Restart", Action::Reboot));
        items.push(menu::Item::new("Shut down", Action::PowerOff));
        items
    }

    fn desktop_menu(&self) -> Vec<menu::Item> {
        let mut items = self.launcher_items();
        items.push(menu::Item::separator());
        items.push(menu::Item::new("Tile windows", Action::TileAll));
        items.push(menu::Item::new("Cascade windows", Action::CascadeAll));
        if !self.windows.is_empty() {
            items.push(menu::Item::new("Close all", Action::CloseAll));
        }
        items
    }

    fn window_menu(&self, id: u64) -> Vec<menu::Item> {
        let maximised = self
            .index_of(id)
            .map(|index| self.windows[index].is_maximised())
            .unwrap_or(false);
        alloc::vec![
            menu::Item::new(
                if maximised { "Restore" } else { "Maximise" },
                Action::ToggleMaximise(id)
            )
            .with_hint("alt+up"),
            menu::Item::new("Minimise", Action::Minimise(id)).with_hint("ctrl+m"),
            menu::Item::new("Fullscreen", Action::Fullscreen(id)).with_hint("F11"),
            menu::Item::separator(),
            menu::Item::new("Snap left", Action::SnapLeft(id)).with_hint("alt+left"),
            menu::Item::new("Snap right", Action::SnapRight(id)).with_hint("alt+right"),
            menu::Item::separator(),
            menu::Item::new("Close", Action::Close(id)).with_hint("ctrl+w"),
        ]
    }

    fn run_action(&mut self, action: Action) {
        self.damage.mark_full();
        match action {
            Action::Launch(name) => {
                self.open(name);
            }
            Action::Close(id) => self.close(id),
            Action::CloseAll => {
                let ids: Vec<u64> = self.windows.iter().map(|window| window.id).collect();
                for id in ids {
                    self.close(id);
                }
            }
            Action::Minimise(id) => self.minimise(id),
            Action::ToggleMaximise(id) => self.toggle_maximise(id),
            Action::Fullscreen(id) => self.set_fullscreen(id, true),
            Action::SnapLeft(id) => self.snap_to(id, Snap::Left),
            Action::SnapRight(id) => self.snap_to(id, Snap::Right),
            Action::Raise(id) => {
                if let Some(index) = self.index_of(id) {
                    self.windows[index].minimised = false;
                }
                self.raise(id);
            }
            Action::TileAll => self.tile_all(),
            Action::CascadeAll => self.cascade_all(),
            Action::MinimiseAll => {
                let ids: Vec<u64> = self.windows.iter().map(|window| window.id).collect();
                for id in ids {
                    self.minimise(id);
                }
            }
            Action::ToggleScanlines => {
                let on = crt::toggle_scanlines();
                self.set_status(if on { "scanlines on" } else { "scanlines off" }, 2000);
            }
            Action::ToggleFps => self.show_fps = !self.show_fps,
            Action::Reboot => crate::arch::acpi::reboot(),
            Action::PowerOff => {
                crate::power_off();
                self.set_status("this machine ignored every shutdown route", 4000);
            }
            Action::None => {}
        }
    }

    fn taskbar_button_rects(&self) -> Vec<(u64, Rect)> {
        let mut rects = Vec::new();
        let mut x = 104;
        for window in self.windows.iter() {
            if x > self.width - 260 {
                break;
            }
            rects.push((
                window.id,
                Rect::new(x, self.height - theme::TASKBAR_HEIGHT + 4, 130, 20),
            ));
            x += 134;
        }
        rects
    }

    fn on_taskbar_press(&mut self, x: i32, y: i32) {
        if x < 96 {
            // Anchored to the button, not the click, and Menu::new flips it
            // upwards because there is no room below.
            let items = self.taskbar_menu();
            self.menu = Some(Menu::new(
                items,
                6,
                self.height - theme::TASKBAR_HEIGHT,
                self.screen(),
            ));
            return;
        }

        for (id, rect) in self.taskbar_button_rects() {
            if rect.contains(x, y) {
                let Some(index) = self.index_of(id) else {
                    return;
                };
                if self.windows[index].minimised {
                    self.windows[index].minimised = false;
                    self.raise(id);
                } else if self.focused == Some(id) {
                    self.minimise(id);
                } else {
                    self.raise(id);
                }
                return;
            }
        }
    }

    // --- per-frame update and composition ---

    pub fn tick_apps(&mut self) {
        let now = pit::uptime_ms();
        let ids: Vec<u64> = self.windows.iter().map(|window| window.id).collect();
        for id in ids {
            let Some(index) = self.index_of(id) else {
                continue;
            };
            let mut response = AppResponse::default();
            self.windows[index].app.tick(now, &mut response);
            self.apply(id, response);
        }
    }

    /// Composite, unless nothing has changed since the last frame.
    pub fn compose(&mut self) {
        self.follow_resolution();

        // Repaint window contents first: an app that reported itself dirty
        // damages the area its window covers.
        let focused = self.focused;
        let mut repaint_damage: Vec<Rect> = Vec::new();
        for window in self.windows.iter_mut() {
            if window.minimised {
                continue;
            }
            window.sync_content_size();
            if window.repaint_if_needed(Some(window.id) == focused) {
                repaint_damage.push(window.content_rect());
            }
        }
        for rect in repaint_damage {
            self.damage.add(rect);
        }

        // The taskbar clock has to tick even on an otherwise still screen.
        let now = rtc::now();
        if now.second != self.last_second {
            self.last_second = now.second;
            self.damage.add(Rect::new(
                0,
                self.height - theme::TASKBAR_HEIGHT,
                self.width,
                theme::TASKBAR_HEIGHT,
            ));
        }
        if !self.status.is_empty() && pit::ticks() >= self.status_until {
            self.status.clear();
            self.damage.add(Rect::new(
                0,
                self.height - theme::TASKBAR_HEIGHT,
                self.width,
                theme::TASKBAR_HEIGHT,
            ));
        }

        if self.damage.is_empty() {
            self.clock.skipped += 1;
            return;
        }

        self.previous_cursor = (self.cursor_x, self.cursor_y);
        self.frames += 1;
        self.clock.begin();

        let regions: Vec<Rect> = if self.damage.full {
            alloc::vec![self.screen()]
        } else {
            self.damage
                .rects
                .iter()
                .map(|rect| rect.intersect(&self.screen()))
                .filter(|rect| !rect.is_empty())
                .collect()
        };
        self.damage.clear();

        for region in &regions {
            self.paint(*region);
        }
        for region in &regions {
            fb::present_rect(*region);
        }

        self.clock.end();
    }

    /// Paint one region. Every step clips to `region`, so a partial redraw is
    /// the same pipeline with a smaller scissor.
    fn paint(&mut self, region: Rect) {
        let focused = self.focused;
        let windows = &self.windows;
        let backdrop = &self.backdrop;
        let (width, height) = (self.width, self.height);

        // A window that is opaque and covers the whole region makes the
        // backdrop, and everything below it, redundant.
        let mut first_visible = 0usize;
        for (index, window) in windows.iter().enumerate() {
            if window.minimised || !window.app.opaque() {
                continue;
            }
            let covered = window.content_rect();
            if covered.x <= region.x
                && covered.y <= region.y
                && covered.right() >= region.right()
                && covered.bottom() >= region.bottom()
            {
                first_visible = index;
            }
        }
        let skip_backdrop = first_visible > 0;

        fb::with_back(|screen| {
            screen.set_clip(region);
            if !skip_backdrop {
                screen.blit(backdrop, 0, 0);
            }

            for window in windows.iter().skip(first_visible) {
                if window.minimised {
                    continue;
                }
                // Cheap rejection: nothing to do for a window outside the region.
                if window
                    .frame
                    .inset(-theme::SHADOW)
                    .intersect(&region)
                    .is_empty()
                {
                    continue;
                }
                let content = window.content_rect();
                if window.fullscreen {
                    screen.set_clip(content.intersect(&region));
                    screen.blit(&window.content, content.x, content.y);
                    screen.set_clip(region);
                    continue;
                }

                theme::draw_shadow(screen, window.frame);
                theme::draw_frame(
                    screen,
                    window.frame,
                    &window.title,
                    Some(window.id) == focused,
                    window.icon,
                    window.is_maximised(),
                );
                screen.set_clip(content.intersect(&region));
                screen.blit(&window.content, content.x, content.y);
                screen.set_clip(region);

                if !window.is_maximised() {
                    let grip = theme::resize_grip(window.frame);
                    for step in 0..3 {
                        let offset = step * 4;
                        screen.line(
                            grip.right() - 2 - offset,
                            grip.bottom() - 2,
                            grip.right() - 2,
                            grip.bottom() - 2 - offset,
                            palette::BORDER_BRIGHT,
                        );
                    }
                }
            }
        });

        // Snap preview, taskbar, menu, then the pointer -- which must be last
        // so it is never hidden by the chrome it is pointing at.
        self.draw_snap_preview(region);
        let taskbar = Rect::new(
            0,
            height - theme::TASKBAR_HEIGHT,
            width,
            theme::TASKBAR_HEIGHT,
        );
        if !self.windows.iter().any(|window| window.fullscreen)
            && !taskbar.intersect(&region).is_empty()
        {
            self.draw_taskbar(region);
        }
        if let Some(menu) = &self.menu {
            if !menu
                .rect
                .inset(-theme::SHADOW)
                .intersect(&region)
                .is_empty()
            {
                let menu_ref = menu;
                fb::with_back(|screen| {
                    screen.set_clip(region);
                    menu_ref.draw(screen);
                });
            }
        }

        let (cursor_x, cursor_y, pressed) = (self.cursor_x, self.cursor_y, self.left_down);
        // A captured pointer is not on screen any more; drawing it would leave
        // an arrow parked over the game.
        let show_cursor = self.grab.is_none();
        let scanlines = crt::scanlines_enabled();
        fb::with_back(|screen| {
            screen.set_clip(region);
            if show_cursor {
                cursor::draw(screen, cursor_x, cursor_y, pressed);
            }
            screen.reset_clip();
            if scanlines {
                crt::apply_scanlines_rect(screen, region);
            }
        });
    }

    fn draw_snap_preview(&self, region: Rect) {
        if self.snap == Snap::None {
            return;
        }
        let work = self.work_area();
        let rect = match self.snap {
            Snap::Maximise => work,
            Snap::Left => Rect::new(work.x, work.y, work.w / 2, work.h),
            Snap::Right => Rect::new(work.x + work.w / 2, work.y, work.w - work.w / 2, work.h),
            Snap::None => return,
        };
        fb::with_back(|screen| {
            screen.set_clip(region);
            screen.fill_rect_blend(rect, palette::AMBER, 42);
            screen.stroke_rect(rect, palette::AMBER);
            screen.stroke_rect(rect.inset(1), palette::AMBER_DIM);
        });
    }

    fn draw_taskbar(&mut self, region: Rect) {
        let height = self.height;
        let width = self.width;
        let focused = self.focused;
        let windows = &self.windows;
        let buttons = self.taskbar_button_rects();
        let status = if pit::ticks() < self.status_until {
            Some(self.status.clone())
        } else {
            None
        };
        let fps = if self.show_fps {
            Some(format!(
                "{} fps  {} ms  {} skipped",
                self.clock.frames_per_second(),
                self.clock.millis(),
                self.clock.skipped
            ))
        } else {
            None
        };
        let menu_open = self.menu.is_some();

        fb::with_back(|screen| {
            let bar = Rect::new(
                0,
                height - theme::TASKBAR_HEIGHT,
                width,
                theme::TASKBAR_HEIGHT,
            );
            screen.set_clip(bar.intersect(&region));
            screen.gradient_v(
                bar,
                palette::rgb(0x16, 0x1D, 0x30),
                palette::rgb(0x0A, 0x0E, 0x18),
            );
            screen.hline(0, bar.y, width, palette::AMBER_DIM);

            // Launcher button.
            if menu_open {
                screen.fill_rect(
                    Rect::new(2, bar.y + 3, 92, theme::TASKBAR_HEIGHT - 6),
                    palette::rgb(0x25, 0x2E, 0x4A),
                );
            }
            screen.glyph(
                glyph::LOGO,
                10,
                bar.y + 6,
                palette::AMBER,
                Weight::Regular,
                1,
            );
            screen.text_ex("HALCYON", 26, bar.y + 6, palette::AMBER, Weight::Bold, 1);
            screen.vline(96, bar.y + 4, theme::TASKBAR_HEIGHT - 8, palette::BORDER);

            for (id, rect) in &buttons {
                let Some(window) = windows.iter().find(|window| window.id == *id) else {
                    continue;
                };
                let active = Some(*id) == focused && !window.minimised;
                screen.fill_rect(
                    *rect,
                    if active {
                        palette::rgb(0x25, 0x2E, 0x4A)
                    } else {
                        palette::rgb(0x11, 0x16, 0x26)
                    },
                );
                screen.stroke_rect(
                    *rect,
                    if active {
                        palette::AMBER_DIM
                    } else {
                        palette::BORDER
                    },
                );
                let color = if window.minimised {
                    palette::TEXT_FAINT
                } else if active {
                    palette::AMBER
                } else {
                    palette::TEXT_DIM
                };
                screen.glyph(
                    window.icon,
                    rect.x + 5,
                    rect.y + 2,
                    color,
                    Weight::Regular,
                    1,
                );
                let previous = screen.set_clip(
                    Rect::new(rect.x + 18, rect.y, rect.w - 22, rect.h).intersect(&region),
                );
                screen.text(&window.title, rect.x + 18, rect.y + 2, color);
                screen.set_clip(previous);
            }

            let right_edge = width - 8 * 8 - 12;
            let text_left = buttons.last().map(|(_, r)| r.right() + 10).unwrap_or(104);
            if let Some(text) = fps {
                let text_width = text.chars().count() as i32 * theme::CELL_W;
                screen.text(
                    &text,
                    right_edge - text_width - 16,
                    bar.y + 6,
                    palette::CYAN_DIM,
                );
            } else if let Some(text) = status {
                let previous = screen.set_clip(
                    Rect::new(
                        text_left,
                        bar.y,
                        right_edge - text_left - 8,
                        theme::TASKBAR_HEIGHT,
                    )
                    .intersect(&region),
                );
                screen.text(&text, text_left, bar.y + 6, palette::CYAN_DIM);
                screen.set_clip(previous);
            }

            let now = rtc::now();
            let clock = if now.is_valid() {
                [
                    b'0' + now.hour / 10,
                    b'0' + now.hour % 10,
                    b':',
                    b'0' + now.minute / 10,
                    b'0' + now.minute % 10,
                    b':',
                    b'0' + now.second / 10,
                    b'0' + now.second % 10,
                ]
            } else {
                *b"--:--:--"
            };
            let clock_text = core::str::from_utf8(&clock).unwrap_or("--:--:--");
            screen.text_ex(
                clock_text,
                right_edge,
                bar.y + 6,
                palette::AMBER,
                Weight::Bold,
                1,
            );
        });
    }

    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    pub fn frames_per_second(&self) -> u64 {
        self.clock.frames_per_second()
    }

    pub fn frame_millis(&self) -> u64 {
        self.clock.millis()
    }

    pub fn skipped_frames(&self) -> u64 {
        self.clock.skipped
    }
}

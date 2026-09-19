//! The compositor and window manager.
//!
//! One thread owns the whole desktop: it drains the input queues, updates
//! window state, repaints any window whose app is dirty, and composites the
//! screen. Windows are kept bottom-to-top in `windows`, so the last entry is
//! the one on top and hit testing walks the list backwards.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::arch::pit;
use crate::drivers::rtc;
use crate::gfx::crt;
use crate::gfx::draw::{Rect, Surface};
use crate::gfx::fb;
use crate::gfx::font::{glyph, Weight};
use crate::gfx::palette;
use crate::input::{self, Key, KeyEvent};

use super::cursor;
use super::theme;
use super::window::{App, AppResponse, Window, WindowMouse};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    None,
    Move { id: u64, dx: i32, dy: i32 },
    Resize { id: u64, dx: i32, dy: i32 },
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

pub struct Desktop {
    windows: Vec<Window>,
    next_id: u64,
    focused: Option<u64>,
    cursor_x: i32,
    cursor_y: i32,
    left_down: bool,
    drag: Drag,
    backdrop: Surface,
    width: i32,
    height: i32,
    registry: Vec<AppEntry>,
    menu_open: bool,
    menu_rect: Rect,
    /// Cascade offset for the next window opened.
    spawn_offset: i32,
    pub frames: u64,
    status: String,
    status_until: u64,
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

        Self {
            windows: Vec::new(),
            next_id: 1,
            focused: None,
            cursor_x: width / 2,
            cursor_y: height / 2,
            left_down: false,
            drag: Drag::None,
            backdrop,
            width,
            height,
            registry,
            menu_open: false,
            menu_rect: Rect::EMPTY,
            spawn_offset: 0,
            frames: 0,
            status: String::new(),
            status_until: 0,
        }
    }

    pub fn set_status(&mut self, text: &str, ms: u64) {
        self.status = text.to_string();
        self.status_until = pit::ticks() + ms;
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

        let frame_width = width.min(self.width - 40);
        let frame_height = height.min(self.height - theme::TASKBAR_HEIGHT - 40);
        let x = (40 + offset).min(self.width - frame_width - 8);
        let y = (40 + offset).min(self.height - theme::TASKBAR_HEIGHT - frame_height - 8);

        let id = self.next_id;
        self.next_id += 1;
        let window = Window::new(
            id,
            title,
            icon,
            Rect::new(x, y, frame_width, frame_height),
            build(),
        );
        self.windows.push(window);
        self.focused = Some(id);
        Some(id)
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.windows.iter().position(|window| window.id == id)
    }

    fn raise(&mut self, id: u64) {
        if let Some(index) = self.index_of(id) {
            let window = self.windows.remove(index);
            self.windows.push(window);
            self.focused = Some(id);
        }
    }

    fn close(&mut self, id: u64) {
        if let Some(index) = self.index_of(id) {
            self.windows.remove(index);
        }
        if self.focused == Some(id) {
            self.focused = self.windows.last().map(|window| window.id);
        }
    }

    fn focus_next(&mut self) {
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
            Some(index) => visible[(index + 1) % visible.len()],
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

    // --- event handling ---

    pub fn handle_input(&mut self) {
        while let Some(event) = input::pop_key() {
            self.on_key(&event);
        }
        while let Some(event) = input::pop_mouse() {
            self.on_mouse(event);
        }
    }

    fn on_key(&mut self, event: &KeyEvent) {
        if !event.pressed {
            return;
        }

        // Desktop-level bindings first.
        if event.modifiers.alt && event.key == Key::Tab {
            self.focus_next();
            return;
        }
        if event.modifiers.ctrl && event.key == Key::Char {
            if let Some('w') = event.character {
                if let Some(id) = self.focused {
                    self.close(id);
                }
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
            _ => None,
        };
        if let Some(index) = launch_index {
            if let Some(entry) = self.registry.get(index) {
                let name = entry.name;
                self.open(name);
            }
            return;
        }
        if event.key == Key::Escape && self.menu_open {
            self.menu_open = false;
            return;
        }

        let Some(id) = self.focused else { return };
        let Some(index) = self.index_of(id) else {
            return;
        };
        let mut response = AppResponse::default();
        self.windows[index].app.on_key(event, &mut response);
        self.windows[index].needs_paint = true;
        self.apply(id, response);
    }

    fn on_mouse(&mut self, event: input::MouseEvent) {
        self.cursor_x = (self.cursor_x + event.dx).clamp(0, self.width - 1);
        self.cursor_y = (self.cursor_y + event.dy).clamp(0, self.height - 1);
        let (x, y) = (self.cursor_x, self.cursor_y);

        if event.left_changed {
            self.left_down = event.left;
            if event.left {
                self.on_press(x, y);
            } else {
                self.drag = Drag::None;
                self.forward_mouse(x, y, false, true, false);
            }
            return;
        }

        // Dragging takes priority over anything under the pointer.
        match self.drag {
            Drag::Move { id, dx, dy } => {
                if let Some(index) = self.index_of(id) {
                    let frame = self.windows[index].frame;
                    let new_x = (x - dx).clamp(-frame.w + 60, self.width - 60);
                    let new_y = (y - dy).clamp(0, self.height - theme::TASKBAR_HEIGHT - 8);
                    self.windows[index].frame = Rect::new(new_x, new_y, frame.w, frame.h);
                }
                return;
            }
            Drag::Resize { id, dx, dy } => {
                if let Some(index) = self.index_of(id) {
                    let frame = self.windows[index].frame;
                    let (min_w, min_h) = self.windows[index].app.min_size();
                    let width = (x - frame.x + dx).clamp(min_w, self.width - frame.x);
                    let height = (y - frame.y + dy)
                        .clamp(min_h, self.height - frame.y - theme::TASKBAR_HEIGHT);
                    self.windows[index].frame = Rect::new(frame.x, frame.y, width, height);
                    self.windows[index].sync_content_size();
                }
                return;
            }
            Drag::None => {}
        }

        if event.dx != 0 || event.dy != 0 {
            self.forward_mouse(x, y, false, false, true);
        }
    }

    fn on_press(&mut self, x: i32, y: i32) {
        // Taskbar.
        if y >= self.height - theme::TASKBAR_HEIGHT {
            self.on_taskbar_press(x, y);
            return;
        }

        // Launcher menu.
        if self.menu_open {
            if self.menu_rect.contains(x, y) {
                let row = (y - self.menu_rect.y - 6) / 20;
                if row >= 0 && (row as usize) < self.registry.len() {
                    let name = self.registry[row as usize].name;
                    self.menu_open = false;
                    self.open(name);
                }
                return;
            }
            self.menu_open = false;
        }

        let Some(id) = self.window_at(x, y) else {
            return;
        };
        self.raise(id);
        let Some(index) = self.index_of(id) else {
            return;
        };
        let frame = self.windows[index].frame;

        if theme::close_button(frame).contains(x, y) {
            self.close(id);
            return;
        }
        if theme::minimise_button(frame).contains(x, y) {
            self.windows[index].minimised = true;
            self.focused = self
                .windows
                .iter()
                .rev()
                .find(|window| !window.minimised)
                .map(|window| window.id);
            return;
        }
        if theme::resize_grip(frame).contains(x, y) {
            self.drag = Drag::Resize {
                id,
                dx: frame.right() - x,
                dy: frame.bottom() - y,
            };
            return;
        }
        if theme::title_bar(frame).contains(x, y) {
            self.drag = Drag::Move {
                id,
                dx: x - frame.x,
                dy: y - frame.y,
            };
            return;
        }

        self.forward_mouse(x, y, true, false, false);
    }

    fn on_taskbar_press(&mut self, x: i32, y: i32) {
        let _ = y;
        if x < 96 {
            self.menu_open = !self.menu_open;
            let height = self.registry.len() as i32 * 20 + 12;
            self.menu_rect = Rect::new(
                6,
                self.height - theme::TASKBAR_HEIGHT - height - 4,
                200,
                height,
            );
            return;
        }

        // Window buttons.
        let mut button_x = 104;
        let ids: Vec<u64> = self.windows.iter().map(|window| window.id).collect();
        for id in ids {
            let rect = Rect::new(button_x, self.height - theme::TASKBAR_HEIGHT + 4, 130, 20);
            if rect.contains(x, y) {
                if let Some(index) = self.index_of(id) {
                    if self.windows[index].minimised {
                        self.windows[index].minimised = false;
                        self.raise(id);
                    } else if self.focused == Some(id) {
                        self.windows[index].minimised = true;
                    } else {
                        self.raise(id);
                    }
                }
                return;
            }
            button_x += 134;
        }
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
            left: self.left_down,
            right: false,
            pressed,
            released,
            moved,
        };
        let mut response = AppResponse::default();
        self.windows[index].app.on_mouse(&event, &mut response);
        self.windows[index].needs_paint = true;
        self.apply(id, response);
    }

    fn apply(&mut self, id: u64, response: AppResponse) {
        if let Some(title) = response.retitle {
            if let Some(index) = self.index_of(id) {
                self.windows[index].title = title;
            }
        }
        if let Some(name) = response.launch {
            self.open(&name);
        }
        if response.close {
            self.close(id);
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

    pub fn compose(&mut self) {
        self.frames += 1;

        let focused = self.focused;
        for window in self.windows.iter_mut() {
            if window.minimised {
                continue;
            }
            window.sync_content_size();
            window.repaint_if_needed(Some(window.id) == focused);
        }

        let backdrop = &self.backdrop;
        let windows = &self.windows;
        let (cursor_x, cursor_y, pressed) = (self.cursor_x, self.cursor_y, self.left_down);

        fb::with_back(|screen| {
            screen.reset_clip();
            screen.blit(backdrop, 0, 0);

            for window in windows.iter() {
                if window.minimised {
                    continue;
                }
                theme::draw_shadow(screen, window.frame);
                theme::draw_frame(
                    screen,
                    window.frame,
                    &window.title,
                    Some(window.id) == focused,
                    window.icon,
                );
                let content = theme::content_for(window.frame);
                let previous = screen.set_clip(content);
                screen.blit(&window.content, content.x, content.y);
                screen.set_clip(previous);

                // Resize grip: three diagonal ticks in the corner.
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

            cursor::draw(screen, cursor_x, cursor_y, pressed);
        });

        self.draw_taskbar();
        self.draw_menu();

        fb::with_back(|screen| {
            screen.reset_clip();
            crt::apply_scanlines(screen);
        });
        fb::present();
    }

    fn draw_taskbar(&mut self) {
        let height = self.height;
        let width = self.width;
        let focused = self.focused;
        let windows = &self.windows;
        let frames = self.frames;
        let status = if pit::ticks() < self.status_until {
            Some(self.status.clone())
        } else {
            None
        };

        fb::with_back(|screen| {
            let bar = Rect::new(0, height - theme::TASKBAR_HEIGHT, width, theme::TASKBAR_HEIGHT);
            screen.reset_clip();
            screen.gradient_v(
                bar,
                palette::rgb(0x16, 0x1D, 0x30),
                palette::rgb(0x0A, 0x0E, 0x18),
            );
            screen.hline(0, bar.y, width, palette::AMBER_DIM);

            // Launcher button.
            screen.glyph(
                glyph::LOGO,
                10,
                bar.y + 6,
                palette::AMBER,
                Weight::Regular,
                1,
            );
            screen.text_ex(
                "HALCYON",
                26,
                bar.y + 6,
                palette::AMBER,
                Weight::Bold,
                1,
            );
            screen.vline(96, bar.y + 4, theme::TASKBAR_HEIGHT - 8, palette::BORDER);

            // One button per window.
            let mut x = 104;
            for window in windows.iter() {
                let rect = Rect::new(x, bar.y + 4, 130, 20);
                let active = Some(window.id) == focused && !window.minimised;
                screen.fill_rect(
                    rect,
                    if active {
                        palette::rgb(0x25, 0x2E, 0x4A)
                    } else {
                        palette::rgb(0x11, 0x16, 0x26)
                    },
                );
                screen.stroke_rect(
                    rect,
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
                screen.glyph(window.icon, rect.x + 5, rect.y + 2, color, Weight::Regular, 1);
                let previous = screen.set_clip(Rect::new(rect.x + 18, rect.y, rect.w - 22, rect.h));
                screen.text(&window.title, rect.x + 18, rect.y + 2, color);
                screen.set_clip(previous);
                x += 134;
                if x > width - 260 {
                    break;
                }
            }

            // Status message, then the clock.
            if let Some(text) = status {
                screen.text(&text, x + 8, bar.y + 6, palette::CYAN_DIM);
            }

            let now = rtc::now();
            let mut clock = [0u8; 8];
            if now.is_valid() {
                clock = [
                    b'0' + now.hour / 10,
                    b'0' + now.hour % 10,
                    b':',
                    b'0' + now.minute / 10,
                    b'0' + now.minute % 10,
                    b':',
                    b'0' + now.second / 10,
                    b'0' + now.second % 10,
                ];
            }
            let clock_text = core::str::from_utf8(&clock).unwrap_or("--:--:--");
            screen.text_ex(
                clock_text,
                width - 8 * 8 - 12,
                bar.y + 6,
                palette::AMBER,
                Weight::Bold,
                1,
            );

            // A quiet frame counter: proof the compositor is actually running.
            let mut digits = [0u8; 12];
            let mut count = 0;
            let mut value = frames;
            if value == 0 {
                digits[0] = b'0';
                count = 1;
            }
            while value > 0 && count < 12 {
                digits[count] = b'0' + (value % 10) as u8;
                value /= 10;
                count += 1;
            }
            digits[..count].reverse();
            if let Ok(text) = core::str::from_utf8(&digits[..count]) {
                screen.text(text, width - 8 * 8 - 12 - 60, bar.y + 6, palette::TEXT_FAINT);
            }
        });
    }

    fn draw_menu(&mut self) {
        if !self.menu_open {
            return;
        }
        let rect = self.menu_rect;
        let registry = &self.registry;
        fb::with_back(|screen| {
            screen.reset_clip();
            theme::draw_shadow(screen, rect);
            screen.panel(rect, palette::PANEL_RAISED, palette::AMBER_DIM);
            let mut y = rect.y + 6;
            for entry in registry.iter() {
                screen.glyph(
                    entry.icon,
                    rect.x + 8,
                    y + 2,
                    palette::CYAN,
                    Weight::Regular,
                    1,
                );
                screen.text(entry.title, rect.x + 26, y + 2, palette::TEXT);
                y += 20;
            }
        });
    }

    pub fn window_count(&self) -> usize {
        self.windows.len()
    }
}

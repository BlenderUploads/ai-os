//! Windows and the interface applications implement.

use alloc::boxed::Box;
use alloc::string::{String, ToString};

use crate::gfx::draw::{Rect, Surface};
use crate::input::KeyEvent;

/// A mouse event in window-content coordinates.
#[derive(Clone, Copy, Debug)]
pub struct WindowMouse {
    pub x: i32,
    pub y: i32,
    pub left: bool,
    pub right: bool,
    pub pressed: bool,
    pub released: bool,
    pub moved: bool,
}

/// What an application can ask the desktop to do for it.
#[derive(Default)]
pub struct AppResponse {
    pub close: bool,
    pub retitle: Option<String>,
    /// Launch another app by its registry name.
    pub launch: Option<String>,
}

pub trait App: Send {
    fn draw(&mut self, surface: &mut Surface, focused: bool);

    fn on_key(&mut self, _event: &KeyEvent, _response: &mut AppResponse) {}

    fn on_mouse(&mut self, _event: &WindowMouse, _response: &mut AppResponse) {}

    /// Called once per composited frame, whether or not the window is focused.
    fn tick(&mut self, _now_ms: u64, _response: &mut AppResponse) {}

    /// Does the content need repainting? Returning false lets the compositor
    /// reuse the existing content surface.
    fn dirty(&self) -> bool {
        true
    }

    fn clear_dirty(&mut self) {}

    fn min_size(&self) -> (i32, i32) {
        (240, 140)
    }
}

pub struct Window {
    pub id: u64,
    pub title: String,
    pub icon: u8,
    /// Frame rectangle in screen coordinates (includes the title bar).
    pub frame: Rect,
    pub minimised: bool,
    pub content: Surface,
    pub app: Box<dyn App>,
    pub needs_paint: bool,
}

impl Window {
    pub fn new(id: u64, title: &str, icon: u8, frame: Rect, app: Box<dyn App>) -> Self {
        let content = super::theme::content_for(frame);
        Self {
            id,
            title: title.to_string(),
            icon,
            frame,
            minimised: false,
            content: Surface::new(content.w.max(1) as usize, content.h.max(1) as usize),
            app,
            needs_paint: true,
        }
    }

    pub fn content_rect(&self) -> Rect {
        super::theme::content_for(self.frame)
    }

    /// Keep the content surface in step with the frame after a resize.
    pub fn sync_content_size(&mut self) {
        let rect = self.content_rect();
        let (width, height) = (rect.w.max(1) as usize, rect.h.max(1) as usize);
        if self.content.width != width || self.content.height != height {
            self.content.resize(width, height);
            self.needs_paint = true;
        }
    }

    pub fn repaint_if_needed(&mut self, focused: bool) {
        if self.needs_paint || self.app.dirty() {
            self.content.reset_clip();
            self.app.draw(&mut self.content, focused);
            self.app.clear_dirty();
            self.needs_paint = false;
        }
    }
}

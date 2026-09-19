//! Built-in applications and the launcher registry.

pub mod about;
pub mod monitor;

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use crate::gfx::font::glyph;
use crate::ui::desktop::AppEntry;
use crate::ui::window::App;

/// Everything the launcher offers, in F-key order.
pub fn registry() -> Vec<AppEntry> {
    vec![
        AppEntry {
            name: "monitor",
            title: "System Monitor",
            icon: glyph::CHIP,
            width: 520,
            height: 460,
            build: || Box::new(monitor::Monitor::new()) as Box<dyn App>,
        },
        AppEntry {
            name: "about",
            title: "About HALCYON",
            icon: glyph::LOGO,
            width: 560,
            height: 330,
            build: || Box::new(about::About::new()) as Box<dyn App>,
        },
    ]
}

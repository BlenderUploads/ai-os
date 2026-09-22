//! Built-in applications and the launcher registry.

pub mod about;
pub mod calculator;
pub mod devices;
#[cfg(feature = "doom")]
pub mod doom;
pub mod editor;
pub mod files;
pub mod help;
pub mod monitor;
pub mod paint;
pub mod settings;
pub mod shell;
pub mod snake;
pub mod terminal;
pub mod tetris;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::gfx::font::glyph;
use crate::sync::SpinLock;
use crate::ui::desktop::AppEntry;
use crate::ui::window::App;

/// Which file the next editor window should open.
///
/// The file browser sets this before asking the desktop to launch the editor,
/// which keeps `AppEntry::build` a plain `fn` with no captured state.
static EDITOR_TARGET: SpinLock<Option<String>> = SpinLock::new(None);

pub fn set_editor_target(path: &str) {
    *EDITOR_TARGET.lock() = Some(path.to_string());
}

fn take_editor_target() -> Option<String> {
    EDITOR_TARGET.lock().take()
}

/// Everything the launcher offers, in F-key order.
pub fn registry() -> Vec<AppEntry> {
    #[allow(unused_mut)] // `mut` is only needed when the doom feature is on
    let mut entries = vec![
        AppEntry {
            name: "terminal",
            title: "Terminal",
            icon: glyph::ARROW_RIGHT,
            width: 680,
            height: 430,
            build: || Box::new(terminal::Terminal::new()) as Box<dyn App>,
        },
        AppEntry {
            name: "files",
            title: "Files",
            icon: glyph::BLOCK,
            width: 600,
            height: 360,
            build: || Box::new(files::Files::new()) as Box<dyn App>,
        },
        AppEntry {
            name: "editor",
            title: "Editor",
            icon: glyph::BULLET,
            width: 560,
            height: 380,
            build: || {
                Box::new(match take_editor_target() {
                    Some(path) => editor::Editor::open(&path),
                    None => editor::Editor::new(),
                }) as Box<dyn App>
            },
        },
        AppEntry {
            name: "monitor",
            title: "System Monitor",
            icon: glyph::CHIP,
            width: 520,
            height: 460,
            build: || Box::new(monitor::Monitor::new()) as Box<dyn App>,
        },
        AppEntry {
            name: "devices",
            title: "Devices",
            icon: glyph::CHIP,
            width: 620,
            height: 420,
            build: || Box::new(devices::Devices::new()) as Box<dyn App>,
        },
        AppEntry {
            name: "settings",
            title: "Settings",
            icon: glyph::CHIP,
            width: 620,
            height: 460,
            build: || Box::new(settings::Settings::new()) as Box<dyn App>,
        },
        AppEntry {
            name: "calculator",
            title: "Calculator",
            icon: glyph::DITHER,
            width: 280,
            height: 380,
            build: || Box::new(calculator::Calculator::new()) as Box<dyn App>,
        },
        AppEntry {
            name: "tetris",
            title: "Tetris",
            icon: glyph::HALF_BLOCK,
            width: 380,
            height: 470,
            build: || Box::new(tetris::Tetris::new()) as Box<dyn App>,
        },
        AppEntry {
            name: "snake",
            title: "Snake",
            icon: glyph::HALF_BLOCK,
            width: 460,
            height: 380,
            build: || Box::new(snake::Snake::new()) as Box<dyn App>,
        },
        AppEntry {
            name: "paint",
            title: "Paint",
            icon: glyph::DITHER,
            width: 520,
            height: 360,
            build: || Box::new(paint::Paint::new()) as Box<dyn App>,
        },
        AppEntry {
            name: "help",
            title: "Manual",
            icon: glyph::BULLET,
            width: 620,
            height: 400,
            build: || Box::new(help::Help::new()) as Box<dyn App>,
        },
        AppEntry {
            name: "about",
            title: "About HALCYON",
            icon: glyph::LOGO,
            width: 560,
            height: 330,
            build: || Box::new(about::About::new()) as Box<dyn App>,
        },
    ];

    // DOOM goes near the front when it is built in, so it gets a function key.
    #[cfg(feature = "doom")]
    entries.insert(
        6,
        AppEntry {
            name: "doom",
            title: "DOOM",
            icon: glyph::BLOCK,
            width: doom::DOOM_WIDTH as i32 + 2,
            height: doom::DOOM_HEIGHT as i32 + 23,
            build: || Box::new(doom::Doom::new()) as Box<dyn App>,
        },
    );

    entries
}

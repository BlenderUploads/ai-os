//! Pop-up menus: the launcher, and the context menus on the desktop, the
//! taskbar and window title bars.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::gfx::draw::{Rect, Surface};
use crate::gfx::font::Weight;
use crate::gfx::palette;

use super::theme;

pub const ROW_HEIGHT: i32 = 20;
const SEPARATOR_HEIGHT: i32 = 7;
const PADDING: i32 = 6;

#[derive(Clone, PartialEq, Eq)]
pub enum Action {
    Launch(&'static str),
    Close(u64),
    CloseAll,
    Minimise(u64),
    ToggleMaximise(u64),
    Fullscreen(u64),
    SnapLeft(u64),
    SnapRight(u64),
    Raise(u64),
    TileAll,
    CascadeAll,
    MinimiseAll,
    ToggleScanlines,
    ToggleFps,
    Reboot,
    PowerOff,
    None,
}

#[derive(Clone)]
pub struct Item {
    pub label: String,
    pub hint: String,
    pub icon: u8,
    pub action: Action,
    pub separator: bool,
    pub enabled: bool,
}

impl Item {
    pub fn new(label: &str, action: Action) -> Self {
        Self {
            label: label.to_string(),
            hint: String::new(),
            icon: 0,
            action,
            separator: false,
            enabled: true,
        }
    }

    pub fn with_hint(mut self, hint: &str) -> Self {
        self.hint = hint.to_string();
        self
    }

    pub fn with_icon(mut self, icon: u8) -> Self {
        self.icon = icon;
        self
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    pub fn separator() -> Self {
        Self {
            label: String::new(),
            hint: String::new(),
            icon: 0,
            action: Action::None,
            separator: true,
            enabled: false,
        }
    }

    fn height(&self) -> i32 {
        if self.separator {
            SEPARATOR_HEIGHT
        } else {
            ROW_HEIGHT
        }
    }
}

pub struct Menu {
    pub items: Vec<Item>,
    pub rect: Rect,
    /// Index under the pointer, for hover highlighting.
    pub hovered: Option<usize>,
}

impl Menu {
    /// Place a menu with its top-left near (x, y), nudged to stay on screen.
    pub fn new(items: Vec<Item>, x: i32, y: i32, screen: Rect) -> Self {
        let width = items
            .iter()
            .map(|item| {
                let hint = if item.hint.is_empty() {
                    0
                } else {
                    item.hint.chars().count() as i32 * theme::CELL_W + 20
                };
                item.label.chars().count() as i32 * theme::CELL_W + hint + 46
            })
            .max()
            .unwrap_or(120)
            .clamp(140, 320);
        let height: i32 = items.iter().map(|item| item.height()).sum::<i32>() + PADDING * 2;

        // Prefer down-right, but flip rather than run off the edge.
        let x = if x + width > screen.right() {
            (x - width).max(screen.x)
        } else {
            x
        };
        let y = if y + height > screen.bottom() {
            (y - height).max(screen.y)
        } else {
            y
        };

        Self {
            items,
            rect: Rect::new(x, y, width, height),
            hovered: None,
        }
    }

    /// Which item is at this screen position, if any.
    pub fn item_at(&self, x: i32, y: i32) -> Option<usize> {
        if !self.rect.contains(x, y) {
            return None;
        }
        let mut cursor = self.rect.y + PADDING;
        for (index, item) in self.items.iter().enumerate() {
            let height = item.height();
            if y >= cursor && y < cursor + height {
                return if item.separator || !item.enabled {
                    None
                } else {
                    Some(index)
                };
            }
            cursor += height;
        }
        None
    }

    pub fn hover(&mut self, x: i32, y: i32) -> bool {
        let found = self.item_at(x, y);
        let changed = found != self.hovered;
        self.hovered = found;
        changed
    }

    pub fn draw(&self, surface: &mut Surface) {
        theme::draw_shadow(surface, self.rect);
        surface.panel(self.rect, palette::PANEL_RAISED, palette::AMBER_DIM);

        let mut y = self.rect.y + PADDING;
        for (index, item) in self.items.iter().enumerate() {
            if item.separator {
                surface.hline(
                    self.rect.x + 8,
                    y + SEPARATOR_HEIGHT / 2,
                    self.rect.w - 16,
                    palette::BORDER,
                );
                y += SEPARATOR_HEIGHT;
                continue;
            }

            let hovered = self.hovered == Some(index);
            if hovered {
                surface.fill_rect(
                    Rect::new(self.rect.x + 2, y, self.rect.w - 4, ROW_HEIGHT),
                    palette::rgb(0x27, 0x31, 0x50),
                );
            }

            let color = if !item.enabled {
                palette::TEXT_FAINT
            } else if hovered {
                palette::AMBER
            } else {
                palette::TEXT
            };

            if item.icon != 0 {
                surface.glyph(item.icon, self.rect.x + 8, y + 2, color, Weight::Regular, 1);
            }
            surface.text(&item.label, self.rect.x + 26, y + 2, color);
            if !item.hint.is_empty() {
                let width = item.hint.chars().count() as i32 * theme::CELL_W;
                surface.text(
                    &item.hint,
                    self.rect.right() - width - 10,
                    y + 2,
                    palette::TEXT_FAINT,
                );
            }
            y += ROW_HEIGHT;
        }
    }
}

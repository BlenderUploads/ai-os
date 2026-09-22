//! Settings: the things about this machine you are allowed to change.
//!
//! Three pages, driven the same way — a list of rows, one of them selected,
//! up/down to move and enter or a click to act. Keeping every page the same
//! shape means the whole app is one key handler and one renderer.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::audio;
use crate::gfx::draw::{Rect, Surface};
use crate::gfx::font::Weight;
use crate::gfx::palette::{self, Color};
use crate::gfx::{fb, modeset};
use crate::input::{Key, KeyEvent};
use crate::ui::theme::CELL_H;
use crate::ui::window::{App, AppResponse, WindowMouse};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Display,
    Sound,
    System,
}

const PAGES: [(Page, &str); 3] = [
    (Page::Display, "DISPLAY"),
    (Page::Sound, "SOUND"),
    (Page::System, "SYSTEM"),
];
const TAB_HEIGHT: i32 = 24;
const ROW_HEIGHT: i32 = CELL_H + 8;
const LIST_TOP: i32 = TAB_HEIGHT + 34;

/// What pressing enter (or clicking) on a row does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Nothing: a heading or a line of explanation.
    None,
    SetResolution(u32, u32),
    ToggleScanlines,
    SetVolume(u32),
    TestTone,
    Reboot,
    PowerOff,
}

struct Row {
    label: String,
    detail: String,
    action: Action,
    /// Drawn with the amber marker: the mode currently in use, the volume
    /// currently set, and so on.
    current: bool,
}

impl Row {
    fn heading(text: &str) -> Self {
        Self {
            label: text.to_string(),
            detail: String::new(),
            action: Action::None,
            current: false,
        }
    }

    fn note(text: &str) -> Self {
        Self {
            label: String::new(),
            detail: text.to_string(),
            action: Action::None,
            current: false,
        }
    }

    fn item(label: &str, detail: &str, action: Action) -> Self {
        Self {
            label: label.to_string(),
            detail: detail.to_string(),
            action,
            current: false,
        }
    }

    fn selectable(&self) -> bool {
        self.action != Action::None
    }
}

pub struct Settings {
    page: Page,
    selected: usize,
    scroll: usize,
    /// The last thing that happened, shown along the bottom.
    message: String,
    message_ok: bool,
    /// Geometry of the last surface drawn. The key handler needs both to work
    /// out what a click landed on and how far a page of scrolling goes, and
    /// neither is passed to it.
    last_width: i32,
    last_visible: usize,
    dirty: bool,
}

impl Settings {
    pub fn new() -> Self {
        let mut settings = Self {
            page: Page::Display,
            selected: 0,
            scroll: 0,
            message: String::new(),
            message_ok: true,
            last_width: 520,
            last_visible: 12,
            dirty: true,
        };
        settings.select_first();
        settings
    }

    fn rows(&self) -> Vec<Row> {
        match self.page {
            Page::Display => self.display_rows(),
            Page::Sound => self.sound_rows(),
            Page::System => self.system_rows(),
        }
    }

    fn display_rows(&self) -> Vec<Row> {
        let (width, height) = fb::dimensions();
        let (_, _, depth, fast) = fb::FRAMEBUFFER.lock().describe();

        let mut rows = Vec::new();
        rows.push(Row::heading("Screen"));
        rows.push(Row::note(&format!(
            "{}x{} at {} bits per pixel, {}",
            width,
            height,
            depth,
            if fast { "direct blit" } else { "converted" }
        )));
        rows.push(Row::note(&modeset::describe()));
        rows.push(Row::note(""));

        let modes = modeset::modes();
        if modes.is_empty() {
            rows.push(Row::heading("Resolution"));
            rows.push(Row::note(
                "This screen's mode cannot be changed while HALCYON is running.",
            ));
            if let Some(reason) = modeset::unavailable() {
                rows.push(Row::note(reason.describe()));
            }
            rows.push(Row::note(""));
            rows.push(Row::note(
                "The mode is set by GRUB before the kernel starts. Restart and",
            ));
            rows.push(Row::note(
                "use the boot menu's \"choose a screen resolution\" submenu.",
            ));
        } else {
            rows.push(Row::heading("Resolution"));
            for (mode_width, mode_height) in modes {
                let current = mode_width as usize == width && mode_height as usize == height;
                let mut row = Row::item(
                    &format!("{} x {}", mode_width, mode_height),
                    ratio_name(mode_width, mode_height),
                    Action::SetResolution(mode_width, mode_height),
                );
                row.current = current;
                rows.push(row);
            }
        }

        rows.push(Row::note(""));
        rows.push(Row::heading("Appearance"));
        let mut scanlines = Row::item(
            "CRT scanlines",
            if crate::gfx::crt::scanlines_enabled() {
                "on"
            } else {
                "off"
            },
            Action::ToggleScanlines,
        );
        scanlines.current = crate::gfx::crt::scanlines_enabled();
        rows.push(scanlines);
        rows
    }

    fn sound_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        rows.push(Row::heading("Device"));
        rows.push(Row::note(&match crate::drivers::ac97::describe() {
            Some(description) => description,
            None => "no AC'97 codec: this machine has no sound".to_string(),
        }));
        rows.push(Row::note(&audio::status()));

        if !audio::present() {
            return rows;
        }

        rows.push(Row::note(""));
        rows.push(Row::heading("Volume"));
        let current = crate::drivers::ac97::volume();
        for level in [0u32, 25, 50, 75, 100] {
            let mut row = Row::item(
                &format!("{:>3}%", level),
                if level == 0 { "muted" } else { "" },
                Action::SetVolume(level),
            );
            row.current = level == current;
            rows.push(row);
        }

        rows.push(Row::note(""));
        rows.push(Row::heading("Check"));
        rows.push(Row::item(
            "Play a test tone",
            "440 Hz for half a second",
            Action::TestTone,
        ));
        rows
    }

    fn system_rows(&self) -> Vec<Row> {
        let uptime = crate::arch::pit::uptime_ms() / 1000;
        let (total, total_unit) = crate::mm::format_bytes(crate::mm::FRAMES.lock().total_bytes());

        let mut rows = Vec::new();
        rows.push(Row::heading("This machine"));
        rows.push(Row::note(&format!("HALCYON v{}", crate::VERSION)));
        rows.push(Row::note(&format!(
            "up {}h {:02}m {:02}s, {} {} of RAM",
            uptime / 3600,
            (uptime / 60) % 60,
            uptime % 60,
            total,
            total_unit
        )));
        rows.push(Row::note(
            "Nothing here is written to disk, so none of it survives a restart.",
        ));

        rows.push(Row::note(""));
        rows.push(Row::heading("Power"));
        rows.push(Row::item("Restart", "", Action::Reboot));
        rows.push(Row::item("Shut down", "via ACPI", Action::PowerOff));
        rows
    }

    /// Put the selection on the first row that does anything.
    fn select_first(&mut self) {
        let rows = self.rows();
        self.selected = rows.iter().position(Row::selectable).unwrap_or(0);
        self.scroll = 0;
    }

    fn move_selection(&mut self, delta: i32) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let mut index = self.selected as i32;
        // Step past the headings and notes rather than letting the marker land
        // on something that cannot be pressed.
        for _ in 0..rows.len() {
            index += delta;
            if index < 0 {
                index = rows.len() as i32 - 1;
            } else if index >= rows.len() as i32 {
                index = 0;
            }
            if rows[index as usize].selectable() {
                self.selected = index as usize;
                return;
            }
        }
    }

    fn activate(&mut self, index: usize) {
        let rows = self.rows();
        let Some(row) = rows.get(index) else { return };
        match row.action {
            Action::None => {}
            Action::SetResolution(width, height) => {
                match crate::gfx::set_resolution(width, height) {
                    Ok(()) => {
                        self.set_message(&format!("now running at {}x{}", width, height), true)
                    }
                    Err(reason) => self.set_message(reason, false),
                }
            }
            Action::ToggleScanlines => {
                let on = crate::gfx::crt::toggle_scanlines();
                self.set_message(if on { "scanlines on" } else { "scanlines off" }, true);
            }
            Action::SetVolume(level) => {
                crate::drivers::ac97::set_volume(level);
                self.set_message(&format!("volume {}%", level), true);
            }
            Action::TestTone => {
                if audio::tone(440, 500) {
                    self.set_message("playing 440 Hz", true);
                } else {
                    self.set_message("there is no sound device to play it on", false);
                }
            }
            Action::Reboot => crate::arch::acpi::reboot(),
            Action::PowerOff => {
                crate::power_off();
                self.set_message("the firmware declined to power the machine down", false);
            }
        }
        self.dirty = true;
    }

    fn set_message(&mut self, text: &str, ok: bool) {
        self.message = text.to_string();
        self.message_ok = ok;
    }

    fn page_rects(&self, width: i32) -> Vec<(Page, Rect)> {
        let each = (width / PAGES.len() as i32).max(1);
        PAGES
            .iter()
            .enumerate()
            .map(|(index, (page, _))| (*page, Rect::new(index as i32 * each, 0, each, TAB_HEIGHT)))
            .collect()
    }

    /// Scroll just far enough that the selected row is on screen.
    fn reveal_selection(&mut self) {
        let visible = self.last_visible.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
        let rows = self.rows().len();
        self.scroll = self.scroll.min(rows.saturating_sub(visible.min(rows)));
    }

    fn visible_rows(&self, height: i32) -> usize {
        (((height - LIST_TOP - 26) / ROW_HEIGHT).max(1)) as usize
    }
}

/// The shape of a mode, which is more useful than its pixel count when you are
/// trying to work out which one matches the monitor in front of you.
fn ratio_name(width: u32, height: u32) -> &'static str {
    let ratio = width * 100 / height.max(1);
    match ratio {
        133 => "4:3",
        125 => "5:4",
        160 => "16:10",
        177 | 178 => "16:9",
        _ => "",
    }
}

impl App for Settings {
    fn draw(&mut self, surface: &mut Surface, _focused: bool) {
        surface.clear(palette::PANEL);
        let width = surface.width as i32;
        let height = surface.height as i32;
        self.last_width = width;

        surface.fill_rect(
            Rect::new(0, 0, width, TAB_HEIGHT),
            palette::rgb(0x0C, 0x11, 0x1E),
        );
        for (page, rect) in self.page_rects(width) {
            let active = page == self.page;
            if active {
                surface.fill_rect(rect, palette::PANEL);
                surface.hline(rect.x, rect.y, rect.w, palette::AMBER);
            }
            let label = PAGES
                .iter()
                .find(|(candidate, _)| *candidate == page)
                .map(|(_, label)| *label)
                .unwrap_or("");
            surface.text_ex(
                label,
                rect.x + 12,
                rect.y + 4,
                if active {
                    palette::AMBER
                } else {
                    palette::TEXT_DIM
                },
                if active {
                    Weight::Bold
                } else {
                    Weight::Regular
                },
                1,
            );
        }
        surface.hline(0, TAB_HEIGHT, width, palette::BORDER);

        let rows = self.rows();
        let visible = self.visible_rows(height);
        self.last_visible = visible;
        let start = self.scroll.min(rows.len().saturating_sub(1));

        let mut y = LIST_TOP - 22;
        for (offset, row) in rows.iter().skip(start).take(visible).enumerate() {
            let index = start + offset;
            let selected = index == self.selected && row.selectable();
            let rect = Rect::new(6, y, width - 12, ROW_HEIGHT - 2);

            if selected {
                surface.fill_rect(rect, palette::rgb(0x1E, 0x26, 0x3E));
                surface.fill_rect(Rect::new(rect.x, rect.y, 2, rect.h), palette::AMBER);
            }

            if !row.label.is_empty() {
                let (colour, weight): (Color, Weight) = if !row.selectable() {
                    (palette::CYAN, Weight::Bold)
                } else if selected {
                    (palette::TEXT_BRIGHT, Weight::Bold)
                } else {
                    (palette::TEXT, Weight::Regular)
                };
                surface.text_ex(&row.label, 18, y + 4, colour, weight, 1);
            }
            if !row.detail.is_empty() {
                surface.text(
                    &row.detail,
                    if row.label.is_empty() { 18 } else { 170 },
                    y + 4,
                    palette::TEXT_DIM,
                );
            }
            if row.current {
                surface.text_ex("in use", width - 70, y + 4, palette::AMBER, Weight::Bold, 1);
            }
            y += ROW_HEIGHT;
        }

        if rows.len() > visible {
            surface.text(
                &format!("{} more below", rows.len() - (start + visible)),
                width - 130,
                height - CELL_H - 3,
                palette::TEXT_FAINT,
            );
        }

        surface.hline(0, height - 24, width, palette::BORDER);
        if self.message.is_empty() {
            surface.text(
                "up/down to move, enter to apply, tab for the next page",
                12,
                height - CELL_H - 3,
                palette::TEXT_FAINT,
            );
        } else {
            surface.text(
                &self.message,
                12,
                height - CELL_H - 3,
                if self.message_ok {
                    palette::OK
                } else {
                    palette::ERROR
                },
            );
        }

        self.dirty = false;
    }

    fn on_key(&mut self, event: &KeyEvent, _response: &mut AppResponse) {
        if !event.pressed {
            return;
        }
        self.dirty = true;
        match event.key {
            Key::Tab => {
                let index = PAGES
                    .iter()
                    .position(|(page, _)| *page == self.page)
                    .unwrap_or(0);
                let step = if event.modifiers.shift {
                    PAGES.len() - 1
                } else {
                    1
                };
                self.page = PAGES[(index + step) % PAGES.len()].0;
                self.message.clear();
                self.select_first();
            }
            Key::Down => self.move_selection(1),
            Key::Up => self.move_selection(-1),
            Key::Home => self.select_first(),
            Key::PageDown => {
                for _ in 0..self.last_visible.max(1) {
                    self.move_selection(1);
                }
            }
            Key::PageUp => {
                for _ in 0..self.last_visible.max(1) {
                    self.move_selection(-1);
                }
            }
            Key::Enter => {
                let index = self.selected;
                self.activate(index);
            }
            Key::Char if event.character == Some(' ') => {
                let index = self.selected;
                self.activate(index);
            }
            _ => {}
        }

        self.reveal_selection();
    }

    fn on_mouse(&mut self, event: &WindowMouse, _response: &mut AppResponse) {
        if !event.pressed {
            return;
        }
        self.dirty = true;

        if event.y < TAB_HEIGHT {
            for (page, rect) in self.page_rects(self.last_width.max(1)) {
                if rect.contains(event.x, event.y) {
                    self.page = page;
                    self.message.clear();
                    self.select_first();
                    return;
                }
            }
            return;
        }

        let offset = (event.y - (LIST_TOP - 22)) / ROW_HEIGHT;
        if offset < 0 {
            return;
        }
        let index = self.scroll + offset as usize;
        let rows = self.rows();
        if rows.get(index).map(Row::selectable).unwrap_or(false) {
            self.selected = index;
            self.activate(index);
        }
    }

    fn tick(&mut self, _now_ms: u64, _response: &mut AppResponse) {
        // The sound and system pages show live numbers.
        if self.page != Page::Display {
            self.dirty = true;
        }
    }

    fn dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn min_size(&self) -> (i32, i32) {
        (520, 400)
    }
}

//! What this machine is made of: CPU, memory map, PCI bus, firmware.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::arch::{cpu, pit};
use crate::boot;
use crate::drivers::pci;
use crate::gfx::draw::{Rect, Surface};
use crate::gfx::font::{glyph, Weight};
use crate::gfx::palette::{self, Color};
use crate::input::{Key, KeyEvent};
use crate::mm;
use crate::ui::theme::CELL_H;
use crate::ui::window::{App, AppResponse, WindowMouse};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Cpu,
    Pci,
    Memory,
}

const TABS: [(Tab, &str); 3] = [
    (Tab::Cpu, "CPU"),
    (Tab::Pci, "PCI"),
    (Tab::Memory, "MEMORY"),
];
const TAB_HEIGHT: i32 = 24;

pub struct Devices {
    tab: Tab,
    /// PCI enumeration walks every bus, so it is done once and kept.
    pci_cache: Vec<String>,
    scroll: usize,
    dirty: bool,
}

impl Devices {
    pub fn new() -> Self {
        Self {
            tab: Tab::Cpu,
            pci_cache: pci::enumerate()
                .iter()
                .map(|device| device.describe())
                .collect(),
            scroll: 0,
            dirty: true,
        }
    }

    fn lines(&self) -> Vec<(String, Color)> {
        match self.tab {
            Tab::Cpu => self.cpu_lines(),
            Tab::Pci => self.pci_lines(),
            Tab::Memory => self.memory_lines(),
        }
    }

    fn cpu_lines(&self) -> Vec<(String, Color)> {
        let info = cpu::identify();
        let mut lines = Vec::new();
        let brand = if info.brand_str().is_empty() {
            info.vendor_str().to_string()
        } else {
            info.brand_str().to_string()
        };
        lines.push((brand, palette::AMBER));
        lines.push((String::new(), palette::TEXT));
        lines.push((
            format!("vendor          {}", info.vendor_str()),
            palette::TEXT,
        ));
        lines.push((
            format!(
                "family/model    {:#x} / {:#x}  stepping {}",
                info.family, info.model, info.stepping
            ),
            palette::TEXT,
        ));
        lines.push((String::new(), palette::TEXT));
        lines.push(("features".to_string(), palette::CYAN));
        for (name, present) in [
            ("NX  (no-execute pages)", info.has_nx),
            ("PAT (page attribute table)", info.has_pat),
            ("SSE2", info.has_sse2),
            ("APIC", info.has_apic),
            ("invariant TSC", info.has_invariant_tsc),
        ] {
            lines.push((
                format!("  {:<28}{}", name, if present { "yes" } else { "no" }),
                if present {
                    palette::TEXT
                } else {
                    palette::TEXT_FAINT
                },
            ));
        }
        lines.push((String::new(), palette::TEXT));
        lines.push(("timing".to_string(), palette::CYAN));
        lines.push((
            format!("  PIT                         {} Hz", pit::TICK_HZ),
            palette::TEXT,
        ));
        let uptime = pit::uptime_ms();
        lines.push((
            format!(
                "  uptime                      {}.{:03} s",
                uptime / 1000,
                uptime % 1000
            ),
            palette::TEXT,
        ));
        lines.push((
            format!("  timestamp counter           {}", cpu::rdtsc()),
            palette::TEXT_DIM,
        ));
        lines
    }

    fn pci_lines(&self) -> Vec<(String, Color)> {
        let mut lines = alloc::vec![(
            format!("{} device(s) on the PCI bus", self.pci_cache.len()),
            palette::AMBER
        )];
        lines.push((
            "bus:dev.fn  vendor:id   description".to_string(),
            palette::TEXT_FAINT,
        ));
        for entry in &self.pci_cache {
            lines.push((entry.clone(), palette::TEXT));
        }
        if self.pci_cache.is_empty() {
            lines.push((
                "nothing responded — this machine may have no PCI bus".to_string(),
                palette::WARN,
            ));
        }
        lines
    }

    fn memory_lines(&self) -> Vec<(String, Color)> {
        let frames = mm::FRAMES.lock();
        let (total, used, free) = (
            frames.total_bytes(),
            frames.used_bytes(),
            frames.free_bytes(),
        );
        let (bitmap_phys, bitmap_bytes) = frames.bitmap_location();
        drop(frames);
        let heap = mm::heap::ALLOCATOR.stats();

        let show = |label: &str, bytes: u64| {
            let (value, unit) = mm::format_bytes(bytes);
            format!("  {:<22}{:>6} {:<5}({} bytes)", label, value, unit, bytes)
        };

        let mut lines = alloc::vec![("physical memory".to_string(), palette::AMBER)];
        lines.push((show("total", total), palette::TEXT));
        lines.push((show("in use", used), palette::TEXT));
        lines.push((show("free", free), palette::TEXT));
        lines.push((
            format!(
                "  {:<22}{:#x} ({} bytes)",
                "frame bitmap at", bitmap_phys, bitmap_bytes
            ),
            palette::TEXT_DIM,
        ));

        lines.push((String::new(), palette::TEXT));
        lines.push(("kernel heap".to_string(), palette::AMBER));
        lines.push((show("mapped", heap.mapped), palette::TEXT));
        lines.push((show("allocated", heap.allocated as u64), palette::TEXT));
        lines.push((
            format!(
                "  {:<22}{} live, {} served, {} free blocks",
                "allocations", heap.live_allocations, heap.total_allocations, heap.free_blocks
            ),
            palette::TEXT,
        ));

        lines.push((String::new(), palette::TEXT));
        lines.push(("address space".to_string(), palette::AMBER));
        for (label, base) in [
            ("kernel image", boot::KERNEL_VMA),
            ("physical map", mm::paging::PHYSMAP_BASE),
            ("framebuffer", mm::paging::FRAMEBUFFER_BASE),
            ("heap", mm::paging::HEAP_BASE),
        ] {
            lines.push((format!("  {:<22}{:#018x}", label, base), palette::TEXT));
        }
        lines.push((
            format!(
                "  {:<22}{}",
                "framebuffer caching",
                if mm::paging::write_combining_available() {
                    "write-combining (PAT entry 4)"
                } else {
                    "uncached"
                }
            ),
            palette::TEXT_DIM,
        ));
        lines
    }

    fn tab_rects(&self, width: i32) -> Vec<(Tab, Rect)> {
        let each = (width / TABS.len() as i32).min(120);
        TABS.iter()
            .enumerate()
            .map(|(index, (tab, _))| (*tab, Rect::new(index as i32 * each, 0, each, TAB_HEIGHT)))
            .collect()
    }
}

impl App for Devices {
    fn draw(&mut self, surface: &mut Surface, _focused: bool) {
        surface.clear(palette::PANEL);
        let width = surface.width as i32;
        let height = surface.height as i32;

        surface.fill_rect(
            Rect::new(0, 0, width, TAB_HEIGHT),
            palette::rgb(0x0C, 0x11, 0x1E),
        );
        for ((tab, rect), (_, label)) in self.tab_rects(width).into_iter().zip(TABS.iter()) {
            let active = tab == self.tab;
            if active {
                surface.fill_rect(rect, palette::PANEL);
                surface.hline(rect.x, rect.y, rect.w, palette::AMBER);
            }
            surface.text_ex(
                label,
                rect.x + 10,
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

        let lines = self.lines();
        let visible = ((height - TAB_HEIGHT - 20) / CELL_H).max(1) as usize;
        let start = self.scroll.min(lines.len().saturating_sub(1));
        let mut y = TAB_HEIGHT + 6;
        for (text, colour) in lines.iter().skip(start).take(visible) {
            surface.text(text, 10, y, *colour);
            y += CELL_H;
        }

        if lines.len() > visible {
            let footer = format!(
                "{}-{} of {}   up/down or page keys to scroll",
                start + 1,
                (start + visible).min(lines.len()),
                lines.len()
            );
            surface.text(&footer, 10, height - CELL_H - 2, palette::TEXT_FAINT);
        }
        surface.glyph(
            glyph::ARROW_RIGHT,
            width - 30,
            height - CELL_H - 2,
            palette::TEXT_FAINT,
            Weight::Regular,
            1,
        );
        surface.text("tab", width - 22, height - CELL_H - 2, palette::TEXT_FAINT);

        self.dirty = false;
    }

    fn on_key(&mut self, event: &KeyEvent, _response: &mut AppResponse) {
        if !event.pressed {
            return;
        }
        self.dirty = true;
        match event.key {
            Key::Tab => {
                let index = TABS
                    .iter()
                    .position(|(tab, _)| *tab == self.tab)
                    .unwrap_or(0);
                self.tab = TABS[(index + 1) % TABS.len()].0;
                self.scroll = 0;
            }
            Key::Down => self.scroll += 1,
            Key::Up => self.scroll = self.scroll.saturating_sub(1),
            Key::PageDown => self.scroll += 10,
            Key::PageUp => self.scroll = self.scroll.saturating_sub(10),
            Key::Home => self.scroll = 0,
            _ => {}
        }
        // Clamp here rather than in draw, so a held key cannot run away.
        let count = self.lines().len();
        self.scroll = self.scroll.min(count.saturating_sub(1));
    }

    fn on_mouse(&mut self, event: &WindowMouse, _response: &mut AppResponse) {
        if !event.pressed || event.y >= TAB_HEIGHT {
            return;
        }
        // Width is not known here, so recompute from the last known layout.
        for (tab, rect) in self.tab_rects(event.x.max(1) + 400) {
            if rect.contains(event.x, event.y) {
                self.tab = tab;
                self.scroll = 0;
                self.dirty = true;
                return;
            }
        }
    }

    fn tick(&mut self, now_ms: u64, _response: &mut AppResponse) {
        // The CPU and memory tabs show live figures; refresh twice a second.
        if self.tab != Tab::Pci && now_ms % 500 < 20 {
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
        (520, 300)
    }
}

//! System monitor: memory, heap, threads, and a live load graph.

use alloc::format;
use alloc::vec::Vec;

use crate::arch::pit;
use crate::gfx::draw::{Rect, Surface};
use crate::gfx::font::Weight;
use crate::gfx::palette;
use crate::mm;
use crate::task;
use crate::ui::window::{App, AppResponse};

const HISTORY: usize = 120;

pub struct Monitor {
    dirty: bool,
    heap_history: Vec<u32>,
    last_sample: u64,
}

impl Monitor {
    pub fn new() -> Self {
        Self {
            dirty: true,
            heap_history: Vec::new(),
            last_sample: 0,
        }
    }
}

fn bar(surface: &mut Surface, rect: Rect, fraction: u32, color: palette::Color) {
    surface.fill_rect(rect, palette::PANEL_SUNKEN);
    let filled = (rect.w * fraction as i32 / 100).clamp(0, rect.w);
    surface.fill_rect(Rect::new(rect.x, rect.y, filled, rect.h), color);
    surface.stroke_rect(rect, palette::BORDER);
}

impl App for Monitor {
    fn draw(&mut self, surface: &mut Surface, _focused: bool) {
        let width = surface.width as i32;
        surface.clear(palette::PANEL);

        let frames = mm::FRAMES.lock();
        let total = frames.total_bytes();
        let used = frames.used_bytes();
        let usable = frames.usable_bytes().max(1);
        drop(frames);
        let heap = mm::heap::ALLOCATOR.stats();

        let mut y = 10;
        surface.text_ex("PHYSICAL MEMORY", 12, y, palette::CYAN, Weight::Bold, 1);
        y += 20;

        let percent = (used * 100 / usable) as u32;
        bar(surface, Rect::new(12, y, width - 24, 14), percent, palette::AMBER);
        y += 18;
        let (used_value, used_unit) = mm::format_bytes(used);
        let (total_value, total_unit) = mm::format_bytes(total);
        surface.text(
            &format!(
                "{} {} of {} {} in use  ({}%)",
                used_value, used_unit, total_value, total_unit, percent
            ),
            12,
            y,
            palette::TEXT,
        );
        y += 26;

        surface.text_ex("KERNEL HEAP", 12, y, palette::CYAN, Weight::Bold, 1);
        y += 20;
        let heap_percent = if heap.mapped > 0 {
            (heap.allocated as u64 * 100 / heap.mapped) as u32
        } else {
            0
        };
        bar(
            surface,
            Rect::new(12, y, width - 24, 14),
            heap_percent,
            palette::CYAN,
        );
        y += 18;
        let (mapped_value, mapped_unit) = mm::format_bytes(heap.mapped);
        surface.text(
            &format!(
                "{} live allocations, {} {} mapped, {} free blocks",
                heap.live_allocations, mapped_value, mapped_unit, heap.free_blocks
            ),
            12,
            y,
            palette::TEXT,
        );
        y += 16;
        surface.text(
            &format!("{} allocations served since boot", heap.total_allocations),
            12,
            y,
            palette::TEXT_DIM,
        );
        y += 26;

        // Heap history graph.
        let graph = Rect::new(12, y, width - 24, 46);
        surface.fill_rect(graph, palette::PANEL_SUNKEN);
        surface.stroke_rect(graph, palette::BORDER);
        if self.heap_history.len() > 1 {
            let peak = self.heap_history.iter().copied().max().unwrap_or(1).max(1);
            for (index, value) in self.heap_history.iter().enumerate() {
                let x = graph.x + 1 + (index as i32 * (graph.w - 2) / HISTORY as i32);
                let height = (*value as i64 * (graph.h - 2) as i64 / peak as i64) as i32;
                surface.vline(x, graph.bottom() - 1 - height, height, palette::CYAN_DIM);
            }
        }
        surface.text("heap in use", graph.x + 4, graph.y + 3, palette::TEXT_FAINT);
        y += 56;

        surface.text_ex("THREADS", 12, y, palette::CYAN, Weight::Bold, 1);
        y += 20;
        surface.text("id   name            state      ticks", 12, y, palette::TEXT_FAINT);
        y += 16;

        for thread in task::snapshot() {
            if y > surface.height as i32 - 34 {
                break;
            }
            let state = match thread.state {
                task::State::Running => "running",
                task::State::Ready => "ready",
                task::State::Sleeping(_) => "sleeping",
                task::State::Finished => "finished",
            };
            let color = if thread.current {
                palette::AMBER
            } else {
                palette::TEXT
            };
            surface.text(&format!("{:<4}", thread.id), 12, y, color);
            let name: alloc::string::String = thread.name.chars().take(15).collect();
            surface.text(&name, 12 + 5 * 8, y, color);
            surface.text(state, 12 + 21 * 8, y, palette::TEXT_DIM);
            surface.text(&format!("{}", thread.ticks_used), 12 + 32 * 8, y, palette::TEXT_DIM);
            y += 16;
        }

        y = surface.height as i32 - 22;
        surface.hline(0, y - 6, width, palette::BORDER);
        let uptime = pit::uptime_ms();
        surface.text(
            &format!(
                "uptime {}.{:03}s   {} context switches",
                uptime / 1000,
                uptime % 1000,
                task::total_switches()
            ),
            12,
            y,
            palette::TEXT_DIM,
        );

        self.dirty = false;
    }

    fn tick(&mut self, now_ms: u64, _response: &mut AppResponse) {
        // Sample four times a second rather than every frame.
        if now_ms.saturating_sub(self.last_sample) < 250 {
            return;
        }
        self.last_sample = now_ms;
        let heap = mm::heap::ALLOCATOR.stats();
        self.heap_history.push(heap.allocated as u32);
        while self.heap_history.len() > HISTORY {
            self.heap_history.remove(0);
        }
        self.dirty = true;
    }

    fn dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn min_size(&self) -> (i32, i32) {
        (440, 400)
    }
}

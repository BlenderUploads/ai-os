//! Programmable interval timer — HALCYON's heartbeat.
//!
//! Runs at 1000 Hz, which gives millisecond uptime, drives the scheduler's
//! pre-emption quantum, and paces the compositor.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::interrupts::{self, TrapFrame};
use crate::arch::port::outb;

const CHANNEL0: u16 = 0x40;
const COMMAND: u16 = 0x43;
const BASE_FREQUENCY: u32 = 1_193_182;

pub const TICK_HZ: u32 = 1000;

static TICKS: AtomicU64 = AtomicU64::new(0);

/// Optional hook the scheduler installs; returns the frame to resume.
static mut TICK_HOOK: Option<fn(&mut TrapFrame) -> *mut TrapFrame> = None;

pub fn init() {
    let divisor = (BASE_FREQUENCY / TICK_HZ) as u16;
    unsafe {
        // channel 0, lo/hi byte, mode 2 (rate generator), binary
        outb(COMMAND, 0x34);
        outb(CHANNEL0, (divisor & 0xFF) as u8);
        outb(CHANNEL0, (divisor >> 8) as u8);
    }
    interrupts::register_irq(interrupts::IRQ_TIMER, on_tick);
}

pub fn set_tick_hook(hook: fn(&mut TrapFrame) -> *mut TrapFrame) {
    crate::arch::port::without_interrupts(|| unsafe { TICK_HOOK = Some(hook) });
}

fn on_tick(frame: &mut TrapFrame) -> *mut TrapFrame {
    TICKS.fetch_add(1, Ordering::Relaxed);
    match unsafe { TICK_HOOK } {
        Some(hook) => hook(frame),
        None => frame as *mut TrapFrame,
    }
}

/// Milliseconds since the timer was started.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn uptime_ms() -> u64 {
    ticks() * 1000 / TICK_HZ as u64
}

/// Busy-wait. Only for driver bring-up; threads should sleep instead.
pub fn delay_ms(ms: u64) {
    let target = ticks() + ms * TICK_HZ as u64 / 1000;
    while ticks() < target {
        core::hint::spin_loop();
    }
}

//! PC speaker, driven from PIT channel 2.
//!
//! Many laptops have no speaker wired up any more, in which case all of this
//! is a harmless no-op.

use crate::arch::pit;
use crate::arch::port::{inb, outb};

const PIT_CHANNEL2: u16 = 0x42;
const PIT_COMMAND: u16 = 0x43;
const SPEAKER_GATE: u16 = 0x61;
const BASE_FREQUENCY: u32 = 1_193_182;

pub fn start(frequency: u32) {
    if frequency < 20 || frequency > 20_000 {
        return;
    }
    let divisor = (BASE_FREQUENCY / frequency) as u16;
    unsafe {
        // Channel 2, lo/hi byte, square wave.
        outb(PIT_COMMAND, 0xB6);
        outb(PIT_CHANNEL2, (divisor & 0xFF) as u8);
        outb(PIT_CHANNEL2, (divisor >> 8) as u8);
        // Bits 0 and 1 gate the timer through to the speaker.
        let gate = inb(SPEAKER_GATE);
        outb(SPEAKER_GATE, gate | 0x03);
    }
}

pub fn stop() {
    unsafe {
        let gate = inb(SPEAKER_GATE);
        outb(SPEAKER_GATE, gate & !0x03);
    }
}

/// Blocking tone. Only for short UI sounds.
pub fn beep(frequency: u32, milliseconds: u64) {
    start(frequency);
    pit::delay_ms(milliseconds);
    stop();
}

/// The rising three-note figure HALCYON plays once the desktop is up.
pub fn boot_chime() {
    for (frequency, duration) in [(523, 90), (659, 90), (988, 140)] {
        beep(frequency, duration);
        pit::delay_ms(20);
    }
}

pub fn blip() {
    beep(1400, 12);
}

pub fn error_tone() {
    beep(220, 90);
}

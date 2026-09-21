//! Sound output.
//!
//! A ring of interleaved stereo samples sits between whatever is generating
//! audio and the driver that plays it. Producers never block: if the ring is
//! full the surplus is dropped, because a producer that stalls waiting for the
//! codec is worse than a producer that skips a few milliseconds.
//!
//! There is exactly one consumer — the AC'97 service thread — and in practice
//! exactly one producer, but the ring is locked either way since the two run
//! on different threads.

use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::drivers::ac97;
use crate::sync::SpinLock;

pub use crate::drivers::ac97::SAMPLE_RATE;

/// Stereo frames the ring holds: about 170 ms at 48 kHz, which is enough to
/// ride out a slow frame from the producer without the sound tearing.
pub const RING_FRAMES: usize = 8192;

struct Ring {
    samples: [i16; RING_FRAMES * 2],
    read: usize,
    filled: usize,
}

impl Ring {
    const fn new() -> Self {
        Self {
            samples: [0; RING_FRAMES * 2],
            read: 0,
            filled: 0,
        }
    }
}

static RING: SpinLock<Ring> = SpinLock::new(Ring::new());
static PRESENT: AtomicBool = AtomicBool::new(false);
/// Set by the first successful write. Until then the codec stays idle.
static STARTED: AtomicBool = AtomicBool::new(false);
static DELIVERED: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Probe for an audio device and, if there is one, start feeding it.
pub fn init() -> bool {
    let present = ac97::init();
    PRESENT.store(present, Ordering::Release);
    if present {
        ac97::spawn_service();
    }
    present
}

pub fn present() -> bool {
    PRESENT.load(Ordering::Acquire)
}

/// Has anything been played yet? The driver thread stays asleep until it has.
pub fn started() -> bool {
    STARTED.load(Ordering::Acquire)
}

/// Stereo frames currently waiting to be played.
pub fn queued_frames() -> usize {
    RING.lock().filled
}

/// Room for how many more stereo frames.
pub fn space_frames() -> usize {
    RING_FRAMES - RING.lock().filled
}

/// Queue interleaved stereo samples. Returns the number of *frames* accepted;
/// whatever did not fit is dropped.
pub fn write(samples: &[i16]) -> usize {
    if !present() {
        return 0;
    }
    let wanted = samples.len() / 2;
    let taken = {
        let mut ring = RING.lock();
        let taken = wanted.min(RING_FRAMES - ring.filled);
        let mut cursor = (ring.read + ring.filled) % RING_FRAMES;
        for frame in 0..taken {
            ring.samples[cursor * 2] = samples[frame * 2];
            ring.samples[cursor * 2 + 1] = samples[frame * 2 + 1];
            cursor = (cursor + 1) % RING_FRAMES;
        }
        ring.filled += taken;
        taken
    };
    if taken < wanted {
        DROPPED.fetch_add((wanted - taken) as u64, Ordering::Relaxed);
    }
    if taken > 0 {
        STARTED.store(true, Ordering::Release);
    }
    taken
}

/// Fill `destination` with interleaved stereo samples, returning how many
/// frames were available. Called by the driver thread, with its own lock held.
pub(crate) fn read_into(destination: &mut [i16]) -> usize {
    let wanted = destination.len() / 2;
    let taken = {
        let mut ring = RING.lock();
        let taken = wanted.min(ring.filled);
        let mut cursor = ring.read;
        for frame in 0..taken {
            destination[frame * 2] = ring.samples[cursor * 2];
            destination[frame * 2 + 1] = ring.samples[cursor * 2 + 1];
            cursor = (cursor + 1) % RING_FRAMES;
        }
        ring.read = cursor;
        ring.filled -= taken;
        taken
    };
    DELIVERED.fetch_add(taken as u64, Ordering::Relaxed);
    taken
}

/// Queue a test tone, on its own thread.
///
/// The ring holds 170 ms, so anything longer has to be fed in instalments, and
/// doing that from the desktop thread would stop the desktop. Returns false if
/// there is nothing to play it on.
pub fn tone(hz: u32, ms: u32) -> bool {
    if !present() {
        return false;
    }
    let hz = hz.clamp(20, 12_000) as u64;
    let ms = ms.clamp(10, 5_000) as u64;
    crate::task::spawn("tone", tone_thread, (ms << 32) | hz);
    true
}

/// A triangle wave. The kernel is soft-float, so this is all integers — and a
/// triangle is a good deal kinder to the ear than the square wave the PC
/// speaker manages.
extern "C" fn tone_thread(argument: u64) {
    const CHUNK: usize = 256;
    const AMPLITUDE: i32 = 7000;
    /// Frames of fade at each end, so the tone does not start with a click.
    const FADE: u64 = SAMPLE_RATE as u64 / 200;

    let hz = argument & 0xFFFF_FFFF;
    let ms = argument >> 32;
    let total = SAMPLE_RATE as u64 * ms / 1000;
    // 16.16 phase step, which keeps the pitch exact without a division per
    // sample and without touching the FPU.
    let step = (hz << 16) / SAMPLE_RATE as u64;

    let mut buffer = [0i16; CHUNK * 2];
    let mut produced = 0u64;
    let mut phase = 0u64;

    while produced < total {
        let frames = CHUNK.min((total - produced) as usize);
        for frame in 0..frames {
            let index = produced + frame as u64;
            let position = (phase & 0xFFFF) as i32;
            let wave = if position < 0x8000 {
                position * 2 - 0x8000
            } else {
                0x8000 - (position - 0x8000) * 2
            };
            let fade = (index + 1).min(total - index).min(FADE);
            let value = (wave * AMPLITUDE / 0x8000) * fade as i32 / FADE as i32;
            buffer[frame * 2] = value as i16;
            buffer[frame * 2 + 1] = value as i16;
            phase += step;
        }

        let mut offset = 0;
        while offset < frames {
            let accepted = write(&buffer[offset * 2..frames * 2]);
            if accepted == 0 {
                // The ring is full, which means it is playing. Wait for room.
                crate::task::sleep_ms(8);
            } else {
                offset += accepted;
            }
        }
        produced += frames as u64;
    }

    crate::task::exit_current()
}

/// One line for the device browser and the system monitor.
pub fn status() -> String {
    if !present() {
        return String::from("audio: no device");
    }
    let delivered = DELIVERED.load(Ordering::Relaxed);
    let dropped = DROPPED.load(Ordering::Relaxed);
    format!(
        "audio: {} Hz stereo, {} frames played, {} queued, {} dropped",
        SAMPLE_RATE,
        delivered,
        queued_frames(),
        dropped
    )
}

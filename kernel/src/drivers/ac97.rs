//! Intel AC'97 audio output.
//!
//! Just enough of the specification to get PCM out of a codec: no capture, no
//! MIDI, no interrupt line. A kernel thread polls the current-index register
//! every few milliseconds and tops the descriptor list back up from the ring
//! in [`crate::audio`], which is a great deal simpler than an IRQ handler and,
//! with 10 ms buffers, indistinguishable in practice.
//!
//! This is the one device HALCYON drives by DMA, so it is also the one place
//! that needs physically contiguous memory below 4 GiB — the descriptor
//! addresses are 32 bits wide.

use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::arch::pit;
use crate::arch::port::{inb, inl, outb, outl, outw};
use crate::drivers::pci;
use crate::mm::frame::FRAME_SIZE;
use crate::mm::{paging, FRAMES};
use crate::sync::SpinLock;
use crate::task;

/// The rate AC'97 codecs always support. Variable-rate audio is optional and
/// unnecessary: the mixer resamples to this instead.
pub const SAMPLE_RATE: u32 = 48_000;

/// Stereo frames in one DMA buffer. 512 at 48 kHz is 10.7 ms.
const BUFFER_FRAMES: usize = 512;
const BUFFER_SAMPLES: usize = BUFFER_FRAMES * 2;
const BUFFER_BYTES: usize = BUFFER_SAMPLES * 2;
/// Fixed by the hardware: the buffer descriptor list is 32 entries.
const BUFFER_COUNT: usize = 32;
/// How far ahead of the play cursor to keep the list topped up (~64 ms).
const TARGET_QUEUED: usize = 6;

const DATA_BYTES: usize = BUFFER_BYTES * BUFFER_COUNT;
/// One page for the descriptor list, the rest for the audio itself.
const DMA_PAGES: u64 = 1 + (DATA_BYTES as u64 + FRAME_SIZE - 1) / FRAME_SIZE;

// Native Audio Mixer registers, off BAR0.
const NAM_RESET: u16 = 0x00;
const NAM_MASTER_VOLUME: u16 = 0x02;
const NAM_PCM_VOLUME: u16 = 0x18;

// Native Audio Bus Master registers, off BAR1.
const NABM_PCM_OUT: u16 = 0x10;
const NABM_GLOB_CNT: u16 = 0x2C;
const NABM_GLOB_STA: u16 = 0x30;

// ...and the registers within one bus-master "box".
const BOX_BDBAR: u16 = 0x00;
const BOX_CIV: u16 = 0x04;
const BOX_LVI: u16 = 0x05;
const BOX_STATUS: u16 = 0x06;
const BOX_CONTROL: u16 = 0x0B;

const CONTROL_RUN: u8 = 1 << 0;
const CONTROL_RESET: u8 = 1 << 1;
/// Every write-one-to-clear bit in the status register.
const STATUS_ACK_ALL: u16 = 0x1C;

const GLOB_CNT_COLD_RESET: u32 = 1 << 1;
const GLOB_STA_PRIMARY_READY: u32 = 1 << 8;

/// One buffer descriptor, exactly as the hardware reads it.
#[repr(C)]
#[derive(Clone, Copy)]
struct Descriptor {
    address: u32,
    /// In samples, not frames, and not bytes.
    samples: u16,
    flags: u16,
}

pub struct Ac97 {
    mixer: u16,
    bus_master: u16,
    data: *mut i16,
    dma_phys: u64,
    /// Next descriptor slot to refill.
    next: usize,
    running: bool,
    vendor: u16,
    id: u16,
    codec_ready: bool,
}

// The raw pointers are into the physical map and live for as long as the
// machine does; nothing else ever aliases them.
unsafe impl Send for Ac97 {}

static DEVICE: SpinLock<Option<Ac97>> = SpinLock::new(None);
/// Master output level, 0-100. Kept outside the device so it survives a reset
/// and can be read back without knowing whether there is a codec at all.
static VOLUME: AtomicU32 = AtomicU32::new(100);

pub fn volume() -> u32 {
    VOLUME.load(Ordering::Relaxed)
}

/// Set the master output level, 0 (muted) to 100.
pub fn set_volume(percent: u32) {
    let percent = percent.min(100);
    VOLUME.store(percent, Ordering::Relaxed);
    if let Some(device) = DEVICE.lock().as_ref() {
        device.apply_volume(percent);
    }
}

/// Find an AC'97 codec, wake it up, and hand back whether there is one.
pub fn init() -> bool {
    let devices = pci::enumerate();
    let Some(device) = devices
        .iter()
        .find(|device| device.class == 0x04 && device.subclass == 0x01)
    else {
        return false;
    };

    let (Some(mixer), Some(bus_master)) = (device.io_base(0), device.io_base(1)) else {
        crate::serial_println!("[snd ] AC'97 at {} has memory-mapped BARs; skipping", {
            device.describe()
        });
        return false;
    };
    device.enable_bus_master();

    let Some(dma_phys) = FRAMES.lock().alloc_contiguous(DMA_PAGES) else {
        crate::serial_println!("[snd ] no contiguous memory for the AC'97 buffers");
        return false;
    };
    // The descriptor list stores 32-bit addresses, so the whole region has to
    // sit below 4 GiB. The allocator scans upwards from zero, so this only
    // fails on a machine with no low memory left at all.
    if dma_phys + DMA_PAGES * FRAME_SIZE > u32::MAX as u64 {
        let mut frames = FRAMES.lock();
        for page in 0..DMA_PAGES {
            frames.free(dma_phys + page * FRAME_SIZE);
        }
        crate::serial_println!("[snd ] AC'97 buffers landed above 4 GiB; skipping");
        return false;
    }

    let data_phys = dma_phys + FRAME_SIZE;
    // The list itself is never touched again once the hardware has its
    // address, so it does not need keeping.
    let descriptors = unsafe { paging::phys_ptr::<Descriptor>(dma_phys) };
    let data = unsafe { paging::phys_ptr::<i16>(data_phys) };

    unsafe {
        core::ptr::write_bytes(data as *mut u8, 0, DATA_BYTES);
        for index in 0..BUFFER_COUNT {
            descriptors.add(index).write(Descriptor {
                address: (data_phys + (index * BUFFER_BYTES) as u64) as u32,
                samples: BUFFER_SAMPLES as u16,
                flags: 0,
            });
        }
    }

    let mut card = Ac97 {
        mixer,
        bus_master,
        data,
        dma_phys,
        next: 0,
        running: false,
        vendor: device.vendor,
        id: device.id,
        codec_ready: false,
    };
    card.reset();

    crate::serial_println!(
        "[snd ] AC'97 {:04x}:{:04x} mixer {:#06x} bus master {:#06x} codec {} buffers at {:#x}",
        card.vendor,
        card.id,
        mixer,
        bus_master,
        if card.codec_ready { "ready" } else { "silent" },
        dma_phys
    );

    *DEVICE.lock() = Some(card);
    true
}

impl Ac97 {
    /// Write the master attenuator: six bits a side, 1.5 dB a step, zero
    /// loudest, and bit 15 mutes outright.
    fn apply_volume(&self, percent: u32) {
        let value = if percent == 0 {
            0x8000
        } else {
            let attenuation = ((100 - percent) * 63 / 100) as u16;
            (attenuation << 8) | attenuation
        };
        unsafe { outw(self.mixer + NAM_MASTER_VOLUME, value) };
    }

    fn pcm_out(&self) -> u16 {
        self.bus_master + NABM_PCM_OUT
    }

    fn reset(&mut self) {
        unsafe {
            // Bring the controller out of cold reset with interrupts left off.
            outl(self.bus_master + NABM_GLOB_CNT, GLOB_CNT_COLD_RESET);
            for _ in 0..100 {
                if inl(self.bus_master + NABM_GLOB_STA) & GLOB_STA_PRIMARY_READY != 0 {
                    self.codec_ready = true;
                    break;
                }
                pit::delay_ms(2);
            }

            // Any write to the codec's register 0 resets it.
            outw(self.mixer + NAM_RESET, 0);
            pit::delay_ms(2);

            // PCM out is a 5-bit attenuator where 0x08 is unity and 0x00 would
            // add 12 dB of gain the mixer has no headroom for. The master
            // level is whatever Settings last asked for.
            outw(self.mixer + NAM_PCM_VOLUME, 0x0808);

            // Reset the PCM-out engine, which also zeroes CIV and LVI.
            outb(self.pcm_out() + BOX_CONTROL, CONTROL_RESET);
            for _ in 0..100 {
                if inb(self.pcm_out() + BOX_CONTROL) & CONTROL_RESET == 0 {
                    break;
                }
                pit::delay_ms(1);
            }

            outl(self.pcm_out() + BOX_BDBAR, self.dma_phys as u32);
            outb(self.pcm_out() + BOX_LVI, 0);
        }
        self.apply_volume(VOLUME.load(Ordering::Relaxed));
        self.next = 0;
        self.running = false;
    }

    /// Refill every descriptor the engine has finished with.
    fn service(&mut self) {
        let box_base = self.pcm_out();
        unsafe {
            // Clear whatever is latched. A sticky error bit stops the engine
            // dead, and there is no interrupt handler to notice.
            outw(box_base + BOX_STATUS, STATUS_ACK_ALL);

            let current = inb(box_base + BOX_CIV) as usize % BUFFER_COUNT;
            // The target is well under BUFFER_COUNT, so next == current can
            // only mean the engine has caught up, never that it is a whole
            // lap behind.
            let mut queued = (self.next + BUFFER_COUNT - current) % BUFFER_COUNT;

            while queued < TARGET_QUEUED {
                let slot = self.next;
                let buffer = core::slice::from_raw_parts_mut(
                    self.data.add(slot * BUFFER_SAMPLES),
                    BUFFER_SAMPLES,
                );
                let frames = crate::audio::read_into(buffer);
                // Short reads and silence are the same thing to the codec.
                buffer[frames * 2..].fill(0);
                self.next = (slot + 1) % BUFFER_COUNT;
                queued += 1;
            }

            outb(
                box_base + BOX_LVI,
                ((self.next + BUFFER_COUNT - 1) % BUFFER_COUNT) as u8,
            );
            if !self.running {
                outb(box_base + BOX_CONTROL, CONTROL_RUN);
                self.running = true;
            }
        }
    }

    fn describe(&self) -> String {
        format!(
            "AC'97 {:04x}:{:04x}  mixer {:#06x}  bus master {:#06x}  {} Hz stereo{}",
            self.vendor,
            self.id,
            self.mixer,
            self.bus_master,
            SAMPLE_RATE,
            if self.codec_ready {
                ""
            } else {
                "  (codec never reported ready)"
            }
        )
    }
}

/// Keeps the descriptor list fed. Idle until something actually asks for sound,
/// so a machine that never plays anything never touches the codec.
extern "C" fn service_thread(_: u64) {
    loop {
        if crate::audio::started() {
            if let Some(device) = DEVICE.lock().as_mut() {
                device.service();
            }
            task::sleep_ms(4);
        } else {
            task::sleep_ms(50);
        }
    }
}

/// Start the thread that keeps the hardware fed.
pub fn spawn_service() {
    task::spawn("audio", service_thread, 0);
}

pub fn describe() -> Option<String> {
    DEVICE.lock().as_ref().map(|device| device.describe())
}

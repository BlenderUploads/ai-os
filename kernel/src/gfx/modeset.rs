//! Changing the screen resolution after boot.
//!
//! The mode HALCYON starts in comes from GRUB, which sets it before the kernel
//! has run a single instruction. Changing it afterwards normally means calling
//! the VESA BIOS, and the VESA BIOS is 16-bit code that cannot be called from
//! long mode without an emulator.
//!
//! There is one way out that needs no BIOS at all: the Bochs DISPI interface,
//! a pair of I/O ports that take a width, a height and a depth directly. It is
//! implemented by the standard display adapter in QEMU, Bochs and VirtualBox —
//! so a VM can change resolution while it runs, and a real machine cannot. The
//! Settings app says which of those it is looking at rather than offering
//! something that will not work.

use alloc::string::String;
use alloc::vec::Vec;

use crate::arch::port::{inw, outw};
use crate::boot::FramebufferInfo;
use crate::drivers::pci;
use crate::sync::SpinLock;

const DISPI_INDEX: u16 = 0x01CE;
const DISPI_DATA: u16 = 0x01CF;

const INDEX_ID: u16 = 0;
const INDEX_XRES: u16 = 1;
const INDEX_YRES: u16 = 2;
const INDEX_BPP: u16 = 3;
const INDEX_ENABLE: u16 = 4;
const INDEX_BANK: u16 = 5;
const INDEX_VIRT_WIDTH: u16 = 6;
const INDEX_VIRT_HEIGHT: u16 = 7;
const INDEX_X_OFFSET: u16 = 8;
const INDEX_Y_OFFSET: u16 = 9;
const INDEX_VIDEO_MEMORY_64K: u16 = 10;

const ENABLE_ENABLED: u16 = 0x01;
const ENABLE_LFB: u16 = 0x40;

/// The interface has been revised a few times; every revision answers with one
/// of these, and anything else means the ports belong to something else.
const ID_MIN: u16 = 0xB0C0;
const ID_MAX: u16 = 0xB0C5;

/// The modes on offer, largest first. Anything the adapter has no memory for,
/// or that would not fit the mapped framebuffer window, is filtered out.
const CANDIDATES: [(u32, u32); 10] = [
    (1920, 1200),
    (1920, 1080),
    (1680, 1050),
    (1600, 900),
    (1440, 900),
    (1366, 768),
    (1280, 1024),
    (1280, 720),
    (1024, 768),
    (800, 600),
];

#[derive(Clone, Copy)]
pub struct Adapter {
    /// Revision of the DISPI interface the adapter reports.
    pub id: u16,
    /// Linear framebuffer base, which does not move when the mode changes.
    pub lfb_phys: u64,
    pub vram_bytes: u64,
    pub vendor: u16,
    pub device: u16,
}

/// Why mode setting is unavailable, in a form the Settings app can print.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// No DISPI registers answered. A real graphics card, in other words.
    NoDispi,
    /// The registers answered but do not describe the screen we are drawing on.
    NotOurFramebuffer,
    /// There is no framebuffer at all.
    NoFramebuffer,
}

impl Unavailable {
    pub fn describe(self) -> &'static str {
        match self {
            Unavailable::NoDispi => "this adapter has no mode registers HALCYON knows how to drive",
            Unavailable::NotOurFramebuffer => {
                "no display adapter claims the framebuffer the bootloader handed over"
            }
            Unavailable::NoFramebuffer => "there is no framebuffer",
        }
    }
}

static ADAPTER: SpinLock<Option<Adapter>> = SpinLock::new(None);
static STATUS: SpinLock<Option<Unavailable>> = SpinLock::new(Some(Unavailable::NoFramebuffer));
/// Bytes of address space the framebuffer window covers, which caps how large
/// a mode may be: a bigger one would draw past the end of the mapping.
static WINDOW_BYTES: SpinLock<u64> = SpinLock::new(0);

unsafe fn read(index: u16) -> u16 {
    outw(DISPI_INDEX, index);
    inw(DISPI_DATA)
}

unsafe fn write(index: u16, value: u16) {
    outw(DISPI_INDEX, index);
    outw(DISPI_DATA, value);
}

/// Adapters known to implement the DISPI registers. The probe is gated on this
/// list rather than done blind: ports 0x1CE/0x1CF belong to the emulated VGA
/// here, but on some real chipsets they are a configuration index/data pair,
/// and a hobby OS has no business writing to those on someone's laptop.
const KNOWN: [(u16, u16); 3] = [
    (0x1234, 0x1111), // QEMU and Bochs standard VGA
    (0x80EE, 0xBEEF), // VirtualBox VBoxVGA
    (0x15AD, 0x0405), // VMware SVGA II, and VirtualBox's VMSVGA
];

/// Look for an adapter we can drive. `window_bytes` is how much of the
/// framebuffer aperture the kernel mapped, which bounds the modes on offer.
pub fn init(framebuffer: Option<FramebufferInfo>, window_bytes: u64) {
    *WINDOW_BYTES.lock() = window_bytes;

    let Some(framebuffer) = framebuffer.filter(|fb| fb.addr != 0) else {
        *STATUS.lock() = Some(Unavailable::NoFramebuffer);
        return;
    };

    // Find the display controller first, and confirm its aperture really is
    // the screen being drawn on: a machine with two adapters must not have the
    // mode changed on the one nobody is looking at. Any of the memory BARs may
    // be the aperture — the standard adapter puts it first, VMware's puts the
    // FIFO there and the framebuffer second.
    let devices = pci::enumerate();
    let Some(device) = devices.iter().find(|device| {
        device.class == 0x03
            && (0..6).any(|index| {
                let bar = device.bar(index);
                bar & 1 == 0 && (bar & 0xFFFF_FFF0) as u64 == framebuffer.addr & !0xF
            })
    }) else {
        *STATUS.lock() = Some(Unavailable::NotOurFramebuffer);
        crate::serial_println!("[gfx ] no display BAR matches the framebuffer at {:#x}", {
            framebuffer.addr
        });
        return;
    };

    if !KNOWN.contains(&(device.vendor, device.id)) {
        *STATUS.lock() = Some(Unavailable::NoDispi);
        crate::serial_println!(
            "[gfx ] display {:04x}:{:04x} is not a known DISPI adapter; mode is fixed",
            device.vendor,
            device.id
        );
        return;
    }

    let id = unsafe { read(INDEX_ID) };
    if !(ID_MIN..=ID_MAX).contains(&id) {
        *STATUS.lock() = Some(Unavailable::NoDispi);
        crate::serial_println!("[gfx ] no Bochs DISPI interface (id {:#06x})", id);
        return;
    }

    let vram_bytes = unsafe { read(INDEX_VIDEO_MEMORY_64K) } as u64 * 64 * 1024;

    let adapter = Adapter {
        id,
        lfb_phys: framebuffer.addr,
        vram_bytes,
        vendor: device.vendor,
        device: device.id,
    };
    crate::serial_println!(
        "[gfx ] DISPI {:#06x} on {:04x}:{:04x}, {} MiB of video memory, window {} MiB",
        id,
        adapter.vendor,
        adapter.device,
        vram_bytes / (1024 * 1024),
        window_bytes / (1024 * 1024)
    );

    *ADAPTER.lock() = Some(adapter);
    *STATUS.lock() = None;
}

pub fn adapter() -> Option<Adapter> {
    *ADAPTER.lock()
}

/// Why the resolution cannot be changed, or None when it can be.
pub fn unavailable() -> Option<Unavailable> {
    *STATUS.lock()
}

pub fn describe() -> String {
    match adapter() {
        Some(adapter) => alloc::format!(
            "{:04x}:{:04x} via Bochs DISPI {:#06x}, {} MiB video memory",
            adapter.vendor,
            adapter.device,
            adapter.id,
            adapter.vram_bytes / (1024 * 1024)
        ),
        None => match unavailable() {
            Some(reason) => String::from(reason.describe()),
            None => String::from("unknown"),
        },
    }
}

/// Is `width` x `height` at 32 bits per pixel something this machine can do?
pub fn supports(width: u32, height: u32) -> bool {
    let Some(adapter) = adapter() else {
        return false;
    };
    let bytes = width as u64 * height as u64 * 4;
    bytes <= adapter.vram_bytes && bytes <= *WINDOW_BYTES.lock()
}

/// Every offered mode, largest first.
pub fn modes() -> Vec<(u32, u32)> {
    if adapter().is_none() {
        return Vec::new();
    }
    CANDIDATES
        .iter()
        .copied()
        .filter(|(width, height)| supports(*width, *height))
        .collect()
}

/// Switch the adapter to a new mode.
///
/// Returns the new (width, height, pitch); the framebuffer's physical address
/// does not move, so the kernel's mapping stays valid and only the geometry
/// the compositor works from has to change.
pub fn set_mode(width: u32, height: u32) -> Result<(u32, u32, u32), &'static str> {
    if adapter().is_none() {
        return Err("no adapter that can change modes");
    }
    if !supports(width, height) {
        return Err("not enough video memory for that mode");
    }

    unsafe {
        // The adapter must be disabled while the geometry registers change;
        // writing them live is undefined and, on Bochs, ignored.
        write(INDEX_ENABLE, 0);
        write(INDEX_XRES, width as u16);
        write(INDEX_YRES, height as u16);
        write(INDEX_BPP, 32);
        write(INDEX_BANK, 0);
        write(INDEX_VIRT_WIDTH, width as u16);
        write(INDEX_VIRT_HEIGHT, height as u16);
        write(INDEX_X_OFFSET, 0);
        write(INDEX_Y_OFFSET, 0);
        write(INDEX_ENABLE, ENABLE_ENABLED | ENABLE_LFB);

        // Read the geometry back rather than assuming it took: an adapter is
        // free to round a mode, and drawing to a guess would corrupt the
        // screen rather than merely look wrong.
        let actual_width = read(INDEX_XRES) as u32;
        let actual_height = read(INDEX_YRES) as u32;
        let bpp = read(INDEX_BPP) as u32;
        let virtual_width = read(INDEX_VIRT_WIDTH) as u32;

        if bpp != 32 || actual_width == 0 || actual_height == 0 {
            return Err("the adapter refused the mode");
        }
        let pitch = virtual_width.max(actual_width) * 4;
        if actual_height as u64 * pitch as u64 > *WINDOW_BYTES.lock() {
            return Err("the adapter chose a mode larger than the mapped window");
        }

        crate::serial_println!(
            "[gfx ] mode set to {}x{}x{} pitch {}",
            actual_width,
            actual_height,
            bpp,
            pitch
        );
        Ok((actual_width, actual_height, pitch))
    }
}

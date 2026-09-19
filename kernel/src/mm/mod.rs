//! Memory management: physical frames, paging, and the kernel heap.

pub mod frame;
pub mod heap;
pub mod paging;

use crate::boot::BootInfo;
use crate::sync::SpinLock;

pub static FRAMES: SpinLock<frame::FrameAllocator> = SpinLock::new(frame::FrameAllocator::empty());

pub struct MemoryInfo {
    pub space: paging::AddressSpace,
    pub total_bytes: u64,
    pub usable_bytes: u64,
}

/// Bring up the whole memory subsystem.
///
/// Must run while the bootstrap identity map is still active: the frame
/// allocator writes its bitmap through a physical address, and the new page
/// tables are built the same way. `paging::init` switches CR3 and moves the
/// physical window to PHYSMAP_BASE before returning.
pub unsafe fn init(info: &BootInfo) -> MemoryInfo {
    let (total_bytes, usable_bytes, space) = {
        let mut frames = FRAMES.lock();
        frames.init(info);
        let total = frames.total_bytes();
        let usable = frames.usable_bytes();
        let space = paging::init(info, &mut frames);
        (total, usable, space)
    };

    heap::ALLOCATOR.init();

    MemoryInfo {
        space,
        total_bytes,
        usable_bytes,
    }
}

/// Human-readable byte count into a small stack buffer.
pub fn format_bytes(bytes: u64) -> (u64, &'static str) {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024 && unit < UNITS.len() - 1 {
        value /= 1024;
        unit += 1;
    }
    (value, UNITS[unit])
}

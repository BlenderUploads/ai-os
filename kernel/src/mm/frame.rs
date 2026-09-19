//! Physical frame allocator.
//!
//! A flat bitmap over every 4 KiB frame the firmware reported. One bit per
//! frame, set means in use. The bitmap itself is carved out of the first
//! usable region big enough to hold it that sits clear of the kernel image and
//! the bootloader's modules.

use crate::boot::{BootInfo, MemoryKind};

pub const FRAME_SIZE: u64 = 4096;

pub struct FrameAllocator {
    bitmap: *mut u64,
    /// Where the bitmap physically lives. The pointer above starts out as this
    /// same value (the bootstrap identity-maps it) and is rebased onto the
    /// physical map once the real address space is live.
    bitmap_phys: u64,
    bitmap_words: usize,
    total_frames: u64,
    used_frames: u64,
    /// Where the last search stopped, so sequential allocation stays O(1).
    hint: u64,
    /// Frames that exist but are not usable RAM (holes, MMIO, firmware).
    reserved_frames: u64,
}

unsafe impl Send for FrameAllocator {}

impl FrameAllocator {
    pub const fn empty() -> Self {
        Self {
            bitmap: core::ptr::null_mut(),
            bitmap_phys: 0,
            bitmap_words: 0,
            total_frames: 0,
            used_frames: 0,
            hint: 0,
            reserved_frames: 0,
        }
    }

    #[inline]
    fn test(&self, frame: u64) -> bool {
        let word = (frame / 64) as usize;
        let bit = frame % 64;
        unsafe { *self.bitmap.add(word) & (1 << bit) != 0 }
    }

    #[inline]
    fn set(&mut self, frame: u64) {
        let word = (frame / 64) as usize;
        let bit = frame % 64;
        unsafe { *self.bitmap.add(word) |= 1 << bit };
    }

    #[inline]
    fn clear(&mut self, frame: u64) {
        let word = (frame / 64) as usize;
        let bit = frame % 64;
        unsafe { *self.bitmap.add(word) &= !(1 << bit) };
    }

    fn mark_range(&mut self, start: u64, end: u64, used: bool) {
        let first = start / FRAME_SIZE;
        let last = (end + FRAME_SIZE - 1) / FRAME_SIZE;
        for frame in first..last.min(self.total_frames) {
            let was_used = self.test(frame);
            if used && !was_used {
                self.set(frame);
                self.used_frames += 1;
            } else if !used && was_used {
                self.clear(frame);
                self.used_frames -= 1;
            }
        }
    }

    /// Build the allocator from the firmware memory map.
    ///
    /// Called while the bootstrap's identity mapping is still active, so
    /// physical addresses are directly writable.
    pub unsafe fn init(&mut self, info: &BootInfo) {
        let mut highest = 0u64;
        for region in info.regions() {
            if region.kind == MemoryKind::Usable {
                highest = highest.max(region.base + region.length);
            }
        }
        self.total_frames = highest / FRAME_SIZE;
        self.bitmap_words = ((self.total_frames + 63) / 64) as usize;
        let bitmap_bytes = (self.bitmap_words * 8) as u64;

        // The bitmap must live somewhere already-usable and out of the way of
        // the kernel, the modules and the low 1 MiB.
        let (kernel_start, kernel_end) = crate::boot::kernel_image_extent();
        let barrier = kernel_end.max(info.reclaim_after).max(0x100000);
        let mut bitmap_phys = 0u64;
        for region in info.regions() {
            if region.kind != MemoryKind::Usable {
                continue;
            }
            let candidate = align_up(region.base.max(barrier), FRAME_SIZE);
            if candidate + bitmap_bytes <= region.base + region.length {
                bitmap_phys = candidate;
                break;
            }
        }
        assert!(bitmap_phys != 0, "no room for the physical frame bitmap");

        self.bitmap_phys = bitmap_phys;
        self.bitmap = bitmap_phys as *mut u64;

        // Start with everything in use, then release what the firmware calls
        // usable. That way holes in the map are reserved by construction.
        core::ptr::write_bytes(self.bitmap, 0xFF, self.bitmap_words);
        self.used_frames = self.total_frames;

        for region in info.regions() {
            if region.kind == MemoryKind::Usable {
                self.mark_range(region.base, region.base + region.length, false);
            }
        }
        self.reserved_frames = self.used_frames;

        // Now claim back everything that must not be handed out.
        self.mark_range(0, 0x100000, true); // real-mode memory, BIOS structures
        self.mark_range(kernel_start, kernel_end, true);
        self.mark_range(bitmap_phys, bitmap_phys + bitmap_bytes, true);
        for module in info.modules() {
            self.mark_range(module.start, module.end, true);
        }
        self.hint = 0;
    }

    /// Re-point the bitmap at its physical-map address.
    ///
    /// Must be called immediately after CR3 is switched: the bitmap pointer
    /// was a bare physical address that only worked under the bootstrap's
    /// identity mapping, and the very next allocation would fault without this.
    pub unsafe fn rebase(&mut self, phys_offset: u64) {
        self.bitmap = (phys_offset + self.bitmap_phys) as *mut u64;
    }

    pub fn bitmap_location(&self) -> (u64, usize) {
        (self.bitmap_phys, self.bitmap_words * 8)
    }

    pub fn alloc(&mut self) -> Option<u64> {
        for _ in 0..2 {
            while self.hint < self.total_frames {
                // Skip 64 frames at a time when a whole word is taken.
                let word = (self.hint / 64) as usize;
                if unsafe { *self.bitmap.add(word) } == u64::MAX {
                    self.hint = (word as u64 + 1) * 64;
                    continue;
                }
                if !self.test(self.hint) {
                    let frame = self.hint;
                    self.set(frame);
                    self.used_frames += 1;
                    self.hint += 1;
                    return Some(frame * FRAME_SIZE);
                }
                self.hint += 1;
            }
            self.hint = 0; // wrap once
        }
        None
    }

    /// Allocate `count` physically contiguous frames.
    pub fn alloc_contiguous(&mut self, count: u64) -> Option<u64> {
        if count == 0 {
            return None;
        }
        let mut start = 0u64;
        let mut run = 0u64;
        for frame in 0..self.total_frames {
            if self.test(frame) {
                run = 0;
            } else {
                if run == 0 {
                    start = frame;
                }
                run += 1;
                if run == count {
                    for f in start..start + count {
                        self.set(f);
                    }
                    self.used_frames += count;
                    return Some(start * FRAME_SIZE);
                }
            }
        }
        None
    }

    pub fn free(&mut self, phys: u64) {
        let frame = phys / FRAME_SIZE;
        if frame < self.total_frames && self.test(frame) {
            self.clear(frame);
            self.used_frames -= 1;
            self.hint = self.hint.min(frame);
        }
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_frames * FRAME_SIZE
    }

    /// Bytes that are real, usable RAM (excludes firmware holes).
    pub fn usable_bytes(&self) -> u64 {
        (self.total_frames - self.reserved_frames) * FRAME_SIZE
    }

    pub fn used_bytes(&self) -> u64 {
        (self.used_frames - self.reserved_frames) * FRAME_SIZE
    }

    pub fn free_bytes(&self) -> u64 {
        (self.total_frames - self.used_frames) * FRAME_SIZE
    }
}

#[inline]
pub const fn align_up(value: u64, align: u64) -> u64 {
    (value + align - 1) & !(align - 1)
}

#[inline]
pub const fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

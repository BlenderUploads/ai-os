//! 4-level paging.
//!
//! The bootstrap left us with a crude 4 GiB identity map made of 2 MiB pages.
//! Here we build the real address space and switch to it:
//!
//!   0xFFFFFFFF80000000  kernel image, per-section permissions (text RX,
//!                       rodata R, data/bss RW+NX)
//!   0xFFFF800000000000  direct map of all physical RAM (RW+NX, 2 MiB pages)
//!   0xFFFF900000000000  the framebuffer, write-combining where PAT allows
//!   0xFFFFA00000000000  kernel heap, grown on demand
//!
//! Dropping the identity map means a stray write through a low pointer faults
//! instead of quietly corrupting the first 4 GiB of the machine.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::cpu;
use crate::boot::{BootInfo, KERNEL_VMA};
use crate::mm::frame::{align_up, FrameAllocator, FRAME_SIZE};

pub const PRESENT: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
pub const USER: u64 = 1 << 2;
pub const WRITE_THROUGH: u64 = 1 << 3;
pub const CACHE_DISABLE: u64 = 1 << 4;
pub const HUGE: u64 = 1 << 7;
pub const GLOBAL: u64 = 1 << 8;
/// PAT bit for 4 KiB entries (it is bit 12 on huge pages).
pub const PAT_4K: u64 = 1 << 7;
pub const NO_EXECUTE: u64 = 1 << 63;

const ADDRESS_MASK: u64 = 0x000F_FFFF_FFFF_F000;

pub const PHYSMAP_BASE: u64 = 0xFFFF_8000_0000_0000;
pub const FRAMEBUFFER_BASE: u64 = 0xFFFF_9000_0000_0000;
pub const HEAP_BASE: u64 = 0xFFFF_A000_0000_0000;
pub const HEAP_INITIAL: u64 = 8 * 1024 * 1024;
pub const HEAP_MAX: u64 = 512 * 1024 * 1024;

/// Added to a physical address to reach it through the current mapping. Zero
/// while the bootstrap identity map is live, PHYSMAP_BASE afterwards.
static PHYS_OFFSET: AtomicU64 = AtomicU64::new(0);

/// Set once PAT entry 4 is programmed to write-combining.
static WC_AVAILABLE: AtomicU64 = AtomicU64::new(0);

#[inline]
pub fn phys_to_virt(phys: u64) -> u64 {
    PHYS_OFFSET.load(Ordering::Relaxed) + phys
}

/// Reinterpret a physical address as a readable/writable pointer.
///
/// # Safety
/// The caller must ensure the physical range is mapped and not aliased in a
/// way that breaks Rust's aliasing rules.
#[inline]
pub unsafe fn phys_ptr<T>(phys: u64) -> *mut T {
    phys_to_virt(phys) as *mut T
}

extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __bss_end: u8;
}

unsafe fn zero_frame(phys: u64) {
    core::ptr::write_bytes(phys_ptr::<u8>(phys), 0, FRAME_SIZE as usize);
}

unsafe fn entry_ptr(table_phys: u64, index: usize) -> *mut u64 {
    phys_ptr::<u64>(table_phys).add(index)
}

/// Walk one level down, allocating the next table if it is missing.
unsafe fn next_table(table_phys: u64, index: usize, frames: &mut FrameAllocator) -> u64 {
    let entry = entry_ptr(table_phys, index);
    if *entry & PRESENT == 0 {
        let frame = frames
            .alloc()
            .expect("out of physical frames while mapping");
        zero_frame(frame);
        // Intermediate entries stay permissive; the leaf decides. NX on a
        // parent would forbid execution of everything beneath it.
        *entry = frame | PRESENT | WRITABLE;
        frame
    } else {
        assert!(*entry & HUGE == 0, "tried to descend through a huge page");
        *entry & ADDRESS_MASK
    }
}

#[inline]
fn indices(virt: u64) -> (usize, usize, usize, usize) {
    (
        ((virt >> 39) & 0x1FF) as usize,
        ((virt >> 30) & 0x1FF) as usize,
        ((virt >> 21) & 0x1FF) as usize,
        ((virt >> 12) & 0x1FF) as usize,
    )
}

pub unsafe fn map_page(pml4: u64, virt: u64, phys: u64, flags: u64, frames: &mut FrameAllocator) {
    let (i4, i3, i2, i1) = indices(virt);
    let pdpt = next_table(pml4, i4, frames);
    let pd = next_table(pdpt, i3, frames);
    let pt = next_table(pd, i2, frames);
    *entry_ptr(pt, i1) = (phys & ADDRESS_MASK) | flags | PRESENT;
}

/// Map a 2 MiB page. Both addresses must be 2 MiB aligned.
pub unsafe fn map_huge(pml4: u64, virt: u64, phys: u64, flags: u64, frames: &mut FrameAllocator) {
    let (i4, i3, i2, _) = indices(virt);
    let pdpt = next_table(pml4, i4, frames);
    let pd = next_table(pdpt, i3, frames);
    *entry_ptr(pd, i2) = (phys & ADDRESS_MASK) | flags | HUGE | PRESENT;
}

pub unsafe fn map_range(
    pml4: u64,
    virt: u64,
    phys: u64,
    size: u64,
    flags: u64,
    frames: &mut FrameAllocator,
) {
    let pages = align_up(size, FRAME_SIZE) / FRAME_SIZE;
    for page in 0..pages {
        let offset = page * FRAME_SIZE;
        map_page(pml4, virt + offset, phys + offset, flags, frames);
    }
}

/// Is `virt` currently mapped?
pub unsafe fn translate(pml4: u64, virt: u64) -> Option<u64> {
    let (i4, i3, i2, i1) = indices(virt);
    let pml4e = *entry_ptr(pml4, i4);
    if pml4e & PRESENT == 0 {
        return None;
    }
    let pdpte = *entry_ptr(pml4e & ADDRESS_MASK, i3);
    if pdpte & PRESENT == 0 {
        return None;
    }
    if pdpte & HUGE != 0 {
        return Some((pdpte & ADDRESS_MASK) + (virt & 0x3FFF_FFFF));
    }
    let pde = *entry_ptr(pdpte & ADDRESS_MASK, i2);
    if pde & PRESENT == 0 {
        return None;
    }
    if pde & HUGE != 0 {
        return Some((pde & ADDRESS_MASK) + (virt & 0x1F_FFFF));
    }
    let pte = *entry_ptr(pde & ADDRESS_MASK, i1);
    if pte & PRESENT == 0 {
        return None;
    }
    Some((pte & ADDRESS_MASK) + (virt & 0xFFF))
}

#[inline]
pub fn invalidate(virt: u64) {
    unsafe { core::arch::asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags)) };
}

pub fn active_pml4() -> u64 {
    let cr3: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack)) };
    cr3 & ADDRESS_MASK
}

/// Program PAT entry 4 to write-combining, leaving the architectural defaults
/// for entries 0-3 alone. Framebuffer writes then stream instead of going out
/// one uncached store at a time, which is the difference between a usable and
/// an unusable desktop on real hardware.
fn enable_write_combining() -> bool {
    let cpu = cpu::identify();
    if !cpu.has_pat {
        return false;
    }
    const IA32_PAT: u32 = 0x277;
    unsafe {
        let current = cpu::read_msr(IA32_PAT);
        // Entry 4 occupies bits 39:32. 0x01 == WC.
        let updated = (current & !(0xFFu64 << 32)) | (0x01u64 << 32);
        cpu::write_msr(IA32_PAT, updated);
    }
    WC_AVAILABLE.store(1, Ordering::Relaxed);
    true
}

pub fn write_combining_available() -> bool {
    WC_AVAILABLE.load(Ordering::Relaxed) != 0
}

pub struct AddressSpace {
    pub pml4: u64,
    pub framebuffer_virt: u64,
    pub framebuffer_len: u64,
    pub physmap_bytes: u64,
    pub write_combining: bool,
}

/// Build the kernel address space and switch to it.
///
/// Runs with the bootstrap identity map active, so `PHYS_OFFSET` is still 0
/// and physical addresses can be written directly.
pub unsafe fn init(info: &BootInfo, frames: &mut FrameAllocator) -> AddressSpace {
    let write_combining = enable_write_combining();

    let pml4 = frames.alloc().expect("no frame for PML4");
    zero_frame(pml4);

    // --- kernel image, one section at a time so permissions are honest ---
    let text_start = &__text_start as *const u8 as u64;
    let text_end = &__text_end as *const u8 as u64;
    let rodata_start = &__rodata_start as *const u8 as u64;
    let rodata_end = &__rodata_end as *const u8 as u64;
    let data_start = &__data_start as *const u8 as u64;
    let bss_end = &__bss_end as *const u8 as u64;

    let map_section = |pml4: u64, start: u64, end: u64, flags: u64, frames: &mut FrameAllocator| {
        let start = start & !(FRAME_SIZE - 1);
        let end = align_up(end, FRAME_SIZE);
        let mut virt = start;
        while virt < end {
            map_page(pml4, virt, virt - KERNEL_VMA, flags | GLOBAL, frames);
            virt += FRAME_SIZE;
        }
    };

    map_section(pml4, text_start, text_end, PRESENT, frames);
    map_section(pml4, rodata_start, rodata_end, PRESENT | NO_EXECUTE, frames);
    map_section(
        pml4,
        data_start,
        bss_end,
        PRESENT | WRITABLE | NO_EXECUTE,
        frames,
    );

    // --- direct map of physical RAM, 2 MiB pages ---
    let physmap_bytes = align_up(frames.total_bytes(), 0x20_0000);
    let mut offset = 0u64;
    while offset < physmap_bytes {
        map_huge(
            pml4,
            PHYSMAP_BASE + offset,
            offset,
            PRESENT | WRITABLE | NO_EXECUTE | GLOBAL,
            frames,
        );
        offset += 0x20_0000;
    }

    // --- framebuffer ---
    let (framebuffer_virt, framebuffer_len) = match info.framebuffer {
        Some(fb) if fb.addr != 0 => {
            let len = align_up(fb.pitch as u64 * fb.height as u64, FRAME_SIZE);
            let cache_flags = if write_combining {
                PAT_4K
            } else {
                CACHE_DISABLE
            };
            let base = fb.addr & !(FRAME_SIZE - 1);
            let skew = fb.addr - base;
            map_range(
                pml4,
                FRAMEBUFFER_BASE,
                base,
                len + skew,
                PRESENT | WRITABLE | NO_EXECUTE | GLOBAL | cache_flags,
                frames,
            );
            (FRAMEBUFFER_BASE + skew, len)
        }
        _ => (0, 0),
    };

    // --- switch ---
    // From here on the identity map is gone: any physical address must be
    // reached through PHYSMAP_BASE. The frame allocator's bitmap pointer is a
    // bare physical address, so it has to be rebased before the next
    // allocation touches it.
    core::arch::asm!("mov cr3, {}", in(reg) pml4, options(nostack, preserves_flags));
    PHYS_OFFSET.store(PHYSMAP_BASE, Ordering::Relaxed);
    frames.rebase(PHYSMAP_BASE);

    AddressSpace {
        pml4,
        framebuffer_virt,
        framebuffer_len,
        physmap_bytes,
        write_combining,
    }
}

/// Back `size` bytes of kernel virtual space with freshly allocated frames.
/// Used to grow the heap.
pub unsafe fn map_anonymous(
    pml4: u64,
    virt: u64,
    size: u64,
    frames: &mut FrameAllocator,
) -> Result<(), ()> {
    let pages = align_up(size, FRAME_SIZE) / FRAME_SIZE;
    for page in 0..pages {
        let Some(phys) = frames.alloc() else {
            return Err(());
        };
        let target = virt + page * FRAME_SIZE;
        map_page(
            pml4,
            target,
            phys,
            PRESENT | WRITABLE | NO_EXECUTE | GLOBAL,
            frames,
        );
        core::ptr::write_bytes(target as *mut u8, 0, FRAME_SIZE as usize);
        invalidate(target);
    }
    Ok(())
}

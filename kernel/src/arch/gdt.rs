//! Global descriptor table and task state segment.
//!
//! Long mode barely uses segmentation, but we still need a GDT the kernel owns
//! (the bootstrap's lives in identity-mapped memory we intend to reclaim) and a
//! TSS to supply the interrupt stack table. IST1 gives the double-fault handler
//! a known-good stack, which is the difference between seeing a useful error
//! and watching the machine triple-fault.

use core::arch::asm;
use core::mem::size_of;

pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
pub const USER_DATA: u16 = 0x18 | 3;
pub const USER_CODE: u16 = 0x20 | 3;
pub const TSS_SELECTOR: u16 = 0x28;

/// Dedicated stacks for faults that cannot trust the interrupted stack.
const IST_STACK_SIZE: usize = 16 * 1024;
static mut DOUBLE_FAULT_STACK: [u8; IST_STACK_SIZE] = [0; IST_STACK_SIZE];
static mut PAGE_FAULT_STACK: [u8; IST_STACK_SIZE] = [0; IST_STACK_SIZE];

pub const IST_DOUBLE_FAULT: u8 = 1;
pub const IST_PAGE_FAULT: u8 = 2;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct TaskStateSegment {
    reserved0: u32,
    rsp: [u64; 3],
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

impl TaskStateSegment {
    const fn new() -> Self {
        Self {
            reserved0: 0,
            rsp: [0; 3],
            reserved1: 0,
            ist: [0; 7],
            reserved2: 0,
            reserved3: 0,
            iomap_base: size_of::<TaskStateSegment>() as u16,
        }
    }
}

static mut TSS: TaskStateSegment = TaskStateSegment::new();

/// null, kernel code, kernel data, user data, user code, TSS (two slots)
static mut GDT: [u64; 7] = [0; 7];

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

/// Build a 64-bit code/data descriptor.
const fn segment(access: u64, flags: u64) -> u64 {
    // limit 0xFFFFF, base 0 — ignored in long mode but kept well-formed.
    0x0000_FFFF | (access << 40) | (flags << 52) | (0xF << 48)
}

pub fn init() {
    unsafe {
        let tss_addr = &raw const TSS as u64;
        let tss_limit = (size_of::<TaskStateSegment>() - 1) as u64;

        // Interrupt stacks grow down from the top of each buffer.
        let df_top = (&raw const DOUBLE_FAULT_STACK as u64) + IST_STACK_SIZE as u64;
        let pf_top = (&raw const PAGE_FAULT_STACK as u64) + IST_STACK_SIZE as u64;
        TSS.ist[(IST_DOUBLE_FAULT - 1) as usize] = df_top & !0xF;
        TSS.ist[(IST_PAGE_FAULT - 1) as usize] = pf_top & !0xF;

        let gdt = &mut *(&raw mut GDT);
        gdt[0] = 0;
        // access: present | S | exec | rw ; flags: long mode
        gdt[1] = segment(0x9A, 0xA); // kernel code
        gdt[2] = segment(0x92, 0xC); // kernel data
        gdt[3] = segment(0xF2, 0xC); // user data   (DPL 3)
        gdt[4] = segment(0xFA, 0xA); // user code   (DPL 3)

        // A 64-bit TSS descriptor occupies two GDT slots.
        gdt[5] = tss_limit
            | ((tss_addr & 0xFFFF) << 16)
            | (((tss_addr >> 16) & 0xFF) << 32)
            | (0x89 << 40) // present, type = available 64-bit TSS
            | (((tss_addr >> 24) & 0xFF) << 56);
        gdt[6] = tss_addr >> 32;

        let pointer = DescriptorTablePointer {
            limit: (size_of::<[u64; 7]>() - 1) as u16,
            base: &raw const GDT as u64,
        };

        asm!("lgdt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));

        // Reload CS with a far return; the data selectors can just be written.
        asm!(
            "push {code:r}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            code = in(reg) KERNEL_CODE as u64,
            tmp = lateout(reg) _,
            options(preserves_flags),
        );
        asm!(
            "mov ds, {0:x}",
            "mov es, {0:x}",
            "mov ss, {0:x}",
            "mov fs, {0:x}",
            "mov gs, {0:x}",
            in(reg) KERNEL_DATA,
            options(nostack, preserves_flags),
        );

        asm!("ltr {0:x}", in(reg) TSS_SELECTOR, options(nostack, preserves_flags));
    }
}

/// Point the TSS at the stack to use when an interrupt arrives while in ring 3.
/// Unused until HALCYON grows user mode, but the plumbing belongs with the TSS.
pub fn set_kernel_stack(stack_top: u64) {
    unsafe { TSS.rsp[0] = stack_top }
}

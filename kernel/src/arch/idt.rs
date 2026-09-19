//! Interrupt descriptor table.
//!
//! All 256 vectors point at uniform assembly stubs (see `interrupts.rs`), each
//! of which pushes its vector number and a (possibly synthetic) error code
//! before entering the common dispatcher.

use core::arch::asm;
use core::mem::size_of;

use super::gdt;

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    fn set(&mut self, handler: u64, ist: u8) {
        self.offset_low = handler as u16;
        self.selector = gdt::KERNEL_CODE;
        self.ist = ist & 0x7;
        // present | DPL 0 | 64-bit interrupt gate (clears IF on entry)
        self.type_attr = 0x8E;
        self.offset_mid = (handler >> 16) as u16;
        self.offset_high = (handler >> 32) as u32;
        self.reserved = 0;
    }
}

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

static mut IDT: [IdtEntry; 256] = [IdtEntry {
    offset_low: 0,
    selector: 0,
    ist: 0,
    type_attr: 0,
    offset_mid: 0,
    offset_high: 0,
    reserved: 0,
}; 256];

extern "C" {
    static isr_stub_table: u8;
    static isr_stub_table_end: u8;
}

/// Every stub is padded to this stride so the table needs no per-vector labels.
/// `interrupts.rs` static-asserts the same value on the assembly side.
pub const STUB_STRIDE: u64 = 16;

pub fn init() {
    unsafe {
        let base = &isr_stub_table as *const u8 as u64;
        let end = &isr_stub_table_end as *const u8 as u64;

        // The stubs are addressed arithmetically; if one ever outgrows its
        // 16-byte slot every later vector would point into the middle of
        // another stub. Catch that here rather than at the first interrupt.
        assert!(
            end - base == 256 * STUB_STRIDE,
            "ISR stub table is not 256 * STUB_STRIDE bytes"
        );

        let idt = &mut *(&raw mut IDT);
        for (vector, entry) in idt.iter_mut().enumerate() {
            let ist = match vector {
                8 => gdt::IST_DOUBLE_FAULT,
                14 => gdt::IST_PAGE_FAULT,
                _ => 0,
            };
            entry.set(base + vector as u64 * STUB_STRIDE, ist);
        }

        let pointer = DescriptorTablePointer {
            limit: (size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: &raw const IDT as u64,
        };
        asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
    }
}

//! The 8259A pair.
//!
//! HALCYON deliberately uses the legacy PIC rather than the APIC: it is
//! single-CPU, and the PIC is the interrupt controller that every machine old
//! enough to be interesting still implements correctly.

use crate::arch::port::{inb, io_wait, outb};

const MASTER_CMD: u16 = 0x20;
const MASTER_DATA: u16 = 0x21;
const SLAVE_CMD: u16 = 0xA0;
const SLAVE_DATA: u16 = 0xA1;

const ICW1_INIT: u8 = 0x10;
const ICW1_ICW4: u8 = 0x01;
const ICW4_8086: u8 = 0x01;

const EOI: u8 = 0x20;
const READ_ISR: u8 = 0x0B;

/// Vectors 32..47 — clear of the CPU's reserved 0..31.
pub const MASTER_OFFSET: u8 = 32;
pub const SLAVE_OFFSET: u8 = 40;

pub fn init() {
    unsafe {
        // Remap both chips, then mask everything; drivers unmask what they own.
        outb(MASTER_CMD, ICW1_INIT | ICW1_ICW4);
        io_wait();
        outb(SLAVE_CMD, ICW1_INIT | ICW1_ICW4);
        io_wait();

        outb(MASTER_DATA, MASTER_OFFSET);
        io_wait();
        outb(SLAVE_DATA, SLAVE_OFFSET);
        io_wait();

        outb(MASTER_DATA, 4); // slave is on IRQ2
        io_wait();
        outb(SLAVE_DATA, 2); // slave identity
        io_wait();

        outb(MASTER_DATA, ICW4_8086);
        io_wait();
        outb(SLAVE_DATA, ICW4_8086);
        io_wait();

        // Mask all but the cascade line.
        outb(MASTER_DATA, 0xFF & !(1 << 2));
        outb(SLAVE_DATA, 0xFF);
    }
}

pub fn mask(irq: u8) {
    unsafe {
        let (port, bit) = if irq < 8 {
            (MASTER_DATA, irq)
        } else {
            (SLAVE_DATA, irq - 8)
        };
        outb(port, inb(port) | (1 << bit));
    }
}

pub fn unmask(irq: u8) {
    unsafe {
        let (port, bit) = if irq < 8 {
            (MASTER_DATA, irq)
        } else {
            (SLAVE_DATA, irq - 8)
        };
        outb(port, inb(port) & !(1 << bit));
        if irq >= 8 {
            // The cascade line must be open for any slave IRQ to arrive.
            outb(MASTER_DATA, inb(MASTER_DATA) & !(1 << 2));
        }
    }
}

pub fn end_of_interrupt(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(SLAVE_CMD, EOI);
        }
        outb(MASTER_CMD, EOI);
    }
}

/// IRQ7 and IRQ15 can fire spuriously. A real interrupt sets the matching bit
/// in the in-service register; a spurious one does not, and must not be
/// acknowledged.
pub fn is_spurious(irq: u8) -> bool {
    unsafe {
        match irq {
            7 => {
                outb(MASTER_CMD, READ_ISR);
                inb(MASTER_CMD) & (1 << 7) == 0
            }
            15 => {
                outb(SLAVE_CMD, READ_ISR);
                if inb(SLAVE_CMD) & (1 << 7) == 0 {
                    // Still acknowledge the master, which saw a real cascade.
                    outb(MASTER_CMD, EOI);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

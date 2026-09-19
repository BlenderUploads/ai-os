//! Terminal fault reporting.
//!
//! Reports over serial and parks the machine. Once the framebuffer exists this
//! also paints HALCYON's amber-on-black fault screen. It never returns and
//! never allocates, so it stays usable no matter what broke.

use crate::arch::interrupts::TrapFrame;
use crate::arch::port;

pub fn fault(name: &str, trap: &TrapFrame, cr2: Option<u64>) -> ! {
    let _ = (name, trap, cr2);
    crate::serial_println!("[fault] machine halted.");
    port::park()
}

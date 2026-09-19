//! Raw x86 port I/O.

use core::arch::asm;

#[inline(always)]
pub unsafe fn outb(port: u16, value: u8) {
    asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}

#[inline(always)]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags));
    value
}

#[inline(always)]
pub unsafe fn outw(port: u16, value: u16) {
    asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags));
}

#[inline(always)]
pub unsafe fn inw(port: u16) -> u16 {
    let value: u16;
    asm!("in ax, dx", out("ax") value, in("dx") port, options(nomem, nostack, preserves_flags));
    value
}

#[inline(always)]
pub unsafe fn outl(port: u16, value: u32) {
    asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
}

#[inline(always)]
pub unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    asm!("in eax, dx", out("eax") value, in("dx") port, options(nomem, nostack, preserves_flags));
    value
}

/// Short delay by writing to an unused port — the classic way to give slow
/// legacy hardware (the PIC, the 8042) a moment between accesses.
#[inline(always)]
pub unsafe fn io_wait() {
    outb(0x80, 0);
}

#[inline(always)]
pub fn halt() {
    unsafe { asm!("hlt", options(nomem, nostack, preserves_flags)) }
}

#[inline(always)]
pub fn cli() {
    unsafe { asm!("cli", options(nomem, nostack)) }
}

#[inline(always)]
pub fn sti() {
    unsafe { asm!("sti", options(nomem, nostack)) }
}

/// Are interrupts currently enabled? (EFLAGS.IF)
#[inline(always)]
pub fn interrupts_enabled() -> bool {
    let flags: u64;
    unsafe {
        asm!("pushfq; pop {}", out(reg) flags, options(nomem, preserves_flags));
    }
    flags & (1 << 9) != 0
}

/// Run `f` with interrupts masked, restoring the previous state afterwards.
pub fn without_interrupts<T, F: FnOnce() -> T>(f: F) -> T {
    let was_enabled = interrupts_enabled();
    if was_enabled {
        cli();
    }
    let result = f();
    if was_enabled {
        sti();
    }
    result
}

/// Never return. Used by the panic path and by the idle loop of last resort.
pub fn park() -> ! {
    loop {
        cli();
        halt();
    }
}

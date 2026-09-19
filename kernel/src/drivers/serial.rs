//! COM1 serial port — the kernel's debug channel.
//!
//! Everything the boot sequence prints also goes here, which is what makes
//! headless CI testing possible: QEMU pipes COM1 to a file and the smoke test
//! asserts on it.

use core::fmt::{self, Write};

use crate::arch::port::{inb, outb};
use crate::sync::SpinLock;

const COM1: u16 = 0x3F8;

pub struct Serial {
    base: u16,
    ready: bool,
}

impl Serial {
    const fn new(base: u16) -> Self {
        Self { base, ready: false }
    }

    pub fn init(&mut self) {
        unsafe {
            outb(self.base + 1, 0x00); // disable interrupts
            outb(self.base + 3, 0x80); // DLAB on
            outb(self.base, 0x03); // divisor 3 => 38400 baud
            outb(self.base + 1, 0x00);
            outb(self.base + 3, 0x03); // 8N1, DLAB off
            outb(self.base + 2, 0xC7); // FIFO on, clear, 14-byte threshold
            outb(self.base + 4, 0x0B); // RTS/DSR set, OUT2 (IRQ gate)
        }
        self.ready = true;
    }

    fn transmit_empty(&self) -> bool {
        unsafe { inb(self.base + 5) & 0x20 != 0 }
    }

    pub fn write_byte(&mut self, byte: u8) {
        if !self.ready {
            return;
        }
        // Bounded wait: a machine with no real UART must not hang the kernel.
        let mut spins = 0u32;
        while !self.transmit_empty() {
            spins += 1;
            if spins > 100_000 {
                return;
            }
            core::hint::spin_loop();
        }
        unsafe { outb(self.base, byte) }
    }
}

impl Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        Ok(())
    }
}

pub static SERIAL: SpinLock<Serial> = SpinLock::new(Serial::new(COM1));

pub fn init() {
    SERIAL.lock().init();
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let _ = SERIAL.lock().write_fmt(args);
}

/// Write without taking the lock. Only for the panic handler.
#[doc(hidden)]
pub fn _print_forced(args: fmt::Arguments) {
    unsafe {
        SERIAL.force_unlock();
        let _ = SERIAL.get_unchecked().write_fmt(args);
    }
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => { $crate::drivers::serial::_print(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! serial_println {
    () => { $crate::serial_print!("\n") };
    ($($arg:tt)*) => { $crate::serial_print!("{}\n", format_args!($($arg)*)) };
}

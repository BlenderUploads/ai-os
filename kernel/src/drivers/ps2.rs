//! The 8042 keyboard/mouse controller.
//!
//! Every laptop worth booting this on still exposes its built-in keyboard and
//! trackpad through this chip (or the firmware's emulation of it), which is why
//! HALCYON uses it rather than reaching for a USB stack.

use crate::arch::port::{inb, io_wait, outb};

pub const DATA: u16 = 0x60;
pub const STATUS: u16 = 0x64;
pub const COMMAND: u16 = 0x64;

pub const STATUS_OUTPUT_FULL: u8 = 1 << 0;
pub const STATUS_INPUT_FULL: u8 = 1 << 1;
/// Set when the byte in the output buffer came from the second port (mouse).
pub const STATUS_FROM_MOUSE: u8 = 1 << 5;

const CMD_DISABLE_PORT1: u8 = 0xAD;
const CMD_ENABLE_PORT1: u8 = 0xAE;
const CMD_DISABLE_PORT2: u8 = 0xA7;
const CMD_ENABLE_PORT2: u8 = 0xA8;
const CMD_READ_CONFIG: u8 = 0x20;
const CMD_WRITE_CONFIG: u8 = 0x60;
const CMD_TEST_CONTROLLER: u8 = 0xAA;
const CMD_TEST_PORT2: u8 = 0xA9;
/// Prefix meaning "send the next byte to the second port".
const CMD_TO_MOUSE: u8 = 0xD4;

const CONFIG_PORT1_IRQ: u8 = 1 << 0;
const CONFIG_PORT2_IRQ: u8 = 1 << 1;
const CONFIG_PORT1_CLOCK_OFF: u8 = 1 << 4;
const CONFIG_PORT2_CLOCK_OFF: u8 = 1 << 5;
/// Translate set 2 scancodes to set 1 for us.
const CONFIG_TRANSLATE: u8 = 1 << 6;

/// Spin until the controller will accept a write, giving up rather than
/// hanging on a machine without a working 8042.
fn wait_writable() -> bool {
    for _ in 0..100_000 {
        if unsafe { inb(STATUS) } & STATUS_INPUT_FULL == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

fn wait_readable() -> bool {
    for _ in 0..100_000 {
        if unsafe { inb(STATUS) } & STATUS_OUTPUT_FULL != 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

pub fn write_command(command: u8) -> bool {
    if !wait_writable() {
        return false;
    }
    unsafe { outb(COMMAND, command) };
    true
}

pub fn write_data(value: u8) -> bool {
    if !wait_writable() {
        return false;
    }
    unsafe { outb(DATA, value) };
    true
}

pub fn read_data() -> Option<u8> {
    if !wait_readable() {
        return None;
    }
    Some(unsafe { inb(DATA) })
}

/// Read whatever is in the output buffer without waiting. Used from the IRQ
/// handler, where a byte is known to be present.
#[inline]
pub fn read_now() -> u8 {
    unsafe { inb(DATA) }
}

#[inline]
pub fn status() -> u8 {
    unsafe { inb(STATUS) }
}

pub fn flush() {
    for _ in 0..32 {
        if unsafe { inb(STATUS) } & STATUS_OUTPUT_FULL == 0 {
            break;
        }
        unsafe { inb(DATA) };
        unsafe { io_wait() };
    }
}

/// Send a byte to the mouse and collect its acknowledgement.
pub fn mouse_command(command: u8) -> Option<u8> {
    if !write_command(CMD_TO_MOUSE) {
        return None;
    }
    if !write_data(command) {
        return None;
    }
    read_data()
}

pub struct Ps2Status {
    pub controller_ok: bool,
    pub mouse_present: bool,
}

/// Bring the controller up with both ports enabled and interrupts on.
pub fn init(enable_mouse: bool) -> Ps2Status {
    // Quiet both devices so nothing arrives mid-configuration.
    write_command(CMD_DISABLE_PORT1);
    write_command(CMD_DISABLE_PORT2);
    flush();

    // Self-test. A controller that fails this may still work, so the result is
    // reported rather than treated as fatal.
    write_command(CMD_TEST_CONTROLLER);
    let controller_ok = read_data() == Some(0x55);

    write_command(CMD_READ_CONFIG);
    let mut config = read_data().unwrap_or(0);

    config |= CONFIG_PORT1_IRQ | CONFIG_TRANSLATE;
    config &= !CONFIG_PORT1_CLOCK_OFF;

    let mut mouse_present = false;
    if enable_mouse {
        write_command(CMD_TEST_PORT2);
        // 0x00 means the port is there and healthy.
        mouse_present = read_data() == Some(0x00);
        if mouse_present {
            config |= CONFIG_PORT2_IRQ;
            config &= !CONFIG_PORT2_CLOCK_OFF;
        }
    }

    write_command(CMD_WRITE_CONFIG);
    write_data(config);

    write_command(CMD_ENABLE_PORT1);
    if mouse_present {
        write_command(CMD_ENABLE_PORT2);
        // Defaults, then start streaming movement packets.
        mouse_command(0xF6);
        mouse_present = mouse_command(0xF4) == Some(0xFA);
    }

    flush();

    Ps2Status {
        controller_ok,
        mouse_present,
    }
}

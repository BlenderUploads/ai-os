//! PS/2 mouse: three-byte movement packets.

use crate::arch::interrupts::{self, TrapFrame};
use crate::input::{self, MouseEvent};

use super::ps2;

struct State {
    packet: [u8; 3],
    index: usize,
    left: bool,
    right: bool,
    middle: bool,
}

static mut STATE: State = State {
    packet: [0; 3],
    index: 0,
    left: false,
    right: false,
    middle: false,
};

/// Feed one byte from the controller into the packet assembler.
///
/// Public because the keyboard IRQ also drains the shared output buffer and
/// may pull out a byte tagged as the mouse's.
pub fn feed(byte: u8) {
    let state = unsafe { &mut *(&raw mut STATE) };

    // Bit 3 of the first byte is always set. If it is not, we are out of sync
    // -- discard until a plausible header shows up.
    if state.index == 0 && byte & 0x08 == 0 {
        return;
    }

    state.packet[state.index] = byte;
    state.index += 1;
    if state.index < 3 {
        return;
    }
    state.index = 0;

    let flags = state.packet[0];
    // Overflow means the deltas are meaningless; drop the packet.
    if flags & 0xC0 != 0 {
        return;
    }

    let mut dx = state.packet[1] as i32;
    let mut dy = state.packet[2] as i32;
    // The deltas are 9-bit two's complement, with the sign in the flags byte.
    if flags & 0x10 != 0 {
        dx -= 256;
    }
    if flags & 0x20 != 0 {
        dy -= 256;
    }

    let left = flags & 0x01 != 0;
    let right = flags & 0x02 != 0;
    let middle = flags & 0x04 != 0;

    let event = MouseEvent {
        dx,
        // The mouse reports Y upwards; screens count downwards.
        dy: -dy,
        left,
        right,
        middle,
        left_changed: left != state.left,
        right_changed: right != state.right,
        middle_changed: middle != state.middle,
    };
    state.left = left;
    state.right = right;
    state.middle = middle;

    input::push_mouse(event);
}

fn on_irq(frame: &mut TrapFrame) -> *mut TrapFrame {
    loop {
        // As in the keyboard handler: the source bit is only valid before the
        // read that consumes the byte.
        let status = ps2::status();
        if status & ps2::STATUS_OUTPUT_FULL == 0 {
            break;
        }
        let byte = ps2::read_now();
        if status & ps2::STATUS_FROM_MOUSE == 0 {
            // Actually the keyboard's; don't drop it.
            super::keyboard::feed(byte);
        } else {
            feed(byte);
        }
    }
    frame as *mut TrapFrame
}

pub fn init() {
    interrupts::register_irq(interrupts::IRQ_MOUSE, on_irq);
}

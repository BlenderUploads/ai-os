//! PS/2 keyboard, scancode set 1.

use crate::arch::interrupts::{self, TrapFrame};
use crate::input::{self, Key, KeyEvent, Modifiers};

use super::ps2;

/// (make code, unshifted, shifted) for every key that produces a character.
/// Anything not listed here is either a modifier or resolved by `special()`.
static LAYOUT: [(u8, u8, u8); 48] = [
    (0x02, b'1', b'!'),
    (0x03, b'2', b'@'),
    (0x04, b'3', b'#'),
    (0x05, b'4', b'$'),
    (0x06, b'5', b'%'),
    (0x07, b'6', b'^'),
    (0x08, b'7', b'&'),
    (0x09, b'8', b'*'),
    (0x0A, b'9', b'('),
    (0x0B, b'0', b')'),
    (0x0C, b'-', b'_'),
    (0x0D, b'=', b'+'),
    (0x10, b'q', b'Q'),
    (0x11, b'w', b'W'),
    (0x12, b'e', b'E'),
    (0x13, b'r', b'R'),
    (0x14, b't', b'T'),
    (0x15, b'y', b'Y'),
    (0x16, b'u', b'U'),
    (0x17, b'i', b'I'),
    (0x18, b'o', b'O'),
    (0x19, b'p', b'P'),
    (0x1A, b'[', b'{'),
    (0x1B, b']', b'}'),
    (0x1E, b'a', b'A'),
    (0x1F, b's', b'S'),
    (0x20, b'd', b'D'),
    (0x21, b'f', b'F'),
    (0x22, b'g', b'G'),
    (0x23, b'h', b'H'),
    (0x24, b'j', b'J'),
    (0x25, b'k', b'K'),
    (0x26, b'l', b'L'),
    (0x27, b';', b':'),
    (0x28, b'\'', b'"'),
    (0x29, b'`', b'~'),
    (0x2B, b'\\', b'|'),
    (0x2C, b'z', b'Z'),
    (0x2D, b'x', b'X'),
    (0x2E, b'c', b'C'),
    (0x2F, b'v', b'V'),
    (0x30, b'b', b'B'),
    (0x31, b'n', b'N'),
    (0x32, b'm', b'M'),
    (0x33, b',', b'<'),
    (0x34, b'.', b'>'),
    (0x35, b'/', b'?'),
    (0x39, b' ', b' '),
];

fn character(code: u8, shift: bool) -> Option<u8> {
    LAYOUT
        .iter()
        .find(|(make, _, _)| *make == code)
        .map(|(_, plain, shifted)| if shift { *shifted } else { *plain })
}

/// Non-character keys on the main block.
fn special(code: u8) -> Option<Key> {
    Some(match code {
        0x01 => Key::Escape,
        0x0E => Key::Backspace,
        0x0F => Key::Tab,
        0x1C => Key::Enter,
        0x3A => Key::CapsLock,
        0x3B => Key::F1,
        0x3C => Key::F2,
        0x3D => Key::F3,
        0x3E => Key::F4,
        0x3F => Key::F5,
        0x40 => Key::F6,
        0x41 => Key::F7,
        0x42 => Key::F8,
        0x43 => Key::F9,
        0x44 => Key::F10,
        0x57 => Key::F11,
        0x58 => Key::F12,
        _ => return None,
    })
}

/// Keys reached through the 0xE0 prefix.
fn extended(code: u8) -> Option<Key> {
    Some(match code {
        0x48 => Key::Up,
        0x50 => Key::Down,
        0x4B => Key::Left,
        0x4D => Key::Right,
        0x47 => Key::Home,
        0x4F => Key::End,
        0x49 => Key::PageUp,
        0x51 => Key::PageDown,
        0x52 => Key::Insert,
        0x53 => Key::Delete,
        0x1C => Key::Enter,
        _ => return None,
    })
}

struct State {
    extended_pending: bool,
    shift_left: bool,
    shift_right: bool,
    ctrl: bool,
    alt: bool,
    caps: bool,
}

static mut STATE: State = State {
    extended_pending: false,
    shift_left: false,
    shift_right: false,
    ctrl: false,
    alt: false,
    caps: false,
};

fn on_irq(frame: &mut TrapFrame) -> *mut TrapFrame {
    // Drain every byte the controller has; one IRQ can cover several.
    loop {
        // Sample the status before reading: reading DATA clears both the
        // output-full flag and the bit saying which device the byte came from.
        let status = ps2::status();
        if status & ps2::STATUS_OUTPUT_FULL == 0 {
            break;
        }
        let byte = ps2::read_now();
        if status & ps2::STATUS_FROM_MOUSE != 0 {
            // Belongs to the mouse; hand it over rather than losing sync.
            super::mouse::feed(byte);
        } else {
            feed(byte);
        }
    }
    frame as *mut TrapFrame
}

/// Process one scancode byte. Public so the mouse IRQ can pass along a byte it
/// finds is actually the keyboard's.
pub fn feed(byte: u8) {
    let state = unsafe { &mut *(&raw mut STATE) };

    if byte == 0xE0 {
        state.extended_pending = true;
        return;
    }

    let released = byte & 0x80 != 0;
    let code = byte & 0x7F;
    let was_extended = core::mem::replace(&mut state.extended_pending, false);

    // Modifiers first: they change the meaning of everything else.
    if !was_extended {
        match code {
            0x2A => {
                state.shift_left = !released;
                return;
            }
            0x36 => {
                state.shift_right = !released;
                return;
            }
            0x1D => {
                state.ctrl = !released;
                return;
            }
            0x38 => {
                state.alt = !released;
                return;
            }
            0x3A => {
                if !released {
                    state.caps = !state.caps;
                }
                // Fall through so Caps Lock is still reported as a key.
            }
            _ => {}
        }
    } else if code == 0x1D {
        state.ctrl = !released;
        return;
    } else if code == 0x38 {
        state.alt = !released;
        return;
    }

    let shift = state.shift_left || state.shift_right;
    let modifiers = Modifiers {
        shift,
        ctrl: state.ctrl,
        alt: state.alt,
        caps: state.caps,
    };

    let (key, character) = if was_extended {
        (extended(code), None)
    } else if let Some(key) = special(code) {
        (Some(key), None)
    } else if let Some(byte) = character(code, shift) {
        // Caps Lock affects letters only, and inverts the shift decision.
        let byte = if state.caps && byte.is_ascii_alphabetic() {
            if shift {
                byte.to_ascii_lowercase()
            } else {
                byte.to_ascii_uppercase()
            }
        } else {
            byte
        };
        (Some(Key::Char), Some(byte as char))
    } else {
        (None, None)
    };

    let Some(key) = key else { return };

    input::push_key(KeyEvent {
        key,
        character,
        pressed: !released,
        modifiers,
    });
}

pub fn init() {
    interrupts::register_irq(interrupts::IRQ_KEYBOARD, on_irq);
}

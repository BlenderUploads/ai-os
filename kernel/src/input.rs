//! Input events.
//!
//! Drivers push from interrupt context; the desktop pops from thread context.
//! Both queues are fixed-size rings behind interrupt-masking spinlocks, so a
//! burst of typing while the compositor is busy drops the oldest events rather
//! than allocating or blocking inside an IRQ.

use crate::sync::SpinLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char,
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    CapsLock,
    /// The modifiers, reported as keys in their own right as well as in
    /// `Modifiers`. Most apps ignore them; a game does not.
    Shift,
    Control,
    Alt,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub caps: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct KeyEvent {
    pub key: Key,
    pub character: Option<char>,
    pub pressed: bool,
    pub modifiers: Modifiers,
}

#[derive(Clone, Copy, Debug)]
pub struct MouseEvent {
    pub dx: i32,
    pub dy: i32,
    pub left: bool,
    pub right: bool,
    pub middle: bool,
    pub left_changed: bool,
    pub right_changed: bool,
    pub middle_changed: bool,
}

const QUEUE_CAPACITY: usize = 128;

struct Ring<T: Copy> {
    items: [Option<T>; QUEUE_CAPACITY],
    head: usize,
    tail: usize,
    dropped: u64,
}

impl<T: Copy> Ring<T> {
    const fn new() -> Self {
        Self {
            items: [None; QUEUE_CAPACITY],
            head: 0,
            tail: 0,
            dropped: 0,
        }
    }

    fn push(&mut self, item: T) {
        let next = (self.tail + 1) % QUEUE_CAPACITY;
        if next == self.head {
            // Full: discard the oldest so the most recent input survives.
            self.head = (self.head + 1) % QUEUE_CAPACITY;
            self.dropped += 1;
        }
        self.items[self.tail] = Some(item);
        self.tail = next;
    }

    fn pop(&mut self) -> Option<T> {
        if self.head == self.tail {
            return None;
        }
        let item = self.items[self.head].take();
        self.head = (self.head + 1) % QUEUE_CAPACITY;
        item
    }
}

static KEYS: SpinLock<Ring<KeyEvent>> = SpinLock::new(Ring::new());
static MICE: SpinLock<Ring<MouseEvent>> = SpinLock::new(Ring::new());

pub fn push_key(event: KeyEvent) {
    KEYS.lock().push(event);
}

pub fn pop_key() -> Option<KeyEvent> {
    KEYS.lock().pop()
}

pub fn push_mouse(event: MouseEvent) {
    MICE.lock().push(event);
}

pub fn pop_mouse() -> Option<MouseEvent> {
    MICE.lock().pop()
}

pub fn dropped_events() -> (u64, u64) {
    (KEYS.lock().dropped, MICE.lock().dropped)
}

/// Block until a key is pressed. Only sensible before the scheduler exists.
pub fn wait_for_key() -> KeyEvent {
    loop {
        if let Some(event) = pop_key() {
            if event.pressed {
                return event;
            }
        }
        crate::arch::port::halt();
    }
}

//! CMOS real-time clock.

use crate::arch::port::{inb, outb};

const ADDRESS: u16 = 0x70;
const DATA: u16 = 0x71;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    pub fn is_valid(&self) -> bool {
        self.month >= 1 && self.month <= 12 && self.day >= 1 && self.day <= 31 && self.hour < 24
    }
}

unsafe fn read_register(register: u8) -> u8 {
    // Preserve the NMI-disable bit in the high position.
    outb(ADDRESS, (inb(ADDRESS) & 0x80) | (register & 0x7F));
    inb(DATA)
}

unsafe fn update_in_progress() -> bool {
    read_register(0x0A) & 0x80 != 0
}

fn from_bcd(value: u8) -> u8 {
    (value & 0x0F) + ((value >> 4) * 10)
}

/// Read the wall clock.
///
/// The RTC can be mid-update when sampled, so the read is repeated until two
/// consecutive passes agree — the standard way to avoid catching a rollover.
pub fn now() -> DateTime {
    unsafe {
        let mut last = sample();
        for _ in 0..16 {
            let current = sample();
            if current == last {
                return decode(current, read_register(0x0B));
            }
            last = current;
        }
        decode(last, read_register(0x0B))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Raw {
    second: u8,
    minute: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u8,
    century: u8,
}

unsafe fn sample() -> Raw {
    let mut spins = 0;
    while update_in_progress() {
        spins += 1;
        if spins > 100_000 {
            break;
        }
        core::hint::spin_loop();
    }
    Raw {
        second: read_register(0x00),
        minute: read_register(0x02),
        hour: read_register(0x04),
        day: read_register(0x07),
        month: read_register(0x08),
        year: read_register(0x09),
        century: read_register(0x32),
    }
}

fn decode(raw: Raw, status_b: u8) -> DateTime {
    let binary = status_b & 0x04 != 0;
    let twenty_four_hour = status_b & 0x02 != 0;

    let convert = |value: u8| if binary { value } else { from_bcd(value) };

    let second = convert(raw.second);
    let minute = convert(raw.minute);

    // In 12-hour mode bit 7 of the hour register marks PM, and it survives the
    // BCD conversion, so it has to be stripped first.
    let pm = !twenty_four_hour && (raw.hour & 0x80) != 0;
    let mut hour = convert(raw.hour & 0x7F);
    if !twenty_four_hour {
        if pm && hour < 12 {
            hour += 12;
        } else if !pm && hour == 12 {
            hour = 0;
        }
    }

    let day = convert(raw.day);
    let month = convert(raw.month);
    let year_in_century = convert(raw.year) as u16;
    let century = if raw.century != 0 && raw.century != 0xFF {
        convert(raw.century) as u16
    } else {
        20
    };

    DateTime {
        year: century * 100 + year_in_century,
        month,
        day,
        hour,
        minute,
        second,
    }
}

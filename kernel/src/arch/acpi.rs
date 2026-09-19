//! Just enough ACPI to turn the machine off.
//!
//! Proper shutdown needs an AML interpreter to evaluate the `\_S5` object.
//! HALCYON does the well-worn alternative: scan the DSDT byte stream for the
//! `_S5_` name and decode the small package that follows it by hand. That is
//! enough for the one thing we want, and far short of an AML interpreter.

use crate::arch::port::outw;
use crate::mm::paging::phys_to_virt;

#[derive(Clone, Copy, Default)]
pub struct PowerOff {
    pub pm1a_control: u16,
    pub pm1b_control: u16,
    pub slp_typ_a: u16,
    pub slp_typ_b: u16,
}

const SLP_EN: u16 = 1 << 13;

unsafe fn table_signature(phys: u64) -> [u8; 4] {
    let pointer = phys_to_virt(phys) as *const u8;
    [*pointer, *pointer.add(1), *pointer.add(2), *pointer.add(3)]
}

unsafe fn table_length(phys: u64) -> u32 {
    core::ptr::read_unaligned(phys_to_virt(phys + 4) as *const u32)
}

/// Locate a table by signature via the RSDT or XSDT.
unsafe fn find_table(rsdp_phys: u64, wanted: &[u8; 4]) -> Option<u64> {
    let rsdp = phys_to_virt(rsdp_phys) as *const u8;
    if &core::slice::from_raw_parts(rsdp, 8)[..] != b"RSD PTR " {
        return None;
    }
    let revision = *rsdp.add(15);

    let (root_phys, entry_size) = if revision >= 2 {
        let xsdt = core::ptr::read_unaligned(rsdp.add(24) as *const u64);
        if xsdt != 0 {
            (xsdt, 8usize)
        } else {
            (
                core::ptr::read_unaligned(rsdp.add(16) as *const u32) as u64,
                4usize,
            )
        }
    } else {
        (
            core::ptr::read_unaligned(rsdp.add(16) as *const u32) as u64,
            4usize,
        )
    };

    if root_phys == 0 {
        return None;
    }
    let length = table_length(root_phys) as usize;
    if length < 36 {
        return None;
    }
    let count = (length - 36) / entry_size;
    for index in 0..count {
        let slot = phys_to_virt(root_phys + 36 + (index * entry_size) as u64);
        let entry = if entry_size == 8 {
            core::ptr::read_unaligned(slot as *const u64)
        } else {
            core::ptr::read_unaligned(slot as *const u32) as u64
        };
        if entry == 0 {
            continue;
        }
        if &table_signature(entry) == wanted {
            return Some(entry);
        }
    }
    None
}

/// Work out how to power the machine down, from the FADT and the DSDT.
pub unsafe fn discover(rsdp_phys: u64) -> Option<PowerOff> {
    let fadt = find_table(rsdp_phys, b"FACP")?;
    let fadt_length = table_length(fadt);
    let base = phys_to_virt(fadt) as *const u8;

    let pm1a_control = core::ptr::read_unaligned(base.add(64) as *const u32) as u16;
    let pm1b_control = core::ptr::read_unaligned(base.add(68) as *const u32) as u16;

    // 64-bit X_DSDT wins where present.
    let dsdt = if fadt_length >= 148 {
        let extended = core::ptr::read_unaligned(base.add(140) as *const u64);
        if extended != 0 {
            extended
        } else {
            core::ptr::read_unaligned(base.add(40) as *const u32) as u64
        }
    } else {
        core::ptr::read_unaligned(base.add(40) as *const u32) as u64
    };
    if dsdt == 0 || &table_signature(dsdt) != b"DSDT" {
        return None;
    }

    let dsdt_length = table_length(dsdt) as usize;
    if dsdt_length <= 36 {
        return None;
    }
    let aml = core::slice::from_raw_parts(phys_to_virt(dsdt + 36) as *const u8, dsdt_length - 36);

    // Find `_S5_` and decode the package that follows.
    let mut index = 0;
    while index + 8 < aml.len() {
        if &aml[index..index + 4] != b"_S5_" {
            index += 1;
            continue;
        }

        // It should be introduced by NameOp (0x08), optionally preceded by a
        // root-scope backslash, and followed by PackageOp (0x12).
        let preceded_by_name_op = (index >= 1 && aml[index - 1] == 0x08)
            || (index >= 2 && aml[index - 2] == 0x08 && aml[index - 1] == b'\\');
        if !preceded_by_name_op || aml[index + 4] != 0x12 {
            index += 1;
            continue;
        }

        // Skip _S5_, PackageOp, the PkgLength (whose top two bits give the
        // number of extra length bytes), and the element count.
        let mut cursor = index + 5;
        cursor += ((aml[cursor] & 0xC0) >> 6) as usize + 2;
        if cursor >= aml.len() {
            return None;
        }

        // Each element is either a byte constant (0x0A prefix) or a small
        // integer encoded directly.
        if aml[cursor] == 0x0A {
            cursor += 1;
        }
        let slp_typ_a = (aml.get(cursor).copied().unwrap_or(0) as u16) << 10;
        cursor += 1;
        if aml.get(cursor).copied() == Some(0x0A) {
            cursor += 1;
        }
        let slp_typ_b = (aml.get(cursor).copied().unwrap_or(0) as u16) << 10;

        return Some(PowerOff {
            pm1a_control,
            pm1b_control,
            slp_typ_a,
            slp_typ_b,
        });
    }
    None
}

/// Try every shutdown route we know. Returns only if all of them fail.
pub fn power_off(config: Option<PowerOff>) {
    if let Some(config) = config {
        if config.pm1a_control != 0 {
            unsafe { outw(config.pm1a_control, config.slp_typ_a | SLP_EN) };
        }
        if config.pm1b_control != 0 {
            unsafe { outw(config.pm1b_control, config.slp_typ_b | SLP_EN) };
        }
    }

    // Hypervisor shutdown ports, for when ACPI is unavailable or ignored.
    unsafe {
        outw(0x604, 0x2000); // QEMU (and newer Bochs)
        outw(0xB004, 0x2000); // Bochs / older QEMU
        outw(0x4004, 0x3400); // VirtualBox
    }
}

/// Reset via the 8042, falling back to a deliberate triple fault.
pub fn reboot() -> ! {
    use crate::arch::port::{inb, outb};
    unsafe {
        // Pulse the CPU reset line.
        for _ in 0..16 {
            if inb(0x64) & 0x02 == 0 {
                break;
            }
        }
        outb(0x64, 0xFE);

        crate::arch::pit::delay_ms(50);

        // Still here: load a zero-length IDT and fault, which forces a reset.
        #[repr(C, packed)]
        struct NullIdt {
            limit: u16,
            base: u64,
        }
        let null = NullIdt { limit: 0, base: 0 };
        core::arch::asm!("lidt [{}]", in(reg) &null, options(readonly, nostack));
        core::arch::asm!("int3", options(nostack));
    }
    crate::arch::port::park()
}

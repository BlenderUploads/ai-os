//! PCI enumeration over the legacy 0xCF8/0xCFC configuration ports.
//!
//! HALCYON drives none of these devices -- this is here so the machine can
//! tell you what it is made of.

use alloc::string::String;
use alloc::vec::Vec;

use crate::arch::port::{inl, outl};

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

fn read_config(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address = 0x8000_0000
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC);
    unsafe {
        outl(CONFIG_ADDRESS, address);
        inl(CONFIG_DATA)
    }
}

#[derive(Clone, Copy)]
pub struct Device {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor: u16,
    pub id: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision: u8,
}

impl Device {
    pub fn class_name(&self) -> &'static str {
        match (self.class, self.subclass) {
            (0x00, _) => "unclassified",
            (0x01, 0x01) => "IDE controller",
            (0x01, 0x06) => "SATA controller",
            (0x01, 0x08) => "NVMe controller",
            (0x01, _) => "storage controller",
            (0x02, _) => "network controller",
            (0x03, _) => "display controller",
            (0x04, 0x03) => "audio device",
            (0x04, _) => "multimedia device",
            (0x05, _) => "memory controller",
            (0x06, 0x00) => "host bridge",
            (0x06, 0x01) => "ISA bridge",
            (0x06, 0x04) => "PCI-to-PCI bridge",
            (0x06, _) => "bridge",
            (0x07, _) => "communication controller",
            (0x08, _) => "system peripheral",
            (0x09, _) => "input device",
            (0x0C, 0x03) => "USB controller",
            (0x0C, _) => "serial bus controller",
            (0x0D, _) => "wireless controller",
            _ => "device",
        }
    }

    /// Vendors common enough to be worth naming.
    pub fn vendor_name(&self) -> &'static str {
        match self.vendor {
            0x8086 => "Intel",
            0x1022 | 0x1002 => "AMD",
            0x10DE => "NVIDIA",
            0x1234 => "QEMU",
            0x1AF4 => "Red Hat / virtio",
            0x15AD => "VMware",
            0x80EE => "VirtualBox",
            0x14E4 => "Broadcom",
            0x10EC => "Realtek",
            0x168C => "Qualcomm Atheros",
            0x1969 => "Attansic",
            0x1106 => "VIA",
            0x1039 => "SiS",
            _ => "",
        }
    }

    pub fn describe(&self) -> String {
        let vendor = self.vendor_name();
        if vendor.is_empty() {
            alloc::format!(
                "{:02x}:{:02x}.{}  {:04x}:{:04x}  {}",
                self.bus,
                self.device,
                self.function,
                self.vendor,
                self.id,
                self.class_name()
            )
        } else {
            alloc::format!(
                "{:02x}:{:02x}.{}  {:04x}:{:04x}  {} {}",
                self.bus,
                self.device,
                self.function,
                self.vendor,
                self.id,
                vendor,
                self.class_name()
            )
        }
    }
}

fn probe(bus: u8, device: u8, function: u8) -> Option<Device> {
    let identity = read_config(bus, device, function, 0x00);
    let vendor = (identity & 0xFFFF) as u16;
    if vendor == 0xFFFF {
        return None;
    }
    let classes = read_config(bus, device, function, 0x08);
    Some(Device {
        bus,
        device,
        function,
        vendor,
        id: (identity >> 16) as u16,
        class: (classes >> 24) as u8,
        subclass: (classes >> 16) as u8,
        prog_if: (classes >> 8) as u8,
        revision: classes as u8,
    })
}

/// Brute-force scan of every bus. Slower than following bridges, but it cannot
/// miss a device behind a bridge HALCYON does not understand.
pub fn enumerate() -> Vec<Device> {
    let mut devices = Vec::new();
    for bus in 0..=255u16 {
        for device in 0..32u8 {
            let Some(first) = probe(bus as u8, device, 0) else {
                continue;
            };
            devices.push(first);

            // Bit 7 of the header type marks a multi-function device.
            let header_type = (read_config(bus as u8, device, 0, 0x0C) >> 16) as u8;
            if header_type & 0x80 == 0 {
                continue;
            }
            for function in 1..8u8 {
                if let Some(extra) = probe(bus as u8, device, function) {
                    devices.push(extra);
                }
            }
        }
    }
    devices
}

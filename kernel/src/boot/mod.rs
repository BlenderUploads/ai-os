//! Multiboot2 handoff: the bootstrap assembly and the parser for the
//! information structure GRUB leaves behind.

use core::arch::global_asm;

global_asm!(include_str!("boot.S"), options(att_syntax));

pub const KERNEL_VMA: u64 = 0xFFFF_FFFF_8000_0000;

extern "C" {
    static __image_phys_start: u8;
    static __kernel_phys_end: u8;
}

/// Physical extent of the loaded kernel image, so the frame allocator never
/// hands out memory we are running from.
pub fn kernel_image_extent() -> (u64, u64) {
    unsafe {
        (
            &__image_phys_start as *const u8 as u64,
            &__kernel_phys_end as *const u8 as u64,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryKind {
    Usable,
    Reserved,
    AcpiReclaimable,
    AcpiNvs,
    Defective,
}

#[derive(Clone, Copy, Debug)]
pub struct MemoryRegion {
    pub base: u64,
    pub length: u64,
    pub kind: MemoryKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FramebufferKind {
    Indexed,
    Rgb,
    EgaText,
    Unknown,
}

#[derive(Clone, Copy, Debug)]
pub struct FramebufferInfo {
    pub addr: u64,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    pub bpp: u8,
    pub kind: FramebufferKind,
    pub red_shift: u8,
    pub red_size: u8,
    pub green_shift: u8,
    pub green_size: u8,
    pub blue_shift: u8,
    pub blue_size: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct Module {
    pub start: u64,
    pub end: u64,
    /// The string GRUB's `module2` line gave it, which is how the kernel tells
    /// the initrd from, say, a DOOM IWAD.
    pub name: [u8; 32],
    pub name_len: usize,
}

impl Module {
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("")
    }

    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }
}

pub const MAX_REGIONS: usize = 64;
pub const MAX_MODULES: usize = 8;
const MAX_CMDLINE: usize = 128;

pub struct BootInfo {
    pub regions: [MemoryRegion; MAX_REGIONS],
    pub region_count: usize,
    pub modules: [Module; MAX_MODULES],
    pub module_count: usize,
    pub framebuffer: Option<FramebufferInfo>,
    pub rsdp: Option<u64>,
    pub bootloader: [u8; 64],
    pub bootloader_len: usize,
    pub cmdline: [u8; MAX_CMDLINE],
    pub cmdline_len: usize,
    /// Physical end of everything the bootloader placed in memory (kernel plus
    /// modules plus the info structure itself).
    pub reclaim_after: u64,
}

impl BootInfo {
    const fn empty() -> Self {
        const EMPTY_REGION: MemoryRegion = MemoryRegion {
            base: 0,
            length: 0,
            kind: MemoryKind::Reserved,
        };
        const EMPTY_MODULE: Module = Module {
            start: 0,
            end: 0,
            name: [0; 32],
            name_len: 0,
        };
        Self {
            regions: [EMPTY_REGION; MAX_REGIONS],
            region_count: 0,
            modules: [EMPTY_MODULE; MAX_MODULES],
            module_count: 0,
            framebuffer: None,
            rsdp: None,
            bootloader: [0; 64],
            bootloader_len: 0,
            cmdline: [0; MAX_CMDLINE],
            cmdline_len: 0,
            reclaim_after: 0,
        }
    }

    pub fn bootloader_name(&self) -> &str {
        core::str::from_utf8(&self.bootloader[..self.bootloader_len]).unwrap_or("?")
    }

    pub fn cmdline(&self) -> &str {
        core::str::from_utf8(&self.cmdline[..self.cmdline_len]).unwrap_or("")
    }

    /// Total usable RAM in bytes.
    pub fn usable_memory(&self) -> u64 {
        self.regions[..self.region_count]
            .iter()
            .filter(|r| r.kind == MemoryKind::Usable)
            .map(|r| r.length)
            .sum()
    }

    pub fn regions(&self) -> &[MemoryRegion] {
        &self.regions[..self.region_count]
    }

    pub fn modules(&self) -> &[Module] {
        &self.modules[..self.module_count]
    }
}

unsafe fn read<T>(addr: u64) -> T {
    core::ptr::read_unaligned(addr as *const T)
}

unsafe fn copy_cstr(addr: u64, dest: &mut [u8]) -> usize {
    let mut len = 0;
    while len < dest.len() {
        let byte = read::<u8>(addr + len as u64);
        if byte == 0 {
            break;
        }
        dest[len] = byte;
        len += 1;
    }
    len
}

/// Walk the multiboot2 tag list. `mbi_phys` is identity-mapped at this point.
pub unsafe fn parse(mbi_phys: u64) -> BootInfo {
    let mut info = BootInfo::empty();

    let total_size = read::<u32>(mbi_phys) as u64;
    info.reclaim_after = mbi_phys + total_size;

    let mut cursor = mbi_phys + 8;
    let end = mbi_phys + total_size;

    while cursor + 8 <= end {
        let tag_type = read::<u32>(cursor);
        let tag_size = read::<u32>(cursor + 4) as u64;
        if tag_size < 8 {
            break;
        }

        match tag_type {
            0 => break, // end tag
            1 => {
                info.cmdline_len = copy_cstr(cursor + 8, &mut info.cmdline);
            }
            2 => {
                info.bootloader_len = copy_cstr(cursor + 8, &mut info.bootloader);
            }
            3 => {
                if info.module_count < MAX_MODULES {
                    let start = read::<u32>(cursor + 8) as u64;
                    let module_end = read::<u32>(cursor + 12) as u64;
                    let mut name = [0u8; 32];
                    let name_len = copy_cstr(cursor + 16, &mut name);
                    info.modules[info.module_count] = Module {
                        start,
                        end: module_end,
                        name,
                        name_len,
                    };
                    info.module_count += 1;
                    if module_end > info.reclaim_after {
                        info.reclaim_after = module_end;
                    }
                }
            }
            6 => {
                let entry_size = read::<u32>(cursor + 8) as u64;
                if entry_size >= 24 {
                    let mut entry = cursor + 16;
                    while entry + entry_size <= cursor + tag_size {
                        if info.region_count < MAX_REGIONS {
                            let base = read::<u64>(entry);
                            let length = read::<u64>(entry + 8);
                            let kind = match read::<u32>(entry + 16) {
                                1 => MemoryKind::Usable,
                                3 => MemoryKind::AcpiReclaimable,
                                4 => MemoryKind::AcpiNvs,
                                5 => MemoryKind::Defective,
                                _ => MemoryKind::Reserved,
                            };
                            if length > 0 {
                                info.regions[info.region_count] =
                                    MemoryRegion { base, length, kind };
                                info.region_count += 1;
                            }
                        }
                        entry += entry_size;
                    }
                }
            }
            8 => {
                let addr = read::<u64>(cursor + 8);
                let pitch = read::<u32>(cursor + 16);
                let width = read::<u32>(cursor + 20);
                let height = read::<u32>(cursor + 24);
                let bpp = read::<u8>(cursor + 28);
                let raw_kind = read::<u8>(cursor + 29);
                let kind = match raw_kind {
                    0 => FramebufferKind::Indexed,
                    1 => FramebufferKind::Rgb,
                    2 => FramebufferKind::EgaText,
                    _ => FramebufferKind::Unknown,
                };
                // For direct RGB the colour layout follows at offset 32.
                let (rs, rz, gs, gz, bs, bz) = if kind == FramebufferKind::Rgb {
                    (
                        read::<u8>(cursor + 32),
                        read::<u8>(cursor + 33),
                        read::<u8>(cursor + 34),
                        read::<u8>(cursor + 35),
                        read::<u8>(cursor + 36),
                        read::<u8>(cursor + 37),
                    )
                } else {
                    (16, 8, 8, 8, 0, 8)
                };
                info.framebuffer = Some(FramebufferInfo {
                    addr,
                    pitch,
                    width,
                    height,
                    bpp,
                    kind,
                    red_shift: rs,
                    red_size: rz,
                    green_shift: gs,
                    green_size: gz,
                    blue_shift: bs,
                    blue_size: bz,
                });
            }
            14 => info.rsdp = Some(cursor + 8), // ACPI 1.0 RSDP
            15 => {
                // ACPI 2.0+ RSDP wins if both are present.
                info.rsdp = Some(cursor + 8);
            }
            _ => {}
        }

        // Tags are padded to an 8-byte boundary.
        cursor += (tag_size + 7) & !7;
    }

    info
}

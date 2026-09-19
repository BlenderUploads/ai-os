//! CPUID: what are we actually running on?

use core::arch::asm;

fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx);
    unsafe {
        // LLVM reserves rbx, so shuffle it through another register.
        asm!(
            "mov {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            tmp = out(reg) ebx,
            inout("eax") leaf => eax,
            inout("ecx") subleaf => ecx,
            out("edx") edx,
            options(nostack, preserves_flags),
        );
    }
    (eax, ebx, ecx, edx)
}

pub struct CpuInfo {
    pub vendor: [u8; 12],
    pub brand: [u8; 48],
    pub brand_len: usize,
    pub family: u32,
    pub model: u32,
    pub stepping: u32,
    pub has_apic: bool,
    pub has_pat: bool,
    pub has_sse2: bool,
    pub has_nx: bool,
    pub has_invariant_tsc: bool,
}

impl CpuInfo {
    pub fn vendor_str(&self) -> &str {
        core::str::from_utf8(&self.vendor).unwrap_or("unknown")
    }

    pub fn brand_str(&self) -> &str {
        core::str::from_utf8(&self.brand[..self.brand_len])
            .unwrap_or("unknown")
            .trim()
    }
}

pub fn identify() -> CpuInfo {
    let (_, ebx, ecx, edx) = cpuid(0, 0);
    let mut vendor = [0u8; 12];
    vendor[0..4].copy_from_slice(&ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&edx.to_le_bytes());
    vendor[8..12].copy_from_slice(&ecx.to_le_bytes());

    let (version, _, feature_ecx, feature_edx) = cpuid(1, 0);
    let _ = feature_ecx;
    let stepping = version & 0xF;
    let base_model = (version >> 4) & 0xF;
    let base_family = (version >> 8) & 0xF;
    let ext_model = (version >> 16) & 0xF;
    let ext_family = (version >> 20) & 0xFF;

    let family = if base_family == 0xF {
        base_family + ext_family
    } else {
        base_family
    };
    let model = if base_family == 0x6 || base_family == 0xF {
        (ext_model << 4) | base_model
    } else {
        base_model
    };

    // Brand string lives in extended leaves 0x80000002..4.
    let mut brand = [0u8; 48];
    let (max_ext, _, _, _) = cpuid(0x8000_0000, 0);
    let mut brand_len = 0;
    if max_ext >= 0x8000_0004 {
        for (index, leaf) in (0x8000_0002u32..=0x8000_0004).enumerate() {
            let (a, b, c, d) = cpuid(leaf, 0);
            let offset = index * 16;
            brand[offset..offset + 4].copy_from_slice(&a.to_le_bytes());
            brand[offset + 4..offset + 8].copy_from_slice(&b.to_le_bytes());
            brand[offset + 8..offset + 12].copy_from_slice(&c.to_le_bytes());
            brand[offset + 12..offset + 16].copy_from_slice(&d.to_le_bytes());
        }
        brand_len = brand.iter().position(|&b| b == 0).unwrap_or(48);
    }

    let has_nx = if max_ext >= 0x8000_0001 {
        cpuid(0x8000_0001, 0).3 & (1 << 20) != 0
    } else {
        false
    };
    let has_invariant_tsc = if max_ext >= 0x8000_0007 {
        cpuid(0x8000_0007, 0).3 & (1 << 8) != 0
    } else {
        false
    };

    CpuInfo {
        vendor,
        brand,
        brand_len,
        family,
        model,
        stepping,
        has_apic: feature_edx & (1 << 9) != 0,
        has_pat: feature_edx & (1 << 16) != 0,
        has_sse2: feature_edx & (1 << 26) != 0,
        has_nx,
        has_invariant_tsc,
    }
}

/// Read the timestamp counter — used for sub-millisecond timing.
#[inline]
pub fn rdtsc() -> u64 {
    let (low, high): (u32, u32);
    unsafe {
        asm!("rdtsc", out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags));
    }
    ((high as u64) << 32) | low as u64
}

pub unsafe fn read_msr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high,
         options(nomem, nostack, preserves_flags));
    ((high as u64) << 32) | low as u64
}

pub unsafe fn write_msr(msr: u32, value: u64) {
    asm!("wrmsr", in("ecx") msr, in("eax") value as u32, in("edx") (value >> 32) as u32,
         options(nomem, nostack, preserves_flags));
}

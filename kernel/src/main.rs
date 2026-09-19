//! HALCYON — a from-scratch x86_64 operating system.
//!
//! No Linux, no libc, no third-party crates: `core`, `alloc`, and the code in
//! this repository.

#![no_std]
#![no_main]

pub mod arch;
pub mod boot;
pub mod drivers;
pub mod panic_screen;
pub mod sync;

use core::panic::PanicInfo;

use arch::port;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Entry point, called from the bootstrap in `boot/boot.S` once the CPU is in
/// long mode and running out of the higher half.
#[no_mangle]
pub extern "C" fn kmain(mbi_phys: u64) -> ! {
    drivers::serial::init();

    serial_println!();
    serial_println!("HALCYON v{} — cold start", VERSION);
    serial_println!("[boot] long mode entered, higher half active");

    let info = unsafe { boot::parse(mbi_phys) };

    serial_println!("[boot] loader: {}", info.bootloader_name());
    if !info.cmdline().is_empty() {
        serial_println!("[boot] cmdline: {}", info.cmdline());
    }

    let (kstart, kend) = boot::kernel_image_extent();
    serial_println!(
        "[boot] kernel image: {:#x}..{:#x} ({} KiB)",
        kstart,
        kend,
        (kend - kstart) / 1024
    );

    serial_println!(
        "[mem ] {} regions, {} MiB usable",
        info.region_count,
        info.usable_memory() / (1024 * 1024)
    );
    for region in info.regions() {
        serial_println!(
            "[mem ]   {:#012x}..{:#012x}  {:?}",
            region.base,
            region.base + region.length,
            region.kind
        );
    }

    match info.framebuffer {
        Some(fb) => serial_println!(
            "[gfx ] framebuffer {}x{}x{} pitch {} at {:#x} ({:?})",
            fb.width,
            fb.height,
            fb.bpp,
            fb.pitch,
            fb.addr,
            fb.kind
        ),
        None => serial_println!("[gfx ] no framebuffer supplied by the bootloader"),
    }

    for (index, module) in info.modules().iter().enumerate() {
        serial_println!(
            "[boot] module {}: {:#x}..{:#x} ({} KiB)",
            index,
            module.start,
            module.end,
            (module.end - module.start) / 1024
        );
    }

    let cpu = arch::cpu::identify();
    serial_println!(
        "[cpu ] {} (family {:#x} model {:#x} stepping {})",
        cpu.brand_str(),
        cpu.family,
        cpu.model,
        cpu.stepping
    );
    serial_println!(
        "[cpu ] vendor {} | pat:{} nx:{} sse2:{} apic:{}",
        cpu.vendor_str(),
        cpu.has_pat,
        cpu.has_nx,
        cpu.has_sse2,
        cpu.has_apic
    );

    arch::interrupts::init();
    serial_println!("[intr] GDT, TSS, IDT loaded; PIC remapped to 32..47");

    arch::pit::init();
    arch::interrupts::enable();
    serial_println!("[intr] interrupts enabled, PIT at {} Hz", arch::pit::TICK_HZ);

    // Prove the timer actually fires before anything depends on it.
    let start = arch::pit::ticks();
    let spin_until = arch::cpu::rdtsc() + 200_000_000;
    while arch::cpu::rdtsc() < spin_until {
        core::hint::spin_loop();
    }
    let elapsed = arch::pit::ticks() - start;
    if elapsed > 0 {
        serial_println!("[intr] timer advanced {} ticks — IRQ0 is live", elapsed);
    } else {
        serial_println!("[intr] WARNING: timer did not advance");
    }

    serial_println!("[boot] HALCYON-BOOT-OK");

    // A deliberate fault, only when asked for: `selftest=fault` on the GRUB
    // command line. Proves the exception path reports rather than triple-faults.
    if info.cmdline().contains("selftest=fault") {
        serial_println!("[test] triggering a deliberate page fault...");
        unsafe {
            // Canonical (bits 63:47 all clear) but far outside anything we map,
            // so the CPU takes a real page fault rather than a #GP for a
            // non-canonical address.
            let bad = 0x0000_0008_0000_0000_u64 as *mut u64;
            core::ptr::write_volatile(bad, 1);
        }
    }

    serial_println!("[boot] nothing further implemented yet; parking.");

    port::park()
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    drivers::serial::_print_forced(format_args!("\n\n*** HALCYON PANIC ***\n{}\n", info));
    port::park()
}

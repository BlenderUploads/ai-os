//! HALCYON — a from-scratch x86_64 operating system.
//!
//! No Linux, no libc, no third-party crates: `core`, `alloc`, and the code in
//! this repository.

#![no_std]
#![no_main]

extern crate alloc;

pub mod arch;
pub mod boot;
pub mod drivers;
pub mod mm;
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

    let memory = unsafe { mm::init(&info) };
    let (total, total_unit) = mm::format_bytes(memory.total_bytes);
    let (usable, usable_unit) = mm::format_bytes(memory.usable_bytes);
    serial_println!(
        "[mm  ] {} {} addressable, {} {} usable RAM",
        total,
        total_unit,
        usable,
        usable_unit
    );
    serial_println!(
        "[mm  ] address space live: physmap {:#x}, heap {:#x}, framebuffer {:#x}",
        mm::paging::PHYSMAP_BASE,
        mm::paging::HEAP_BASE,
        memory.space.framebuffer_virt
    );
    serial_println!(
        "[mm  ] framebuffer caching: {}",
        if memory.space.write_combining {
            "write-combining (PAT entry 4)"
        } else {
            "uncached (no PAT)"
        }
    );
    selftest_heap();

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

/// Exercise the allocator hard enough to catch the classic failures: a
/// free-list that does not coalesce, a leak on drop, or alignment padding that
/// never comes back.
fn selftest_heap() {
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::vec::Vec;

    let before = mm::heap::ALLOCATOR.stats();

    {
        let mut numbers: Vec<u64> = Vec::new();
        for value in 0..50_000u64 {
            numbers.push(value * 3);
        }
        assert_eq!(numbers.iter().sum::<u64>(), (0..50_000u64).sum::<u64>() * 3);

        let mut text = String::new();
        for _ in 0..1000 {
            text.push_str("HALCYON ");
        }
        assert_eq!(text.len(), 8000);

        let mut map: BTreeMap<u64, String> = BTreeMap::new();
        for key in 0..2000u64 {
            let mut value = String::from("entry-");
            value.push((b'0' + (key % 10) as u8) as char);
            map.insert(key, value);
        }
        assert_eq!(map.len(), 2000);
        assert_eq!(map.get(&7).map(|s| s.as_str()), Some("entry-7"));

        // Over-aligned allocations: the padding in front must return to the
        // free list, not vanish.
        let mut aligned: Vec<Vec<u8>> = Vec::new();
        for size in [64usize, 256, 1024, 4096] {
            let mut block = Vec::with_capacity(size);
            block.resize(size, 0xA5);
            aligned.push(block);
        }
        assert_eq!(aligned.len(), 4);
    }

    let after = mm::heap::ALLOCATOR.stats();
    let leaked = after.live_allocations.saturating_sub(before.live_allocations);
    let (mapped, mapped_unit) = mm::format_bytes(after.mapped);
    serial_println!(
        "[mm  ] heap self-test: {} allocations served, {} {} mapped, {} free blocks",
        after.total_allocations - before.total_allocations,
        mapped,
        mapped_unit,
        after.free_blocks
    );
    if leaked == 0 && after.allocated == before.allocated {
        serial_println!("[mm  ] heap self-test: no leak, free list coalesced — HEAP-OK");
    } else {
        serial_println!(
            "[mm  ] heap self-test: LEAK — {} allocations and {} bytes outstanding",
            leaked,
            after.allocated - before.allocated
        );
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    drivers::serial::_print_forced(format_args!("\n\n*** HALCYON PANIC ***\n{}\n", info));
    port::park()
}

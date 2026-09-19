//! HALCYON — a from-scratch x86_64 operating system.
//!
//! No Linux, no libc, no third-party crates: `core`, `alloc`, and the code in
//! this repository.

#![no_std]
#![no_main]

extern crate alloc;

pub mod arch;
pub mod boot;
pub mod bootscreen;
pub mod drivers;
pub mod gfx;
pub mod mm;
pub mod panic_screen;
pub mod sync;

use alloc::format;
use core::panic::PanicInfo;

use arch::port;
use bootscreen::{BootScreen, Status};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Entry point, called from the bootstrap in `boot/boot.S` once the CPU is in
/// long mode and running out of the higher half.
#[no_mangle]
pub extern "C" fn kmain(mbi_phys: u64) -> ! {
    drivers::serial::init();
    serial_println!();
    serial_println!("HALCYON v{} — cold start", VERSION);

    // The multiboot structure lives in low memory, reachable only while the
    // bootstrap identity map is still active. Parse it into owned storage now.
    let info = unsafe { boot::parse(mbi_phys) };
    log_boot_info(&info);

    let cpu = arch::cpu::identify();
    serial_println!(
        "[cpu ] {} | vendor {} | pat:{} nx:{} apic:{}",
        cpu.brand_str(),
        cpu.vendor_str(),
        cpu.has_pat,
        cpu.has_nx,
        cpu.has_apic
    );

    arch::interrupts::init();
    arch::pit::init();
    arch::interrupts::enable();

    // Switches CR3 and brings up the heap; the identity map is gone afterwards.
    let memory = unsafe { mm::init(&info) };

    let display = match info.framebuffer {
        Some(framebuffer) => gfx::fb::init(&framebuffer, memory.space.framebuffer_virt),
        None => Err(gfx::fb::InitError::NoFramebuffer),
    };

    let mut screen = BootScreen::new();
    screen.step(
        Status::Ok,
        format!("HALCYON v{}  —  {}", VERSION, info.bootloader_name()),
    );
    screen.step(
        Status::Ok,
        format!("long mode, higher half at {:#x}", boot::KERNEL_VMA),
    );

    let brand = cpu.brand_str();
    screen.step(
        Status::Ok,
        format!(
            "cpu: {}",
            if brand.is_empty() { cpu.vendor_str() } else { brand }
        ),
    );
    screen.step(
        Status::Ok,
        format!("interrupts: IDT loaded, PIT at {} Hz", arch::pit::TICK_HZ),
    );

    let (total, total_unit) = mm::format_bytes(memory.total_bytes);
    screen.step(
        Status::Ok,
        format!(
            "memory: {} {} RAM, 4-level paging, heap live",
            total, total_unit
        ),
    );

    match &display {
        Ok(()) => {
            let (width, height, depth, fast) = gfx::fb::FRAMEBUFFER.lock().describe();
            screen.step(
                Status::Ok,
                format!(
                    "display: {}x{}x{} {}",
                    width,
                    height,
                    depth,
                    if fast { "(direct blit)" } else { "(converted)" }
                ),
            );
            screen.step(
                Status::Ok,
                format!(
                    "framebuffer cache: {}",
                    if memory.space.write_combining {
                        "write-combining"
                    } else {
                        "uncached"
                    }
                ),
            );
        }
        Err(error) => {
            screen.step(Status::Fail, format!("display unavailable: {:?}", error));
        }
    }

    match selftest_heap() {
        Ok(report) => screen.step(Status::Ok, report),
        Err(report) => screen.step(Status::Fail, report),
    }

    for (index, module) in info.modules().iter().enumerate() {
        screen.step(
            Status::Info,
            format!(
                "module {}: {} KiB at {:#x}",
                index,
                (module.end - module.start) / 1024,
                module.start
            ),
        );
    }

    serial_println!("[boot] HALCYON-BOOT-OK");

    if info.cmdline().contains("selftest=fault") {
        screen.step(
            Status::Warn,
            format!("self-test: raising a deliberate page fault"),
        );
        arch::pit::delay_ms(400);
        unsafe {
            // Canonical (bits 63:47 clear) but far outside anything we map, so
            // the CPU takes a real page fault rather than a #GP.
            core::ptr::write_volatile(0x0000_0008_0000_0000_u64 as *mut u64, 1);
        }
    }

    screen.step(Status::Info, format!("desktop not yet implemented; idle"));

    loop {
        port::halt();
    }
}

fn log_boot_info(info: &boot::BootInfo) {
    serial_println!("[boot] loader: {}", info.bootloader_name());
    if !info.cmdline().is_empty() {
        serial_println!("[boot] cmdline: {}", info.cmdline());
    }
    let (start, end) = boot::kernel_image_extent();
    serial_println!(
        "[boot] kernel image {:#x}..{:#x} ({} KiB)",
        start,
        end,
        (end - start) / 1024
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
        None => serial_println!("[gfx ] bootloader supplied no framebuffer"),
    }
}

/// Exercise the allocator hard enough to catch the classic failures: a
/// free-list that does not coalesce, a leak on drop, or alignment padding that
/// never comes back.
fn selftest_heap() -> Result<alloc::string::String, alloc::string::String> {
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
    let served = after.total_allocations - before.total_allocations;

    if leaked == 0 && after.allocated == before.allocated {
        serial_println!("[mm  ] heap self-test: HEAP-OK");
        Ok(format!(
            "heap: {} allocations, no leak, {} free block(s)",
            served, after.free_blocks
        ))
    } else {
        serial_println!("[mm  ] heap self-test: LEAK");
        Err(format!(
            "heap: LEAK — {} allocations, {} bytes outstanding",
            leaked,
            after.allocated - before.allocated
        ))
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    drivers::serial::_print_forced(format_args!("\n\n*** HALCYON PANIC ***\n{}\n", info));
    port::park()
}

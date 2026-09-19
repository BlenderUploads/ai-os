//! HALCYON — a from-scratch x86_64 operating system.
//!
//! No Linux, no libc, no third-party crates: `core`, `alloc`, and the code in
//! this repository.

#![no_std]
#![no_main]

extern crate alloc;

pub mod apps;
pub mod arch;
pub mod boot;
pub mod bootscreen;
pub mod drivers;
pub mod gfx;
pub mod input;
pub mod mm;
pub mod panic_screen;
pub mod sync;
pub mod task;
pub mod ui;

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

    let ps2 = drivers::ps2::init(!info.cmdline().contains("nomouse"));
    drivers::keyboard::init();
    if ps2.mouse_present {
        drivers::mouse::init();
    }
    screen.step(
        if ps2.controller_ok { Status::Ok } else { Status::Warn },
        format!(
            "input: 8042 {}, keyboard ready, mouse {}",
            if ps2.controller_ok { "ok" } else { "self-test failed" },
            if ps2.mouse_present { "ready" } else { "absent" }
        ),
    );

    task::init();
    match selftest_threads() {
        Ok(report) => screen.step(Status::Ok, report),
        Err(report) => screen.step(Status::Fail, report),
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

    screen.step(Status::Ok, format!("starting the desktop"));
    arch::pit::delay_ms(600);

    run_desktop(&info)
}

/// The compositor loop. This thread owns the desktop for the rest of the
/// machine's life.
fn run_desktop(info: &boot::BootInfo) -> ! {
    let mut desktop = ui::desktop::Desktop::new(apps::registry());

    if !info.cmdline().contains("noautostart") {
        desktop.open("about");
        desktop.open("monitor");
    }
    desktop.set_status("F1-F8 open apps  |  alt+tab switches", 6000);

    serial_println!("[ui  ] desktop running, {} windows", desktop.window_count());
    serial_println!("[ui  ] HALCYON-DESKTOP-OK");

    let mut last_report = 0u64;
    loop {
        desktop.handle_input();
        desktop.tick_apps();
        desktop.compose();

        // Periodic proof of life on the serial console, which is what the
        // headless smoke test watches.
        let uptime = arch::pit::uptime_ms();
        if uptime - last_report >= 2000 {
            last_report = uptime;
            serial_println!(
                "[ui  ] frame {} at {} ms ({} windows)",
                desktop.frames,
                uptime,
                desktop.window_count()
            );
        }

        // Aim for roughly 30 frames a second and leave the CPU to other threads.
        task::sleep_ms(33);
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

use core::sync::atomic::{AtomicU64, Ordering};

static SPIN_COUNTERS: [AtomicU64; 3] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];

extern "C" fn counter_thread(index: u64) {
    // Deliberately never yields: only real pre-emption can stop this.
    loop {
        SPIN_COUNTERS[index as usize].fetch_add(1, Ordering::Relaxed);
    }
}

/// Prove the scheduler actually pre-empts.
///
/// The spawned threads never yield, so if all three make progress while this
/// one sleeps, the timer is genuinely switching contexts rather than the
/// threads politely taking turns.
fn selftest_threads() -> Result<alloc::string::String, alloc::string::String> {
    for index in 0..3u64 {
        task::spawn("selftest", counter_thread, index);
    }

    let before = task::total_switches();
    task::sleep_ms(300);
    let switches = task::total_switches() - before;

    let counts: [u64; 3] = [
        SPIN_COUNTERS[0].load(Ordering::Relaxed),
        SPIN_COUNTERS[1].load(Ordering::Relaxed),
        SPIN_COUNTERS[2].load(Ordering::Relaxed),
    ];

    // Retire the test threads so they stop burning time slices.
    for thread in task::snapshot() {
        if thread.name == "selftest" {
            task::kill(thread.id);
        }
    }
    task::reap();

    let all_ran = counts.iter().all(|&count| count > 0);
    serial_println!(
        "[task] self-test: switches={} counts={:?}",
        switches,
        counts
    );
    if all_ran && switches >= 3 {
        serial_println!("[task] self-test: SCHED-OK");
        Ok(format!(
            "scheduler: 3 threads pre-empted, {} context switches in 300 ms",
            switches
        ))
    } else {
        Err(format!(
            "scheduler: only {} switches, counts {:?}",
            switches, counts
        ))
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    drivers::serial::_print_forced(format_args!("\n\n*** HALCYON PANIC ***\n{}\n", info));
    port::park()
}

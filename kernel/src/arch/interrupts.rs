//! Interrupt dispatch: the Rust side of `isr.S`.

use core::arch::{asm, global_asm};

use crate::arch::pic;
use crate::arch::port;

global_asm!(include_str!("isr.S"), options(att_syntax));

/// Register state at the point of an interrupt, laid out to match the push
/// order in `isr_common`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TrapFrame {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub vector: u64,
    pub error_code: u64,
    // Pushed by the CPU.
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

pub const IRQ_BASE: u64 = 32;
pub const IRQ_TIMER: u8 = 0;
pub const IRQ_KEYBOARD: u8 = 1;
pub const IRQ_CASCADE: u8 = 2;
pub const IRQ_MOUSE: u8 = 12;

const EXCEPTION_NAMES: [&str; 32] = [
    "divide error",
    "debug",
    "non-maskable interrupt",
    "breakpoint",
    "overflow",
    "bound range exceeded",
    "invalid opcode",
    "device not available",
    "double fault",
    "coprocessor segment overrun",
    "invalid TSS",
    "segment not present",
    "stack-segment fault",
    "general protection fault",
    "page fault",
    "reserved",
    "x87 floating-point exception",
    "alignment check",
    "machine check",
    "SIMD floating-point exception",
    "virtualisation exception",
    "control protection exception",
    "reserved",
    "reserved",
    "reserved",
    "reserved",
    "reserved",
    "hypervisor injection exception",
    "VMM communication exception",
    "security exception",
    "reserved",
    "reserved",
];

pub type IrqHandler = fn(&mut TrapFrame) -> *mut TrapFrame;

static mut IRQ_HANDLERS: [Option<IrqHandler>; 16] = [None; 16];

/// Install a handler for a hardware IRQ (0-15) and unmask it.
pub fn register_irq(irq: u8, handler: IrqHandler) {
    assert!(irq < 16, "IRQ out of range");
    port::without_interrupts(|| unsafe {
        IRQ_HANDLERS[irq as usize] = Some(handler);
    });
    pic::unmask(irq);
}

pub fn init() {
    super::gdt::init();
    super::idt::init();
    pic::init();
}

pub fn enable() {
    port::sti();
}

fn read_cr2() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

/// Called from `isr_common`. Returns the frame to restore, which lets the
/// scheduler switch threads by handing back a different one.
#[no_mangle]
pub extern "C" fn interrupt_dispatch(frame: *mut TrapFrame) -> *mut TrapFrame {
    let trap = unsafe { &mut *frame };
    let vector = trap.vector;

    if vector < 32 {
        handle_exception(trap);
    }

    if (IRQ_BASE..IRQ_BASE + 16).contains(&vector) {
        let irq = (vector - IRQ_BASE) as u8;
        let handler = unsafe { IRQ_HANDLERS[irq as usize] };
        let next = match handler {
            Some(handler) => handler(trap),
            None => frame,
        };
        // Spurious IRQ7/IRQ15 must not be acknowledged.
        if !pic::is_spurious(irq) {
            pic::end_of_interrupt(irq);
        }
        return next;
    }

    crate::serial_println!("[irq ] unexpected vector {}", vector);
    frame
}

fn handle_exception(trap: &mut TrapFrame) -> ! {
    let vector = trap.vector as usize;
    let name = EXCEPTION_NAMES.get(vector).copied().unwrap_or("unknown");

    // Breakpoints are used by the boot self-test; report and continue would
    // need care, so for now every exception is terminal but explained.
    crate::serial_println!();
    crate::serial_println!("*** CPU EXCEPTION: {} (vector {}) ***", name, vector);
    crate::serial_println!(
        "  rip={:#018x} cs={:#x} rflags={:#x}",
        trap.rip,
        trap.cs,
        trap.rflags
    );
    crate::serial_println!("  rsp={:#018x} ss={:#x}", trap.rsp, trap.ss);
    crate::serial_println!("  error_code={:#x}", trap.error_code);
    if vector == 14 {
        let cr2 = read_cr2();
        let code = trap.error_code;
        crate::serial_println!(
            "  faulting address {:#018x} ({}, {}, {})",
            cr2,
            if code & 1 != 0 { "protection violation" } else { "not present" },
            if code & 2 != 0 { "write" } else { "read" },
            if code & 4 != 0 { "user" } else { "kernel" },
        );
    }
    crate::serial_println!(
        "  rax={:#018x} rbx={:#018x} rcx={:#018x}",
        trap.rax,
        trap.rbx,
        trap.rcx
    );
    crate::serial_println!(
        "  rdx={:#018x} rsi={:#018x} rdi={:#018x}",
        trap.rdx,
        trap.rsi,
        trap.rdi
    );
    crate::serial_println!("  rbp={:#018x} r8 ={:#018x}", trap.rbp, trap.r8);

    crate::panic_screen::fault(name, trap, if vector == 14 { Some(read_cr2()) } else { None })
}

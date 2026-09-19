//! Pre-emptive kernel threads.
//!
//! The whole switch mechanism falls out of how `isr_common` is written: it
//! calls the dispatcher with a pointer to the interrupted register frame and
//! then restores whatever frame the dispatcher hands back. A thread is
//! therefore nothing more than a stack with a saved frame on it, and switching
//! is picking a different pointer to return.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::arch::gdt;
use crate::arch::interrupts::TrapFrame;
use crate::arch::pit;
use crate::sync::SpinLock;

pub const DEFAULT_STACK_SIZE: usize = 64 * 1024;

/// Software interrupt used to give up the rest of a time slice.
pub const YIELD_VECTOR: u8 = 0x80;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ready,
    Running,
    /// Waiting for `pit::ticks()` to reach the stored deadline.
    Sleeping(u64),
    Finished,
}

pub struct Thread {
    pub id: u64,
    pub name: String,
    pub state: State,
    /// Where this thread's register frame sits on its own stack.
    frame: *mut TrapFrame,
    /// Kept alive so the stack is not freed under a running thread.
    _stack: Option<Box<[u8]>>,
    pub ticks_used: u64,
    pub switches: u64,
}

unsafe impl Send for Thread {}

struct Scheduler {
    threads: Vec<Thread>,
    current: usize,
    next_id: u64,
    switches: u64,
    started: bool,
}

impl Scheduler {
    const fn new() -> Self {
        Self {
            threads: Vec::new(),
            current: 0,
            next_id: 0,
            switches: 0,
            started: false,
        }
    }

    /// Round-robin over runnable threads, waking any whose sleep has expired.
    fn pick_next(&mut self, now: u64) -> usize {
        let count = self.threads.len();
        for thread in self.threads.iter_mut() {
            if let State::Sleeping(deadline) = thread.state {
                if now >= deadline {
                    thread.state = State::Ready;
                }
            }
        }
        for offset in 1..=count {
            let index = (self.current + offset) % count;
            match self.threads[index].state {
                State::Ready => return index,
                _ => continue,
            }
        }
        // Nothing else can run; stay where we are if we still can, else idle
        // (thread 0 is the idle thread and is always runnable).
        if matches!(self.threads[self.current].state, State::Running) {
            self.current
        } else {
            0
        }
    }
}

static SCHEDULER: SpinLock<Scheduler> = SpinLock::new(Scheduler::new());

/// Build a register frame that will start `entry(argument)` when restored.
///
/// The frame sits just below the thread's initial stack pointer, and a return
/// address pointing at `thread_exit` sits above it, so a thread function that
/// simply returns is cleaned up rather than jumping into nothing.
fn prepare_stack(stack: &mut [u8], entry: extern "C" fn(u64), argument: u64) -> *mut TrapFrame {
    let base = stack.as_mut_ptr() as u64;
    let top = (base + stack.len() as u64) & !0xF;

    // SysV wants rsp ≡ 8 (mod 16) on entry to a function, which is exactly
    // what leaving one return address on the stack produces.
    let entry_rsp = top - 8;
    unsafe { core::ptr::write(entry_rsp as *mut u64, thread_exit as *const () as u64) };

    let frame_address = entry_rsp - core::mem::size_of::<TrapFrame>() as u64;
    let frame = frame_address as *mut TrapFrame;

    unsafe {
        core::ptr::write(
            frame,
            TrapFrame {
                rdi: argument,
                rip: entry as usize as u64,
                cs: gdt::KERNEL_CODE as u64,
                // IF set: the thread starts with interrupts enabled, which is
                // what makes it pre-emptible from its very first instruction.
                rflags: 0x202,
                rsp: entry_rsp,
                ss: gdt::KERNEL_DATA as u64,
                ..Default::default()
            },
        );
    }
    frame
}

/// Register the currently executing context as thread 0.
///
/// Its frame is filled in by the first timer tick, which is the first time the
/// running context actually has a saved frame to point at.
pub fn init() {
    let mut scheduler = SCHEDULER.lock();
    assert!(scheduler.threads.is_empty(), "scheduler already initialised");
    scheduler.threads.push(Thread {
        id: 0,
        name: String::from("kernel"),
        state: State::Running,
        frame: core::ptr::null_mut(),
        _stack: None,
        ticks_used: 0,
        switches: 0,
    });
    scheduler.next_id = 1;
    drop(scheduler);

    pit::set_tick_hook(tick);
}

pub fn spawn(name: &str, entry: extern "C" fn(u64), argument: u64) -> u64 {
    spawn_with_stack(name, entry, argument, DEFAULT_STACK_SIZE)
}

pub fn spawn_with_stack(
    name: &str,
    entry: extern "C" fn(u64),
    argument: u64,
    stack_size: usize,
) -> u64 {
    let mut stack: Box<[u8]> = vec![0u8; stack_size].into_boxed_slice();
    let frame = prepare_stack(&mut stack, entry, argument);

    let mut scheduler = SCHEDULER.lock();
    let id = scheduler.next_id;
    scheduler.next_id += 1;
    scheduler.threads.push(Thread {
        id,
        name: String::from(name),
        state: State::Ready,
        frame,
        _stack: Some(stack),
        ticks_used: 0,
        switches: 0,
    });
    id
}

/// Called from the timer IRQ. Returns the frame to resume.
fn tick(frame: &mut TrapFrame) -> *mut TrapFrame {
    let now = pit::ticks();
    let mut scheduler = SCHEDULER.lock();
    if scheduler.threads.is_empty() {
        return frame as *mut TrapFrame;
    }

    let current = scheduler.current;
    scheduler.threads[current].ticks_used += 1;
    scheduler.threads[current].frame = frame as *mut TrapFrame;
    scheduler.started = true;

    let next = scheduler.pick_next(now);
    if next == current {
        return frame as *mut TrapFrame;
    }

    if scheduler.threads[current].state == State::Running {
        scheduler.threads[current].state = State::Ready;
    }
    scheduler.threads[next].state = State::Running;
    scheduler.threads[next].switches += 1;
    scheduler.current = next;
    scheduler.switches += 1;
    scheduler.threads[next].frame
}

/// Handler for the `int 0x80` yield. Same logic as a timer tick.
pub fn on_yield(frame: &mut TrapFrame) -> *mut TrapFrame {
    tick(frame)
}

/// Give up the rest of this time slice.
pub fn yield_now() {
    unsafe {
        core::arch::asm!("int 0x80", options(nostack));
    }
}

/// Mark the current thread asleep and switch away.
pub fn sleep_ms(ms: u64) {
    if ms == 0 {
        yield_now();
        return;
    }
    let deadline = pit::ticks() + ms * pit::TICK_HZ as u64 / 1000;
    {
        let mut scheduler = SCHEDULER.lock();
        let current = scheduler.current;
        scheduler.threads[current].state = State::Sleeping(deadline);
    }
    yield_now();
    // A tick could have fired between setting the deadline and yielding; loop
    // until the time has genuinely passed.
    while pit::ticks() < deadline {
        yield_now();
    }
}

/// Where a thread function lands if it returns.
extern "C" fn thread_exit() -> ! {
    {
        let mut scheduler = SCHEDULER.lock();
        let current = scheduler.current;
        scheduler.threads[current].state = State::Finished;
    }
    loop {
        yield_now();
    }
}

/// Mark a thread finished. It stops being scheduled; `reap` frees its stack.
pub fn kill(id: u64) -> bool {
    let mut scheduler = SCHEDULER.lock();
    for thread in scheduler.threads.iter_mut() {
        if thread.id == id && thread.id != 0 {
            thread.state = State::Finished;
            return true;
        }
    }
    false
}

pub fn current_id() -> u64 {
    let scheduler = SCHEDULER.lock();
    scheduler
        .threads
        .get(scheduler.current)
        .map(|thread| thread.id)
        .unwrap_or(0)
}

#[derive(Clone)]
pub struct ThreadInfo {
    pub id: u64,
    pub name: String,
    pub state: State,
    pub ticks_used: u64,
    pub switches: u64,
    pub current: bool,
}

pub fn snapshot() -> Vec<ThreadInfo> {
    let scheduler = SCHEDULER.lock();
    scheduler
        .threads
        .iter()
        .enumerate()
        .map(|(index, thread)| ThreadInfo {
            id: thread.id,
            name: thread.name.clone(),
            state: thread.state,
            ticks_used: thread.ticks_used,
            switches: thread.switches,
            current: index == scheduler.current,
        })
        .collect()
}

pub fn total_switches() -> u64 {
    SCHEDULER.lock().switches
}

/// Drop finished threads and reclaim their stacks.
pub fn reap() -> usize {
    let mut scheduler = SCHEDULER.lock();
    let current = scheduler.current;
    let current_id = scheduler.threads[current].id;
    let before = scheduler.threads.len();
    // Never reap the running thread: we are executing on its stack, and
    // dropping the Box would free it out from under us.
    scheduler
        .threads
        .retain(|thread| thread.state != State::Finished || thread.id == current_id);
    let after = scheduler.threads.len();
    if before != after {
        // Indices shifted; find where the running thread went.
        scheduler.current = scheduler
            .threads
            .iter()
            .position(|thread| thread.id == current_id)
            .unwrap_or(0);
    }
    before - after
}

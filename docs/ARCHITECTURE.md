# How HALCYON is put together

A tour from the first instruction to the desktop, in the order things happen.

## 1. The bootloader hands over

GRUB loads the kernel through **Multiboot2** and enters at `_start` in 32-bit
protected mode with paging off. Two consequences shape everything early:

- The entry point must be reachable from a 32-bit register, so the bootstrap is
  linked low (1 MiB) even though the rest of the kernel is linked for the
  higher half. CI checks this.
- The kernel is an ELF64 whose higher-half segments carry a *physical* load
  address via `AT()` in the linker script. GRUB's ELF loader honours `p_paddr`,
  which is what lets one image be linked at `0xFFFFFFFF80000000` and loaded at
  `0x100000`.

`kernel/src/boot/boot.S` validates the multiboot magic, confirms the CPU has
long mode via CPUID leaf `0x80000001`, builds boot page tables, and switches on
long mode. See [MEMORY-MAP.md](MEMORY-MAP.md) for the addresses.

Two details matter and are easy to get wrong:

- **SSE must be enabled before any Rust runs.** The target is built soft-float,
  but the FPU still has to be in a sane state, so `CR0.EM` is cleared and
  `CR4.OSFXSR` set in the 64-bit stub.
- **The multiboot pointer must survive the transition.** It is moved out of
  `%rbp` immediately, because the first thing a compiler does with `%rbp` is
  use it as a frame pointer. This was an actual bug during development: the
  tag walk silently read zeros.

## 2. Interrupts

`arch/gdt.rs` loads a GDT with a TSS supplying the interrupt stack table. Double
faults and page faults get their own stacks, which is the difference between
seeing a useful error and watching the machine triple-fault.

`arch/isr.S` generates all 256 entry points with `.rept`, each padded to exactly
16 bytes, so the IDT is filled arithmetically as `isr_stub_table + vector * 16`
instead of needing 256 labels. `idt::init` asserts the table really spans
`256 * STUB_STRIDE`, because a stub that outgrew its slot would silently point
every later vector into the middle of another one.

The dispatcher has an unusual signature:

```rust
extern "C" fn interrupt_dispatch(frame: *mut TrapFrame) -> *mut TrapFrame
```

It returns the frame to restore. `isr_common` does `mov rsp, rax` before popping
registers, so returning a *different* frame switches the machine to a different
stack. This is the whole context-switch mechanism (see §5).

Interrupts are the legacy 8259 PIC remapped to vectors 32..47, not the APIC.
HALCYON is single-CPU, and the PIC is the controller every machine old enough to
be interesting still implements correctly. IRQ7 and IRQ15 are checked against
the in-service register so spurious interrupts are not acknowledged.

## 3. Memory

Three layers, in `kernel/src/mm/`:

**Frames** (`frame.rs`) — one bit per 4 KiB frame. The bitmap starts entirely
set and is cleared only for regions the firmware calls usable, so holes in the
memory map are reserved by construction rather than by enumeration. The kernel
image, the low 1 MiB, the initrd and the bitmap itself are then claimed back.

**Paging** (`paging.rs`) — builds the real address space and switches CR3. The
bootstrap's identity map is *not* carried over: a stray write through a low
pointer should fault, not quietly corrupt the first 4 GiB. Kernel sections get
honest permissions (text RX, rodata R, data/bss RW+NX).

One subtlety worth knowing: the frame allocator's bitmap pointer is a bare
physical address that only works under the bootstrap mapping, so it is rebased
onto the physical map in the same breath as the CR3 switch. Without that the
very next allocation page-faults.

The framebuffer is mapped write-combining through PAT entry 4 where the CPU
supports it. On real hardware this is the difference between a usable desktop
and a slideshow.

**Heap** (`heap.rs`) — an address-sorted free list that coalesces with both
neighbours on release. Every allocation is rounded to 16 bytes and aligned to at
least 16, which is what makes it leak-free: padding produced when honouring a
larger alignment is itself a multiple of 16, so it is either zero or big enough
to return to the list. The heap grows by mapping fresh frames, up to 512 MiB.

## 4. Graphics

Everything draws into a RAM `Surface` and one pass pushes it to the screen.
Read-modify-write against uncached video memory is ruinous on real hardware, so
the compositor never does it.

`Surface` carries a clip rectangle and the full primitive set. Window contents
and the screen are the same type, which is why the widget code never knows where
it will end up.

The CRT pass darkens alternate scanlines. It runs over the whole screen every
frame, so it avoids the divide in `palette::scale`: subtracting `c >> 2` per
channel is a shift, a mask and a subtract. The mask stops each channel's borrow
bleeding into the one below. The vignette is far more expensive and is baked
once into a cached backdrop surface that is blitted per frame.

The font is drawn in `tools/mkfont.py` and generated into a committed
`font_data.rs`. See [FONT.md](FONT.md).

### Changing resolution

GRUB sets the video mode before the kernel runs an instruction, and changing it
afterwards normally means calling the VESA BIOS — 16-bit code that cannot be
called from long mode without an emulator.

`gfx/modeset.rs` takes the one route that needs no BIOS: the Bochs DISPI
registers, a pair of I/O ports that take a width and a height directly. The
standard adapter in QEMU, Bochs, VirtualBox and VMware implements them; a real
graphics card does not. The probe is deliberately narrow — the PCI display
device has to claim the framebuffer the bootloader handed over *and* be one of
three known adapter IDs — because ports 0x1CE/0x1CF are a configuration
index/data pair on some real chipsets, and writing to those on someone's laptop
is not a risk worth taking for a feature that would not work there anyway.

The framebuffer's physical address does not move when the mode changes, so the
mapping does not have to be rebuilt; paging maps 16 MiB of the aperture at boot
rather than exactly the booted mode, which is enough for anything on offer. The
compositor asks the framebuffer its size once a frame and rebuilds the backdrop
and window geometry when the answer changes, so nothing else in the system has
to know mode setting exists.

Where it is unavailable, the boot menu's resolution submenu does the same job
one restart earlier — `set gfxpayload` *after* the `multiboot2` line, because
that command overwrites gfxpayload from the kernel's own framebuffer tag.

## 5. Threads

Given §2, a thread is a stack with a saved trap frame on it, and a context
switch is the timer handler returning a different pointer. `prepare_stack`
synthesises a frame with `IF` already set, plus a return address pointing at
`thread_exit` so a thread function that simply returns is cleaned up rather than
jumping into nothing.

Scheduling is round-robin at the 1 kHz timer tick, with `int 0x80` as an
explicit yield. The idle thread is skipped in the normal rotation and chosen
only when nothing else can run, where it halts the CPU until the next interrupt.

`reap()` never collects the running thread — we are executing on its stack.

## 6. Sound

The one device HALCYON drives by DMA. `drivers/ac97.rs` finds an Intel AC'97
codec on the PCI bus, allocates 17 physically contiguous pages below 4 GiB (the
buffer descriptors hold 32-bit addresses), and fills a 32-entry descriptor list
with 512-frame buffers — 10.7 ms each at 48 kHz.

There is no interrupt handler. A kernel thread polls the current-index register
every 4 ms and refills whatever the engine has finished with, keeping about six
buffers ahead of the play cursor. At that buffer size polling is both simpler
than an IRQ path and impossible to tell apart from one, and the thread sleeps at
50 ms until something actually asks for sound, so a machine that never plays
anything never touches the codec.

`audio.rs` is the ring between the two: producers write interleaved stereo and
never block, because a producer stalling on the codec is worse than one skipping
a few milliseconds. Short reads are played as silence.

## 7. The desktop

One thread owns everything: it drains the input queues, lets each app tick,
repaints any window whose app reports itself dirty, composites, and sleeps to
pace itself at roughly 30 fps.

Windows are kept bottom-to-top in a vector, so the last is on top and hit
testing walks backwards. Apps implement a small trait (`ui/window.rs`) and draw
into their own surface. See [ADDING-AN-APP.md](ADDING-AN-APP.md).

The `App` trait is deliberately **not** `Send`: the desktop and every app it owns
run on one thread, and requiring `Send` would rule out ORACLE's `Rc`-based
environments for no benefit.

## 8. ORACLE

A small Lisp (`kernel/src/oracle/`) plus a keyword-matched persona. Lists are
vectors rather than cons pairs; tail positions loop rather than recurse. See
[ORACLE.md](ORACLE.md).

## What is deliberately missing

- **No disk writes.** There is no block-device write path anywhere in the tree.
  This is what makes booting HALCYON on a real machine safe.
- **No network stack.** No driver, no TCP, no sockets.
- **One audio driver.** AC'97 only; no Intel HD Audio, no capture, no MIDI.
- **No USB.** Input is PS/2 via the 8042. See [HARDWARE.md](HARDWARE.md) for
  what that means for your laptop.
- **No user mode.** Everything runs in ring 0. The GDT has the ring-3
  descriptors and the TSS has `rsp0` plumbed, but nothing uses them yet.
- **No SMP.** One CPU.

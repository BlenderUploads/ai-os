# Address space

## Virtual layout

| base | contents |
|---|---|
| `0xFFFF_FFFF_8000_0000` | kernel image, per-section permissions |
| `0xFFFF_A000_0000_0000` | kernel heap, grown on demand to 512 MiB |
| `0xFFFF_9000_0000_0000` | framebuffer, write-combining where PAT allows |
| `0xFFFF_8000_0000_0000` | direct map of all physical RAM, 2 MiB pages, NX |
| *(nothing below)* | unmapped — a stray low pointer faults |

The bootstrap's 4 GiB identity map exists only until `paging::init` switches
CR3. Dropping it is the point: a null or small-integer pointer dereference
should be a page fault, not a silent write into the first few gigabytes of the
machine.

`phys_to_virt(p)` adds a global offset that is `0` before the switch and
`PHYSMAP_BASE` after. Any code holding a bare physical address across the
switch has to be rebased — the frame allocator's bitmap is the one place that
applies, and it is handled in the same function that writes CR3.

## Kernel section permissions

| section | flags |
|---|---|
| `.text` | present, read, execute, global |
| `.rodata` | present, read, **NX**, global |
| `.data` / `.bss` | present, read, write, **NX**, global |

## Physical layout during boot

| address | contents |
|---|---|
| `0x0000_0000`–`0x0010_0000` | real-mode memory and BIOS structures — never allocated |
| `0x0010_0000` | `.boot`: multiboot header, 32-bit stub, boot GDT |
| *(next pages)* | `.bootbss`: PML4, two PDPTs, four PDs, 16 KiB boot stack |
| *(next)* | the higher-half kernel image, loaded via `AT()` |
| *(after the kernel)* | the initrd, placed by GRUB |
| *(first fitting gap)* | the frame bitmap |
| *(lowest 68 KiB run)* | the AC'97 buffers, if there is a codec |

The audio buffers are the only allocation in the system with a physical
constraint: the AC'97 descriptor list holds 32-bit addresses, so its 17
contiguous pages have to sit below 4 GiB. The frame allocator scans upwards
from zero, so in practice they land within the first few megabytes of free RAM
and the check never fires.

The bootstrap page tables map:

- **Identity, 0–4 GiB**, four page directories of 2 MiB pages. Four gigabytes
  rather than one because the framebuffer usually sits high (QEMU puts it at
  `0xFD00_0000`) and early code needs to reach it.
- **Higher half**, `0xFFFF_FFFF_8000_0000` → physical `0`, one directory. The
  PML4 index for that address is 511 and the PDPT index is 510, which is why the
  stub writes exactly those two slots.

## Why the kernel is at −2 GiB

`x86_64-unknown-none` compiles with the **kernel code model**, which assumes all
code and static data live in the top 2 GiB of the address space so that
references fit in a sign-extended 32-bit displacement. `0xFFFF_FFFF_8000_0000`
is the bottom of that window.

## Sizes

| | |
|---|---|
| frame | 4 KiB |
| physical map page | 2 MiB |
| heap initial / maximum | 8 MiB / 512 MiB |
| heap minimum block and alignment | 16 bytes |
| kernel stack (boot thread) | 64 KiB |
| per-thread stack | 64 KiB |
| double-fault and page-fault IST stacks | 16 KiB each |
| frame bitmap | 1 bit per frame — 16 KiB per GiB of RAM |
| AC'97 DMA region | 17 contiguous frames: 1 descriptor list + 32 × 2 KiB buffers |
| audio ring | 8192 stereo frames — 32 KiB, about 170 ms at 48 kHz |

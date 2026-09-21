# DOOM on HALCYON

DOOM runs on HALCYON as a window like any other app: menus, the 3D renderer,
the HUD, keyboard controls, saving. It is the real engine, not a lookalike.

## Read this first: licensing

**The DOOM engine is GPL-2. The rest of HALCYON is MIT.** Linking them produces
a combined work covered by the GPL, so this is deliberately opt-in:

| build | contents | licence of the result |
|---|---|---|
| `make iso` | HALCYON alone; `doom/` is never compiled | MIT |
| `make doom-iso` | HALCYON with the DOOM engine linked in | **GPL-2** |

The default build does not touch this directory. If you distribute
`halcyon-doom.iso`, you are distributing a GPL-2 work and its terms apply —
including offering the source, which is this repository.

## What is vendored here

| | |
|---|---|
| `src/` | [doomgeneric](https://github.com/ozkl/doomgeneric), a port-friendly fork of id Software's GPL release, minus its platform backends |
| `LICENSE` | GPL-2, as shipped with that source |
| `include/`, `libc.c` | **written for HALCYON**, not vendored |

The engine source is unmodified. Everything HALCYON-specific is in the C
library and in `kernel/src/apps/doom.rs`.

## How the port works

doomgeneric already factors the platform out into six functions. HALCYON
implements them in `kernel/src/apps/doom.rs`:

```
DG_Init            allocate the shared frame buffer
DG_DrawFrame       copy DOOM's 640x400 output where the compositor can reach it
DG_SleepMs         task::sleep_ms
DG_GetTicksMs      the PIT
DG_GetKey          pop a translated key from the window's queue
DG_SetWindowTitle  retitle the window (which is why it says "Freedoom: Phase 1")
```

The harder half is that DOOM expects a C library, and HALCYON has none.
`libc.c` is the smallest one that satisfies it: allocation, strings, character
classes, `printf` and friends, file I/O, and the five maths functions DOOM
actually calls (`sin`, `tan`, `atan`, `fabs`, `abs` — all during start-up table
generation, none in a hot loop). It rests on eight `hal_*` hooks the kernel
exports.

Three decisions worth knowing about:

- **`malloc` carries its own size header.** Rust's allocator needs the layout
  back at free time, and C's `free` does not supply one.

- **Reads borrow the kernel's bytes.** A `FILE` opened for reading points
  straight into the filesystem's storage. The IWAD is nearly 30 MB and is
  mounted in place from the GRUB module it arrived in — it is never copied, not
  at boot and not at open.

- **DOOM gets its own thread.** `doomgeneric_Tick` blocks internally, so
  driving it from the compositor would stall every other window on a slow
  frame. This is also why the scheduler now saves vector registers across
  context switches: the engine is compiled with SSE, the kernel is soft-float,
  and a thread's FPU state has to survive being pre-empted.

`I_Error` lands in `hal_panic`, which reports into the window and retires the
game thread rather than taking the machine down. A crash in DOOM is a dead
window, not a dead desktop.

## The IWAD

The engine ships with no game data. `make doom-iso` calls
`tools/fetch-doom-wad.sh`, which prefers an IWAD already on your machine and
otherwise fetches **Freedoom** — BSD-licensed, freely redistributable, and
what the screenshots show.

To play the real thing, drop your own `doom1.wad`, `doom.wad` or `doom2.wad` in
as `build/doom.wad` before building. The commercial WADs are not
redistributable, which is why none is included.

The WAD is loaded as its own GRUB module, tagged `doom`, and mounted at
`/doom.wad`.

## Controls

Arrow keys move and turn, `ctrl` fires, `space` opens doors and flips switches,
`alt` strafes, number keys select weapons, `esc` opens the menu. **F11** makes
the window fullscreen — F11 is handled by the window manager and never reaches
the game.

## Rebuilding the vendored source

```sh
git clone --depth 1 https://github.com/ozkl/doomgeneric
# copy doomgeneric/doomgeneric/*.{c,h} into src/, minus doomgeneric_*.c backends
```

The file list is the `SRC_DOOM` variable in doomgeneric's own Makefile.

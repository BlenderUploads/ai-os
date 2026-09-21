# DOOM on HALCYON

DOOM runs on HALCYON as a window like any other app: menus, the 3D renderer,
the HUD, keyboard and mouse controls, sound, saving. It is the real engine, not
a lookalike.

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
| `include/`, `libc.c`, `sound.c` | **written for HALCYON**, not vendored |

`src/` is unmodified, byte for byte. Everything HALCYON-specific lives beside
it — the C library, the sound module, and `kernel/src/apps/doom.rs`.

One consequence worth recording: `src/i_sound.c` includes `<SDL_mixer.h>`
whenever the sound path is compiled in, and never calls anything from it. Rather
than edit that line out, `include/SDL_mixer.h` is an empty header.

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

It has no mouse hook at all, so pointer movement goes in the way the engine's
own backends do it: `D_PostEvent` with an `ev_mouse`. That call belongs to the
game thread, so the compositor queues the deltas and the game thread drains
them between frames.

The harder half is that DOOM expects a C library, and HALCYON has none.
`libc.c` is the smallest one that satisfies it: allocation, strings, character
classes, `printf` and friends, file I/O, and the five maths functions DOOM
actually calls (`sin`, `tan`, `atan`, `fabs`, `abs` — all during start-up table
generation, none in a hot loop). It rests on a handful of `hal_*` hooks the
kernel exports.

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

## Sound

doomgeneric leaves `DG_sound_module` for the platform to define, and `sound.c`
is HALCYON's: sixteen voices, mixing DMX sound lumps into interleaved stereo and
resampling them with 16.16 fixed-point stepping — DOOM's effects are 8-bit mono
at 11 or 22 kHz and the codec wants 16-bit stereo at 48 kHz. The result goes
into a ring buffer in `kernel/src/audio.rs`, and `kernel/src/drivers/ac97.rs`
feeds it to an AC'97 codec by DMA.

It is self-clocking: `I_UpdateSound` runs once a frame and tops the ring up to
about 64 ms, so the mixer can only run as fast as the codec drains it. Volume
and stereo separation use vanilla DOOM's own formula, because that is what the
game was balanced against.

**Music is not implemented.** DOOM's music is MUS, which would need converting
to MIDI and then synthesising — an OPL emulator or a wavetable, either of which
is more work than the whole effects path above. The music module is present but
declines everything, so the game runs with sound and silence where the music
would be.

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
`alt` strafes, `shift` runs, number keys select weapons, `esc` opens the menu.
Those three modifiers reach the game because the keyboard driver reports them as
keys in their own right as well as as flags — every other application only wants
the flags, but a game needs to know when the trigger goes down.

**Click in the window to aim with the mouse.** The pointer is captured: the
cursor disappears and raw movement turns the player, with the left button firing
and the right strafing. **ctrl+G** gives the pointer back, and so does anything
that moves the focus elsewhere. Vertical movement does nothing — vanilla DOOM
walks forward and back on the mouse's Y axis, since it has no vertical aiming to
spend it on, and that surprises anyone who has used a mouse since 1993.

**F11** makes the window fullscreen — F11 is handled by the window manager and
never reaches the game. F1 to F10 belong to the desktop's launcher, so DOOM's
own function keys only work in fullscreen.

## Rebuilding the vendored source

```sh
git clone --depth 1 https://github.com/ozkl/doomgeneric
# copy doomgeneric/doomgeneric/*.{c,h} into src/, minus doomgeneric_*.c backends
```

The file list is the `SRC_DOOM` variable in doomgeneric's own Makefile.

# Running HALCYON on real hardware

## The short version

`halcyon.iso` is a hybrid image. Write it to a USB stick and it boots on both
legacy BIOS and 64-bit UEFI machines. It runs entirely from RAM and **never
writes to any disk** — there is no block-device write path anywhere in the
source tree — so booting it on a machine you care about cannot alter what is
already on it.

Requirements:

- A 64-bit x86 CPU (anything from a 2005 Athlon 64 or 2006 Core 2 onwards).
- About 64 MiB of RAM. It will use more if you have it.
- A keyboard the firmware presents through the 8042 controller. This is the one
  real compatibility risk; see below.

## Writing the ISO to a USB stick

**Linux / macOS** — replace `sdX` with your stick, and check twice:

```sh
sudo dd if=halcyon.iso of=/dev/sdX bs=4M status=progress conv=fsync
```

**Windows** — [Rufus](https://rufus.ie) in "DD image" mode, or
[balenaEtcher](https://etcher.balena.io).

**Ventoy** — copy `halcyon.iso` onto the Ventoy partition. This is the least
destructive option if you already use it.

Then boot from USB: usually F12, F9, Esc or Del at power-on, depending on the
vendor.

## In a virtual machine

| | |
|---|---|
| **QEMU (BIOS)** | `qemu-system-x86_64 -m 512M -cdrom halcyon.iso` |
| **QEMU (UEFI)** | add `-bios /usr/share/OVMF/OVMF_CODE_4M.fd` |
| **VirtualBox** | New VM, type "Other/Unknown 64-bit", 512 MB RAM, attach the ISO as an optical disc. Enable EFI only if you want to test that path. |
| **VMware** | Same idea; "Other 64-bit" guest. |

No virtual disk is needed. HALCYON never looks for one.

## Known limitations, honestly

**USB-only keyboards may not work.** HALCYON talks to the 8042 PS/2 controller.
Laptop built-in keyboards are nearly always wired to it, and most desktop
firmware provides "USB legacy emulation" that makes a USB keyboard appear there
too. But some modern UEFI machines drop that emulation once the firmware hands
off, and HALCYON has no USB stack to fall back on. If the machine boots to the
desktop but ignores your typing, that is what happened.

Workarounds, in order of likelihood:

1. Look in the firmware setup for "USB Legacy Support", "USB Keyboard Support"
   or "XHCI Hand-off" and enable legacy / disable hand-off.
2. Boot the **legacy BIOS / CSM** entry rather than UEFI, which usually restores
   8042 emulation.
3. Use a laptop with a built-in keyboard.

**The trackpad may not move the cursor.** Same cause. Many laptop trackpads are
PS/2 and work; some are I2C-attached and will not. An external PS/2 mouse works
where a port exists. The GUI is usable from the keyboard alone: F1–F7 open
applications, alt+tab cycles, ctrl+W closes.

**The resolution.** HALCYON asks GRUB for 1920×1080×32, flagged optional. GRUB
turns that into a list — the exact mode, then that size at any depth, then
`auto` — and works down it, so a machine that cannot manage 1080p gets the best
mode it has rather than nothing. The kernel adapts to whatever arrives: other
sizes work, and so do 24- and 16-bit depths. Only direct-colour modes are
supported; if your firmware offers nothing but a palette mode, the boot screen
will say the display is unavailable.

To change it, it matters a great deal whether you are on a virtual machine:

- **In a VM** (QEMU, Bochs, VirtualBox), the standard display adapter has the
  Bochs DISPI registers, which take a width and a height directly and need no
  BIOS call. The **Settings** app lists the modes and switches between them
  while HALCYON runs.
- **On real hardware**, it does not, and HALCYON has no driver for your
  graphics card. The mode has to be chosen before the kernel starts: the boot
  menu's **"choose a screen resolution"** entry does that, and each mode falls
  back through plainer ones so a machine that cannot manage it still boots.
  Settings will say so rather than offering a switch that does nothing.

**If the mouse misbehaves**, the GRUB menu's **safe mode** entry boots without
touching the PS/2 mouse at all. The desktop is fully usable from the keyboard.

**Sound needs an AC'97 codec.** HALCYON has one audio driver, for Intel AC'97
(PCI class 04:01). That covers a great many machines from roughly 1999 to 2008
and the default codec in QEMU and VirtualBox, but not Intel HD Audio, which
replaced it on later hardware. The device browser's **PCI** tab says which you
have, and the boot screen says whether the codec answered. With no AC'97 the
machine simply runs silent — nothing else changes.

**No suspend, no battery reporting, no backlight control, no music in DOOM**
(see [doom/README.md](../doom/README.md)). The PC speaker is still used for the
boot chime and `beep`, which many laptops no longer have at all.

## If something goes wrong

- **It hangs before any HALCYON logo appears.** The bootloader never reached the
  kernel. Try the other firmware mode.
- **A red "HALCYON HAS STOPPED" screen.** The kernel faulted and is telling you
  where. Photograph it; the `rip` and faulting address are what matter. Nothing
  was written anywhere.
- **The machine reboots immediately.** A triple fault before the exception
  handler was installed. The serial console is the only way to see more: boot
  with a null-modem cable or, in a VM, `-serial stdio`.
- **You want the boot log.** The last GRUB entry mirrors everything to COM1 at
  38400 8N1.

## Verifying the download

Each release ships a `halcyon.iso.sha256` next to it:

```sh
sha256sum -c halcyon.iso.sha256
```

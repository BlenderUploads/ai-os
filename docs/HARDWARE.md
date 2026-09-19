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

**Unusual framebuffer formats.** HALCYON asks GRUB for 1024×768×32, but the
request is flagged optional, so GRUB falls back to whatever the hardware can
actually provide and the kernel adapts — other sizes work, and so do 24- and
16-bit depths. Only direct-colour modes are supported: if your firmware offers
nothing but a palette mode, the boot screen will say the display is unavailable.

**If the mouse misbehaves**, the GRUB menu's **safe mode** entry boots without
touching the PS/2 mouse at all. The desktop is fully usable from the keyboard.

**No suspend, no battery reporting, no backlight control, no sound beyond the
PC speaker** (which many laptops no longer have — `beep` will simply be silent).

## If something goes wrong

- **It hangs before any HALCYON logo appears.** The bootloader never reached the
  kernel. Try the other firmware mode.
- **A red "HALCYON HAS STOPPED" screen.** The kernel faulted and is telling you
  where. Photograph it; the `rip` and faulting address are what matter. Nothing
  was written anywhere.
- **The machine reboots immediately.** A triple fault before the exception
  handler was installed. The serial console is the only way to see more: boot
  with a null-modem cable or, in a VM, `-serial stdio`.
- **You want the boot log.** The third GRUB entry mirrors everything to COM1 at
  38400 8N1.

## Verifying the download

Each release ships a `halcyon.iso.sha256` next to it:

```sh
sha256sum -c halcyon.iso.sha256
```

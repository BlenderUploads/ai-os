#!/usr/bin/env python3
"""Headless boot test for HALCYON.

Boots the ISO in QEMU with no display, captures the serial log, optionally
drives the GUI through the QEMU monitor (keystrokes, mouse) and saves
screenshots as PNG.

    tools/smoke.py --iso build/halcyon.iso --firmware bios
    tools/smoke.py --iso build/halcyon.iso --screenshots build/shots

Exit status is non-zero if a required marker never appears, a panic shows up,
or the boot times out -- so CI can depend on it.
"""

import argparse
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time

OVMF_CODE = "/usr/share/OVMF/OVMF_CODE_4M.fd"
OVMF_VARS = "/usr/share/OVMF/OVMF_VARS_4M.fd"

# Markers the kernel must print on a healthy boot.
REQUIRED = ["HALCYON v", "HALCYON-BOOT-OK"]
# Any of these in the log means the boot went wrong.
FORBIDDEN = ["HALCYON PANIC", "CPU EXCEPTION", "!!! UNHANDLED"]


class Monitor:
    """QEMU monitor over a TCP socket."""

    def __init__(self, port):
        self.port = port
        self.sock = None

    def connect(self, timeout=30.0):
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                self.sock = socket.create_connection(("127.0.0.1", self.port), 1.0)
                self.sock.settimeout(2.0)
                self._drain()
                return True
            except OSError:
                time.sleep(0.2)
        return False

    def _drain(self):
        try:
            while True:
                if not self.sock.recv(65536):
                    return
        except OSError:
            pass

    def cmd(self, command):
        if not self.sock:
            return ""
        self.sock.sendall((command + "\n").encode())
        time.sleep(0.12)
        try:
            return self.sock.recv(65536).decode(errors="replace")
        except OSError:
            return ""

    def screendump(self, path):
        self.cmd(f"screendump {path}")
        # screendump is asynchronous; wait for the file to stop growing.
        for _ in range(50):
            if os.path.exists(path) and os.path.getsize(path) > 0:
                size = os.path.getsize(path)
                time.sleep(0.1)
                if os.path.getsize(path) == size:
                    return True
            time.sleep(0.1)
        return False

    def close(self):
        if self.sock:
            try:
                self.sock.close()
            except OSError:
                pass


# QEMU 'sendkey' names for characters that are not bare letters/digits.
KEYMAP = {
    " ": "spc",
    "-": "minus",
    "=": "equal",
    "[": "bracket_left",
    "]": "bracket_right",
    ";": "semicolon",
    "'": "apostrophe",
    "`": "grave_accent",
    "\\": "backslash",
    ",": "comma",
    ".": "dot",
    "/": "slash",
    "\n": "ret",
    "\t": "tab",
}
SHIFTED = {
    "!": "1", "@": "2", "#": "3", "$": "4", "%": "5", "^": "6",
    "&": "7", "*": "8", "(": "9", ")": "0", "_": "minus", "+": "equal",
    "{": "bracket_left", "}": "bracket_right", ":": "semicolon",
    '"': "apostrophe", "~": "grave_accent", "|": "backslash",
    "<": "comma", ">": "dot", "?": "slash",
}


def key_sequence(text):
    """Turn a string into QEMU sendkey arguments."""
    for char in text:
        if char in SHIFTED:
            yield f"shift-{SHIFTED[char]}"
        elif char in KEYMAP:
            yield KEYMAP[char]
        elif char.isupper():
            yield f"shift-{char.lower()}"
        elif char.isalnum():
            yield char
        # anything else is silently skipped


def type_text(monitor, text, delay=0.045):
    for key in key_sequence(text):
        monitor.cmd(f"sendkey {key}")
        time.sleep(delay)


def ppm_to_png(ppm_path, png_path):
    """Convert QEMU's PPM screendump to PNG with no image library."""
    import struct
    import zlib

    with open(ppm_path, "rb") as handle:
        data = handle.read()

    # Parse the P6 header, skipping comments.
    fields, pos = [], 0
    while len(fields) < 4:
        while pos < len(data) and data[pos : pos + 1].isspace():
            pos += 1
        if data[pos : pos + 1] == b"#":
            while pos < len(data) and data[pos] != 0x0A:
                pos += 1
            continue
        start = pos
        while pos < len(data) and not data[pos : pos + 1].isspace():
            pos += 1
        fields.append(data[start:pos])
    pos += 1

    if fields[0] != b"P6":
        raise ValueError(f"unexpected screendump format {fields[0]!r}")
    width, height = int(fields[1]), int(fields[2])
    pixels = data[pos : pos + width * height * 3]

    raw = bytearray()
    for row in range(height):
        raw.append(0)  # PNG filter type: none
        raw += pixels[row * width * 3 : (row + 1) * width * 3]

    def chunk(tag, payload):
        body = tag + payload
        return struct.pack(">I", len(payload)) + body + struct.pack(">I", zlib.crc32(body))

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 6))
    png += chunk(b"IEND", b"")

    with open(png_path, "wb") as handle:
        handle.write(png)
    return width, height


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--iso", required=True)
    parser.add_argument("--firmware", choices=["bios", "uefi"], default="bios")
    parser.add_argument("--timeout", type=float, default=90.0)
    parser.add_argument("--boot-wait", type=float, default=14.0,
                        help="seconds to let GRUB and the kernel settle")
    parser.add_argument("--screenshots", metavar="DIR",
                        help="capture screenshots into DIR")
    parser.add_argument("--script", metavar="FILE",
                        help="file of monitor/typing directives to run")
    parser.add_argument("--memory", default="512M")
    parser.add_argument("--expect", action="append", default=[],
                        help="extra string that must appear in the serial log")
    args = parser.parse_args()

    workdir = tempfile.mkdtemp(prefix="halcyon-smoke-")
    serial_log = os.path.join(workdir, "serial.log")
    monitor_port = 45000 + (os.getpid() % 10000)

    cmd = [
        "qemu-system-x86_64",
        "-m", args.memory,
        "-cdrom", args.iso,
        "-display", "none",
        "-serial", f"file:{serial_log}",
        "-monitor", f"tcp:127.0.0.1:{monitor_port},server,nowait",
        "-no-reboot",
        "-rtc", "base=utc",
    ]
    if args.firmware == "uefi":
        vars_copy = os.path.join(workdir, "OVMF_VARS.fd")
        shutil.copy(OVMF_VARS, vars_copy)
        cmd += [
            "-drive", f"if=pflash,format=raw,unit=0,file={OVMF_CODE},readonly=on",
            "-drive", f"if=pflash,format=raw,unit=1,file={vars_copy}",
        ]

    print(f"[smoke] firmware={args.firmware} iso={args.iso}")
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)

    monitor = Monitor(monitor_port)
    monitor.connect()

    shots = []
    try:
        # Let GRUB time out and the kernel come up.
        deadline = time.time() + args.timeout
        settled = time.time() + args.boot_wait
        while time.time() < settled and time.time() < deadline:
            if proc.poll() is not None:
                break
            time.sleep(0.5)

        if args.screenshots:
            os.makedirs(args.screenshots, exist_ok=True)
            ppm = os.path.join(workdir, "boot.ppm")
            if monitor.screendump(ppm):
                png = os.path.join(args.screenshots, f"01-boot-{args.firmware}.png")
                size = ppm_to_png(ppm, png)
                shots.append((png, size))

        if args.script:
            with open(args.script) as handle:
                steps = [line.rstrip("\n") for line in handle
                         if line.strip() and not line.lstrip().startswith("#")]
            shot_index = len(shots) + 1
            for step in steps:
                verb, _, rest = step.partition(" ")
                if verb == "type":
                    type_text(monitor, rest + "\n")
                elif verb == "keys":
                    for key in rest.split():
                        monitor.cmd(f"sendkey {key}")
                        time.sleep(0.06)
                elif verb == "wait":
                    time.sleep(float(rest))
                elif verb == "mon":
                    monitor.cmd(rest)
                elif verb == "shot" and args.screenshots:
                    ppm = os.path.join(workdir, f"s{shot_index}.ppm")
                    if monitor.screendump(ppm):
                        name = rest.strip() or f"step{shot_index}"
                        png = os.path.join(
                            args.screenshots, f"{shot_index:02d}-{name}-{args.firmware}.png")
                        size = ppm_to_png(ppm, png)
                        shots.append((png, size))
                    shot_index += 1
    finally:
        monitor.close()
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()

    log = ""
    if os.path.exists(serial_log):
        with open(serial_log, errors="replace") as handle:
            log = handle.read()

    print("---- serial log ----")
    print(log.strip() or "(empty)")
    print("--------------------")

    for path, (width, height) in shots:
        print(f"[smoke] screenshot {path} ({width}x{height})")

    failures = []
    for marker in REQUIRED + args.expect:
        if marker not in log:
            failures.append(f"missing marker: {marker!r}")
    for marker in FORBIDDEN:
        if marker in log:
            failures.append(f"forbidden marker present: {marker!r}")

    if failures:
        print("[smoke] FAIL")
        for failure in failures:
            print(f"        {failure}")
        return 1

    print("[smoke] PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())

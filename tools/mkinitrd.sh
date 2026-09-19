#!/usr/bin/env bash
# Pack the initrd directory into a plain USTAR archive.
# HALCYON's tarfs reads this read-only at boot; the RAMFS overlays it.
set -euo pipefail

SRC="${1:?usage: mkinitrd.sh <srcdir> <out.tar>}"
OUT="${2:?usage: mkinitrd.sh <srcdir> <out.tar>}"

mkdir -p "$(dirname "$OUT")"

# --format=ustar keeps headers in the 512-byte layout the kernel parser expects.
# Deterministic metadata so the ISO hash only changes when content does.
tar --format=ustar \
    --sort=name \
    --owner=0 --group=0 --numeric-owner \
    --mtime='@0' \
    -cf "$OUT" -C "$SRC" .

echo "initrd: $OUT ($(du -h "$OUT" | cut -f1), $(tar -tf "$OUT" | wc -l) entries)"

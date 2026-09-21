#!/usr/bin/env bash
# Put a playable IWAD at build/doom.wad.
#
# Prefers whatever is already on the machine, then Freedoom, which is
# BSD-licensed and freely redistributable. Your own doom1.wad or doom.wad
# works just as well -- drop it in as build/doom.wad and this does nothing.
set -euo pipefail

OUT="${1:-build/doom.wad}"
mkdir -p "$(dirname "$OUT")"

if [ -s "$OUT" ]; then
    echo "iwad: $OUT already present ($(du -h "$OUT" | cut -f1))"
    exit 0
fi

# 1. An IWAD already installed locally.
for candidate in \
    /usr/share/games/doom/freedoom1.wad \
    /usr/share/games/doom/freedoom2.wad \
    /usr/share/doom/freedoom1.wad \
    /usr/share/games/doom/doom1.wad \
    "${DOOM_WAD:-}"
do
    if [ -n "$candidate" ] && [ -s "$candidate" ]; then
        cp "$candidate" "$OUT"
        echo "iwad: copied $candidate"
        exit 0
    fi
done

# 2. Fetch the Freedoom package and unpack just the WAD.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
if command -v apt-get >/dev/null 2>&1; then
    echo "iwad: fetching Freedoom..."
    ( cd "$work" && apt-get download freedoom >/dev/null 2>&1 ) || true
    deb="$(ls "$work"/freedoom_*.deb 2>/dev/null | head -1 || true)"
    if [ -n "$deb" ]; then
        dpkg-deb -x "$deb" "$work/extract"
        found="$(find "$work/extract" -name 'freedoom1.wad' | head -1)"
        if [ -n "$found" ]; then
            cp "$found" "$OUT"
            echo "iwad: installed Freedoom Phase 1 ($(du -h "$OUT" | cut -f1))"
            exit 0
        fi
    fi
fi

cat >&2 <<'MESSAGE'
Could not obtain an IWAD automatically.

Put one at build/doom.wad yourself -- Freedoom from https://freedoom.github.io
(BSD-licensed), or your own doom1.wad / doom.wad / doom2.wad -- and run the
build again.
MESSAGE
exit 1

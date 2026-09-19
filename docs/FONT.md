# The font

HALCYON draws its own. `tools/mkfont.py` holds the glyph art and emits
`kernel/src/gfx/font_data.rs`, which is committed — building the kernel never
runs the generator.

## Why not borrow one

The obvious candidates all come with obligations. GNU Unifont's Debian
packaging is plain GPL-2+ with no font embedding exception. Terminus is OFL-1.1.
Both are perfectly good fonts and neither is a burden in most projects, but
baking their glyph data into a kernel binary means reasoning about what that
does to the binary's licence — for a system whose whole premise is that it owes
nothing to anything.

Drawing ninety-odd glyphs is an afternoon's work. It also means the letterforms
were chosen to sit next to the rest of the interface rather than inherited from
somewhere else.

## Metrics

Each glyph is drawn in a **5 wide by 9 tall** grid: seven rows above the
baseline — the classic 5×7 terminal proportion — plus two rows of descender for
`g j p q y ,` and `;`.

That grid is emitted into an **8×16 cell**, with one column of left bearing and
the drawing starting at row 3. Text therefore lands on tidy 16-pixel rows with
comfortable leading, and the cell width divides evenly into every sensible
screen width.

```
        col 0 1 2 3 4 5 6 7
row  0   . . . . . . . .     <- 3 rows of headroom
     1   . . . . . . . .
     2   . . . . . . . .
     3   . # # # . . . .     <- drawing starts here
     4   . # . . # . . .
     5   . # . . # . . .
     6   . # # # # . . .        'A'
     7   . # . . # . . .
     8   . # . . # . . .
     9   . # . . # . . .
    10   . . . . . . . .     <- baseline
    11   . . . . . . . .     <- descender rows
    12   . . . . . . . .
    13   . . . . . . . .
    14   . . . . . . . .
    15   . . . . . . . .
```

One byte per scanline, bit 7 leftmost.

## Changing it

Glyphs live in the `GLYPHS` dictionary in `tools/mkfont.py`, written as nine
space-separated rows of five characters, `#` for ink:

```python
"A": ".###. #...# #...# ##### #...# #...# #...# ..... .....",
```

Regenerate with:

```sh
python3 tools/mkfont.py > kernel/src/gfx/font_data.rs
(cd kernel && cargo fmt)
```

The second step matters only because the committed file has been through
`cargo fmt` with the rest of the tree; skipping it leaves a formatting-only diff.

The generator validates the shape of every entry, so a row of the wrong length
fails loudly rather than producing a quietly mangled glyph. Anything left
undrawn renders as a hollow box, which makes a missing glyph obvious instead of
invisible.

## Beyond ASCII

The control-code range holds a few drawing marks the interface needs — a logo
diamond, a chip, blocks, a bullet, and four arrows — named in
`gfx::font::glyph`.

`gfx::font::code_of` folds the typographic characters that turn up in ordinary
Rust string literals onto their ASCII equivalents: em and en dashes to `-`,
curly quotes to `'` and `"`, ellipsis to `.`, non-breaking space to a space. Without
that, an em dash in a `format!` string renders as a missing-glyph box — which is
exactly how the mapping came to be written.

## Bold

There is no separate bold face. `Weight::Bold` smears each scanline one pixel to
the right (`bits | (bits >> 1)`), which is how bitmap terminals have always
faked it and is more than convincing at this size.

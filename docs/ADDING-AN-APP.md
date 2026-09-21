# Adding an application

Applications are structs implementing `ui::window::App`. The compositor gives
each one a `Surface` to draw into and routes input to it; nothing else is
required.

## 1. Write the app

`kernel/src/apps/clock.rs`:

```rust
use crate::drivers::rtc;
use crate::gfx::draw::Surface;
use crate::gfx::font::Weight;
use crate::gfx::palette;
use crate::ui::window::{App, AppResponse};

pub struct Clock {
    dirty: bool,
}

impl Clock {
    pub fn new() -> Self {
        Self { dirty: true }
    }
}

impl App for Clock {
    fn draw(&mut self, surface: &mut Surface, _focused: bool) {
        surface.clear(palette::PANEL);
        let now = rtc::now();
        let text = alloc::format!("{:02}:{:02}:{:02}", now.hour, now.minute, now.second);
        surface.text_ex(
            &text,
            20,
            20,
            palette::AMBER,
            Weight::Bold,
            4, // scale
        );
        self.dirty = false;
    }

    // Called every composited frame. Only ask for a repaint when something
    // actually changed, or the compositor redraws you 30 times a second for
    // nothing.
    fn tick(&mut self, _now_ms: u64, _response: &mut AppResponse) {
        self.dirty = true;
    }

    fn dirty(&self) -> bool {
        self.dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    fn min_size(&self) -> (i32, i32) {
        (280, 120)
    }
}
```

## 2. Register it

In `kernel/src/apps/mod.rs`, add the module and an entry to `registry()`:

```rust
pub mod clock;

// ...inside registry():
AppEntry {
    name: "clock",            // what `run clock` and the launcher use
    title: "Clock",           // the title bar
    icon: glyph::CHIP,        // see gfx::font::glyph
    width: 320,
    height: 160,
    build: || Box::new(clock::Clock::new()) as Box<dyn App>,
},
```

Position in the vector decides the function key: the first entry is F1.

## 3. Build and look at it

```sh
make iso
make run                       # interactive
python3 tools/smoke.py --iso build/halcyon.iso --firmware bios \
    --boot-wait 20 --screenshots build/shots
```

The smoke harness can drive it, too — see `tools/scripts/*.txt` for the verbs
(`type`, `keys`, `click`, `drag`, `moveto`, `shot`).

## What you get

**Drawing.** `Surface` has `clear`, `fill_rect`, `stroke_rect`, `panel`,
`gradient_v`, `hline`, `vline`, `line`, `disc`, `circle`, `blit`, `blit_keyed`,
`shade`, `scroll_up`, and text via `text`, `text_bold`, `text_ex` (scale and
weight), `text_centered` and `text_glow`. `set_clip` returns the previous clip
so you can restore it.

Coordinates are relative to your surface, which is the window's content area —
you never see screen coordinates.

**Input.** `on_key(&KeyEvent, &mut AppResponse)` when focused;
`on_mouse(&WindowMouse, &mut AppResponse)` with content-relative coordinates and
`pressed` / `released` / `moved` flags. Key releases arrive too, which is why
every app here opens with `if !event.pressed { return; }`.

**Talking back.** Set fields on `AppResponse`:

- `close: true` — close my window
- `retitle: Some(...)` — change the title bar
- `launch: Some("name")` — open another app
- `fullscreen: Some(true)` — fill the screen, no chrome
- `grab_pointer: Some(true)` — capture the pointer

Capturing the pointer freezes the cursor, stops it being drawn, and starts
delivering raw `dx`/`dy` on `WindowMouse` instead of moving positions — what a
game wants and nothing else does. The compositor can take it back without being
asked (ctrl+G, or the focus moving), so implement `on_pointer_grab(bool)` and
believe it rather than your own request.

**The rest of the kernel** is a normal module away: `crate::fs::FS` for files,
`crate::task` for threads, `crate::mm` for memory statistics,
`crate::drivers::*` for hardware.

## Conventions worth following

- Keep `dirty()` honest. The compositor skips repainting a clean window, and
  that is most of why the desktop is cheap.
- Do slow work in `tick` behind a timer, not in `draw`. The monitor samples its
  graph four times a second rather than 30.
- Use the palette in `gfx::palette` rather than raw colours, so the system looks
  like one system.
- Apps are not `Send` and run on the desktop thread. Do not block in them; if
  you need to wait, `task::sleep_ms` from a thread you spawned instead.

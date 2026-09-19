//! ORACLE's conversational side.
//!
//! This is a keyword-matched response table, not a language model. There is no
//! model in HALCYON and no network stack to reach one -- ORACLE says so itself
//! when asked. It exists because a machine like this should answer when you
//! talk to it, and because the answers are a decent way to document the system.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::arch::{cpu, pit};
use crate::mm;
use crate::task;

/// Keywords, and the reply when one of them appears in the question.
const RESPONSES: &[(&[&str], &str)] = &[
    (
        &["who are you", "what are you", "your name"],
        "I am ORACLE: the interpreter built into HALCYON's shell, wearing a\n\
         conversational hat. Parenthesised input I evaluate as Lisp. Plain\n\
         English I match against a table of answers I was written with. There is\n\
         no language model on this machine and no network to reach one -- what\n\
         you get from me is a lookup, honestly labelled.",
    ),
    (
        &["are you ai", "are you an ai", "are you intelligent", "are you conscious", "chatgpt", "gpt", "llm", "language model"],
        "No. I want to be clear about that, since the repository this came from\n\
         is called ai-os. I am a few hundred lines of pattern matching over a\n\
         fixed table. The interesting part of this machine is underneath me:\n\
         the paging code, the scheduler, the compositor. Try `mem` or `ps`.",
    ),
    (
        &["what is halcyon", "about halcyon", "what is this", "what am i running"],
        "HALCYON is an operating system written from nothing. No Linux kernel,\n\
         no libc, no third-party libraries. The bootstrap that put this CPU into\n\
         long mode, the page tables under every address you can name, the\n\
         scheduler switching between threads, the font these letters are drawn\n\
         with -- all of it was written for this machine.",
    ),
    (
        &["how do you work", "how does this work", "architecture"],
        "GRUB loads the kernel through Multiboot2 and drops us in 32-bit mode.\n\
         A bootstrap builds page tables, enters long mode and jumps to the\n\
         higher half. From there: a bitmap frame allocator, 4-level paging, a\n\
         free-list heap, a round-robin scheduler driven by the PIT, PS/2 input,\n\
         and a compositor that draws into RAM and blits once per frame.",
    ),
    (
        &["help", "what can you do", "commands"],
        "Type `help` for the command list. Anything in (parentheses) is\n\
         evaluated as Lisp -- try (+ 1 2), (mem), (threads), (pci) or\n\
         (map (lambda (x) (* x x)) (range 10)).",
    ),
    (
        &["memory", "ram", "how much memory"],
        "__MEMORY__",
    ),
    (
        &["uptime", "how long", "running for"],
        "__UPTIME__",
    ),
    (
        &["cpu", "processor", "what cpu"],
        "__CPU__",
    ),
    (
        &["thread", "process", "scheduler", "multitask"],
        "__THREADS__",
    ),
    (
        &["disk", "hard drive", "storage", "save", "write to disk", "safe"],
        "Nothing here touches a disk. HALCYON has no block-device write path at\n\
         all -- the code to do it does not exist. Files live in RAM, seeded from\n\
         the initrd, and vanish at power-off. That is why booting this on real\n\
         hardware is safe: it cannot alter what is already on the machine.",
    ),
    (
        &["network", "internet", "wifi", "online"],
        "There is no network stack. No driver, no TCP, no sockets. This machine\n\
         cannot talk to anything but you.",
    ),
    (
        &["why", "what is the point", "purpose"],
        "Someone wanted to see whether an operating system could be built from\n\
         the first instruction upward, and then actually boot it on real\n\
         hardware. You are looking at the answer.",
    ),
    (
        &["hello", "hi ", "hey", "greetings"],
        "Hello. The machine is yours -- `help` lists what it will do.",
    ),
    (
        &["thank", "thanks", "cheers"],
        "A pleasure. I am a lookup table, but a well-mannered one.",
    ),
    (
        &["lisp", "language", "syntax", "oracle"],
        "The shell reads Lisp. Lists are vectors rather than cons pairs, which\n\
         costs improper lists and buys a simpler interpreter. Special forms:\n\
         quote, if, define, set!, lambda, let, begin, while, cond, and, or.\n\
         Tail positions loop instead of recursing, so tail recursion is free.",
    ),
    (
        &["font", "letters", "typeface", "text"],
        "The font was drawn for this system, glyph by glyph, in tools/mkfont.py.\n\
         Five pixels wide and seven tall above the baseline, with two rows of\n\
         descender, inside an 8x16 cell. Borrowing one would have meant\n\
         borrowing its licence too.",
    ),
    (
        &["colour", "color", "scanline", "crt", "look", "theme"],
        "Amber and cyan phosphor on a deep blue-black, with every other scanline\n\
         darkened and the corners pulled down. The vignette is baked into the\n\
         cached backdrop; the scanlines are a shift and a subtract per pixel,\n\
         which is cheap enough to run on every frame.",
    ),
    (
        &["fault", "crash", "panic", "error", "bug"],
        "If something faults you will know: HALCYON paints the exception, the\n\
         faulting address and the register file in red, then halts. Nothing is\n\
         written anywhere. You can provoke it from the GRUB menu's self-test\n\
         entry if you want to see it.",
    ),
    (
        &["who made", "who wrote", "author", "built this"],
        "Written by Claude, an AI assistant made by Anthropic, at the request of\n\
         someone who was bored and had a spare laptop. Every line is in the\n\
         repository, including the mistakes that were found and fixed on the way.",
    ),
];

const FALLBACKS: &[&str] = &[
    "I have no answer written for that. I am a table, not a mind -- try `help`,\n\
     or ask me about memory, threads, the CPU, or what HALCYON is.",
    "That one is not in my table. Ask about the kernel, the scheduler, the\n\
     filesystem or the display, and I will have something to say.",
    "No match. If you meant it as code, wrap it in parentheses and I will\n\
     evaluate it instead: (+ 1 2) works, and so does (mem).",
];

/// Answer a question. `nonce` varies the fallback so repeated misses do not
/// read like a stuck record.
pub fn respond(question: &str, nonce: u64) -> Vec<String> {
    let lowered = question.to_lowercase();

    for (keywords, reply) in RESPONSES {
        if keywords.iter().any(|keyword| lowered.contains(keyword)) {
            return expand(reply);
        }
    }

    let fallback = FALLBACKS[(nonce as usize) % FALLBACKS.len()];
    expand(fallback)
}

/// Substitute the live-data placeholders, then split into display lines.
fn expand(reply: &str) -> Vec<String> {
    let text: String = match reply {
        "__MEMORY__" => {
            let frames = mm::FRAMES.lock();
            let (total, used) = (frames.total_bytes(), frames.used_bytes());
            drop(frames);
            let heap = mm::heap::ALLOCATOR.stats();
            let (total_value, total_unit) = mm::format_bytes(total);
            let (used_value, used_unit) = mm::format_bytes(used);
            let (heap_value, heap_unit) = mm::format_bytes(heap.mapped);
            alloc::format!(
                "This machine has {} {} of RAM, of which {} {} is in use. The kernel\n\
                 heap has {} {} mapped and {} live allocations. Frames are tracked\n\
                 with one bit each in a bitmap; the heap is an address-sorted free\n\
                 list that coalesces on release.",
                total_value,
                total_unit,
                used_value,
                used_unit,
                heap_value,
                heap_unit,
                heap.live_allocations
            )
        }
        "__UPTIME__" => {
            let ms = pit::uptime_ms();
            alloc::format!(
                "Up {}.{:03} seconds, counted by the PIT at 1000 Hz. That same\n\
                 interrupt is what pre-empts every thread on the machine -- {} context\n\
                 switches so far.",
                ms / 1000,
                ms % 1000,
                task::total_switches()
            )
        }
        "__CPU__" => {
            let info = cpu::identify();
            alloc::format!(
                "{}\nVendor {}, family {:#x}, model {:#x}. NX: {}. PAT: {}.\n\
                 HALCYON uses PAT entry 4 to mark the framebuffer write-combining,\n\
                 which is the difference between a usable desktop and a slideshow.",
                if info.brand_str().is_empty() {
                    info.vendor_str()
                } else {
                    info.brand_str()
                },
                info.vendor_str(),
                info.family,
                info.model,
                info.has_nx,
                info.has_pat
            )
        }
        "__THREADS__" => {
            let threads = task::snapshot();
            let mut text = alloc::format!(
                "{} thread(s), pre-empted round-robin by the timer:\n",
                threads.len()
            );
            for thread in threads.iter().take(8) {
                text.push_str(&alloc::format!(
                    "  {} {} ({} ticks)\n",
                    thread.id,
                    thread.name,
                    thread.ticks_used
                ));
            }
            text.push_str("A context switch here is just the interrupt handler returning a\ndifferent register frame than the one it was given.");
            text
        }
        other => other.to_string(),
    };

    text.lines().map(|line| line.to_string()).collect()
}

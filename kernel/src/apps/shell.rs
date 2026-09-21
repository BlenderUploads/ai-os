//! hsh — the HALCYON shell.
//!
//! A line beginning with `(` is handed to ORACLE as Lisp. Otherwise the first
//! word is looked up as a builtin command. `ask` routes the rest of the line to
//! ORACLE's persona.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::arch::{cpu, pit};
use crate::drivers::{pci, rtc, speaker};
use crate::fs;
use crate::gfx::palette::{self, Color};
use crate::mm;
use crate::oracle::Interpreter;
use crate::task;

#[derive(Clone)]
pub struct Line {
    pub text: String,
    pub color: Color,
}

impl Line {
    pub fn new(text: impl Into<String>, color: Color) -> Self {
        Self {
            text: text.into(),
            color,
        }
    }
}

/// What the shell wants the surrounding application to do.
#[derive(Default)]
pub struct ShellEffects {
    pub clear: bool,
    pub launch: Option<String>,
    pub close: bool,
}

pub struct Shell {
    pub interpreter: Interpreter,
    pub cwd: String,
    nonce: u64,
}

const COMMANDS: &[(&str, &str)] = &[
    ("help", "this list"),
    ("ask <question>", "put a question to ORACLE"),
    ("ls [path]", "list files"),
    ("cat <file>", "print a file"),
    ("write <file> <text>", "create or overwrite a file"),
    ("rm <file>", "delete a file"),
    ("echo <text>", "print a line"),
    ("mem", "memory statistics"),
    ("ps", "running threads"),
    ("uptime", "time since boot"),
    ("date", "wall clock from the RTC"),
    ("cpu", "processor identification"),
    ("pci", "enumerate the PCI bus"),
    ("beep [freq] [ms]", "the PC speaker"),
    ("audio [freq] [ms]", "the sound card, and a tone through it"),
    ("clear", "clear the scrollback"),
    ("run <app>", "open an application window"),
    ("reboot", "restart the machine"),
    ("poweroff", "shut the machine down"),
    ("env", "names bound in ORACLE"),
    ("lisp", "the ORACLE language reference"),
    ("exit", "close this terminal"),
];

impl Shell {
    pub fn new() -> Self {
        Self {
            interpreter: Interpreter::new(),
            cwd: String::from("/"),
            nonce: 0,
        }
    }

    pub fn banner(&self) -> Vec<Line> {
        let mut lines = Vec::new();
        lines.push(Line::new("HALCYON shell (hsh)", palette::AMBER));
        lines.push(Line::new(
            "ORACLE is listening. `help` for commands; anything in (parentheses) is evaluated.",
            palette::TEXT_DIM,
        ));
        let filesystem = fs::FS.lock();
        if let Some(file) = filesystem.read("/motd.txt") {
            if let Ok(text) = core::str::from_utf8(file.bytes()) {
                lines.push(Line::new("", palette::TEXT));
                for line in text.lines() {
                    lines.push(Line::new(line.to_string(), palette::CYAN_DIM));
                }
            }
        }
        lines
    }

    /// Run one line of input and return everything it printed.
    pub fn execute(&mut self, input: &str, effects: &mut ShellEffects) -> Vec<Line> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        self.nonce = self.nonce.wrapping_add(1);

        // Anything parenthesised is code.
        if trimmed.starts_with('(') || trimmed.starts_with('\'') {
            return self.evaluate(trimmed);
        }

        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let command = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("").trim();

        match command {
            "help" => self.help(),
            "ask" => self.ask(rest),
            "ls" => self.list(rest),
            "cat" => self.cat(rest),
            "write" => self.write(rest),
            "rm" => self.remove(rest),
            "echo" => alloc::vec![Line::new(rest.to_string(), palette::TEXT)],
            "mem" => self.memory(),
            "ps" => self.processes(),
            "uptime" => self.uptime(),
            "date" => self.date(),
            "cpu" => self.cpu(),
            "pci" => self.pci(),
            "beep" => self.beep(rest),
            "audio" => self.audio(rest),
            "lisp" => self.lisp_reference(),
            "reboot" => {
                speaker::beep(660, 120);
                crate::arch::acpi::reboot();
            }
            "poweroff" | "shutdown" | "halt" => {
                speaker::beep(440, 160);
                crate::power_off();
                alloc::vec![
                    Line::new(
                        "this machine did not respond to any shutdown route we know",
                        palette::WARN
                    ),
                    Line::new(
                        "it is safe to switch it off at the mains".to_string(),
                        palette::TEXT_DIM
                    ),
                ]
            }
            "env" => self.environment(),
            "clear" => {
                effects.clear = true;
                Vec::new()
            }
            "exit" | "quit" => {
                effects.close = true;
                Vec::new()
            }
            "run" => {
                if rest.is_empty() {
                    alloc::vec![Line::new(
                        "run: name an application (try: monitor, about, editor, files, paint, snake)",
                        palette::WARN
                    )]
                } else {
                    effects.launch = Some(rest.to_string());
                    alloc::vec![Line::new(format!("opening {}", rest), palette::TEXT_DIM)]
                }
            }
            _ => {
                // Bare symbols that happen to be ORACLE names are convenient to
                // evaluate directly -- `mem` and `uptime` work either way.
                if self.interpreter.global.get(command).is_some() {
                    return self.evaluate(&format!("({})", trimmed));
                }
                speaker::error_tone();
                alloc::vec![
                    Line::new(format!("{}: not a command", command), palette::ERROR),
                    Line::new(
                        "try `help`, or `ask` followed by a question".to_string(),
                        palette::TEXT_DIM
                    ),
                ]
            }
        }
    }

    fn evaluate(&mut self, source: &str) -> Vec<Line> {
        let mut lines = Vec::new();
        let result = self.interpreter.run(source);
        for printed in self.interpreter.take_output() {
            lines.push(Line::new(printed, palette::TEXT));
        }
        match result {
            Ok(crate::oracle::Value::Nil) => {}
            Ok(value) => lines.push(Line::new(
                format!("=> {}", value.write_form()),
                palette::CYAN,
            )),
            Err(error) => {
                speaker::error_tone();
                lines.push(Line::new(format!("! {}", error), palette::ERROR));
            }
        }
        lines
    }

    fn help(&self) -> Vec<Line> {
        let mut lines = alloc::vec![Line::new("commands", palette::AMBER)];
        for (name, description) in COMMANDS {
            lines.push(Line::new(
                format!("  {:<22}{}", name, description),
                palette::TEXT,
            ));
        }
        lines.push(Line::new("", palette::TEXT));
        lines.push(Line::new(
            "anything in (parentheses) is ORACLE Lisp; `lisp` lists what it knows",
            palette::TEXT_DIM,
        ));
        lines
    }

    fn lisp_reference(&self) -> Vec<Line> {
        let mut lines = alloc::vec![
            Line::new("ORACLE — a small Lisp", palette::AMBER),
            Line::new("", palette::TEXT),
            Line::new("special forms", palette::CYAN),
            Line::new(
                "  quote if define set! lambda let begin while cond and or",
                palette::TEXT
            ),
            Line::new("", palette::TEXT),
            Line::new("builtins", palette::CYAN),
        ];
        for (names, description) in crate::oracle::builtins::CATALOGUE {
            lines.push(Line::new(
                format!("  {:<22}{}", names, description),
                palette::TEXT,
            ));
        }
        lines.push(Line::new("", palette::TEXT));
        lines.push(Line::new("examples", palette::CYAN));
        for example in [
            "(+ 1 2 3)",
            "(define (square x) (* x x))",
            "(map square (range 1 8))",
            "(fold + 0 (range 101))",
            "(filter (lambda (n) (= 0 (mod n 3))) (range 20))",
            "(print (str \"this machine has been up \" (uptime) \" ms\"))",
        ] {
            lines.push(Line::new(format!("  {}", example), palette::TEXT_DIM));
        }
        lines
    }

    fn ask(&mut self, question: &str) -> Vec<Line> {
        if question.is_empty() {
            return alloc::vec![Line::new(
                "ask what? try: ask what is halcyon",
                palette::WARN
            )];
        }
        let mut lines = alloc::vec![Line::new("ORACLE:", palette::VIOLET)];
        for line in crate::oracle::persona::respond(question, self.nonce) {
            lines.push(Line::new(format!("  {}", line), palette::TEXT_BRIGHT));
        }
        lines
    }

    fn list(&self, path: &str) -> Vec<Line> {
        let filesystem = fs::FS.lock();
        let entries = if path.is_empty() {
            filesystem.list()
        } else {
            filesystem.list_dir(path)
        };
        if entries.is_empty() {
            return alloc::vec![Line::new("(no files)", palette::TEXT_DIM)];
        }
        let mut lines = Vec::new();
        for (name, size, from_initrd) in entries {
            lines.push(Line::new(
                format!(
                    "  {:>7}  {}{}",
                    size,
                    name,
                    if from_initrd { "  (initrd)" } else { "" }
                ),
                if from_initrd {
                    palette::TEXT_DIM
                } else {
                    palette::TEXT
                },
            ));
        }
        lines.push(Line::new(
            format!(
                "  {} file(s), {} bytes",
                lines.len(),
                filesystem.total_bytes()
            ),
            palette::TEXT_FAINT,
        ));
        lines
    }

    fn cat(&self, path: &str) -> Vec<Line> {
        if path.is_empty() {
            return alloc::vec![Line::new("cat: name a file", palette::WARN)];
        }
        let filesystem = fs::FS.lock();
        match filesystem.read(path) {
            Some(file) => match core::str::from_utf8(file.bytes()) {
                Ok(text) => text
                    .lines()
                    .map(|line| Line::new(line.to_string(), palette::TEXT))
                    .collect(),
                Err(_) => alloc::vec![Line::new(
                    format!("{}: {} bytes of binary", path, file.len()),
                    palette::TEXT_DIM
                )],
            },
            None => alloc::vec![Line::new(format!("{}: no such file", path), palette::ERROR)],
        }
    }

    fn write(&self, rest: &str) -> Vec<Line> {
        let mut parts = rest.splitn(2, char::is_whitespace);
        let path = parts.next().unwrap_or("");
        let contents = parts.next().unwrap_or("");
        if path.is_empty() {
            return alloc::vec![Line::new("write: write <file> <text>", palette::WARN)];
        }
        let mut filesystem = fs::FS.lock();
        let mut data = contents.as_bytes().to_vec();
        data.push(b'\n');
        match filesystem.write(path, data) {
            Ok(()) => alloc::vec![Line::new(
                format!("wrote {} ({} bytes)", path, contents.len() + 1),
                palette::OK
            )],
            Err(error) => alloc::vec![Line::new(format!("write: {}", error), palette::ERROR)],
        }
    }

    fn remove(&self, path: &str) -> Vec<Line> {
        if path.is_empty() {
            return alloc::vec![Line::new("rm: name a file", palette::WARN)];
        }
        let mut filesystem = fs::FS.lock();
        match filesystem.remove(path) {
            Ok(()) => alloc::vec![Line::new(format!("removed {}", path), palette::OK)],
            Err(error) => alloc::vec![Line::new(format!("rm: {}", error), palette::ERROR)],
        }
    }

    fn memory(&self) -> Vec<Line> {
        let frames = mm::FRAMES.lock();
        let (total, used, free) = (
            frames.total_bytes(),
            frames.used_bytes(),
            frames.free_bytes(),
        );
        drop(frames);
        let heap = mm::heap::ALLOCATOR.stats();

        let show = |label: &str, bytes: u64| {
            let (value, unit) = mm::format_bytes(bytes);
            Line::new(
                format!("  {:<16}{:>6} {:<4}({} bytes)", label, value, unit, bytes),
                palette::TEXT,
            )
        };

        alloc::vec![
            Line::new("physical memory", palette::AMBER),
            show("total", total),
            show("in use", used),
            show("free", free),
            Line::new("kernel heap", palette::AMBER),
            show("mapped", heap.mapped),
            show("allocated", heap.allocated as u64),
            Line::new(
                format!(
                    "  {:<16}{} live, {} served since boot, {} free blocks",
                    "allocations", heap.live_allocations, heap.total_allocations, heap.free_blocks
                ),
                palette::TEXT
            ),
        ]
    }

    fn processes(&self) -> Vec<Line> {
        let mut lines = alloc::vec![Line::new(
            "  id   name             state      ticks   switches",
            palette::AMBER
        )];
        for thread in task::snapshot() {
            let state = match thread.state {
                task::State::Running => "running",
                task::State::Ready => "ready",
                task::State::Sleeping(_) => "sleeping",
                task::State::Finished => "finished",
            };
            lines.push(Line::new(
                format!(
                    "  {:<5}{:<17}{:<11}{:<8}{}",
                    thread.id, thread.name, state, thread.ticks_used, thread.switches
                ),
                if thread.current {
                    palette::AMBER
                } else {
                    palette::TEXT
                },
            ));
        }
        lines.push(Line::new(
            format!("  {} context switches total", task::total_switches()),
            palette::TEXT_FAINT,
        ));
        lines
    }

    fn uptime(&self) -> Vec<Line> {
        let ms = pit::uptime_ms();
        let seconds = ms / 1000;
        alloc::vec![Line::new(
            format!(
                "up {}h {:02}m {:02}.{:03}s  ({} timer ticks at {} Hz)",
                seconds / 3600,
                (seconds / 60) % 60,
                seconds % 60,
                ms % 1000,
                pit::ticks(),
                pit::TICK_HZ
            ),
            palette::TEXT
        )]
    }

    fn date(&self) -> Vec<Line> {
        let now = rtc::now();
        if !now.is_valid() {
            return alloc::vec![Line::new(
                "the CMOS clock did not answer sensibly",
                palette::WARN
            )];
        }
        alloc::vec![Line::new(
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}  (CMOS RTC, UTC as the firmware set it)",
                now.year, now.month, now.day, now.hour, now.minute, now.second
            ),
            palette::TEXT
        )]
    }

    fn cpu(&self) -> Vec<Line> {
        let info = cpu::identify();
        alloc::vec![
            Line::new(
                if info.brand_str().is_empty() {
                    info.vendor_str().to_string()
                } else {
                    info.brand_str().to_string()
                },
                palette::AMBER
            ),
            Line::new(
                format!(
                    "  vendor {}  family {:#x}  model {:#x}  stepping {}",
                    info.vendor_str(),
                    info.family,
                    info.model,
                    info.stepping
                ),
                palette::TEXT
            ),
            Line::new(
                format!(
                    "  nx:{}  pat:{}  sse2:{}  apic:{}  invariant-tsc:{}",
                    info.has_nx, info.has_pat, info.has_sse2, info.has_apic, info.has_invariant_tsc
                ),
                palette::TEXT_DIM
            ),
        ]
    }

    fn pci(&self) -> Vec<Line> {
        let devices = pci::enumerate();
        let mut lines = alloc::vec![Line::new(
            format!("{} PCI device(s)", devices.len()),
            palette::AMBER
        )];
        for device in devices {
            lines.push(Line::new(format!("  {}", device.describe()), palette::TEXT));
        }
        lines
    }

    fn beep(&self, rest: &str) -> Vec<Line> {
        let mut parts = rest.split_whitespace();
        let frequency: u32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(880);
        let duration: u64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(120);
        speaker::beep(frequency, duration.min(2000));
        alloc::vec![Line::new(
            format!(
                "{} Hz for {} ms (silent if this machine has no speaker)",
                frequency, duration
            ),
            palette::TEXT_DIM
        )]
    }

    fn audio(&self, rest: &str) -> Vec<Line> {
        let mut lines = alloc::vec![Line::new(
            match crate::drivers::ac97::describe() {
                Some(description) => description,
                None => "no AC'97 codec on this machine".to_string(),
            },
            palette::AMBER
        )];
        lines.push(Line::new(crate::audio::status(), palette::TEXT_DIM));

        let mut parts = rest.split_whitespace();
        let frequency: u32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(440);
        let duration: u32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(700);
        if crate::audio::tone(frequency, duration) {
            lines.push(Line::new(
                format!("playing {} Hz for {} ms", frequency, duration),
                palette::TEXT,
            ));
        }
        lines
    }

    fn environment(&self) -> Vec<Line> {
        let names = self.interpreter.global.names();
        let mut lines = alloc::vec![Line::new(
            format!("{} names bound", names.len()),
            palette::AMBER
        )];
        let mut row = String::new();
        for name in names {
            if row.len() + name.len() + 2 > 70 {
                lines.push(Line::new(core::mem::take(&mut row), palette::TEXT));
            }
            if !row.is_empty() {
                row.push_str("  ");
            }
            row.push_str(&name);
        }
        if !row.is_empty() {
            lines.push(Line::new(row, palette::TEXT));
        }
        lines
    }
}

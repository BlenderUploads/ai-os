//! ORACLE's builtin procedures, including the ones that reach into the kernel.

use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::arch::{cpu, pit};
use crate::drivers::{pci, rtc, speaker};
use crate::fs;
use crate::mm;
use crate::task;

use super::eval::Interpreter;
use super::value::{EvalResult, Value};

/// Every builtin, with a one-line description for `help`.
pub const CATALOGUE: &[(&str, &str)] = &[
    ("+ - * / mod", "integer arithmetic"),
    ("= /= < > <= >=", "comparison"),
    ("not", "logical negation"),
    ("list", "build a list"),
    ("car cdr", "first element / everything after it"),
    ("cons", "prepend an element to a list"),
    ("append", "join lists"),
    ("length", "number of elements"),
    ("nth", "element by index"),
    ("reverse", "reverse a list"),
    ("map", "apply a function to each element"),
    ("filter", "keep the elements a predicate accepts"),
    ("fold", "reduce a list with an accumulator"),
    ("range", "(range n) or (range from to)"),
    ("str", "concatenate anything into a string"),
    ("str-len", "length of a string"),
    ("substr", "(substr s start len)"),
    ("upper lower", "change case"),
    ("split", "split a string on a separator"),
    ("chr ord", "character and code point"),
    ("print", "write a line to the terminal"),
    ("type", "name the type of a value"),
    ("nil? list? int? str? fn?", "type predicates"),
    ("uptime ticks", "milliseconds and timer ticks since boot"),
    ("time", "wall clock from the CMOS RTC"),
    ("mem", "memory statistics as a list"),
    ("cpu", "CPU brand string"),
    ("threads", "list of running threads"),
    ("pci", "enumerate the PCI bus"),
    ("beep", "(beep frequency milliseconds)"),
    ("peek poke", "read and write physical memory"),
    ("ls cat write-file rm", "the in-memory filesystem"),
    ("env", "every name currently bound"),
];

fn install_one(
    interpreter: &mut Interpreter,
    name: &'static str,
    function: super::value::BuiltinFn,
) {
    interpreter
        .global
        .define(Rc::from(name), Value::Builtin(name, function));
}

fn need(args: &[Value], count: usize, name: &str) -> Result<(), String> {
    if args.len() != count {
        return Err(format!("{} expects {} argument(s)", name, count));
    }
    Ok(())
}

fn ints(args: &[Value]) -> Result<Vec<i64>, String> {
    args.iter().map(|value| value.as_int()).collect()
}

pub fn install(interpreter: &mut Interpreter) {
    // --- arithmetic ---
    install_one(interpreter, "+", |_, args| {
        Ok(Value::Int(ints(&args)?.iter().sum()))
    });
    install_one(interpreter, "-", |_, args| {
        let values = ints(&args)?;
        match values.split_first() {
            None => Err("- expects at least one argument".to_string()),
            Some((first, [])) => Ok(Value::Int(-first)),
            Some((first, rest)) => Ok(Value::Int(rest.iter().fold(*first, |a, b| a - b))),
        }
    });
    install_one(interpreter, "*", |_, args| {
        Ok(Value::Int(ints(&args)?.iter().product()))
    });
    install_one(interpreter, "/", |_, args| {
        let values = ints(&args)?;
        let Some((first, rest)) = values.split_first() else {
            return Err("/ expects at least one argument".to_string());
        };
        let mut result = *first;
        for value in rest {
            if *value == 0 {
                return Err("division by zero".to_string());
            }
            result /= value;
        }
        Ok(Value::Int(result))
    });
    install_one(interpreter, "mod", |_, args| {
        need(&args, 2, "mod")?;
        let divisor = args[1].as_int()?;
        if divisor == 0 {
            return Err("division by zero".to_string());
        }
        Ok(Value::Int(args[0].as_int()? % divisor))
    });
    install_one(interpreter, "min", |_, args| {
        ints(&args)?
            .into_iter()
            .min()
            .map(Value::Int)
            .ok_or_else(|| "min expects at least one argument".to_string())
    });
    install_one(interpreter, "max", |_, args| {
        ints(&args)?
            .into_iter()
            .max()
            .map(Value::Int)
            .ok_or_else(|| "max expects at least one argument".to_string())
    });
    install_one(interpreter, "abs", |_, args| {
        need(&args, 1, "abs")?;
        Ok(Value::Int(args[0].as_int()?.abs()))
    });

    // --- comparison ---
    install_one(interpreter, "=", |_, args| {
        need(&args, 2, "=")?;
        Ok(Value::Bool(args[0] == args[1]))
    });
    install_one(interpreter, "/=", |_, args| {
        need(&args, 2, "/=")?;
        Ok(Value::Bool(args[0] != args[1]))
    });
    install_one(interpreter, "<", |_, args| {
        need(&args, 2, "<")?;
        Ok(Value::Bool(args[0].as_int()? < args[1].as_int()?))
    });
    install_one(interpreter, ">", |_, args| {
        need(&args, 2, ">")?;
        Ok(Value::Bool(args[0].as_int()? > args[1].as_int()?))
    });
    install_one(interpreter, "<=", |_, args| {
        need(&args, 2, "<=")?;
        Ok(Value::Bool(args[0].as_int()? <= args[1].as_int()?))
    });
    install_one(interpreter, ">=", |_, args| {
        need(&args, 2, ">=")?;
        Ok(Value::Bool(args[0].as_int()? >= args[1].as_int()?))
    });
    install_one(interpreter, "not", |_, args| {
        need(&args, 1, "not")?;
        Ok(Value::Bool(!args[0].is_truthy()))
    });

    // --- lists ---
    install_one(interpreter, "list", |_, args| Ok(Value::list(args)));
    install_one(interpreter, "car", |_, args| {
        need(&args, 1, "car")?;
        Ok(args[0].as_list()?.first().cloned().unwrap_or(Value::Nil))
    });
    install_one(interpreter, "cdr", |_, args| {
        need(&args, 1, "cdr")?;
        let items = args[0].as_list()?;
        Ok(Value::list(items.iter().skip(1).cloned().collect()))
    });
    install_one(interpreter, "cons", |_, args| {
        need(&args, 2, "cons")?;
        let mut items = Vec::with_capacity(1);
        items.push(args[0].clone());
        items.extend(args[1].as_list()?.iter().cloned());
        Ok(Value::list(items))
    });
    install_one(interpreter, "append", |_, args| {
        let mut items = Vec::new();
        for argument in &args {
            items.extend(argument.as_list()?.iter().cloned());
        }
        Ok(Value::list(items))
    });
    install_one(interpreter, "length", |_, args| {
        need(&args, 1, "length")?;
        match &args[0] {
            Value::Str(text) => Ok(Value::Int(text.chars().count() as i64)),
            other => Ok(Value::Int(other.as_list()?.len() as i64)),
        }
    });
    install_one(interpreter, "nth", |_, args| {
        need(&args, 2, "nth")?;
        let index = args[1].as_int()?;
        let items = args[0].as_list()?;
        if index < 0 || index as usize >= items.len() {
            return Ok(Value::Nil);
        }
        Ok(items[index as usize].clone())
    });
    install_one(interpreter, "reverse", |_, args| {
        need(&args, 1, "reverse")?;
        let mut items = args[0].as_list()?.to_vec();
        items.reverse();
        Ok(Value::list(items))
    });
    install_one(interpreter, "range", |_, args| {
        let (start, end) = match args.len() {
            1 => (0, args[0].as_int()?),
            2 => (args[0].as_int()?, args[1].as_int()?),
            _ => return Err("range expects one or two arguments".to_string()),
        };
        if end - start > 100_000 {
            return Err("range is too large".to_string());
        }
        Ok(Value::list((start..end).map(Value::Int).collect()))
    });
    install_one(interpreter, "map", |interpreter, args| {
        need(&args, 2, "map")?;
        let items = args[1].as_list()?.to_vec();
        let mut mapped = Vec::with_capacity(items.len());
        for item in items {
            mapped.push(call(interpreter, args[0].clone(), alloc::vec![item])?);
        }
        Ok(Value::list(mapped))
    });
    install_one(interpreter, "filter", |interpreter, args| {
        need(&args, 2, "filter")?;
        let items = args[1].as_list()?.to_vec();
        let mut kept = Vec::new();
        for item in items {
            if call(interpreter, args[0].clone(), alloc::vec![item.clone()])?.is_truthy() {
                kept.push(item);
            }
        }
        Ok(Value::list(kept))
    });
    install_one(interpreter, "fold", |interpreter, args| {
        need(&args, 3, "fold")?;
        let mut accumulator = args[1].clone();
        for item in args[2].as_list()?.to_vec() {
            accumulator = call(interpreter, args[0].clone(), alloc::vec![accumulator, item])?;
        }
        Ok(accumulator)
    });

    // --- strings ---
    install_one(interpreter, "str", |_, args| {
        let mut text = String::new();
        for argument in &args {
            text.push_str(&argument.display());
        }
        Ok(Value::string(&text))
    });
    install_one(interpreter, "str-len", |_, args| {
        need(&args, 1, "str-len")?;
        Ok(Value::Int(args[0].as_str()?.chars().count() as i64))
    });
    install_one(interpreter, "substr", |_, args| {
        need(&args, 3, "substr")?;
        let text = args[0].as_str()?;
        let start = args[1].as_int()?.max(0) as usize;
        let count = args[2].as_int()?.max(0) as usize;
        let taken: String = text.chars().skip(start).take(count).collect();
        Ok(Value::string(&taken))
    });
    install_one(interpreter, "upper", |_, args| {
        need(&args, 1, "upper")?;
        Ok(Value::string(&args[0].as_str()?.to_uppercase()))
    });
    install_one(interpreter, "lower", |_, args| {
        need(&args, 1, "lower")?;
        Ok(Value::string(&args[0].as_str()?.to_lowercase()))
    });
    install_one(interpreter, "split", |_, args| {
        need(&args, 2, "split")?;
        let text = args[0].as_str()?;
        let separator = args[1].as_str()?;
        if separator.is_empty() {
            return Err("split needs a non-empty separator".to_string());
        }
        Ok(Value::list(
            text.split(separator).map(Value::string).collect(),
        ))
    });
    install_one(interpreter, "chr", |_, args| {
        need(&args, 1, "chr")?;
        let code = args[0].as_int()?;
        let ch = char::from_u32(code as u32).unwrap_or('?');
        let mut buffer = [0u8; 4];
        Ok(Value::string(ch.encode_utf8(&mut buffer)))
    });
    install_one(interpreter, "ord", |_, args| {
        need(&args, 1, "ord")?;
        Ok(Value::Int(
            args[0]
                .as_str()?
                .chars()
                .next()
                .map(|c| c as i64)
                .unwrap_or(0),
        ))
    });

    // --- output and types ---
    install_one(interpreter, "print", |interpreter, args| {
        let mut line = String::new();
        for (index, argument) in args.iter().enumerate() {
            if index > 0 {
                line.push(' ');
            }
            line.push_str(&argument.display());
        }
        interpreter.print(line);
        Ok(Value::Nil)
    });
    install_one(interpreter, "type", |_, args| {
        need(&args, 1, "type")?;
        Ok(Value::string(args[0].type_name()))
    });
    install_one(interpreter, "nil?", |_, args| {
        need(&args, 1, "nil?")?;
        Ok(Value::Bool(matches!(args[0], Value::Nil)))
    });
    install_one(interpreter, "list?", |_, args| {
        need(&args, 1, "list?")?;
        Ok(Value::Bool(matches!(args[0], Value::List(_))))
    });
    install_one(interpreter, "int?", |_, args| {
        need(&args, 1, "int?")?;
        Ok(Value::Bool(matches!(args[0], Value::Int(_))))
    });
    install_one(interpreter, "str?", |_, args| {
        need(&args, 1, "str?")?;
        Ok(Value::Bool(matches!(args[0], Value::Str(_))))
    });
    install_one(interpreter, "fn?", |_, args| {
        need(&args, 1, "fn?")?;
        Ok(Value::Bool(matches!(
            args[0],
            Value::Lambda(_) | Value::Builtin(..)
        )))
    });
    install_one(interpreter, "env", |interpreter, _| {
        Ok(Value::list(
            interpreter
                .global
                .names()
                .iter()
                .map(|name| Value::string(name))
                .collect(),
        ))
    });

    // --- the machine ---
    install_one(interpreter, "uptime", |_, _| {
        Ok(Value::Int(pit::uptime_ms() as i64))
    });
    install_one(interpreter, "ticks", |_, _| {
        Ok(Value::Int(pit::ticks() as i64))
    });
    install_one(interpreter, "time", |_, _| {
        let now = rtc::now();
        Ok(Value::list(alloc::vec![
            Value::Int(now.year as i64),
            Value::Int(now.month as i64),
            Value::Int(now.day as i64),
            Value::Int(now.hour as i64),
            Value::Int(now.minute as i64),
            Value::Int(now.second as i64),
        ]))
    });
    install_one(interpreter, "cpu", |_, _| {
        let info = cpu::identify();
        let brand = if info.brand_str().is_empty() {
            info.vendor_str().to_string()
        } else {
            info.brand_str().to_string()
        };
        Ok(Value::string(&brand))
    });
    install_one(interpreter, "mem", |_, _| {
        let frames = mm::FRAMES.lock();
        let (total, used, free) = (
            frames.total_bytes(),
            frames.used_bytes(),
            frames.free_bytes(),
        );
        drop(frames);
        let heap = mm::heap::ALLOCATOR.stats();
        Ok(Value::list(alloc::vec![
            Value::list(alloc::vec![
                Value::string("total"),
                Value::Int(total as i64)
            ]),
            Value::list(alloc::vec![Value::string("used"), Value::Int(used as i64)]),
            Value::list(alloc::vec![Value::string("free"), Value::Int(free as i64)]),
            Value::list(alloc::vec![
                Value::string("heap-mapped"),
                Value::Int(heap.mapped as i64)
            ]),
            Value::list(alloc::vec![
                Value::string("heap-live"),
                Value::Int(heap.live_allocations as i64)
            ]),
        ]))
    });
    install_one(interpreter, "threads", |_, _| {
        Ok(Value::list(
            task::snapshot()
                .iter()
                .map(|thread| {
                    Value::list(alloc::vec![
                        Value::Int(thread.id as i64),
                        Value::string(&thread.name),
                        Value::Int(thread.ticks_used as i64),
                    ])
                })
                .collect(),
        ))
    });
    install_one(interpreter, "pci", |_, _| {
        Ok(Value::list(
            pci::enumerate()
                .iter()
                .map(|device| Value::string(&device.describe()))
                .collect(),
        ))
    });
    install_one(interpreter, "beep", |_, args| {
        let (frequency, duration) = match args.len() {
            0 => (880, 80),
            1 => (args[0].as_int()? as u32, 80),
            _ => (
                args[0].as_int()? as u32,
                args[1].as_int()?.clamp(1, 2000) as u64,
            ),
        };
        speaker::beep(frequency, duration);
        Ok(Value::Nil)
    });

    // Physical memory access, through the physical map.
    //
    // Reads are bounded to mapped RAM rather than trusting the caller: an
    // address past the physical map is not a HALCYON bug worth a fault screen,
    // it is a typo at a shell prompt, and it should not take the desktop down.
    install_one(interpreter, "peek", |_, args| {
        need(&args, 1, "peek")?;
        let address = checked_phys(args[0].as_int()?, 1)?;
        let value =
            unsafe { core::ptr::read_volatile(mm::paging::phys_to_virt(address) as *const u8) };
        Ok(Value::Int(value as i64))
    });
    install_one(interpreter, "peek32", |_, args| {
        need(&args, 1, "peek32")?;
        let address = checked_phys(args[0].as_int()?, 4)?;
        let value =
            unsafe { core::ptr::read_volatile(mm::paging::phys_to_virt(address) as *const u32) };
        Ok(Value::Int(value as i64))
    });

    // --- filesystem ---
    install_one(interpreter, "ls", |_, _| {
        let filesystem = fs::FS.lock();
        Ok(Value::list(
            filesystem
                .list()
                .iter()
                .map(|(path, size, _)| {
                    Value::list(alloc::vec![Value::string(path), Value::Int(*size as i64)])
                })
                .collect(),
        ))
    });
    install_one(interpreter, "cat", |_, args| {
        need(&args, 1, "cat")?;
        let path = args[0].as_str()?;
        let filesystem = fs::FS.lock();
        match filesystem.read(path) {
            Some(file) => Ok(Value::string(
                core::str::from_utf8(file.bytes()).unwrap_or("<binary>"),
            )),
            None => Err(format!("no such file: {}", path)),
        }
    });
    install_one(interpreter, "write-file", |_, args| {
        need(&args, 2, "write-file")?;
        let path = args[0].as_str()?.to_string();
        let contents = args[1].display();
        let mut filesystem = fs::FS.lock();
        filesystem
            .write(&path, contents.into_bytes())
            .map_err(|error| error.to_string())?;
        Ok(Value::Bool(true))
    });
    install_one(interpreter, "rm", |_, args| {
        need(&args, 1, "rm")?;
        let path = args[0].as_str()?.to_string();
        let mut filesystem = fs::FS.lock();
        filesystem
            .remove(&path)
            .map_err(|error| error.to_string())?;
        Ok(Value::Bool(true))
    });
}

/// Validate a physical address for `peek`, so a mistyped one is an error
/// rather than a page fault.
fn checked_phys(address: i64, width: u64) -> Result<u64, String> {
    if address < 0 {
        return Err("address must not be negative".to_string());
    }
    let address = address as u64;
    let limit = mm::FRAMES.lock().total_bytes();
    if address.saturating_add(width) > limit {
        return Err(format!(
            "{:#x} is outside mapped physical memory (0..{:#x})",
            address, limit
        ));
    }
    Ok(address)
}

/// Apply a callable value — used by `map`, `filter` and `fold`.
fn call(interpreter: &mut Interpreter, callee: Value, args: Vec<Value>) -> EvalResult {
    match callee {
        Value::Builtin(_, function) => function(interpreter, args),
        Value::Lambda(_) => {
            // Rebuild an application form with the arguments already evaluated,
            // quoting each so they are not evaluated a second time.
            let mut form = Vec::with_capacity(args.len() + 1);
            form.push(Value::list(alloc::vec![Value::symbol("quote"), callee]));
            for argument in args {
                form.push(Value::list(alloc::vec![Value::symbol("quote"), argument]));
            }
            let global = interpreter.global.clone();
            interpreter.eval(Value::list(form), &global)
        }
        other => Err(format!("{} is not callable", other.type_name())),
    }
}

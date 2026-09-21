# ORACLE

ORACLE is two things wearing one name: the Lisp the HALCYON shell evaluates, and
the persona that answers when you type `ask`.

## What it is not

It is not a language model. There is no model on this machine and no network
stack to reach one. The conversational side is a keyword-matched table of
answers written by hand, and it says so when asked. The repository is called
`ai-os`; being clear about this seemed the least it could do.

The *interpreter*, on the other hand, is real.

## The language

Anything typed at the shell that begins with `(` or `'` is read and evaluated.

### Representation

Lists are **vectors**, not cons pairs. That costs improper lists — `(a . b)` has
no spelling here — and buys a representation with no cycles, no interior
mutability, and O(1) `length`. For a shell language that is a good trade.

Values: `nil`, booleans, 64-bit integers, strings, symbols, lists, builtins and
lambdas. Integers accept `0x` prefixes. Everything except `nil` and `false` is
true.

### Special forms

```
quote  if  define  set!  lambda (or fn)  let  begin (or do)  while  cond  and  or
```

`define` takes both shapes:

```lisp
(define pi 355)
(define (double x) (* x 2))
```

`lambda` supports a rest parameter:

```lisp
(define (count-args &rest xs) (length xs))
(count-args 1 2 3)   ; => 3
```

### Tail calls

Tail positions in `if`, `begin`, `cond`, `let` and lambda bodies rebind and loop
rather than recursing. This is not a nicety — kernel threads get 64 KiB of
stack, and a tail-recursive loop that consumed a frame per iteration would smash
into whatever is below it. So this works:

```lisp
(define (down n) (if (<= n 0) 'done (down (- n 1))))
(down 200000)        ; => done
```

Non-tail recursion is capped at 256 frames, and `while` at five million
iterations, so a bad expression reports an error instead of taking the desktop
down with it.

### Builtins

Arithmetic `+ - * / mod min max abs`, comparison `= /= < > <= >=`, `not`.

Lists: `list car cdr cons append length nth reverse range map filter fold`.

Strings: `str str-len substr upper lower split chr ord`.

Types: `type nil? list? int? str? fn?`, plus `print` and `env`.

The machine: `uptime ticks time cpu mem threads pci beep tone peek peek32`.
`beep` is the PC speaker, `tone` the sound card — a triangle wave, since the
kernel is soft-float and a sine would need an FPU.

Files: `ls cat write-file rm`.

`help` lists the shell commands; `lisp` lists the language.

### Examples

```lisp
(+ 1 2 3)                                    ; => 6
(define (square x) (* x x))
(map square (range 1 9))                     ; => (1 4 9 16 25 36 49 64)
(fold + 0 (range 101))                       ; => 5050
(filter (lambda (n) (= 0 (mod n 3))) (range 20))
(print (str "up " (uptime) " ms"))
(length (pci))                               ; how many PCI devices
(car (cdr (mem)))                            ; ("used" <bytes>)
```

`/demo.oracle` in the initrd has more; `cat /demo.oracle` to read it.

## The persona

`ask` followed by a question matches keywords against a table in
`kernel/src/oracle/persona.rs`. A few answers are assembled from live data —
ask it about memory, uptime, the CPU or threads and it reads the real numbers
out of the kernel rather than reciting a canned figure.

```
halcyon> ask what is halcyon
halcyon> ask how much memory
halcyon> ask are you an llm
```

If nothing matches it says so, and suggests wrapping the input in parentheses so
it gets evaluated instead.

## Implementation

| file | what it does |
|---|---|
| `value.rs` | values and environments (`Rc<RefCell<Scope>>` chains) |
| `reader.rs` | text to values; comments, strings, escapes, `'` quoting |
| `eval.rs` | the evaluator, with the tail-call loop |
| `builtins.rs` | every builtin, including the kernel hooks |
| `persona.rs` | the response table |

Roughly 1,500 lines all told.

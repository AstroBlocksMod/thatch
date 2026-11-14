# Supported blocks

This page lists what the Scratch frontend compiles. Keep it in step with `crates/thatch-scratch`. Anything not listed compiles to nothing with a warning (`thatch check` lists them); a missing reporter gives 0.

## Events

- when green flag clicked
- when I receive (message)
- broadcast (message), including a message computed by a reporter
- broadcast (message) and wait

## Control

- wait (secs)
- repeat (times)
- forever
- if, if else
- wait until
- repeat until
- while (the hidden block)
- stop all, stop this script, stop other scripts in sprite

## Looks

- say, think
- say for (secs), think for (secs)

Bubbles only show up in the headless host's output; there is no stage yet.

## Data

- variable reporters, set variable to, change variable by, for sprite and stage variables

Lists are not supported yet. Cloud variables compile as ordinary variables, with a warning.

## Operators

- `+`, `-`, `*`, `/`, mod, round
- `<`, `>`, `=`, and, or, not
- join, letter of, length of, contains
- pick random
- abs, floor, ceiling, sqrt, sin, cos, tan, asin, acos, atan, ln, log, e ^, 10 ^

## Sensing

- timer, reset timer

## My blocks

- define, with number or text parameters and boolean parameters
- run without screen refresh
- calls, including recursion

## Known differences

These are the places where a compiled project can behave differently from scratch-vm. Each is a bug to fix or a documented limit, not a design choice.

- `wait` inside a custom block that runs without screen refresh waits for the next frame instead of busy-waiting in place until the time is up, which is what scratch-vm does.
- The input of `repeat` is evaluated once, where scratch-vm evaluates it on every iteration. Only `pick random` inside the input could tell the difference.
- `letter of` on half of a character outside the Basic Multilingual Plane gives U+FFFD instead of a lone surrogate, since Rust strings cannot hold one.
- Cloud variables never sync with a server; they keep their values only for the run.
- Blocks nested more than 1000 deep, counting C blocks and reporters inside inputs, are a compile error that names the block, so a malformed or generated project cannot overflow the compiler's stack. scratch-vm has no such limit.
- Trigonometric results can differ in the last digit from V8's where the platform's `libm` and V8's fdlibm port disagree, before scratch-vm's rounding to ten decimals hides it.

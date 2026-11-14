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

## Motion

- move (steps)
- turn right (degrees), turn left (degrees)
- go to x: y:, go to (sprite, random position or mouse pointer)
- glide (secs) to x: y:, glide (secs) to (sprite, random position or mouse pointer)
- point in direction, point towards (sprite or mouse pointer)
- change x by, set x to, change y by, set y to
- if on edge, bounce
- set rotation style
- x position, y position, direction

The runtime keeps every sprite's position and direction and tells the host when they change; the host draws. Fencing to the stage and bouncing need to know how big the sprite is drawn, which the host supplies through its `measure` callback (see [the ABI](abi.md#writing-a-host)); without one, as in the headless host, they do what scratch-vm does without a renderer: nothing. The mouse pointer sits at the center of the stage until a host reports it.

## Looks

- say, think
- say for (secs), think for (secs)
- show, hide
- switch costume to, next costume, costume (number or name)
- switch backdrop to, next backdrop, backdrop (number or name)
- switch backdrop to and wait (switches without waiting; see below)
- change size by, set size to, size
- change effect by, set effect to, clear graphic effects
- go to front or back layer, go forward or backward (layers)

Costumes are matched as scratch-vm matches them: a number is a costume number, a string a costume name first, then "next costume", "previous costume" (or "next backdrop", "previous backdrop" and "random backdrop" on the backdrop blocks), then a number written as text. A boolean that names no costume counts as a number too, as JavaScript's `Number` reads it, so `true` picks the first costume and `false` the last, as in scratch-vm. Layer changes also change the order in which hats start, as in scratch-vm, which keeps that order apart from the drawing order and moves each by its own rules: a sprite is never drawn behind the stage, but going back a fraction of a layer from just in front of it (0.5, say) puts the sprite's hats after the stage's, as scratch-vm's `moveExecutable` does. Graphic effects are cleared by `stop all`, the stop sign and the green flag, as scratch-vm clears them.

Bubbles show up in the headless host's output; a host that draws receives them through its `say` callback.

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
- (x position, y position, direction, costume #, costume name, size, volume or a variable) of (sprite), and (backdrop #, backdrop name, volume or a variable) of (Stage)

## My blocks

- define, with number or text parameters and boolean parameters
- run without screen refresh
- calls, including recursion

## Known differences

These are the places where a compiled project can behave differently from scratch-vm. Each is a bug to fix or a documented limit, not a design choice.

- `wait` inside a custom block that runs without screen refresh waits for the next frame instead of busy-waiting in place until the time is up, which is what scratch-vm does.
- `switch backdrop to and wait` only switches. The `when backdrop switches to` hats it would wait for are not supported yet.
- `set size to` and `change size by` without a host that can measure the costume (the headless host, for one) store the size as given. scratch-vm without a renderer ignores the blocks altogether, and with one clamps the size between 5 pixels and one and a half stages, which Thatch does as soon as the host provides `measure`.
- `volume` is a number the project sets and `of` reports; there is no sound yet, so nothing changes it.
- The random position of `go to` and `glide to`, and the random direction of `point towards`, come from Thatch's own generator, so they differ from scratch-vm's as any random numbers do.
- The input of `repeat` is evaluated once, where scratch-vm evaluates it on every iteration. Only `pick random` inside the input could tell the difference.
- `letter of` on half of a character outside the Basic Multilingual Plane gives U+FFFD instead of a lone surrogate, since Rust strings cannot hold one.
- Cloud variables never sync with a server; they keep their values only for the run.
- Blocks nested more than 1000 deep, counting C blocks and reporters inside inputs (blocks that merely follow one another do not count), are a compile error that names the block, so a malformed or generated project cannot overflow the compiler's stack. scratch-vm has no such limit.
- Trigonometric results can differ in the last digit from V8's where the platform's `libm` and V8's fdlibm port disagree, before scratch-vm's rounding to ten decimals hides it.

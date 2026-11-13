# Samples

Each directory holds a `project.json` and the `expected.txt` a headless run must print. The expected
output comes from running the same project on scratch-vm, so the tests check Thatch against Scratch
itself, not against what Thatch happened to print. `thatch run samples/<name>/project.json` runs one.

- `hello`: say formatting, casts and arithmetic.
- `strings`: join, letter of, length and comparisons, and how numbers turn into text.
- `procedures`: custom blocks with number, text and boolean parameters, recursion, and warp mode.
- `arguments`: a custom block that changes the variables it was called with still sees the old values.
- `recursion`: the recursion yield rule (a call that repeats one of the last few on the stack yields).
- `broadcast`: broadcast, broadcast and wait, and the order handlers in different sprites run in.
- `loops`: loop yields and how two scripts interleave, with a warp-mode block that finishes first.
- `warp`: warp-mode loops that spin until another script changes a variable, and a stop all that ends a warp loop that never finishes.
- `timing`: waits, wait until and the timer, tested by the order of events.
- `numbers`: NaN handling, hexadecimal and exponent literals, and the hidden while block.
- `motion`: moving, turning and pointing, direction wrapping, going and gliding to points and sprites, and what `of` reports about another sprite and the stage.
- `looks`: costumes and backdrops by number, name and keyword, size, effects, show and hide, and the layer blocks, whose effect shows in the order sprites answer a broadcast.
- `input`: key hats (and that they do not restart), the any-key hat, stage clicks, `key pressed?` with names, letters and codes, `mouse down?`, `mouse x` and `mouse y`, driven by `input.txt`.

A sample with an `input.txt` is run with `--input`: one event per line, starting with the frame it happens before: `5 key down space`, `10 key up left arrow`, `20 mouse 100 -50`, `21 mouse down`, `25 mouse up Cat` (the stage when no target is named). The oracle takes the same file, so expected output with input comes from `node oracle.js project.json input.txt`. Keep events several frames clear of the moment a timed block ends, since scratch-vm runs on the real clock. The C host in the CLI tests takes no input, so it skips these samples.

`crates/thatch-scratch/tests/snapshots/` holds the IR each sample lowers to.

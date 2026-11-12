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

`crates/thatch-scratch/tests/snapshots/` holds the IR each sample lowers to.

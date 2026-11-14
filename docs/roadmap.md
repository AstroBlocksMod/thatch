# Roadmap

This page lists what is planned for Thatch, roughly in the order it will happen. It is a plan, not a promise: priorities move when real projects turn up problems. [Supported blocks](blocks.md) is always the source of truth for what works today.

## Done: the MVP

- [x] A Cargo workspace with the compiler split into reader, values, frontend, IR, codegen and runtime crates
- [x] Scratch value semantics that match scratch-vm (casts, comparison, number formatting, `say` formatting), with tests
- [x] A Scratch frontend with type inference for variables and custom block parameters
- [x] A typed, structured IR with explicit casts and yields
- [x] A Cranelift backend that compiles in memory and into linkable object files
- [x] A runtime with stackful threads and scratch-vm's scheduling rules, built as a static library
- [x] A headless host, `thatch run`, `thatch build`, `thatch check` and `thatch dump`
- [x] Samples whose expected output comes from scratch-vm itself, run both ways by the tests

## Next: a stage

A compiler that cannot draw runs no real project, and the engine-agnostic ABI is the riskiest design decision in the plan. Only a real host proves it, so the first headed host comes before the long tail of blocks.

The headed host is [Scratch Everywhere!](https://github.com/ScratchEverywhere/ScratchEverywhere), the C++ Scratch player, chosen because it is the most reliable native Scratch runtime around. Thatch keeps its own scheduler, which follows scratch-vm more closely than theirs (hat order within a sprite, restarted threads, `wait` in warp mode, the recursion rule), and uses Scratch Everywhere! for what it is good at: rendering, pen, costumes, sound, input and the sensing blocks that need a stage. It is LGPL-3.0, so everything that touches it lives in a separate repository (`thatch-scratch-everywhere`) that pins it as a git submodule, carries one small patch (its two `startThread` functions hand a hat to Thatch instead of running the script themselves), and holds the adapter that implements Thatch's host table and runs the frame loop. Desktop is the target. The library is linked dynamically there, which lets players swap in their own build of it as the LGPL requires; games built this way still ship the LGPL and GPL texts and a notice that they use the library, allow reverse engineering to debug changes to it, and offer the source of the patched library. Consoles are not planned, and most of them would need the LLVM backend anyway, since Cranelift only targets x86-64 and AArch64 among them.

The headless host stays as the reference the samples check against, since Scratch Everywhere!'s own headless mode prints nothing and schedules differently.

- [ ] Sprite state in the runtime (position, direction, size, costume, visibility, effects, layer) and the motion and looks blocks lowered against it, testable headless through the sensing reporters
- [ ] A host table in the ABI: draw, play, read input, start a hat on a key or a click, so a host only implements those; the headless host becomes its first implementation
- [ ] The Scratch Everywhere! host in its own repository: submodule, hook patch and adapter, built with their JIT and DECtalk off (the JIT starts background threads, and DECtalk's license is unclear); Linux first, then macOS and Windows
- [ ] `thatch build --host scratch-everywhere`: links the game against the shared library and writes the license notices next to it
- [ ] Variables synced to Scratch Everywhere!'s copies so its monitors show them
- [ ] Pen
- [ ] Key, mouse and click hats, and the sensing blocks that need a stage
- [ ] Sound

## Then: close the compatibility gaps

- [ ] Lists, including the list blocks and list reporters in `join`
- [ ] Clones: `create clone of`, `when I start as a clone`, `delete this clone`
- [ ] The remaining hats: backdrop switches, timer and loudness hats
- [ ] `answer`, `username`, `days since 2000`, `current` date and time
- [ ] A conformance suite: run the Scratch Foundation's and TurboWarp's test projects through the scratch-vm oracle and compare
- [ ] Fuzzing of `thatch-values` against a Node oracle

## Then: faster

- [ ] Benchmarks against scratch-vm and TurboWarp, run in CI
- [ ] Type inference for strings and booleans, not only numbers
- [ ] Inlining of small custom blocks and loop-invariant code motion in an optimizer crate over the IR
- [ ] Cheaper yields: a per-frame budget check instead of a context switch when only one thread is active
- [ ] An LLVM backend behind the same backend trait, for release builds

## Then: beyond Scratch

- [ ] The superset: custom blocks with return values, static type annotations, compile-time macros and FFI into the host, stored so the Scratch editor still loads the project
- [ ] A Snap! frontend: closures, anonymous functions and nested lists on the same IR and runtime

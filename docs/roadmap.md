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

- [x] A host table in the ABI and a C header for hosts: a host creates the runtime with its callbacks (`say`, the clock), loads the program and steps frames; the headless host is its first implementation
- [x] Sprite state in the runtime (position, direction, size, costume, visibility, effects, layer) and the motion and looks blocks lowered against it, testable headless through the sensing reporters, with a callback that tells the host what to draw
- [x] Input in the host table: keys, mouse, clicks, and the hats they start
- [ ] Stage size and frame rate as values the runtime reports to hosts instead of constants in the ABI, before any host hard-codes 480 by 360 at 30 frames per second
- [ ] The Scratch Everywhere! host in its own repository: submodule, hook patch and adapter, built with their JIT and DECtalk off (the JIT starts background threads, and DECtalk's license is unclear); Linux first, then macOS and Windows
- [ ] `thatch build --host scratch-everywhere`: links the game against the shared library and writes the license notices next to it
- [ ] Variables synced to Scratch Everywhere!'s copies so its monitors show them
- [ ] Pen
- [ ] The sensing blocks that need a stage: touching, touching color, distance to, ask, and dragging sprites
- [ ] Sound
- [ ] A headed host of our own, in Rust on wgpu with no copyleft, that draws sprites and pen and is the reference for screenshot tests. It batches a frame's pen lines and stamps into a few draw calls, in order, which is where 3D projects that fill triangles with the pen spend their time. wgpu is MIT or Apache-2.0, so whether games built with it ship those notices is decided before it does

## Then: close the compatibility gaps

- [ ] Lists, including the list blocks and list reporters in `join`
- [ ] Clones: `create clone of`, `when I start as a clone`, `delete this clone`
- [ ] Scratch's limits by default: at most 300 clones and 200,000 list items, as in scratch-vm
- [ ] Raised limits on request: clone count, list size, stage size, frame rate, fencing and turbo mode, through `thatch` flags and through a project's TurboWarp settings comment (the JSON stage comment that ends in `// _twconfig_`). Thatch says which settings it applied, and a flag ignores the comment. Settings TurboWarp has no field for go in a `// _thatchconfig_` comment of the same shape
- [ ] The remaining hats: backdrop switches, timer and loudness hats
- [ ] `answer`, `username`, `days since 2000`, `current` date and time
- [ ] A conformance suite: run the Scratch Foundation's and TurboWarp's test projects through the scratch-vm oracle and compare
- [ ] Fuzzing of `thatch-values` against a Node oracle

## Then: faster

Speed may change how a value is computed, never which value comes out. Scratch numbers are 64-bit doubles, and add, subtract, multiply, divide and square root are exactly rounded, so doing the same operations on several list items at once in SIMD lanes gives the same bits. Summing a list in a different order does not, and neither do vectorized sine and cosine, so those stay scalar.

- [ ] Benchmarks against scratch-vm and TurboWarp, run in CI
- [ ] Type inference for strings and booleans, not only numbers
- [ ] Numeric lists stored as plain doubles when type inference shows they only ever hold numbers
- [ ] Inlining of small custom blocks and loop-invariant code motion in an optimizer crate over the IR
- [ ] Bounds checks of `item i of list` moved out of a loop once the range of `i` is known
- [ ] Cheaper yields: a per-frame budget check instead of a context switch when only one thread is active, and a check without a runtime call at the end of loops in warp mode (which is inherited, and where the 500 ms limit still applies)
- [ ] SIMD in the runtime for collision tests, `touching color`, list search and pen compositing
- [ ] An LLVM backend behind the same backend trait, for release builds
- [ ] Vectorized loops in the LLVM backend for work done on every item of a list, such as the vertex transforms of 3D engines, only where the result is bit-identical to running the items one by one

## Then: beyond Scratch

- [ ] The superset: custom blocks with return values, static type annotations, compile-time macros and FFI into the host, stored so the Scratch editor still loads the project
- [ ] Macro extensions that expand into standard Scratch blocks, so a project stays portable to Scratch, TurboWarp and goboscript. Each expansion carries a block comment naming its macro, so tools can fold it back. Thatch recognizes the expansions and lowers them onto fast paths that give exactly the result the blocks would, and an expansion someone edited in the Scratch editor takes the general path
- [ ] A Snap! frontend: closures, anonymous functions and nested lists on the same IR and runtime

## Not planned

- GPU compute for a project's own math. 3D projects do their math on the CPU and draw on the GPU, and that split stays: the math gets faster through the items above, the drawing through the wgpu host. Shaders would compute in 32-bit floats, since 64-bit floats are optional in wgpu and missing on Metal, which changes results that feed comparisons such as depth sorting; and the lists in real projects are small enough that copying them to the GPU each frame costs more than it saves.
- NPU backends such as ONNX Runtime, unless a real project needs one.

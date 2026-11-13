# Thatch

Thatch is an optimizing compiler for Scratch projects. Today it builds standalone native executables: it reads a standard `.sb3` file, lowers the blocks to a typed intermediate representation, generates machine code with Cranelift and links the result against a small runtime. There is no browser and no JavaScript engine in the result: a `forever` loop is a loop, a variable is a number in memory, and a custom block is a function.

It is written in Rust and licensed under Apache-2.0 with LLVM exceptions, so the programs it builds can be shipped under any license, along with the [license notices](getting-started.md#licenses-of-a-built-executable) of the libraries its runtime uses.

## What it does today

Thatch compiles and runs projects that use the blocks on the [supported blocks](blocks.md) page: variables, the operators, the control blocks, custom blocks with and without "run without screen refresh", broadcasts, `say` and `think`, waits and the timer, the motion and looks blocks, and the key, mouse and click hats and sensing blocks. Compiled programs run headless and print what sprites say, and a host can draw sprites through the [host table](abi.md#writing-a-host). Every sample in `samples/` prints exactly what scratch-vm prints for it, which is the measure Thatch holds itself to.

What it does not do yet: a host that draws on screen, pen, sound, the sensing blocks that need a stage such as `touching`, and then lists and clones. The [roadmap](roadmap.md) has the plan.

## Where it is going

Thatch is built to grow in three directions without starting over:

- **Engine-agnostic.** Compiled programs talk to the outside world only through a small, versioned [C ABI](abi.md). The headless host is the first implementation; a host that renders through Scratch Everywhere!, another engine, or a plain window implements the same functions.
- **Backend-agnostic.** The [IR](ir.md) knows nothing about Scratch or about Cranelift. Cranelift comes first because it compiles fast, and it already compiles in memory for `thatch run`, which a tiering JIT will build on; an LLVM backend can take the same IR when the last few percent of speed matter more than compile time; and JVM and CLR backends can read it in a serialized form. All of them call the same runtime, so a project keeps scratch-vm's timing however it is compiled.
- **A Scratch superset, and then Snap!.** Once vanilla projects run exactly, the plan is real functions with return values, static types, compile-time macros and FFI into the host, and then a Snap! frontend with closures and nested lists, lowering to the same IR.

Start with [getting started](getting-started.md).

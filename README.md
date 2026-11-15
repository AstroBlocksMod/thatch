# Thatch

Thatch compiles Scratch projects to standalone native executables. It reads a standard `.sb3`, lowers the blocks to a typed intermediate representation, generates machine code with Cranelift and links the result against a small runtime. No browser, no JavaScript engine: a `forever` loop is a loop and a variable is a number in memory.

```sh
cargo build --workspace --release
target/release/thatch run project.sb3        # compile in memory and run headless
target/release/thatch build project.sb3 -o project && ./project
```

The compiler is at the MVP stage: variables, operators, control blocks, custom blocks, broadcasts, waits, `say`, and the motion and looks blocks work, and every sample prints exactly what scratch-vm prints. The runtime keeps every sprite's state and describes it to a host through a C API; a host that draws comes next, then lists and clones. The docs in `docs/` cover [getting started](docs/getting-started.md), the [supported blocks](docs/blocks.md), the [architecture](docs/architecture.md), the [IR](docs/ir.md), the [C ABI](docs/abi.md) and the [roadmap](docs/roadmap.md).

Thatch is free software under the Apache-2.0 license with LLVM exceptions, so the programs it builds can be shipped under any license. They carry the license notices of the open source libraries the runtime uses; [Getting started](docs/getting-started.md#licenses-of-a-built-executable) lists them.

# Getting started

## Building Thatch

You need a Rust toolchain (the version is pinned in `rust-toolchain.toml`; rustup installs it) and a C compiler for the final link.

```sh
cargo build --workspace --release
```

This builds the `thatch` command and `libthatch_runtime.a`, the runtime that every compiled program links against, side by side in `target/release/`.

## Running a project

```sh
thatch run project.sb3
```

compiles the project in memory and runs it headless: the scripts run as they would after a click on the green flag, with a virtual clock that advances a thirtieth of a second per frame, and everything a sprite says is printed as `Sprite says: text`. The run ends when no script is left, or after `--frames N` frames. `--timing` prints the frame count and wall time to stderr, and `--seed N` makes `pick random` repeatable.

`thatch run` also takes a bare `project.json`.

## Building an executable

```sh
thatch build project.sb3 -o project
./project --frames 300
```

writes an object file with Cranelift, then links it with `cc` against `libthatch_runtime.a`, which `thatch` looks for next to itself (`--runtime-lib` or `THATCH_RUNTIME_LIB` point it elsewhere). The executable takes the same `--frames`, `--timing` and `--seed` options as `thatch run`. `--object` writes only the object file.

## Looking inside

```sh
thatch check project.sb3
```

reports every block the compiler does not support yet. Unsupported blocks compile to nothing (a reporter gives 0), with a warning, so a project with a few of them still runs.

```sh
thatch dump project.sb3
```

prints the [IR](ir.md) the Scratch frontend produced, after type inference: which variables became plain numbers, where casts were inserted, where loops yield.

## Samples

`samples/` holds small projects with the output scratch-vm prints for them. `cargo test --workspace` compiles each one both ways and compares. See `samples/README.md` for how the expected output is made.

# Getting started

## Building Thatch

You need a Rust toolchain (the version is pinned in `rust-toolchain.toml`; rustup installs it) and a C compiler for the final link.

```sh
cargo build --workspace --release
```

This builds the `thatch` command and `libthatch_runtime.a`, the runtime that every compiled program links against, side by side in `target/release/`. The examples below call it as `thatch`, so put that directory on your `PATH`:

```sh
export PATH="$PWD/target/release:$PATH"
```

Running `target/release/thatch` directly works too. Keep the two files together, since `thatch build` looks for the runtime library next to the `thatch` executable.

## Running a project

```sh
thatch run project.sb3
```

compiles the project in memory and runs it headless: the scripts run as they would after a click on the green flag, with a virtual clock that advances a thirtieth of a second per frame, and everything a sprite says is printed as `Sprite says: text`. The run ends when no script is left, or after `--frames N` frames. `--timing` prints the frame count and wall time to stderr, and `--seed N` makes `pick random` repeatable.

`thatch run` also takes a bare `project.json`.

A headless run has no keyboard or mouse, so `--input FILE` plays one from a script: one event per line, each starting with the frame it happens before.

```text
5 key down space
10 key up space
15 key down left arrow
20 mouse 100 -50
21 mouse down
25 mouse up Cat
```

`key down` and `key up` take a key as the `when key pressed` menu names it. `mouse X Y` moves the pointer to Scratch coordinates, and `mouse down` and `mouse up` press and release the button over the named sprite, or over the stage when no name is given; naming a target the project lacks is an error. Words may be separated by any whitespace, and a key or sprite name is the rest of the line. The run goes on until no script is left and no event is still to come. `samples/input` is a complete example, and the same script format feeds the scratch-vm oracle in `tools/scratch-vm-oracle`.

## Building an executable

```sh
thatch build project.sb3 -o project
./project --frames 300
```

writes an object file with Cranelift, then links it with `cc` against `libthatch_runtime.a`, which `thatch` looks for next to itself (`--runtime-lib` or `THATCH_RUNTIME_LIB` point it elsewhere). The executable takes the same `--frames`, `--timing`, `--seed` and `--input` options as `thatch run`. `--object` writes only the object file, and `--object --no-main` writes one without `main`, for a host of your own that includes `thatch.h` and drives the program itself (see [the ABI](abi.md#writing-a-host); `crates/thatch-cli/tests/c_host/host.c` is a complete example).

### Licenses of a built executable

You can ship what Thatch builds under any license you like. Thatch's own runtime is Apache-2.0 with LLVM exceptions, which asks nothing of the executables it ends up in. The libraries the runtime is built from do ask for one thing: they are under the MIT license or Apache-2.0 (your choice for each), and either one wants its license text to go along with the executable, for example in a `LICENSES` folder or an "about" screen. They are Rust's standard library, `corosensei`, `libc`, `cfg-if` and `scopeguard`, plus `windows-sys` on Windows. `ryu-js` can be taken under BSL-1.0 instead, which needs no notice in an executable.

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

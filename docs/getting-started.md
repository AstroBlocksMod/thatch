# Getting started

## Building Thatch

You need a Rust toolchain (the version is pinned in `rust-toolchain.toml`; rustup installs it) and a C compiler for the final link.

```sh
cargo build --workspace --release
```

This builds the `thatch` command and `libthatch_runtime.a`, the runtime that every compiled program links against, side by side in `target/release/`, with the runtime's license notices next to them as `libthatch_runtime.licenses.txt`. The examples below call it as `thatch`, so put that directory on your `PATH`:

```sh
export PATH="$PWD/target/release:$PATH"
```

Running `target/release/thatch` directly works too. Keep the three files together, since `thatch build` looks for the runtime library next to the `thatch` executable and for the notices next to the library.

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

writes an object file with Cranelift, then links it with `cc` against `libthatch_runtime.a`, which `thatch` looks for next to itself (`--runtime-lib` or `THATCH_RUNTIME_LIB` point it elsewhere). Next to the executable it writes `project.licenses.txt`, the license notices to ship with it (see below). The executable takes the same `--frames`, `--timing`, `--seed` and `--input` options as `thatch run`. `--object` writes only the object file, and `--object --no-main` writes one without `main`, for a host of your own that includes `thatch.h` and drives the program itself (see [the ABI](abi.md#writing-a-host); `crates/thatch-cli/tests/c_host/host.c` is a complete example).

### Licenses of a built executable

You can ship what Thatch builds under any license you like. Thatch's own runtime is Apache-2.0 with LLVM exceptions, which asks nothing of the executables it ends up in. The libraries the runtime is built from do ask for one thing: they are under the MIT license or Apache-2.0 (your choice for each), and either one wants its license text to go along with the executable.

`thatch build` gathers those texts for you. It writes them next to the executable, in one file named after it with `.licenses.txt` added (`project.licenses.txt` for `project`). Ship that file with the executable, or show its text on an "about" screen, and the duty is met. The file covers:

- Rust's standard library and the libraries built into it, from the copyright file the Rust toolchain comes with
- `cfg-if`, `corosensei`, `libc` and `scopeguard`, each under the MIT license
- on Windows, `windows-sys`, `windows-targets` and the import library for the machine, also under the MIT license

`ryu-js` is part of the runtime too. It is taken under the Boost Software License, which asks for no notice in a compiled program, so the file only names it.

The texts are collected when the runtime library is built, for the platform it is built for, and kept next to it as `libthatch_runtime.licenses.txt`; `thatch build` copies the file that lies next to the library it links. When you link another runtime library with `--runtime-lib`, keep its notices next to it under the same name with `.licenses.txt` in place of `.a`. Without that file `thatch build` warns and writes the notices of the runtime `thatch` itself was built with. If the file starts with `THIS FILE IS INCOMPLETE`, something could not be gathered when the runtime was built (a Rust toolchain that was not installed with rustup may lack the standard library's copyright file, for one); the lines below that heading say what to add by hand.

An object file from `--object` gets no notices of its own, since it is not what you ship. Whatever you link it into needs the notices of the runtime library it is linked with.

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

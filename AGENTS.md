# Thatch

## Overview

Thatch is an ahead-of-time compiler, written in Rust, that turns block-based programs into standalone native executables. It starts with Scratch 3.0 (`.sb3`) and is designed to grow to Snap! later. It is a real optimizing compiler, not an interpreter or a VM packaged with the project. License: Apache-2.0 with LLVM Exceptions (`Apache-2.0 WITH LLVM-exception`, see `LICENSE`). The exception matters here: runtime code that ends up inside a user's compiled executable does not force the Apache attribution terms onto that executable, so people can ship what Thatch builds under any license. Keep it that way. Use the SPDX expression above in every crate's `license` field, and be strict about what gets linked into output binaries (`thatch-runtime`, `thatch-abi`, anything they pull in). No copyleft there at all. Prefer licenses with no attribution requirement for binaries (Apache-2.0 WITH LLVM-exception, Zlib, BSL-1.0, 0BSD, Unlicense). MIT and BSD dependencies in the runtime would require users to ship their notices, so ask before adding one. The compiler itself can use any permissive dependency. Enforce this with `cargo deny` once it is set up.

Goals, in priority order:

1. **Run vanilla Scratch exactly.** A standard `.sb3` must behave as it does in scratch-vm: same casting, same number formatting, same list and string quirks, same frame and thread timing. Speed never justifies a behavior difference.
2. **Native speed.** No browser and no JavaScript engine. Projects compile through a mid-level IR to machine code.
3. **Pluggable runtimes.** Compiled code talks to the outside world only through a versioned C ABI, so the same output can drive ScratchEverywhere (C++), other engines such as Project Tachyon, or a headless CLI runtime.
4. **Pluggable backends.** Codegen sits behind the IR. Cranelift first (fast builds), then LLVM (maximum optimization), with GCC possible later.
5. **A Scratch superset.** Later: compile-time macros, functions with return values, static types and FFI into the host engine, all while still loading every standard `.sb3`.
6. **Snap!.** Later still: first-class closures, anonymous functions and nested lists, added as a new frontend and new IR features, without rewriting the backends or the ABI.

### Layout

A Cargo workspace. The crates, in pipeline order (see `docs/architecture.md` for the picture):

- `crates/thatch-sb3`: reads `.sb3` archives and `project.json` into a typed model. Keeps everything it does not interpret (unknown fields, raw input arrays) in `extra` maps so the model can grow without losing data.
- `crates/thatch-values`: Scratch value semantics (casts, comparison, JavaScript number formatting, `Math` behavior, `say` formatting). Shared by the frontend (for constant folding and type decisions) and the runtime. Must mirror scratch-vm's `Cast` and operator code exactly; every rule has a unit test.
- `crates/thatch-scratch`: the Scratch frontend. Lowers blocks to the IR and runs type inference for variables and custom block parameters to a fixed point. Every supported block is listed in `docs/blocks.md`; keep the two in step. Unsupported blocks warn and compile to nothing.
- `crates/thatch-ir`: the IR: structured and typed, with explicit casts, explicit yields and runtime calls. Language-neutral and backend-neutral. Documented in `docs/ir.md`, which states what frontends and backends promise each other.
- `crates/thatch-codegen`: the Cranelift backend, generic over `cranelift_module::Module` so the same lowering compiles in memory (`thatch run`) and into object files with a C `main` (`thatch build`).
- `crates/thatch-abi`: the C ABI (`#[repr(C)]` layouts, the table of runtime functions with their signatures, hat kinds), `no_std` and dependency-free. Documented in `docs/abi.md`.
- `crates/thatch-runtime`: the runtime linked into every executable: targets and variables, strings, stackful threads (`corosensei`), scratch-vm's scheduler rules, and the headless host. Built as an rlib and as `libthatch_runtime.a`.
- `crates/thatch-cli`: the `thatch` command (`run`, `build`, `check`, `dump`) and the end-to-end tests in `tests/`.

Outside `crates/`: `samples/` (test projects as `project.json` with the `expected.txt` scratch-vm prints for them), `tools/scratch-vm-oracle/` (the Node script that produces those), `docs/` (the Zensical site).

Planned, not yet present: an optimizer crate over the IR, a host table in the ABI with a windowed host, benchmarks, an LLVM backend.

### Rules that keep the layers separate

- The compiler never depends on `thatch-runtime`. Generated code only calls functions declared in `thatch-abi`. When you change an ABI function, update its entry in `RT_FNS`, the runtime definition in `api.rs` (a test checks the two lists match) and every backend call site together.
- The ABI is versioned. Any breaking change bumps `ABI_VERSION`.
- `thatch-ir` and the backends know nothing about Scratch or Snap!. If a backend needs to know what a block is, the IR is missing a concept.
- Language-specific lowering lives in the frontend, not in passes or backends.
- Superset features must not break vanilla projects. Store superset data so that the standard Scratch editor still loads the project, and keep a project that uses no superset features compiling exactly as a vanilla one.

## Scratch semantics (IMPORTANT)

- Read scratch-vm's source for every block you implement or optimize, and match it. Scratch's quirks are the spec: `"10" < "9"` rules, `NaN` handling, whitespace in numeric strings, `join` number formatting, list index parsing ("last", "random", "any"), case-insensitive comparisons and so on. The current scratch-vm is the reference, not an old version: the `say` bubble formatting, for one, changed from `parseFloat(n.toFixed(2))` to `n.toFixed(2)`.
- Thread timing matters as much as values: yields at the end of loops outside warp mode, the redraw request rules, broadcast-and-wait, the order hats start in (front to back across sprites, stage last), the recursion yield rule, and the 30 FPS frame. Get these wrong and many projects break even when every block computes the right value.
- A pass must keep the project behaving exactly as scratch-vm runs it. Document each pass and the conditions that make it safe in `docs/`.
- When the frontend treats a literal or a variable as a number, that must be invisible: a text literal becomes a number only when every block, `say` included, would give the same result (`thatch_values::is_safe_number_literal` is the rule). Known, documented differences are listed at the end of `docs/blocks.md`; add to that list rather than hide a difference.
- jScratch (`../jScratch`, a Kotlin compiler for the same input) has worked through most of these details. Its SB3O library in `sb3o/` is BSD 3-Clause and can be ported here with its notice kept. Its other modules are GPL-3.0 and may only be ported if their copyright holder relicenses them. Never copy code from anyone else's GPL or AGPL project. scratch-vm, TurboWarp and ScratchEverywhere are references to read, not code to paste.

## Building and testing

- `cargo build --workspace` builds the `thatch` command and `libthatch_runtime.a` side by side in `target/<profile>/`, where `thatch build` finds the library. The toolchain is pinned in `rust-toolchain.toml`. Linking needs `cc`.
- `cargo test --workspace` runs the unit tests and the end-to-end tests in `crates/thatch-cli/tests/samples.rs`, which compile every sample both in memory and to a linked executable and compare the output with `samples/<name>/expected.txt`.
- The expected output of a sample comes from scratch-vm, never from Thatch. `tools/scratch-vm-oracle/` has the script: `npm install` there once, then `node oracle.js <project.json>` prints what scratch-vm prints in the same format. When you add a sample, generate its `expected.txt` with it, and check the result makes sense before committing it. A sample's output must not depend on wall-clock timing: test ordering across waits with gaps of several frames, not counts of frames.
- Every new block gets a sample that exercises its quirks, an entry in `docs/blocks.md`, and when its value semantics are subtle, a unit test in `thatch-values`.
- `thatch run project --timing` prints frames and wall time; use it before and after a change that could affect speed until the benchmarks exist.

## Searching the Codebase

Use fast, modern tools instead of the slow defaults:

- [`rg`](https://github.com/BurntSushi/ripgrep) (ripgrep) for text search, not `grep`. It respects `.gitignore` for you. **NOTE**: Prefer to use [`ast-grep`](https://ast-grep.github.io) when applicable as it uses the structure of the code instead.
- [`fd`](https://github.com/sharkdp/fd) for finding files by name, not `find`.
- [`ast-grep`](https://ast-grep.github.io) for structural searches and rewrites, like finding every call to a function or every impl of a trait. Call it as `ast-grep`. The `sg` alias is deprecated, so don't use it.
- `jq` for reading and querying JSON, including `project.json` files.

If your harness has dedicated search or file tools that already wrap these, those are fine too.

## Code Quality

Always lint and format:

- `cargo fmt --all` before committing. `cargo clippy --workspace --all-targets -- -D warnings` must pass. CI runs both.
- Lints are configured once in the workspace `Cargo.toml` under `[workspace.lints]`; every crate opts in with `[lints] workspace = true`.

Use modern code practices:

- Latest stable Rust, edition 2024. Declare shared dependencies once in `[workspace.dependencies]`.
- `unsafe` only at the ABI boundary and in the runtime where it is actually needed, each block with a `// SAFETY:` comment explaining why it holds. Generated code and the runtime share raw pointers by design; keep the invariants written down where the pointer is created.
- Errors: `thiserror` in library crates, `anyhow` only in the CLI. Compile errors shown to users should point at the sprite and block that caused them.
- No panics on bad input. A malformed `.sb3` is a reported error, not a crash.

## Docs (IMPORTANT!!!)

Always update the docs if necessary when changing or adding code. If the docs already contain outdated information, expand your scope to cover that info too.

Docs live in `docs/` as a Zensical site (configured by `zensical.toml` at the root; `zensical build` writes `site/`, which git ignores). Pages: `index.md`, `getting-started.md`, `blocks.md` (supported blocks and known differences, kept in step with the frontend), `architecture.md`, `ir.md`, `abi.md`, `roadmap.md`. Planned: `passes.md` once there is an optimizer, `benchmarks.md` once there are benchmarks, `language-features.md` for the superset.

When a roadmap item is finished, check it off (`- [x]`) where it is. DO NOT move it into an earlier section like "Done: the MVP". The roadmap shows how each goal is progressing, so finished items stay with the section they belong to.

## Diagrams as Mermaid, not ASCII art

Never draw a flow, pipeline or architecture picture out of characters in a code block. Make a Mermaid diagram instead, which Zensical integrates with.

## Outdated packages

PLEASE, PLEASE do not use a package that is old or deprecated. EVEN IF IT IS "INDUSTRY STANDARD" THAT DOES NOT MEAN IT IS GOOD!!! USE THE BEST PACKAGE AND LATEST VERSION LIKE A NORMAL PERSON WOULD!

Check crates.io (`cargo search <name>` or `https://crates.io/api/v1/crates/<name>`) for the current release before adding or bumping a dependency, and check that the crate is still maintained. Cranelift's API moves between releases; the crate sources in `~/.cargo/registry/src/` are the quickest reference when a signature changed.

## Keep things human

Do no use em dashes or other special symbols not normally found in writing. Do not word things in a weird way. Keep it looking human. If you need to use, for example, an arrow icon, use an SVG instead.

Replacing an em dash with a hyphen is NOT a fix. A spaced hyphen doing an em dash's job ("the limit - 4 bytes", "fast - but wrong") is the same tic with a different character. Reword the sentence instead: use a comma, a colon, parentheses, or the actual word ("minus", "to", "through"). Plain hyphens in compound words and numeric ranges like 0-1 are fine.

## Use American spelling

It's fine to be British in the chat if the person is actually in a region where its spelled like that. But if not just use American spelling. The thing itself MUST use American spelling.

<!-- SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception -->

# The Scratch Everywhere! host

This directory turns [Scratch Everywhere!](https://github.com/ScratchEverywhere/ScratchEverywhere), the C++ Scratch player, into a stage for games compiled by Thatch. Thatch runs the scripts; Scratch Everywhere! opens the window, draws the sprites and reads the keyboard and the mouse. The [docs](../../docs/scratch-everywhere.md) describe how the two fit together and what a game built this way has to ship.

Scratch Everywhere! is LGPL-3.0, so this directory is fenced off from the rest of Thatch: nothing outside it depends on anything inside it, and it is not part of the Cargo workspace. [LICENSE](LICENSE) says which files here carry which license.

## What is here

| Path | What it is | License |
| --- | --- | --- |
| `ScratchEverywhere/` | Scratch Everywhere!, pinned as a git submodule | LGPL-3.0 |
| `patches/host-hooks.patch` | The one change to it: a hook that hands every hat block to the host, and a switch for DECtalk | LGPL-3.0-or-later |
| `adapter/thatch_host.cpp` | The adapter: Thatch's host table on top of the library, the frame loop and `main` | Apache-2.0 WITH LLVM-exception |
| `tests/` | A small project with real costumes, the input to play and the bubbles to expect | Apache-2.0 WITH LLVM-exception |
| `CMakeLists.txt` | Builds the patched library, the adapter and the test | Apache-2.0 WITH LLVM-exception |
| `notices.cmake` | Gathers the license texts a game ships because of this host | Apache-2.0 WITH LLVM-exception |
| `LICENSES/` | The text of the GPL, for those notices | a license text, not a licensed work |

## Building

You need CMake 3.21 or newer, a C++17 compiler, git and SDL2 (`libsdl2-dev` on Debian and Ubuntu, `brew install sdl2` on macOS). Build the Cargo workspace first, since the test needs `thatch` and `libthatch_runtime.a`.

```sh
git submodule update --init hosts/scratch-everywhere/ScratchEverywhere
cargo build --workspace
cmake -S hosts/scratch-everywhere -B hosts/scratch-everywhere/build -DCMAKE_BUILD_TYPE=Release
cmake --build hosts/scratch-everywhere/build
ctest --test-dir hosts/scratch-everywhere/build --output-on-failure
```

The first configure downloads the libraries Scratch Everywhere! is built from. The build never changes the submodule: it copies the pinned commit into the build directory and applies the patch to the copy.

The build leaves four files in `build/lib/`: the shared library `libscratch-everywhere`, the adapter `libthatch_host_scratch_everywhere.a`, `licenses.txt` with the license texts a game has to ship because of this host, and `thatch-host.txt`, which tells `thatch build --host scratch-everywhere` what to link and what to put next to a game. The test builds a game with that command and runs it without a window.

To see the test game in a window:

```sh
hosts/scratch-everywhere/build/test/game
```

And to build a game of your own:

```sh
target/debug/thatch build game.sb3 --host scratch-everywhere -o dist/game
```

Space moves the square, a click on it changes its costume.

## Updating Scratch Everywhere!

Move the submodule to the new commit, then configure again. If the patch no longer applies, the configure step says so: apply the old patch to the old commit, rebase the change onto the new one and write the patch again with `git diff`. Keep the header at the top of the patch file. The adapter is compiled against the library's headers, so a change to the classes it uses (`Sprite`, `Scratch`, `Render`, `Input`) shows up as a compile error there.

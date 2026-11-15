//! The runtime linked into every program Thatch compiles.
//!
//! A compiled program is a set of native functions, one per script and one per custom block, that
//! know nothing about scheduling, strings or dynamically typed values: all of that lives here, behind
//! the C ABI declared in `thatch-abi`. The runtime owns the targets and their variables, runs the
//! scripts as cooperative threads on their own stacks (see [`thread`]), steps them the way scratch-vm's
//! sequencer does (see [`scheduler`]) and, as the headless host, prints what sprites say.
//!
//! The same code serves two uses. `thatch run` compiles a project in memory and calls [`run`] with the
//! program's init function. `thatch build` links the program against this crate built as
//! `libthatch_runtime.a`, and the program's `main` calls `thatch_main`, which does the same thing.
//!
//! A host other than the headless one (a window, or Scratch Everywhere!) uses the `thatch_runtime_*`
//! functions in [`api`] through the C header in `thatch-abi`: it creates a runtime with its own
//! `thatch_abi::Host` table, loads the program and steps frames.

mod api;
mod host;
mod scheduler;
mod strings;
mod thread;
mod value;

pub use api::{host_symbols, symbols};
pub use host::{RunOptions, RunReport, run};
pub use scheduler::Runtime;
pub use thatch_abi::InitFn;

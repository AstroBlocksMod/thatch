//! The C ABI between compiled programs and the Thatch runtime.
//!
//! Generated code never links against Rust: it calls the `thatch_*` functions listed in [`RT_FNS`]
//! by name with the C calling convention, reads the few fields of [`ThreadCtx`] laid out here, and
//! exports `thatch_program_init`, which the runtime calls once with a `Runtime` pointer so the
//! program can register its targets, variables, constants and scripts. The runtime is written in
//! Rust and ships as `libthatch_runtime.a`, but any implementation of these functions would do.
//!
//! Every change to a function here is a breaking change for compiled programs until [`ABI_VERSION`]
//! is bumped with it. A program also exports `thatch_program_abi_version`, which returns the
//! version it was compiled against, and passes the same version to `thatch_main`; a host checks it
//! before it calls `thatch_program_init`.
//!
//! The other side of the runtime is the host: the program that owns the window, the clock and the
//! frame loop. A host creates a runtime with a [`Host`] table of callbacks, loads a program into
//! it and steps frames through the functions in [`HOST_API`]. The headless host in `thatch-runtime`
//! is one such host; one written in C or C++ uses the header [`c_header`] generates
//! (`crates/thatch-abi/include/thatch.h`).

#![no_std]

extern crate alloc;

use alloc::string::String;
use core::ffi::c_void;
use core::fmt::Write;

pub const ABI_VERSION: u32 = 3;

/// A dynamically typed Scratch value: a number, a string handle or a boolean.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Value {
    /// One of [`TAG_NUM`], [`TAG_STR`] or [`TAG_BOOL`].
    pub tag: u64,
    /// The `f64` bits of a number, a string handle, or 0 or 1 for a boolean.
    pub bits: u64,
}

pub const TAG_NUM: u64 = 0;
pub const TAG_STR: u64 = 1;
pub const TAG_BOOL: u64 = 2;

/// What a running thread sees of itself. Generated code reads these fields directly; everything
/// else about a thread is private to the runtime behind `inner`.
#[repr(C)]
#[derive(Debug)]
pub struct ThreadCtx {
    /// The numeric variables of the thread's target.
    pub nums: *mut f64,
    /// The dynamically typed variables of the thread's target.
    pub anys: *mut Value,
    /// The numeric variables of the stage.
    pub stage_nums: *mut f64,
    /// The dynamically typed variables of the stage.
    pub stage_anys: *mut Value,
    /// String handles of the program's constant pool, in registration order.
    pub consts: *const *const c_void,
    /// Nonzero once the thread was stopped. Generated code returns at once when it sees this after
    /// any call that can yield.
    pub killed: u64,
    pub inner: *mut c_void,
}

/// Hat kinds for `thatch_add_script`.
pub const HAT_FLAG: i64 = 0;
pub const HAT_BROADCAST: i64 = 1;

/// Types in runtime function signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiType {
    F64,
    I64,
    I8,
    I32,
}

/// A runtime function: its symbol, parameters and return type.
#[derive(Debug, Clone, Copy)]
pub struct RtFn {
    pub name: &'static str,
    pub params: &'static [AbiType],
    pub ret: Option<AbiType>,
}

use AbiType::{F64, I8, I32, I64};

macro_rules! rt_fns {
    ($( $konst:ident = $name:literal ( $($p:ident),* ) $(-> $r:ident)? ; )*) => {
        $( pub const $konst: &str = $name; )*
        /// Every runtime function, with the signature generated code calls it with. Pointers are
        /// `I64`, booleans `I8`.
        pub const RT_FNS: &[RtFn] = &[
            $( RtFn { name: $name, params: &[$($p),*], ret: rt_fns!(@ret $($r)?) }, )*
        ];
    };
    (@ret) => { None };
    (@ret $r:ident) => { Some($r) };
}

rt_fns! {
    // Registration, called from thatch_program_init with the Runtime pointer.
    ADD_CONST = "thatch_add_const"(I64, I64, I64);
    ADD_TARGET = "thatch_add_target"(I64, I64, I64, I8) -> I64;
    ADD_NUM_VAR = "thatch_add_num_var"(I64, I64, I64, I64, F64);
    ADD_ANY_VAR = "thatch_add_any_var"(I64, I64, I64, I64, I64, I64);
    ADD_SCRIPT = "thatch_add_script"(I64, I64, I64, I64, I64, I64);
    // The headless host's entry point: argc, argv, init function, ABI version of the program.
    MAIN = "thatch_main"(I32, I64, I64, I32) -> I32;

    // Strings. Handles returned here belong to the thread until thatch_temps_clear.
    TEMPS_CLEAR = "thatch_temps_clear"(I64);
    NUM_TO_STR = "thatch_num_to_str"(I64, F64) -> I64;
    STR_TO_NUM = "thatch_str_to_num"(I64) -> F64;
    STR_TO_BOOL = "thatch_str_to_bool"(I64) -> I8;
    BOOL_TO_STR = "thatch_bool_to_str"(I64, I8) -> I64;
    JOIN = "thatch_join"(I64, I64, I64) -> I64;
    LETTER_OF = "thatch_letter_of"(I64, F64, I64) -> I64;
    LENGTH = "thatch_length"(I64) -> F64;
    CONTAINS = "thatch_contains"(I64, I64) -> I8;

    // Dynamically typed values, by pointer.
    ANY_TO_NUM = "thatch_any_to_num"(I64) -> F64;
    ANY_TO_STR = "thatch_any_to_str"(I64, I64) -> I64;
    ANY_TO_BOOL = "thatch_any_to_bool"(I64) -> I8;
    // Copies a value into a slot the caller owns, keeping its string alive for the statement, so
    // a custom block argument does not change when the variable it came from does.
    ANY_SNAPSHOT = "thatch_any_snapshot"(I64, I64, I64);
    VAR_SET = "thatch_var_set"(I64, I64);
    VAR_SET_NUM = "thatch_var_set_num"(I64, F64);
    VAR_SET_STR = "thatch_var_set_str"(I64, I64);
    VAR_SET_BOOL = "thatch_var_set_bool"(I64, I8);
    VAR_CHANGE = "thatch_var_change"(I64, F64);
    COMPARE = "thatch_compare"(I64, I64) -> F64;
    COMPARE_NUMS = "thatch_compare_nums"(F64, F64) -> F64;

    // Arithmetic the instruction set lacks.
    MOD = "thatch_mod"(F64, F64) -> F64;
    ROUND = "thatch_round"(F64) -> F64;
    MATHOP = "thatch_mathop"(I64, F64) -> F64;
    RANDOM = "thatch_random"(I64, I64, I64) -> F64;
    TIMER = "thatch_timer"(I64) -> F64;
    RESET_TIMER = "thatch_reset_timer"(I64);

    // Control. After each of these, generated code checks ThreadCtx::killed.
    YIELD = "thatch_yield"(I64);
    WAIT = "thatch_wait"(I64, F64);
    SAY = "thatch_say"(I64, I64, I8);
    SAY_FOR = "thatch_say_for"(I64, I64, F64, I8);
    BROADCAST = "thatch_broadcast"(I64, I64);
    BROADCAST_WAIT = "thatch_broadcast_wait"(I64, I64);
    STOP_ALL = "thatch_stop_all"(I64);
    STOP_OTHERS = "thatch_stop_others"(I64);
    PROC_ENTER = "thatch_proc_enter"(I64, I64, I64, I8);
    PROC_EXIT = "thatch_proc_exit"(I64, I8);
}

/// The symbol compiled programs export for the runtime to call.
pub const PROGRAM_INIT: &str = "thatch_program_init";

/// The symbol of `int32_t thatch_program_abi_version(void)`, which every compiled program exports.
pub const PROGRAM_ABI_VERSION: &str = "thatch_program_abi_version";

/// Finds a runtime function by name.
pub fn rt_fn(name: &str) -> Option<&'static RtFn> {
    RT_FNS.iter().find(|f| f.name == name)
}

// ---- hosts ------------------------------------------------------------------------------------

/// The runtime, opaque to hosts and programs. `Runtime` in `thatch-runtime`.
#[repr(C)]
pub struct OpaqueRuntime {
    _private: [u8; 0],
}

/// A program's init function, `thatch_program_init`.
pub type InitFn = unsafe extern "C" fn(*mut OpaqueRuntime);

/// Called when a target says or thinks `text` (UTF-8, `len` bytes, not NUL-terminated, already cut
/// and formatted as scratch-vm shows it). Empty text clears the bubble.
pub type HostSayFn = unsafe extern "C" fn(
    data: *mut c_void,
    rt: *mut OpaqueRuntime,
    target: i64,
    text: *const u8,
    len: usize,
    think: i8,
);

/// The host's clock in milliseconds, which `wait` and `timer` read. Only differences matter.
pub type HostNowFn = unsafe extern "C" fn(data: *mut c_void) -> f64;

/// What a host gives the runtime: a pointer passed back to every callback, and the callbacks. A
/// null callback means the host does not care, except `now_ms`, where null selects the virtual
/// clock of one frame per thirtieth of a second that makes headless runs repeatable.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Host {
    pub data: *mut c_void,
    pub say: Option<HostSayFn>,
    pub now_ms: Option<HostNowFn>,
}

/// Result codes of `thatch_runtime_load`.
pub const LOAD_OK: i32 = 0;
pub const LOAD_WRONG_ABI_VERSION: i32 = 1;
pub const LOAD_NO_STAGE: i32 = 2;

/// A C parameter: type and name.
#[derive(Debug, Clone, Copy)]
pub struct CParam {
    pub ty: &'static str,
    pub name: &'static str,
}

/// A function in C terms, for the header and for the test that the runtime defines it.
#[derive(Debug, Clone, Copy)]
pub struct CFn {
    pub name: &'static str,
    pub ret: &'static str,
    pub params: &'static [CParam],
    pub doc: &'static str,
}

macro_rules! c_fns {
    ($( $(#[doc = $doc:literal])+ $ret:literal $name:literal ( $( $pty:literal $pname:literal ),* ); )*) => {
        &[ $( CFn { name: $name, ret: $ret, params: &[ $( CParam { ty: $pty, name: $pname } ),* ], doc: concat!($($doc, "\n"),+) }, )* ]
    };
}

/// The functions the runtime exports for hosts. Generated code never calls these.
pub const HOST_API: &[CFn] = c_fns! {
    /// Creates a runtime that reports to `host` (copied) and seeds `pick random` with `seed`.
    "ThatchRuntime *" "thatch_runtime_create"("const ThatchHost *" "host", "uint64_t" "seed");
    /// Checks the program's ABI version (from thatch_program_abi_version) against the runtime's,
    /// then lets the program register itself. Returns THATCH_LOAD_OK or a THATCH_LOAD_* error, in
    /// which case the runtime is empty and may only be destroyed.
    "int32_t" "thatch_runtime_load"("ThatchRuntime *" "rt", "int32_t" "abi_version", "ThatchInitFn" "init");
    /// Resets the timer and starts the green flag scripts.
    "void" "thatch_runtime_green_flag"("ThatchRuntime *" "rt");
    /// Starts every script with the given hat kind (a THATCH_HAT_* value) and argument, such as a
    /// broadcast name, as a broadcast block would.
    "void" "thatch_runtime_start_hats"("ThatchRuntime *" "rt", "int64_t" "hat", "const char *" "arg", "size_t" "len");
    /// Runs one frame: steps the threads until they all wait for the next frame, a redraw was
    /// requested or 75 percent of a frame has passed. Returns 0, or nonzero after an error that
    /// thatch_runtime_last_error describes, after which the runtime may only be destroyed.
    "int32_t" "thatch_runtime_frame"("ThatchRuntime *" "rt");
    /// Whether the last frame asked for a redraw: a sprite said something or a wait began.
    "int8_t" "thatch_runtime_redraw_requested"("const ThatchRuntime *" "rt");
    /// Whether any script is still running or waiting.
    "int8_t" "thatch_runtime_has_threads"("const ThatchRuntime *" "rt");
    /// How many frames have run.
    "uint64_t" "thatch_runtime_frame_count"("const ThatchRuntime *" "rt");
    /// Stops every script and clears every bubble, as the stop sign does.
    "void" "thatch_runtime_stop_all"("ThatchRuntime *" "rt");
    /// How many targets the program registered. The stage is one of them.
    "int64_t" "thatch_runtime_target_count"("const ThatchRuntime *" "rt");
    /// The name of a target as UTF-8, `*len` bytes long, not NUL-terminated. Valid until the
    /// runtime is destroyed. Null for an index out of range.
    "const char *" "thatch_runtime_target_name"("const ThatchRuntime *" "rt", "int64_t" "target", "size_t *" "len");
    /// Whether a target is the stage.
    "int8_t" "thatch_runtime_target_is_stage"("const ThatchRuntime *" "rt", "int64_t" "target");
    /// The message of the last error, UTF-8 and `*len` bytes long, or null when there was none.
    "const char *" "thatch_runtime_last_error"("const ThatchRuntime *" "rt", "size_t *" "len");
    /// Stops every script, lets the suspended ones finish and frees the runtime.
    "void" "thatch_runtime_destroy"("ThatchRuntime *" "rt");
};

/// The header a host written in C or C++ includes. `crates/thatch-abi/include/thatch.h` is this
/// text; a test keeps the two the same.
pub fn c_header() -> String {
    let mut h = String::new();
    let _ = write!(
        h,
        "\
/* The Thatch host API. Generated from crates/thatch-abi/src/lib.rs; do not edit. */
#ifndef THATCH_H
#define THATCH_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern \"C\" {{
#endif

/* The ABI version this header describes. A program reports the version it was compiled for through
 * thatch_program_abi_version, and thatch_runtime_load refuses any other. */
#define THATCH_ABI_VERSION {ABI_VERSION}

/* The runtime. Opaque; create one with thatch_runtime_create. */
typedef struct ThatchRuntime ThatchRuntime;

/* A compiled program exports these two functions. */
typedef void (*ThatchInitFn)(ThatchRuntime *rt);
void thatch_program_init(ThatchRuntime *rt);
int32_t thatch_program_abi_version(void);

/* Hat kinds for thatch_runtime_start_hats. */
#define THATCH_HAT_FLAG {HAT_FLAG}
#define THATCH_HAT_BROADCAST {HAT_BROADCAST}

/* Results of thatch_runtime_load. */
#define THATCH_LOAD_OK {LOAD_OK}
#define THATCH_LOAD_WRONG_ABI_VERSION {LOAD_WRONG_ABI_VERSION}
#define THATCH_LOAD_NO_STAGE {LOAD_NO_STAGE}

/* Called when a target says or thinks text: UTF-8, len bytes, not NUL-terminated, already cut and
 * formatted as scratch-vm shows it. Empty text clears the bubble. */
typedef void (*ThatchSayFn)(void *data, ThatchRuntime *rt, int64_t target, const char *text, size_t len, int8_t think);

/* The host's clock in milliseconds, read by wait and timer. Only differences matter. */
typedef double (*ThatchNowFn)(void *data);

/* What a host gives the runtime: data is passed back to every callback. A null callback means the
 * host does not care, except now_ms, where null selects the virtual clock of one frame per thirtieth
 * of a second that makes headless runs repeatable. */
typedef struct ThatchHost {{
    void *data;
    ThatchSayFn say;
    ThatchNowFn now_ms;
}} ThatchHost;

/* A host creates a runtime, loads a program, starts the green flag and then runs a frame at a time,
 * drawing after each one that requested a redraw. Nothing here is thread-safe; one runtime belongs to
 * one thread. */
"
    );
    for f in HOST_API {
        h.push('\n');
        for (i, line) in f.doc.lines().enumerate() {
            let _ = write!(h, "{} {}", if i == 0 { "/*" } else { " *" }, line.trim());
            if i + 1 == f.doc.lines().count() {
                h.push_str(" */");
            }
            h.push('\n');
        }
        let _ = write!(h, "{}{}{}(", f.ret, if f.ret.ends_with('*') { "" } else { " " }, f.name);
        for (i, p) in f.params.iter().enumerate() {
            if i > 0 {
                h.push_str(", ");
            }
            let _ = write!(h, "{}{}{}", p.ty, if p.ty.ends_with('*') { "" } else { " " }, p.name);
        }
        h.push_str(");\n");
    }
    h.push_str(
        "
#ifdef __cplusplus
}
#endif

#endif
",
    );
    h
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    /// `include/thatch.h` is what `c_header` generates. Run with `THATCH_UPDATE_HEADER=1` to
    /// rewrite it after a change.
    #[test]
    fn header_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("include/thatch.h");
        let generated = c_header();
        if std::env::var_os("THATCH_UPDATE_HEADER").is_some() {
            std::fs::write(&path, &generated).expect("the header is writable");
        }
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            on_disk == generated,
            "include/thatch.h is out of date; run `THATCH_UPDATE_HEADER=1 cargo test -p thatch-abi` and commit it"
        );
    }

    #[test]
    fn host_api_names_are_unique_and_prefixed() {
        for (i, f) in HOST_API.iter().enumerate() {
            assert!(f.name.starts_with("thatch_runtime_"), "{}", f.name);
            assert!(!HOST_API[..i].iter().any(|g| g.name == f.name), "{} twice", f.name);
        }
    }
}

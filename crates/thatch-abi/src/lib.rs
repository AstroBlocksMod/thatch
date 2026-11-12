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

#![no_std]

use core::ffi::c_void;

pub const ABI_VERSION: u32 = 2;

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

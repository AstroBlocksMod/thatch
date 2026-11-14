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

pub const ABI_VERSION: u32 = 5;

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
/// `when key pressed`. The argument is the key as the `when key pressed` menu names it ("space",
/// "left arrow", "a", "0", or "any"), matched without regard to case. A key hat does not restart a
/// thread of its script that is still running.
pub const HAT_KEY: i64 = 2;
/// `when this sprite clicked`, started on the clicked target only.
pub const HAT_SPRITE_CLICKED: i64 = 3;
/// `when stage clicked`, also started on the clicked target only (scratch-vm treats the two click
/// hats alike, so a stage hat on a sprite fires when that sprite is clicked).
pub const HAT_STAGE_CLICKED: i64 = 4;

/// Rotation styles, for `thatch_set_sprite`, `thatch_set_rotation_style` and [`Sprite`].
pub const ROTATE_ALL_AROUND: i64 = 0;
pub const ROTATE_LEFT_RIGHT: i64 = 1;
pub const ROTATE_DONT_ROTATE: i64 = 2;

/// The graphic effects, in the order of [`Sprite::effects`] and of the effect argument of
/// `thatch_set_effect` and `thatch_change_effect`.
pub const EFFECTS: &[&str] = &["color", "fisheye", "whirl", "pixelate", "mosaic", "brightness", "ghost"];
pub const EFFECT_COUNT: usize = 7;

/// The stage, in Scratch coordinates: x runs from -240 to 240, y from -180 to 180.
pub const STAGE_WIDTH: f64 = 480.0;
pub const STAGE_HEIGHT: f64 = 360.0;

/// What a sprite looks like, for the host to draw. The stage has a costume (its backdrop) and
/// effects; its other fields are fixed.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sprite {
    pub x: f64,
    pub y: f64,
    /// -179 to 180, 90 pointing right, as Scratch shows it. The rotation style says how to draw it.
    pub direction: f64,
    /// Percent of the costume's size.
    pub size: f64,
    /// The index of the current costume, counted from 0.
    pub costume: i64,
    /// One of the `ROTATE_*` values.
    pub rotation_style: i32,
    pub visible: i8,
    pub _reserved: [i8; 3],
    /// The graphic effects, in the order of [`EFFECTS`].
    pub effects: [f64; EFFECT_COUNT],
}

/// What `measure` asks a host for.
pub const MEASURE_BOUNDS: i32 = 0;
pub const MEASURE_BOX: i32 = 1;
pub const MEASURE_COSTUME_SIZE: i32 = 2;

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
    // A costume of a target, by name, in costume order.
    ADD_COSTUME = "thatch_add_costume"(I64, I64, I64, I64);
    // A target's initial state: x, y, direction, size, costume index, visible, rotation style,
    // draggable, volume.
    SET_SPRITE = "thatch_set_sprite"(I64, I64, F64, F64, F64, F64, I64, I8, I64, I8, F64);
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

    // Motion. Names of other sprites (and "_mouse_" and "_random_") are string handles.
    MOVE_STEPS = "thatch_move_steps"(I64, F64);
    GO_TO_XY = "thatch_go_to_xy"(I64, F64, F64);
    GO_TO = "thatch_go_to"(I64, I64);
    SET_X = "thatch_set_x"(I64, F64);
    SET_Y = "thatch_set_y"(I64, F64);
    CHANGE_X = "thatch_change_x"(I64, F64);
    CHANGE_Y = "thatch_change_y"(I64, F64);
    TURN = "thatch_turn"(I64, F64);
    POINT_IN_DIRECTION = "thatch_point_in_direction"(I64, F64);
    POINT_TOWARDS = "thatch_point_towards"(I64, I64);
    // Glides yield until the time is up.
    GLIDE = "thatch_glide"(I64, F64, F64, F64);
    GLIDE_TO = "thatch_glide_to"(I64, F64, I64);
    BOUNCE = "thatch_bounce"(I64);
    SET_ROTATION_STYLE = "thatch_set_rotation_style"(I64, I64);
    SPRITE_X = "thatch_sprite_x"(I64) -> F64;
    SPRITE_Y = "thatch_sprite_y"(I64) -> F64;
    SPRITE_DIRECTION = "thatch_sprite_direction"(I64) -> F64;

    // Looks. Costumes and backdrops are chosen by a dynamically typed value: a number is an index,
    // a string a name first.
    SET_VISIBLE = "thatch_set_visible"(I64, I8);
    SWITCH_COSTUME = "thatch_switch_costume"(I64, I64);
    NEXT_COSTUME = "thatch_next_costume"(I64);
    SWITCH_BACKDROP = "thatch_switch_backdrop"(I64, I64);
    NEXT_BACKDROP = "thatch_next_backdrop"(I64);
    SET_SIZE = "thatch_set_size"(I64, F64);
    CHANGE_SIZE = "thatch_change_size"(I64, F64);
    SET_EFFECT = "thatch_set_effect"(I64, I64, F64);
    CHANGE_EFFECT = "thatch_change_effect"(I64, I64, F64);
    CLEAR_EFFECTS = "thatch_clear_effects"(I64);
    GO_TO_LAYER = "thatch_go_to_layer"(I64, I8);
    GO_LAYERS = "thatch_go_layers"(I64, F64);
    SPRITE_SIZE = "thatch_sprite_size"(I64) -> F64;
    COSTUME_NUMBER = "thatch_costume_number"(I64, I8) -> F64;
    COSTUME_NAME = "thatch_costume_name"(I64, I8) -> I64;
    // `property of object`: writes the value into the caller's slot.
    PROPERTY_OF = "thatch_property_of"(I64, I64, I64, I64);

    // Sensing: input the host reported.
    MOUSE_X = "thatch_mouse_x"(I64) -> F64;
    MOUSE_Y = "thatch_mouse_y"(I64) -> F64;
    MOUSE_DOWN = "thatch_mouse_down"(I64) -> I8;
    // `key pressed?` with a dynamically typed key: a name, a letter or a key code.
    KEY_PRESSED = "thatch_key_pressed"(I64, I64) -> I8;
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

/// Called when something about how a target looks changed: position, direction, size, costume,
/// visibility, effects or layer order. `thatch_runtime_sprite` has the new state.
pub type HostSpriteChangedFn = unsafe extern "C" fn(data: *mut c_void, rt: *mut OpaqueRuntime, target: i64);

/// Asks a host that draws about a sprite as it is drawn right now. `what` is `MEASURE_BOUNDS` (the
/// tight bounds of the drawn sprite: left, right, bottom, top in Scratch coordinates), `MEASURE_BOX`
/// (its axis-aligned bounding box, the same way) or `MEASURE_COSTUME_SIZE` (the current costume's
/// width and height before scaling, in the first two values). Returns nonzero when `out` was filled.
pub type HostMeasureFn =
    unsafe extern "C" fn(data: *mut c_void, rt: *mut OpaqueRuntime, target: i64, what: i32, out: *mut f64) -> i8;

/// What a host gives the runtime: a pointer passed back to every callback, and the callbacks. A
/// null callback means the host does not care, except `now_ms`, where null selects the virtual
/// clock of one frame per thirtieth of a second that makes headless runs repeatable. A callback may
/// ask the runtime about its state (`thatch_runtime_target_name` and the like), but must not run a
/// frame, load, start or stop anything, or destroy the runtime.
///
/// `sprite_changed` tells the runtime that the host draws: then a visible change to a sprite asks
/// for a redraw, which ends the frame early, as scratch-vm does with a renderer. `measure` lets the
/// runtime keep sprites fenced to the stage, bounce them off its edges and clamp their size, which
/// all need to know how big a sprite is drawn; without it those do what scratch-vm does without a
/// renderer, which is nothing. Callbacks may read the runtime back through the host API, but not
/// change it.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Host {
    pub data: *mut c_void,
    pub say: Option<HostSayFn>,
    pub now_ms: Option<HostNowFn>,
    pub sprite_changed: Option<HostSpriteChangedFn>,
    pub measure: Option<HostMeasureFn>,
}

/// Result codes of `thatch_runtime_load`.
pub const LOAD_OK: i32 = 0;
pub const LOAD_WRONG_ABI_VERSION: i32 = 1;
pub const LOAD_NO_STAGE: i32 = 2;
pub const LOAD_ALREADY_LOADED: i32 = 3;

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
    /// Creates a runtime that reports to `host` (copied, or null for no callbacks) and seeds `pick
    /// random` with `seed`. `abi_version` is the THATCH_ABI_VERSION the host was compiled with,
    /// which says how big its ThatchHost is; for any other version this returns null and reads
    /// nothing from `host`.
    "ThatchRuntime *" "thatch_runtime_create"("int32_t" "abi_version", "const ThatchHost *" "host", "uint64_t" "seed");
    /// Checks the program's ABI version (from thatch_program_abi_version) against the runtime's,
    /// then lets the program register itself. Returns THATCH_LOAD_OK or a THATCH_LOAD_* error, in
    /// which case the runtime is left empty, as it was created. A runtime holds one program:
    /// loading another into it gives THATCH_LOAD_ALREADY_LOADED and changes nothing.
    "int32_t" "thatch_runtime_load"("ThatchRuntime *" "rt", "int32_t" "abi_version", "ThatchInitFn" "init");
    /// Stops every script and clears every bubble, as the stop sign does, then resets the timer and
    /// starts the green flag scripts, as clicking the green flag does.
    "void" "thatch_runtime_green_flag"("ThatchRuntime *" "rt");
    /// Starts every script with the given hat kind (a THATCH_HAT_* value) and argument, such as a
    /// broadcast name, as a broadcast block would.
    "void" "thatch_runtime_start_hats"("ThatchRuntime *" "rt", "int64_t" "hat", "const char *" "arg", "size_t" "len");
    /// A key went down or up. `key` is the key's name as a browser's KeyboardEvent.key gives it
    /// (" ", "a", "ArrowLeft", "Enter"; the Scratch names "space", "left arrow" and so on work
    /// too), UTF-8 and `len` bytes long. Names longer than one character that are not a known key
    /// are ignored, as scratch-vm ignores modifier keys. Going down starts the `when key pressed`
    /// hats for the key and for "any".
    "void" "thatch_runtime_key"("ThatchRuntime *" "rt", "const char *" "key", "size_t" "len", "int8_t" "down");
    /// The mouse moved to a point in Scratch coordinates (x from -240 to 240, y from -180 to 180,
    /// origin in the middle, y up); the runtime rounds and clamps it as scratch-vm does. A NaN
    /// coordinate is ignored.
    "void" "thatch_runtime_mouse_move"("ThatchRuntime *" "rt", "double" "x", "double" "y");
    /// The mouse button went down or up over `target`, the frontmost target under the pointer as
    /// the host sees it (the stage when no sprite is there), or -1 outside the stage. A click on a
    /// target that is not draggable starts its click hats as the button goes down; on a draggable
    /// one as it comes up. Nothing starts when the button state did not change, or when `dragged`
    /// says the release ends a drag of a sprite.
    "void" "thatch_runtime_mouse_button"("ThatchRuntime *" "rt", "int8_t" "down", "int64_t" "target", "int8_t" "dragged");
    /// Whether the player may drag a target, which decides when its clicks count.
    "int8_t" "thatch_runtime_target_draggable"("const ThatchRuntime *" "rt", "int64_t" "target");
    /// Runs one frame: steps the threads until they all wait for the next frame, a redraw was
    /// requested or 75 percent of a frame has passed. Returns 0, or nonzero after an error that
    /// thatch_runtime_last_error describes, after which the runtime may only be destroyed.
    "int32_t" "thatch_runtime_frame"("ThatchRuntime *" "rt");
    /// Whether the host should draw: something changed what the stage shows (a bubble, for one) or
    /// a wait began, during the last frame or since it ended, or the program was just loaded.
    "int8_t" "thatch_runtime_redraw_requested"("const ThatchRuntime *" "rt");
    /// Whether any script is still running or waiting. Stopped scripts do not count, even before
    /// the next frame lets them finish.
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
    /// Fills in how a target looks right now. Returns 0 for an index out of range.
    "int8_t" "thatch_runtime_sprite"("const ThatchRuntime *" "rt", "int64_t" "target", "ThatchSprite *" "out");
    /// Writes the targets in drawing order, the stage first and the frontmost sprite last, into
    /// `targets`, at most `cap` of them, and returns how many there are in all.
    "size_t" "thatch_runtime_draw_order"("const ThatchRuntime *" "rt", "int64_t *" "targets", "size_t" "cap");
    /// How many costumes a target has.
    "int64_t" "thatch_runtime_costume_count"("const ThatchRuntime *" "rt", "int64_t" "target");
    /// The name of a target's costume as UTF-8, `*len` bytes long, not NUL-terminated. Null for an
    /// index out of range.
    "const char *" "thatch_runtime_costume_name"("const ThatchRuntime *" "rt", "int64_t" "target", "int64_t" "costume", "size_t *" "len");
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
 * thatch_program_abi_version, and thatch_runtime_load refuses any other. A host passes it to
 * thatch_runtime_create, which refuses any other too. */
#define THATCH_ABI_VERSION {ABI_VERSION}

/* The runtime. Opaque; create one with thatch_runtime_create. */
typedef struct ThatchRuntime ThatchRuntime;

/* A compiled program exports these two functions. */
typedef void (*ThatchInitFn)(ThatchRuntime *rt);
void thatch_program_init(ThatchRuntime *rt);
int32_t thatch_program_abi_version(void);

/* Hat kinds for thatch_runtime_start_hats. Keys and clicks have their own functions below, which
 * follow scratch-vm's rules for them; the kinds are listed for completeness. */
#define THATCH_HAT_FLAG {HAT_FLAG}
#define THATCH_HAT_BROADCAST {HAT_BROADCAST}
#define THATCH_HAT_KEY {HAT_KEY}
#define THATCH_HAT_SPRITE_CLICKED {HAT_SPRITE_CLICKED}
#define THATCH_HAT_STAGE_CLICKED {HAT_STAGE_CLICKED}

/* Results of thatch_runtime_load. */
#define THATCH_LOAD_OK {LOAD_OK}
#define THATCH_LOAD_WRONG_ABI_VERSION {LOAD_WRONG_ABI_VERSION}
#define THATCH_LOAD_NO_STAGE {LOAD_NO_STAGE}
#define THATCH_LOAD_ALREADY_LOADED {LOAD_ALREADY_LOADED}

/* The stage, in Scratch coordinates: x runs from -240 to 240, y from -180 to 180. */
#define THATCH_STAGE_WIDTH {STAGE_WIDTH}
#define THATCH_STAGE_HEIGHT {STAGE_HEIGHT}

/* Rotation styles. */
#define THATCH_ROTATE_ALL_AROUND {ROTATE_ALL_AROUND}
#define THATCH_ROTATE_LEFT_RIGHT {ROTATE_LEFT_RIGHT}
#define THATCH_ROTATE_DONT_ROTATE {ROTATE_DONT_ROTATE}

/* The graphic effects, as indices into ThatchSprite.effects. */
#define THATCH_EFFECT_COLOR 0
#define THATCH_EFFECT_FISHEYE 1
#define THATCH_EFFECT_WHIRL 2
#define THATCH_EFFECT_PIXELATE 3
#define THATCH_EFFECT_MOSAIC 4
#define THATCH_EFFECT_BRIGHTNESS 5
#define THATCH_EFFECT_GHOST 6
#define THATCH_EFFECT_COUNT {EFFECT_COUNT}

/* What a target looks like, for the host to draw. The stage has a costume (its backdrop) and
 * effects; its other fields are fixed. */
typedef struct ThatchSprite {{
    double x;
    double y;
    double direction;      /* -179 to 180, 90 pointing right; see rotation_style */
    double size;           /* percent of the costume's size */
    int64_t costume;       /* index of the current costume, from 0 */
    int32_t rotation_style; /* a THATCH_ROTATE_* value */
    int8_t visible;
    int8_t reserved[3];
    double effects[THATCH_EFFECT_COUNT];
}} ThatchSprite;

/* Called when a target says or thinks text: UTF-8, len bytes, not NUL-terminated, already cut and
 * formatted as scratch-vm shows it. Empty text clears the bubble. */
typedef void (*ThatchSayFn)(void *data, ThatchRuntime *rt, int64_t target, const char *text, size_t len, int8_t think);

/* The host's clock in milliseconds, read by wait and timer. Only differences matter. */
typedef double (*ThatchNowFn)(void *data);

/* Called when something about how a target looks changed: position, direction, size, costume,
 * visibility, effects or layer order. thatch_runtime_sprite has the new state. */
typedef void (*ThatchSpriteChangedFn)(void *data, ThatchRuntime *rt, int64_t target);

/* What thatch_runtime asks a host that draws to measure. */
#define THATCH_MEASURE_BOUNDS {MEASURE_BOUNDS}       /* tight bounds of the drawn sprite: left, right, bottom, top */
#define THATCH_MEASURE_BOX {MEASURE_BOX}          /* its axis-aligned bounding box, the same way */
#define THATCH_MEASURE_COSTUME_SIZE {MEASURE_COSTUME_SIZE} /* the current costume's unscaled width and height */

/* Measures a sprite as it is drawn right now, in Scratch coordinates, into out (four values). Returns
 * nonzero when out was filled. */
typedef int8_t (*ThatchMeasureFn)(void *data, ThatchRuntime *rt, int64_t target, int32_t what, double *out);

/* What a host gives the runtime: data is passed back to every callback. A null callback means the
 * host does not care, except now_ms, where null selects the virtual clock of one frame per thirtieth
 * of a second that makes headless runs repeatable. Zero the struct before filling it in, so fields
 * added later are null. A callback may ask the runtime about its state (thatch_runtime_target_name
 * and the like), but must not run a frame, load, start or stop anything, or destroy the runtime.
 *
 * sprite_changed tells the runtime that the host draws: a visible change to a sprite then asks for a
 * redraw, which ends the frame early, as scratch-vm does with a renderer. measure lets the runtime
 * keep sprites on the stage, bounce them off its edges and clamp their size, which all need to know
 * how big a sprite is drawn; without it those do what scratch-vm does without a renderer: nothing.
 * Callbacks may read the runtime back through the functions below, but not change it. */
typedef struct ThatchHost {{
    void *data;
    ThatchSayFn say;
    ThatchNowFn now_ms;
    ThatchSpriteChangedFn sprite_changed;
    ThatchMeasureFn measure;
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
    fn sprite_layout_matches_the_header() {
        // The C struct in the header lists the same fields in the same order; this pins the layout
        // that gives.
        assert_eq!(core::mem::size_of::<Sprite>(), 48 + 8 * EFFECT_COUNT);
        assert_eq!(core::mem::offset_of!(Sprite, costume), 32);
        assert_eq!(core::mem::offset_of!(Sprite, rotation_style), 40);
        assert_eq!(core::mem::offset_of!(Sprite, visible), 44);
        assert_eq!(core::mem::offset_of!(Sprite, effects), 48);
        assert_eq!(EFFECTS.len(), EFFECT_COUNT);
    }

    #[test]
    fn host_api_names_are_unique_and_prefixed() {
        for (i, f) in HOST_API.iter().enumerate() {
            assert!(f.name.starts_with("thatch_runtime_"), "{}", f.name);
            assert!(!HOST_API[..i].iter().any(|g| g.name == f.name), "{} twice", f.name);
        }
    }
}

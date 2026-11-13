//! The C ABI surface: the `thatch_*` functions generated code calls, whose signatures match
//! `thatch_abi::RT_FNS`, and the `thatch_runtime_*` functions hosts call, which match
//! `thatch_abi::HOST_API`. Tests below check both lists.
//!
//! Every function here is `unsafe` because it takes raw pointers from C. Their contracts are the
//! ones `thatch_abi` documents for the C header: a `ThatchRuntime *` from `thatch_runtime_create`
//! used between frames on one thread, a `ThreadCtx *` of the running thread, live string handles
//! and value pointers, and buffers of the stated size.
#![allow(clippy::missing_safety_doc)]

use std::ffi::c_char;
use std::rc::Rc;
use std::slice;

use thatch_abi::{Host, OpaqueRuntime, Sprite, TAG_NUM, TAG_STR, ThreadCtx, Value};
use thatch_values::{MathOp, ValueRef};

use crate::scheduler::{Rt, Runtime};
use crate::sprite::PropertyValue;
use crate::strings::{self, Handle};
use crate::thread::{Frame, ScriptFn, ThreadInner, Yield};
use crate::value::{self, VarSlot};

macro_rules! table {
    ($($f:ident),* $(,)?) => { vec![$((stringify!($f), $f as *const u8)),*] };
}

/// The runtime functions by symbol, for JIT lookup.
pub fn symbols() -> Vec<(&'static str, *const u8)> {
    table![
        thatch_add_const,
        thatch_add_target,
        thatch_add_num_var,
        thatch_add_any_var,
        thatch_add_script,
        thatch_add_costume,
        thatch_set_sprite,
        thatch_main,
        thatch_temps_clear,
        thatch_num_to_str,
        thatch_str_to_num,
        thatch_str_to_bool,
        thatch_bool_to_str,
        thatch_join,
        thatch_letter_of,
        thatch_length,
        thatch_contains,
        thatch_any_to_num,
        thatch_any_to_str,
        thatch_any_to_bool,
        thatch_any_snapshot,
        thatch_var_set,
        thatch_var_set_num,
        thatch_var_set_str,
        thatch_var_set_bool,
        thatch_var_change,
        thatch_compare,
        thatch_compare_nums,
        thatch_mod,
        thatch_round,
        thatch_mathop,
        thatch_random,
        thatch_timer,
        thatch_reset_timer,
        thatch_yield,
        thatch_wait,
        thatch_say,
        thatch_say_for,
        thatch_broadcast,
        thatch_broadcast_wait,
        thatch_stop_all,
        thatch_stop_others,
        thatch_proc_enter,
        thatch_proc_exit,
        thatch_move_steps,
        thatch_go_to_xy,
        thatch_go_to,
        thatch_set_x,
        thatch_set_y,
        thatch_change_x,
        thatch_change_y,
        thatch_turn,
        thatch_point_in_direction,
        thatch_point_towards,
        thatch_glide,
        thatch_glide_to,
        thatch_bounce,
        thatch_set_rotation_style,
        thatch_sprite_x,
        thatch_sprite_y,
        thatch_sprite_direction,
        thatch_set_visible,
        thatch_switch_costume,
        thatch_next_costume,
        thatch_switch_backdrop,
        thatch_next_backdrop,
        thatch_set_size,
        thatch_change_size,
        thatch_set_effect,
        thatch_change_effect,
        thatch_clear_effects,
        thatch_go_to_layer,
        thatch_go_layers,
        thatch_sprite_size,
        thatch_costume_number,
        thatch_costume_name,
        thatch_property_of,
    ]
}

/// The host API by symbol, for the test against `HOST_API`.
pub fn host_symbols() -> Vec<(&'static str, *const u8)> {
    table![
        thatch_runtime_create,
        thatch_runtime_load,
        thatch_runtime_green_flag,
        thatch_runtime_start_hats,
        thatch_runtime_frame,
        thatch_runtime_redraw_requested,
        thatch_runtime_has_threads,
        thatch_runtime_frame_count,
        thatch_runtime_stop_all,
        thatch_runtime_target_count,
        thatch_runtime_target_name,
        thatch_runtime_target_is_stage,
        thatch_runtime_sprite,
        thatch_runtime_draw_order,
        thatch_runtime_costume_count,
        thatch_runtime_costume_name,
        thatch_runtime_last_error,
        thatch_runtime_destroy,
    ]
}

// ---- the host API -----------------------------------------------------------------------------
//
// Hosts hold the runtime as an opaque pointer. `thatch_abi::HOST_API` documents each function.

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_create(abi_version: i32, host: *const Host, seed: u64) -> *mut OpaqueRuntime {
    // A host built against another version has a `ThatchHost` of another size, so it is not read.
    if abi_version != thatch_abi::ABI_VERSION as i32 {
        return std::ptr::null_mut();
    }
    // SAFETY: the host passes its own table of this version, or null for no callbacks at all.
    let host = if host.is_null() {
        Host { data: std::ptr::null_mut(), say: None, now_ms: None, sprite_changed: None, measure: None }
    } else {
        unsafe { *host }
    };
    Runtime::create(host, seed).cast::<OpaqueRuntime>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_load(
    rt: *mut OpaqueRuntime,
    abi_version: i32,
    init: thatch_abi::InitFn,
) -> i32 {
    if abi_version != thatch_abi::ABI_VERSION as i32 {
        return thatch_abi::LOAD_WRONG_ABI_VERSION;
    }
    // SAFETY: `rt` comes from `thatch_runtime_create`, and `init` only registers, it runs no
    // script, so no thread holds the runtime. Each borrow below ends before `init` runs.
    unsafe {
        if (*rt.cast::<Runtime>()).loaded {
            return thatch_abi::LOAD_ALREADY_LOADED;
        }
        init(rt);
        let r = &mut *rt.cast::<Runtime>();
        if !r.targets.iter().any(|t| t.is_stage) {
            r.unload();
            return thatch_abi::LOAD_NO_STAGE;
        }
        r.loaded = true;
        // The host has drawn nothing yet.
        r.host_draw();
        thatch_abi::LOAD_OK
    }
}

/// The runtime a host passes, as a handle for the functions that call the host back.
///
/// # Safety
/// `rt` comes from `thatch_runtime_create`, and the host calls between frames, holding no
/// reference into the runtime.
unsafe fn host_rt(rt: *mut OpaqueRuntime) -> Rt {
    // SAFETY: forwarded from the caller.
    unsafe { Rt::new(rt.cast::<Runtime>()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_green_flag(rt: *mut OpaqueRuntime) {
    // SAFETY: `rt` comes from `thatch_runtime_create`; hosts call this between frames.
    unsafe { host_rt(rt).start_flag() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_start_hats(rt: *mut OpaqueRuntime, hat: i64, arg: *const c_char, len: usize) {
    // SAFETY: as `thatch_runtime_green_flag`; `arg` is `len` bytes the host owns.
    unsafe {
        let arg = text(arg.cast::<u8>(), len as i64).to_owned();
        (*rt.cast::<Runtime>()).start_hats(hat, &arg);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_frame(rt: *mut OpaqueRuntime) -> i32 {
    // SAFETY: `rt` comes from `thatch_runtime_create`, and the host holds no reference into it.
    unsafe {
        match Runtime::frame(rt.cast::<Runtime>()) {
            Ok(()) => 0,
            Err(e) => {
                (*rt.cast::<Runtime>()).last_error = Some(e);
                1
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_redraw_requested(rt: *const OpaqueRuntime) -> i8 {
    // SAFETY: as `thatch_runtime_green_flag`.
    unsafe { i8::from((*rt.cast::<Runtime>()).host_should_draw()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_has_threads(rt: *const OpaqueRuntime) -> i8 {
    // SAFETY: as `thatch_runtime_green_flag`.
    unsafe { i8::from((*rt.cast::<Runtime>()).has_threads()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_frame_count(rt: *const OpaqueRuntime) -> u64 {
    // SAFETY: as `thatch_runtime_green_flag`.
    unsafe { (*rt.cast::<Runtime>()).frame }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_stop_all(rt: *mut OpaqueRuntime) {
    // SAFETY: as `thatch_runtime_green_flag`.
    unsafe { host_rt(rt).stop_all() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_target_count(rt: *const OpaqueRuntime) -> i64 {
    // SAFETY: as `thatch_runtime_green_flag`.
    unsafe { (*rt.cast::<Runtime>()).targets.len() as i64 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_target_name(
    rt: *const OpaqueRuntime,
    target: i64,
    len: *mut usize,
) -> *const c_char {
    // SAFETY: as `thatch_runtime_green_flag`; `len` is the host's to write, or null.
    unsafe {
        let r = &*rt.cast::<Runtime>();
        let Some(t) = usize::try_from(target).ok().and_then(|i| r.targets.get(i)) else {
            if !len.is_null() {
                *len = 0;
            }
            return std::ptr::null();
        };
        if !len.is_null() {
            *len = t.name.len();
        }
        t.name.as_ptr().cast::<c_char>()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_target_is_stage(rt: *const OpaqueRuntime, target: i64) -> i8 {
    // SAFETY: as `thatch_runtime_green_flag`.
    unsafe {
        let r = &*rt.cast::<Runtime>();
        let t = usize::try_from(target).ok().and_then(|i| r.targets.get(i));
        i8::from(t.is_some_and(|t| t.is_stage))
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_sprite(rt: *const OpaqueRuntime, target: i64, out: *mut Sprite) -> i8 {
    // SAFETY: as `thatch_runtime_green_flag`; `out` is the host's to write.
    unsafe {
        let r = &*rt.cast::<Runtime>();
        let sprite = usize::try_from(target).ok().and_then(|i| r.sprite(i));
        match sprite {
            Some(s) if !out.is_null() => {
                *out = s;
                1
            }
            _ => 0,
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_draw_order(rt: *const OpaqueRuntime, targets: *mut i64, cap: usize) -> usize {
    // SAFETY: as `thatch_runtime_green_flag`; `targets` holds `cap` entries the host owns.
    unsafe {
        let r = &*rt.cast::<Runtime>();
        if !targets.is_null() {
            for (i, t) in r.order.iter().take(cap).enumerate() {
                *targets.add(i) = *t as i64;
            }
        }
        r.order.len()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_costume_count(rt: *const OpaqueRuntime, target: i64) -> i64 {
    // SAFETY: as `thatch_runtime_green_flag`.
    unsafe {
        let r = &*rt.cast::<Runtime>();
        let t = usize::try_from(target).ok().and_then(|i| r.targets.get(i));
        t.map_or(0, |t| t.costumes.len() as i64)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_costume_name(
    rt: *const OpaqueRuntime,
    target: i64,
    costume: i64,
    len: *mut usize,
) -> *const c_char {
    // SAFETY: as `thatch_runtime_target_name`.
    unsafe {
        let r = &*rt.cast::<Runtime>();
        let name = usize::try_from(target)
            .ok()
            .and_then(|i| r.targets.get(i))
            .and_then(|t| usize::try_from(costume).ok().and_then(|c| t.costumes.get(c)));
        if !len.is_null() {
            *len = name.map_or(0, |n| n.len());
        }
        name.map_or(std::ptr::null(), |n| n.as_ptr().cast::<c_char>())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_last_error(rt: *const OpaqueRuntime, len: *mut usize) -> *const c_char {
    // SAFETY: as `thatch_runtime_target_name`.
    unsafe {
        let e = (*rt.cast::<Runtime>()).last_error.as_deref().unwrap_or("");
        if !len.is_null() {
            *len = e.len();
        }
        if e.is_empty() { std::ptr::null() } else { e.as_ptr().cast::<c_char>() }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_runtime_destroy(rt: *mut OpaqueRuntime) {
    // SAFETY: `rt` comes from `thatch_runtime_create` and is not used after this.
    unsafe {
        Runtime::shutdown(rt.cast::<Runtime>());
        Runtime::destroy(rt.cast::<Runtime>());
    }
}

// ---- helpers ----------------------------------------------------------------------------------

unsafe fn inner<'a>(ctx: *mut ThreadCtx) -> &'a mut ThreadInner {
    // SAFETY: generated code only passes the context of the running thread, whose inner part lives
    // as long as the thread.
    unsafe { &mut *(*ctx).inner.cast::<ThreadInner>() }
}

/// The running thread's runtime, borrowed for one call that does not call the host. Use [`rt`] for
/// the ones that do.
unsafe fn runtime<'a>(ctx: *mut ThreadCtx) -> &'a mut Runtime {
    // SAFETY: as `inner`; the runtime outlives every thread, and scripts run one at a time.
    unsafe { &mut *inner(ctx).runtime }
}

/// The running thread's runtime as a handle for the calls that reach the host.
unsafe fn rt(ctx: *mut ThreadCtx) -> Rt {
    // SAFETY: as `runtime`; while a thread runs, the scheduler holds no reference to the runtime.
    unsafe { Rt::new(inner(ctx).runtime) }
}

unsafe fn text<'a>(ptr: *const u8, len: i64) -> &'a str {
    if ptr.is_null() || len <= 0 {
        return "";
    }
    // SAFETY: generated code passes pointers into its own constant data.
    let bytes = unsafe { slice::from_raw_parts(ptr, len as usize) };
    std::str::from_utf8(bytes).unwrap_or("")
}

fn temp(inner: &mut ThreadInner, s: String) -> Handle {
    let rc = Rc::new(s);
    let h = strings::handle_of(&rc);
    inner.temps.push(rc);
    h
}

unsafe fn str_arg<'a>(h: Handle) -> &'a str {
    // SAFETY: generated code passes live handles.
    unsafe { strings::str_of(h) }
}

unsafe fn value_arg<'a>(p: *const Value) -> ValueRef<'a> {
    // SAFETY: generated code passes pointers to live values.
    unsafe { value::as_ref(p) }
}

// ---- registration -----------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_add_const(rt: *mut Runtime, ptr: *const u8, len: i64) {
    // SAFETY: called from the program's init function with the runtime it was given.
    unsafe { (*rt).add_const(text(ptr, len).to_owned()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_add_target(rt: *mut Runtime, name: *const u8, len: i64, is_stage: i8) -> i64 {
    // SAFETY: as `thatch_add_const`.
    unsafe { (*rt).add_target(text(name, len).to_owned(), is_stage != 0) as i64 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_add_num_var(rt: *mut Runtime, target: i64, name: *const u8, len: i64, initial: f64) {
    // SAFETY: as `thatch_add_const`.
    unsafe { (*rt).add_num_var(target as usize, text(name, len).to_owned(), initial) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_add_any_var(
    rt: *mut Runtime,
    target: i64,
    name: *const u8,
    len: i64,
    tag: i64,
    bits: i64,
) {
    // SAFETY: as `thatch_add_const`.
    unsafe { (*rt).add_any_var(target as usize, text(name, len).to_owned(), tag as u64, bits as u64) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_add_script(
    rt: *mut Runtime,
    target: i64,
    hat: i64,
    arg: *const u8,
    arg_len: i64,
    func: i64,
) {
    // SAFETY: as `thatch_add_const`; `func` is the address of a compiled script.
    unsafe {
        let func: ScriptFn = std::mem::transmute::<usize, ScriptFn>(func as usize);
        (*rt).add_script(target as usize, hat, text(arg, arg_len).to_owned(), func);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_add_costume(rt: *mut Runtime, target: i64, name: *const u8, len: i64) {
    // SAFETY: as `thatch_add_const`.
    unsafe { (*rt).add_costume(target as usize, text(name, len).to_owned()) }
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn thatch_set_sprite(
    rt: *mut Runtime,
    target: i64,
    x: f64,
    y: f64,
    direction: f64,
    size: f64,
    costume: i64,
    visible: i8,
    rotation_style: i64,
    draggable: i8,
    volume: f64,
) {
    // SAFETY: as `thatch_add_const`.
    unsafe {
        (*rt).set_sprite(
            target as usize,
            x,
            y,
            direction,
            size,
            costume,
            visible != 0,
            rotation_style,
            draggable != 0,
            volume,
        );
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_main(argc: i32, argv: *const *const c_char, init: i64, abi_version: i32) -> i32 {
    // SAFETY: the program's `main` passes its own arguments and init function.
    unsafe {
        let init = std::mem::transmute::<usize, crate::InitFn>(init as usize);
        crate::host::main(argc, argv, init, abi_version as u32)
    }
}

// ---- strings ----------------------------------------------------------------------------------

/// Drops the strings made during the current statement. Strings a caller made before calling a
/// custom block stay, since the callee's parameters may point at them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_temps_clear(ctx: *mut ThreadCtx) {
    // SAFETY: generated code passes its own thread context.
    unsafe {
        let inner = inner(ctx);
        let mark = inner.temp_marks.last().copied().unwrap_or(0);
        inner.temps.truncate(mark);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_num_to_str(ctx: *mut ThreadCtx, n: f64) -> Handle {
    // SAFETY: as `thatch_temps_clear`.
    unsafe { temp(inner(ctx), thatch_values::number_to_string(n)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_str_to_num(h: Handle) -> f64 {
    // SAFETY: generated code passes live handles.
    unsafe { thatch_values::str_to_number(str_arg(h)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_str_to_bool(h: Handle) -> i8 {
    // SAFETY: as `thatch_str_to_num`.
    unsafe { i8::from(thatch_values::str_to_boolean(str_arg(h))) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_bool_to_str(ctx: *mut ThreadCtx, b: i8) -> Handle {
    // SAFETY: as `thatch_temps_clear`.
    unsafe { temp(inner(ctx), thatch_values::bool_to_string(b != 0).to_owned()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_join(ctx: *mut ThreadCtx, a: Handle, b: Handle) -> Handle {
    // SAFETY: as `thatch_str_to_num`.
    unsafe {
        let mut s = String::with_capacity(str_arg(a).len() + str_arg(b).len());
        s.push_str(str_arg(a));
        s.push_str(str_arg(b));
        temp(inner(ctx), s)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_letter_of(ctx: *mut ThreadCtx, index: f64, s: Handle) -> Handle {
    // SAFETY: as `thatch_str_to_num`.
    unsafe { temp(inner(ctx), thatch_values::letter_of(index, str_arg(s))) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_length(s: Handle) -> f64 {
    // SAFETY: as `thatch_str_to_num`.
    unsafe { thatch_values::length(str_arg(s)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_contains(s: Handle, part: Handle) -> i8 {
    // SAFETY: as `thatch_str_to_num`.
    unsafe { i8::from(thatch_values::contains(str_arg(s), str_arg(part))) }
}

// ---- dynamically typed values -----------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_any_to_num(p: *const Value) -> f64 {
    // SAFETY: generated code passes pointers to live values.
    unsafe { thatch_values::to_number(value_arg(p)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_any_to_str(ctx: *mut ThreadCtx, p: *const Value) -> Handle {
    // SAFETY: as `thatch_any_to_num`.
    unsafe {
        let v = *p;
        if v.tag == thatch_abi::TAG_STR {
            // Keep the string alive for the statement even if the variable is reassigned.
            let rc = strings::retain(v.bits as Handle);
            let h = strings::handle_of(&rc);
            inner(ctx).temps.push(rc);
            return h;
        }
        temp(inner(ctx), thatch_values::to_string(value_arg(p)))
    }
}

/// Copies `*src` into `*dst`, a slot on the caller's stack. A string is kept alive in the thread's
/// temporaries, which a custom block call protects until it returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_any_snapshot(ctx: *mut ThreadCtx, dst: *mut Value, src: *const Value) {
    // SAFETY: generated code passes its own context, a writable stack slot and a live value.
    unsafe {
        let v = *src;
        if v.tag == thatch_abi::TAG_STR {
            let rc = strings::retain(v.bits as Handle);
            inner(ctx).temps.push(rc);
        }
        *dst = v;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_any_to_bool(p: *const Value) -> i8 {
    // SAFETY: as `thatch_any_to_num`.
    unsafe { i8::from(thatch_values::to_boolean(value_arg(p))) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_var_set(slot: *mut Value, src: *const Value) {
    // SAFETY: generated code passes a variable slot and a live value.
    unsafe { VarSlot::copy_from(slot, src) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_var_set_num(slot: *mut Value, n: f64) {
    // SAFETY: as `thatch_var_set`.
    unsafe { VarSlot::replace(slot, value::num(n)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_var_set_str(slot: *mut Value, h: Handle) {
    // SAFETY: as `thatch_var_set`.
    unsafe { VarSlot::set_str(slot, h) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_var_set_bool(slot: *mut Value, b: i8) {
    // SAFETY: as `thatch_var_set`.
    unsafe { VarSlot::replace(slot, value::boolean(b != 0)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_var_change(slot: *mut Value, delta: f64) {
    // SAFETY: as `thatch_var_set`.
    unsafe {
        let n = thatch_values::to_number(value_arg(slot)) + delta;
        VarSlot::replace(slot, value::num(n));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_compare(a: *const Value, b: *const Value) -> f64 {
    // SAFETY: as `thatch_any_to_num`.
    unsafe { thatch_values::compare(value_arg(a), value_arg(b)) }
}

#[unsafe(no_mangle)]
pub extern "C" fn thatch_compare_nums(a: f64, b: f64) -> f64 {
    thatch_values::compare(ValueRef::Num(a), ValueRef::Num(b))
}

// ---- arithmetic -------------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn thatch_mod(a: f64, b: f64) -> f64 {
    thatch_values::modulo(a, b)
}

#[unsafe(no_mangle)]
pub extern "C" fn thatch_round(n: f64) -> f64 {
    thatch_values::round(n)
}

/// `op` indexes `thatch_ir::MATH_OPS`.
#[unsafe(no_mangle)]
pub extern "C" fn thatch_mathop(op: i64, n: f64) -> f64 {
    const OPS: [MathOp; 10] = [
        MathOp::Sin,
        MathOp::Cos,
        MathOp::Tan,
        MathOp::Asin,
        MathOp::Acos,
        MathOp::Atan,
        MathOp::Ln,
        MathOp::Log,
        MathOp::Exp,
        MathOp::Pow10,
    ];
    match OPS.get(op as usize) {
        Some(op) => op.apply(n),
        None => f64::NAN,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_random(ctx: *mut ThreadCtx, from: *const Value, to: *const Value) -> f64 {
    // SAFETY: as `thatch_any_to_num`.
    unsafe {
        let r = runtime(ctx).random();
        thatch_values::random(value_arg(from), value_arg(to), r)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_timer(ctx: *mut ThreadCtx) -> f64 {
    // SAFETY: as `thatch_temps_clear`.
    unsafe { rt(ctx).timer() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_reset_timer(ctx: *mut ThreadCtx) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe { rt(ctx).reset_timer() }
}

// ---- control ----------------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_yield(ctx: *mut ThreadCtx) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe { inner(ctx).loop_yield() }
}

/// Waits until `end_ms` on the host's clock, yielding `how` each time. Returns false when the
/// thread was killed meanwhile.
unsafe fn wait_until(ctx: *mut ThreadCtx, end_ms: f64, how: Yield) -> bool {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        loop {
            inner(ctx).suspend(how);
            if inner(ctx).killed() {
                return false;
            }
            if rt(ctx).now_ms() >= end_ms {
                return true;
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_wait(ctx: *mut ThreadCtx, secs: f64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let duration = (1000.0 * thatch_values::num_to_number(secs)).max(0.0);
        let end = rt(ctx).now_ms() + duration;
        runtime(ctx).request_redraw();
        // scratch-vm's `wait` yields (not for a tick), so it is stepped again in every pass of
        // the frame and can end as soon as the clock allows.
        wait_until(ctx, end, Yield::Wait);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_say(ctx: *mut ThreadCtx, message: *const Value, think: i8) {
    // SAFETY: as `thatch_any_to_num`.
    unsafe {
        let text = bubble_text(message);
        let target = inner(ctx).target;
        rt(ctx).say(target, text, think != 0);
    }
}

/// The text of a bubble: numbers are formatted as scratch-vm's `_formatBubbleText` shows them.
unsafe fn bubble_text(message: *const Value) -> String {
    // SAFETY: forwarded from the caller.
    let v = unsafe { *message };
    if v.tag == TAG_NUM {
        return thatch_values::bubble_number(f64::from_bits(v.bits));
    }
    // SAFETY: forwarded from the caller.
    unsafe { thatch_values::to_string(value_arg(message)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_say_for(ctx: *mut ThreadCtx, message: *const Value, secs: f64, think: i8) {
    // SAFETY: as `thatch_any_to_num`.
    unsafe {
        let text = bubble_text(message);
        let target = inner(ctx).target;
        let token = rt(ctx).say(target, text, think != 0);
        let duration = (1000.0 * thatch_values::num_to_number(secs)).max(0.0);
        let end = rt(ctx).now_ms() + duration;
        // scratch-vm waits on a promise that a timer resolves between frames.
        if !wait_until(ctx, end, Yield::Tick) {
            return;
        }
        if runtime(ctx).bubble_token(target) == token {
            rt(ctx).say(target, String::new(), think != 0);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_broadcast(ctx: *mut ThreadCtx, name: Handle) {
    // SAFETY: as `thatch_str_to_num`.
    unsafe {
        let name = str_arg(name).to_owned();
        runtime(ctx).start_hats(thatch_abi::HAT_BROADCAST, &name);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_broadcast_wait(ctx: *mut ThreadCtx, name: Handle) {
    // SAFETY: as `thatch_str_to_num`.
    unsafe {
        let name = str_arg(name).to_owned();
        let started = runtime(ctx).start_hats(thatch_abi::HAT_BROADCAST, &name);
        loop {
            let statuses: Vec<_> = started.iter().filter_map(|id| runtime(ctx).thread_status(*id)).collect();
            let waiting: Vec<_> = statuses.iter().filter(|s| **s != crate::thread::Status::Done).collect();
            if waiting.is_empty() {
                return;
            }
            let all_tick = waiting.iter().all(|s| **s == crate::thread::Status::TickYield);
            inner(ctx).suspend(if all_tick { Yield::Tick } else { Yield::Wait });
            if inner(ctx).killed() {
                return;
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_stop_all(ctx: *mut ThreadCtx) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe { rt(ctx).stop_all() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_stop_others(ctx: *mut ThreadCtx) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let (target, id) = (inner(ctx).target, inner(ctx).id);
        runtime(ctx).stop_others(target, id);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_proc_enter(ctx: *mut ThreadCtx, proc: i64, nesting: i64, warp: i8) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let inner = inner(ctx);
        let recursive = inner.is_recursive(proc, nesting);
        inner.calls.push(Frame { proc, nesting });
        inner.temp_marks.push(inner.temps.len());
        if warp != 0 {
            inner.enter_warp();
        } else if recursive && inner.warp_depth == 0 {
            inner.suspend(Yield::Loop);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_proc_exit(ctx: *mut ThreadCtx, warp: i8) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let inner = inner(ctx);
        inner.calls.pop();
        inner.temp_marks.pop();
        if warp != 0 {
            inner.exit_warp();
        }
    }
}

// ---- motion -----------------------------------------------------------------------------------

/// The index of the thread's target.
unsafe fn target_of(ctx: *mut ThreadCtx) -> usize {
    // SAFETY: as `thatch_temps_clear`.
    unsafe { inner(ctx).target }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_move_steps(ctx: *mut ThreadCtx, steps: f64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).move_steps(target, steps);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_go_to_xy(ctx: *mut ThreadCtx, x: f64, y: f64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).set_xy(target, x, y);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_go_to(ctx: *mut ThreadCtx, name: Handle) {
    // SAFETY: as `thatch_str_to_num`.
    unsafe {
        let target = target_of(ctx);
        let name = str_arg(name).to_owned();
        runtime(ctx).go_to(target, &name);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_set_x(ctx: *mut ThreadCtx, x: f64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).set_x(target, x);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_set_y(ctx: *mut ThreadCtx, y: f64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).set_y(target, y);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_change_x(ctx: *mut ThreadCtx, dx: f64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).change_x(target, dx);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_change_y(ctx: *mut ThreadCtx, dy: f64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).change_y(target, dy);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_turn(ctx: *mut ThreadCtx, degrees: f64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        let direction = runtime(ctx).targets[target].direction + degrees;
        runtime(ctx).set_direction(target, direction);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_point_in_direction(ctx: *mut ThreadCtx, direction: f64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).set_direction(target, direction);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_point_towards(ctx: *mut ThreadCtx, name: Handle) {
    // SAFETY: as `thatch_str_to_num`.
    unsafe {
        let target = target_of(ctx);
        let name = str_arg(name).to_owned();
        runtime(ctx).point_towards(target, &name);
    }
}

/// `glide`: moves a little every time the thread is stepped until the time is up, then lands on
/// the end point. A duration of 0 or less lands at once without yielding.
unsafe fn glide(ctx: *mut ThreadCtx, secs: f64, end_x: f64, end_y: f64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        let duration_ms = secs * 1000.0;
        if secs <= 0.0 {
            runtime(ctx).set_xy(target, end_x, end_y);
            return;
        }
        let (start_x, start_y) = (runtime(ctx).targets[target].x, runtime(ctx).targets[target].y);
        let started = rt(ctx).now_ms();
        loop {
            inner(ctx).suspend(Yield::Wait);
            if inner(ctx).killed() {
                return;
            }
            let elapsed = rt(ctx).now_ms() - started;
            if elapsed < duration_ms {
                let frac = elapsed / duration_ms;
                let x = start_x + frac * (end_x - start_x);
                let y = start_y + frac * (end_y - start_y);
                runtime(ctx).set_xy(target, x, y);
            } else {
                runtime(ctx).set_xy(target, end_x, end_y);
                return;
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_glide(ctx: *mut ThreadCtx, secs: f64, x: f64, y: f64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe { glide(ctx, secs, x, y) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_glide_to(ctx: *mut ThreadCtx, secs: f64, name: Handle) {
    // SAFETY: as `thatch_str_to_num`.
    unsafe {
        let name = str_arg(name).to_owned();
        if let Some((x, y)) = runtime(ctx).target_xy(&name) {
            glide(ctx, secs, x, y);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_bounce(ctx: *mut ThreadCtx) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).bounce(target);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_set_rotation_style(ctx: *mut ThreadCtx, style: i64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).set_rotation_style(target, style);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_sprite_x(ctx: *mut ThreadCtx) -> f64 {
    // SAFETY: as `thatch_temps_clear`.
    unsafe { thatch_values::limit_precision(runtime(ctx).targets[target_of(ctx)].x) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_sprite_y(ctx: *mut ThreadCtx) -> f64 {
    // SAFETY: as `thatch_temps_clear`.
    unsafe { thatch_values::limit_precision(runtime(ctx).targets[target_of(ctx)].y) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_sprite_direction(ctx: *mut ThreadCtx) -> f64 {
    // SAFETY: as `thatch_temps_clear`.
    unsafe { runtime(ctx).targets[target_of(ctx)].direction }
}

// ---- looks ------------------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_set_visible(ctx: *mut ThreadCtx, visible: i8) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).set_visible(target, visible != 0);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_switch_costume(ctx: *mut ThreadCtx, costume: *const Value) {
    // SAFETY: as `thatch_any_to_num`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).switch_costume(target, value_arg(costume), false, false);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_next_costume(ctx: *mut ThreadCtx) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        let next = runtime(ctx).targets[target].costume as f64 + 1.0;
        runtime(ctx).switch_costume(target, ValueRef::Num(next), false, true);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_switch_backdrop(ctx: *mut ThreadCtx, backdrop: *const Value) {
    // SAFETY: as `thatch_any_to_num`.
    unsafe {
        let stage = runtime(ctx).stage_index();
        runtime(ctx).switch_costume(stage, value_arg(backdrop), true, false);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_next_backdrop(ctx: *mut ThreadCtx) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let stage = runtime(ctx).stage_index();
        let next = runtime(ctx).targets[stage].costume as f64 + 1.0;
        runtime(ctx).switch_costume(stage, ValueRef::Num(next), true, true);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_set_size(ctx: *mut ThreadCtx, size: f64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).set_size(target, size);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_change_size(ctx: *mut ThreadCtx, change: f64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        let size = runtime(ctx).targets[target].size + change;
        runtime(ctx).set_size(target, size);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_set_effect(ctx: *mut ThreadCtx, effect: i64, value: f64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).set_effect(target, usize::try_from(effect).unwrap_or(usize::MAX), value);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_change_effect(ctx: *mut ThreadCtx, effect: i64, change: f64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).change_effect(target, usize::try_from(effect).unwrap_or(usize::MAX), change);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_clear_effects(ctx: *mut ThreadCtx) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).clear_effects(target);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_go_to_layer(ctx: *mut ThreadCtx, front: i8) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).go_to_layer(target, front != 0);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_go_layers(ctx: *mut ThreadCtx, layers: f64) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = target_of(ctx);
        runtime(ctx).move_layer(target, layers);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_sprite_size(ctx: *mut ThreadCtx) -> f64 {
    // SAFETY: as `thatch_temps_clear`.
    unsafe { thatch_values::round(runtime(ctx).targets[target_of(ctx)].size) }
}

/// The target a costume reporter asks about: the stage for backdrops, else the thread's own.
unsafe fn costume_target(ctx: *mut ThreadCtx, backdrop: i8) -> usize {
    // SAFETY: as `thatch_temps_clear`.
    unsafe { if backdrop != 0 { runtime(ctx).stage_index() } else { target_of(ctx) } }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_costume_number(ctx: *mut ThreadCtx, backdrop: i8) -> f64 {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = costume_target(ctx, backdrop);
        runtime(ctx).targets[target].costume as f64 + 1.0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_costume_name(ctx: *mut ThreadCtx, backdrop: i8) -> Handle {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        let target = costume_target(ctx, backdrop);
        let name = runtime(ctx).costume_name(target);
        let h = strings::handle_of(&name);
        inner(ctx).temps.push(name);
        h
    }
}

/// `property of object`: writes the value into `out`, a slot on the caller's stack, keeping any
/// string alive for the statement.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_property_of(ctx: *mut ThreadCtx, out: *mut Value, property: Handle, object: Handle) {
    // SAFETY: as `thatch_str_to_num`; `out` is a writable slot of the running thread.
    unsafe {
        let value = runtime(ctx).property_of(str_arg(property), str_arg(object));
        *out = match value {
            PropertyValue::Num(n) => value::num(n),
            PropertyValue::Str(s) => {
                let h = strings::handle_of(&s);
                inner(ctx).temps.push(s);
                Value { tag: TAG_STR, bits: h as u64 }
            }
            PropertyValue::Var(v) => {
                if v.tag == TAG_STR {
                    inner(ctx).temps.push(strings::retain(v.bits as Handle));
                }
                v
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn init_stage(rt: *mut OpaqueRuntime) {
        // SAFETY: the runtime passes itself.
        unsafe { thatch_add_target(rt.cast::<Runtime>(), c"Stage".as_ptr().cast::<u8>(), 5, 1) };
    }

    unsafe extern "C" fn init_sprite_only(rt: *mut OpaqueRuntime) {
        // SAFETY: as `init_stage`.
        unsafe { thatch_add_target(rt.cast::<Runtime>(), c"Cat".as_ptr().cast::<u8>(), 3, 0) };
    }

    #[test]
    fn hosts_of_another_abi_version_are_refused() {
        // SAFETY: no host table is read for a wrong version.
        let rt = unsafe { thatch_runtime_create(thatch_abi::ABI_VERSION as i32 + 1, std::ptr::null(), 1) };
        assert!(rt.is_null());
    }

    #[test]
    fn a_runtime_loads_one_program_and_a_failed_load_leaves_it_empty() {
        let version = thatch_abi::ABI_VERSION as i32;
        // SAFETY: a runtime without callbacks, used and destroyed here.
        unsafe {
            let rt = thatch_runtime_create(version, std::ptr::null(), 1);
            assert_eq!(thatch_runtime_load(rt, version, init_sprite_only), thatch_abi::LOAD_NO_STAGE);
            assert_eq!(thatch_runtime_target_count(rt), 0);
            assert_eq!(thatch_runtime_load(rt, version, init_stage), thatch_abi::LOAD_OK);
            assert_eq!(thatch_runtime_target_count(rt), 1);
            assert_eq!(thatch_runtime_redraw_requested(rt), 1, "nothing is drawn yet after a load");
            assert_eq!(thatch_runtime_load(rt, version, init_stage), thatch_abi::LOAD_ALREADY_LOADED);
            assert_eq!(thatch_runtime_target_count(rt), 1);
            thatch_runtime_destroy(rt);
        }
    }

    #[test]
    fn every_abi_function_has_a_symbol() {
        let symbols = symbols();
        for f in thatch_abi::RT_FNS {
            assert!(symbols.iter().any(|(name, _)| *name == f.name), "missing symbol {}", f.name);
        }
        assert_eq!(symbols.len(), thatch_abi::RT_FNS.len());
    }

    #[test]
    fn every_host_api_function_has_a_symbol() {
        let symbols = host_symbols();
        for f in thatch_abi::HOST_API {
            assert!(symbols.iter().any(|(name, _)| *name == f.name), "missing symbol {}", f.name);
        }
        assert_eq!(symbols.len(), thatch_abi::HOST_API.len());
    }
}

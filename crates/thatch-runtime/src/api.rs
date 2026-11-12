//! The C ABI surface: the `thatch_*` functions generated code calls. Signatures match
//! `thatch_abi::RT_FNS`, which a test below checks.

use std::ffi::c_char;
use std::rc::Rc;
use std::slice;

use thatch_abi::{TAG_NUM, ThreadCtx, Value};
use thatch_values::{MathOp, ValueRef};

use crate::scheduler::Runtime;
use crate::strings::{self, Handle};
use crate::thread::{Frame, ScriptFn, ThreadInner, Yield};
use crate::value::{self, VarSlot};

/// The runtime functions by symbol, for JIT lookup.
pub fn symbols() -> Vec<(&'static str, *const u8)> {
    macro_rules! table {
        ($($f:ident),* $(,)?) => { vec![$((stringify!($f), $f as *const u8)),*] };
    }
    table![
        thatch_add_const,
        thatch_add_target,
        thatch_add_num_var,
        thatch_add_any_var,
        thatch_add_script,
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
    ]
}

// ---- helpers ----------------------------------------------------------------------------------

unsafe fn inner<'a>(ctx: *mut ThreadCtx) -> &'a mut ThreadInner {
    // SAFETY: generated code only passes the context of the running thread, whose inner part lives
    // as long as the thread.
    unsafe { &mut *(*ctx).inner.cast::<ThreadInner>() }
}

unsafe fn runtime<'a>(ctx: *mut ThreadCtx) -> &'a mut Runtime {
    // SAFETY: as `inner`; the runtime outlives every thread, and scripts run one at a time.
    unsafe { &mut *inner(ctx).runtime }
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
    unsafe { runtime(ctx).timer() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_reset_timer(ctx: *mut ThreadCtx) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe { runtime(ctx).reset_timer() }
}

// ---- control ----------------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_yield(ctx: *mut ThreadCtx) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe { inner(ctx).loop_yield() }
}

/// Waits for the next frame until `end_ms` on the virtual clock. Returns false when the thread was
/// killed meanwhile.
unsafe fn wait_until(ctx: *mut ThreadCtx, end_ms: f64) -> bool {
    // SAFETY: as `thatch_temps_clear`.
    unsafe {
        loop {
            inner(ctx).suspend(Yield::Tick);
            if inner(ctx).killed() {
                return false;
            }
            if runtime(ctx).now_ms() >= end_ms {
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
        let end = runtime(ctx).now_ms() + duration;
        runtime(ctx).request_redraw();
        wait_until(ctx, end);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_say(ctx: *mut ThreadCtx, message: *const Value, think: i8) {
    // SAFETY: as `thatch_any_to_num`.
    unsafe {
        let text = bubble_text(message);
        let target = inner(ctx).target;
        runtime(ctx).say(target, text, think != 0);
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
        let token = runtime(ctx).say(target, text, think != 0);
        let duration = (1000.0 * thatch_values::num_to_number(secs)).max(0.0);
        let end = runtime(ctx).now_ms() + duration;
        if !wait_until(ctx, end) {
            return;
        }
        if runtime(ctx).bubble_token(target) == token {
            runtime(ctx).say(target, String::new(), think != 0);
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
            inner(ctx).suspend(if all_tick { Yield::Tick } else { Yield::Loop });
            if inner(ctx).killed() {
                return;
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn thatch_stop_all(ctx: *mut ThreadCtx) {
    // SAFETY: as `thatch_temps_clear`.
    unsafe { runtime(ctx).stop_all() }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_abi_function_has_a_symbol() {
        let symbols = symbols();
        for f in thatch_abi::RT_FNS {
            assert!(symbols.iter().any(|(name, _)| *name == f.name), "missing symbol {}", f.name);
        }
        assert_eq!(symbols.len(), thatch_abi::RT_FNS.len());
    }
}

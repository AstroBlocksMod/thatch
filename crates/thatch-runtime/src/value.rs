//! Dynamically typed values in variable storage and on the stack.
//!
//! A [`Value`] in a variable owns its string (see [`strings::into_raw`]). A `Value` that generated
//! code builds on its stack to pass to the runtime only borrows the string from the thread's
//! temporaries. Both are read the same way; only [`VarSlot`] writes know about ownership.

use thatch_abi::{TAG_BOOL, TAG_NUM, TAG_STR, Value};
use thatch_values::ValueRef;

use crate::strings::{self, Handle};

/// Reads a value as a borrowed [`ValueRef`].
///
/// # Safety
/// `v` must point at a `Value` whose string handle, if any, is alive.
pub unsafe fn as_ref<'a>(v: *const Value) -> ValueRef<'a> {
    // SAFETY: the caller promises a valid pointer.
    let v = unsafe { &*v };
    match v.tag {
        TAG_NUM => ValueRef::Num(f64::from_bits(v.bits)),
        // SAFETY: a string value carries a live handle, by the contract of this function.
        TAG_STR => ValueRef::Str(unsafe { strings::str_of(v.bits as Handle) }),
        TAG_BOOL => ValueRef::Bool(v.bits != 0),
        _ => ValueRef::Num(0.0),
    }
}

pub fn num(n: f64) -> Value {
    Value { tag: TAG_NUM, bits: n.to_bits() }
}

pub fn boolean(b: bool) -> Value {
    Value { tag: TAG_BOOL, bits: u64::from(b) }
}

/// A variable slot: a `Value` that owns its string.
pub struct VarSlot;

impl VarSlot {
    /// Replaces the slot's value, releasing the string it held.
    ///
    /// # Safety
    /// `slot` must point at a variable's `Value`, and `new` must own its string (see
    /// [`strings::into_raw`]).
    pub unsafe fn replace(slot: *mut Value, new: Value) {
        // SAFETY: the caller promises `slot` is a live variable slot.
        let old = unsafe { std::mem::replace(&mut *slot, new) };
        if old.tag == TAG_STR {
            // SAFETY: a variable slot owns its string.
            unsafe { strings::release(old.bits as Handle) };
        }
    }

    /// Stores a copy of `src`, taking a new reference to its string.
    ///
    /// # Safety
    /// As [`VarSlot::replace`], and `src` must point at a readable `Value`.
    pub unsafe fn copy_from(slot: *mut Value, src: *const Value) {
        // SAFETY: the caller promises `src` is readable.
        let v = unsafe { *src };
        let owned = if v.tag == TAG_STR {
            // SAFETY: a string value carries a live handle.
            let rc = unsafe { strings::retain(v.bits as Handle) };
            Value { tag: TAG_STR, bits: strings::into_raw(rc) as u64 }
        } else {
            v
        };
        // SAFETY: forwarded from the caller.
        unsafe { Self::replace(slot, owned) };
    }

    /// Stores a string, taking a new reference to it.
    ///
    /// # Safety
    /// As [`VarSlot::replace`], and `h` must be a live handle.
    pub unsafe fn set_str(slot: *mut Value, h: Handle) {
        // SAFETY: forwarded from the caller.
        let rc = unsafe { strings::retain(h) };
        // SAFETY: forwarded from the caller.
        unsafe { Self::replace(slot, Value { tag: TAG_STR, bits: strings::into_raw(rc) as u64 }) };
    }
}

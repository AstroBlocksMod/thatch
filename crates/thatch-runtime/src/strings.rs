//! String handles.
//!
//! Generated code holds strings as opaque handles: pointers to reference-counted Rust strings. A
//! handle that a runtime function returns is owned by the running thread's temporaries, which the
//! generated code clears at the end of each statement. Variables keep their own reference, so a
//! string survives in a variable after the temporaries that produced it are gone.

use std::ffi::c_void;
use std::rc::Rc;

/// A borrowed handle to a string, as generated code passes it.
pub type Handle = *const c_void;

/// Reads the string behind a handle.
///
/// # Safety
/// `h` must come from [`handle_of`] on a string that is still alive.
pub unsafe fn str_of<'a>(h: Handle) -> &'a str {
    // SAFETY: the caller promises the handle points at a live `Rc<String>` allocation.
    unsafe { (*h.cast::<String>()).as_str() }
}

/// The handle of a string, borrowed from its `Rc`.
pub fn handle_of(s: &Rc<String>) -> Handle {
    Rc::as_ptr(s).cast::<c_void>()
}

/// Takes a new owning reference to the string behind a handle.
///
/// # Safety
/// `h` must come from [`handle_of`] on a string that is still alive.
pub unsafe fn retain(h: Handle) -> Rc<String> {
    let p = h.cast::<String>();
    // SAFETY: the handle came from `Rc::as_ptr`, so the pointer is a valid `Rc` allocation.
    unsafe {
        Rc::increment_strong_count(p);
        Rc::from_raw(p)
    }
}

/// Turns an owned reference into a raw handle that keeps the string alive until [`release`].
pub fn into_raw(s: Rc<String>) -> Handle {
    Rc::into_raw(s).cast::<c_void>()
}

/// Drops the reference that [`into_raw`] leaked.
///
/// # Safety
/// `h` must come from [`into_raw`] and not have been released before.
pub unsafe fn release(h: Handle) {
    // SAFETY: the caller promises the handle holds a reference from `into_raw`.
    unsafe { drop(Rc::from_raw(h.cast::<String>())) }
}

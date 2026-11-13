//! Threads: one running script each, on its own stack.
//!
//! Scratch scripts are cooperative green threads. A script runs until the end of a loop iteration,
//! a wait or something else that yields, then the next script runs. Each thread here is a stackful
//! coroutine, so generated code is plain code: a `forever` loop is a loop that calls `thatch_yield`
//! at the end of each iteration, and a custom block is a function call that may yield from any depth.
//!
//! A thread is never torn down from outside while it is suspended, because that would mean
//! unwinding through generated frames. When a thread is stopped it is marked killed and resumed one
//! more time: every yield point in the generated code and in the runtime checks the flag and returns,
//! so the coroutine finishes on its own.

use std::ffi::c_void;
use std::ptr;
use std::rc::Rc;
use std::time::Instant;

use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, Yielder};
use thatch_abi::ThreadCtx;

use crate::scheduler::Runtime;

/// The stack size of a thread. Mapped lazily, so only the pages a script uses cost memory.
const STACK_SIZE: usize = 4 << 20;

/// How long "run without screen refresh" may hog a frame, as in scratch-vm.
const WARP_TIME_MS: u128 = 500;

/// What a thread tells the scheduler when it suspends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Yield {
    /// The end of a loop iteration: the thread is still active and may be stepped again in this
    /// frame.
    Loop,
    /// Waiting for something, such as a timer or other threads (scratch-vm's `STATUS_YIELD`): the
    /// thread is stepped again in every pass of the frame that other active threads keep going,
    /// but does not keep the frame going itself.
    Wait,
    /// Waiting for the next frame.
    Tick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Not started yet.
    Fresh,
    /// Stepped in this frame and still active.
    Running,
    /// Stepped in this frame and waiting (see [`Yield::Wait`]).
    Waiting,
    /// Waiting for the next frame.
    TickYield,
    Done,
}

/// The entry point of a compiled script.
pub type ScriptFn = unsafe extern "C" fn(*mut ThreadCtx);

/// A procedure call that is in progress, for scratch-vm's recursion rule.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub proc: i64,
    /// How many loops and conditionals enclose the call site in its function.
    pub nesting: i64,
}

/// The runtime's private part of a thread, behind `ThreadCtx::inner`.
pub struct ThreadInner {
    pub runtime: *mut Runtime,
    pub yielder: *const Yielder<(), Yield>,
    pub ctx: *mut ThreadCtx,
    pub id: u64,
    pub target: usize,
    /// Strings made by runtime calls during the current statement.
    pub temps: Vec<Rc<String>>,
    /// The length of `temps` when each active procedure call began; clearing stops there.
    pub temp_marks: Vec<usize>,
    pub warp_depth: u32,
    pub warp_started: Instant,
    pub warp_yields: u32,
    pub calls: Vec<Frame>,
}

pub struct Thread {
    pub id: u64,
    pub target: usize,
    pub script: usize,
    pub status: Status,
    pub killed: bool,
    /// Set by a `stop all` from a thread earlier in the current pass: scratch-vm still steps the
    /// threads after it once more before dropping them, so this one is killed after its step.
    pub stop_after_step: bool,
    pub ctx: Box<ThreadCtx>,
    pub inner: Box<ThreadInner>,
    func: ScriptFn,
    /// Taken out while the thread runs, so it cannot move with the thread list.
    coroutine: Option<Coroutine<(), Yield, (), DefaultStack>>,
}

impl Thread {
    pub fn new(runtime: *mut Runtime, id: u64, target: usize, script: usize, func: ScriptFn, ctx: ThreadCtx) -> Self {
        let mut ctx = Box::new(ctx);
        let inner = Box::new(ThreadInner {
            runtime,
            yielder: ptr::null(),
            ctx: ptr::addr_of_mut!(*ctx),
            id,
            target,
            temps: Vec::new(),
            temp_marks: Vec::new(),
            warp_depth: 0,
            warp_started: Instant::now(),
            warp_yields: 0,
            calls: Vec::new(),
        });
        ctx.inner = ptr::addr_of!(*inner).cast_mut().cast::<c_void>();
        Self {
            id,
            target,
            script,
            status: Status::Fresh,
            killed: false,
            stop_after_step: false,
            ctx,
            inner,
            func,
            coroutine: None,
        }
    }

    /// Marks the thread stopped. It finishes the next time it is stepped.
    pub fn kill(&mut self) {
        self.killed = true;
        self.ctx.killed = 1;
    }

    /// Whether the thread is suspended in the middle of its script.
    pub fn suspended(&self) -> bool {
        self.coroutine.as_ref().is_some_and(|c| c.started() && !c.done())
    }

    /// Runs the thread until it yields or finishes. The coroutine is taken out of the thread for the
    /// duration, since the script may start or restart threads and move the list.
    pub fn take_coroutine(&mut self) -> Result<Coroutine<(), Yield, (), DefaultStack>, String> {
        if let Some(c) = self.coroutine.take() {
            return Ok(c);
        }
        let stack = DefaultStack::new(STACK_SIZE).map_err(|e| format!("could not allocate a thread stack: {e}"))?;
        let inner: *mut ThreadInner = ptr::addr_of_mut!(*self.inner);
        let ctx: *mut ThreadCtx = ptr::addr_of_mut!(*self.ctx);
        let func = self.func;
        Ok(Coroutine::with_stack(stack, move |yielder: &Yielder<(), Yield>, ()| {
            // SAFETY: the thread outlives its coroutine, and only this coroutine touches `inner`
            // while it runs.
            unsafe {
                (*inner).yielder = yielder;
                func(ctx);
            }
        }))
    }

    pub fn put_coroutine(&mut self, c: Coroutine<(), Yield, (), DefaultStack>) {
        self.coroutine = Some(c);
    }

    /// Resumes a suspended coroutine so it can finish, when the thread is killed.
    pub fn finish(&mut self) {
        let Some(mut c) = self.coroutine.take() else { return };
        if !c.started() {
            return;
        }
        for _ in 0..8 {
            if c.done() {
                return;
            }
            let _ = c.resume(());
        }
        if !c.done() {
            // The script keeps yielding after being killed, which generated code never does. Leak
            // the coroutine rather than unwind through its frames.
            std::mem::forget(c);
        }
    }
}

impl Drop for Thread {
    /// Dropping a suspended coroutine would unwind its stack through generated frames, which
    /// cannot unwind. The scheduler finishes threads before it drops them; any thread that reaches
    /// this point still suspended (after an error, say) leaks its stack instead.
    fn drop(&mut self) {
        if let Some(c) = self.coroutine.take()
            && c.started()
            && !c.done()
        {
            std::mem::forget(c);
        }
    }
}

impl ThreadInner {
    /// Suspends the thread. Returns once the scheduler resumes it; the caller then checks `killed`.
    pub fn suspend(&mut self, kind: Yield) {
        // SAFETY: `yielder` is set when the coroutine starts and stays valid while it runs, which is
        // the only time runtime functions execute on this thread.
        unsafe { (*self.yielder).suspend(kind) };
    }

    pub fn killed(&self) -> bool {
        // SAFETY: `ctx` lives as long as the thread.
        unsafe { (*self.ctx).killed != 0 }
    }

    /// The end of a loop iteration. In warp mode the thread keeps going, unless it has held the
    /// frame for too long.
    pub fn loop_yield(&mut self) {
        if self.warp_depth == 0 {
            self.suspend(Yield::Loop);
            return;
        }
        self.warp_yields = self.warp_yields.wrapping_add(1);
        if self.warp_yields.is_multiple_of(256) && self.warp_started.elapsed().as_millis() >= WARP_TIME_MS {
            self.suspend(Yield::Loop);
            self.warp_started = Instant::now();
        }
    }

    pub fn enter_warp(&mut self) {
        if self.warp_depth == 0 {
            self.warp_started = Instant::now();
            self.warp_yields = 0;
        }
        self.warp_depth += 1;
    }

    pub fn exit_warp(&mut self) {
        self.warp_depth = self.warp_depth.saturating_sub(1);
    }

    /// scratch-vm's recursion rule (`Thread.isRecursiveCall`): a call is recursive when a call to
    /// the same procedure is among the six stack entries below it. Each enclosing loop or
    /// conditional is one entry, and so is each call.
    pub fn is_recursive(&self, proc: i64, nesting: i64) -> bool {
        let mut distance = nesting;
        for frame in self.calls.iter().rev() {
            distance += 1;
            if distance > 6 {
                return false;
            }
            if frame.proc == proc {
                return true;
            }
            distance += frame.nesting;
        }
        false
    }
}

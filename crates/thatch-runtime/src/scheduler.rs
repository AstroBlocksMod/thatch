//! The runtime state and the scheduler.
//!
//! A frame steps the threads in passes, as scratch-vm's sequencer does: a pass steps every thread
//! that is not done and not waiting for the next frame, in list order, including threads started
//! during the pass. Passes repeat while some stepped thread is still active (a thread waiting on a
//! timer or on other threads is stepped but does not count), no redraw was requested and less than
//! 75 percent of the frame time has gone by. The clock that `wait` and `timer` read comes from the
//! host; without one it is virtual, one thirtieth of a second per frame, so headless runs are
//! repeatable. Only the frame budget is always real time.
//!
//! The runtime talks back to its host through the [`Host`] table it was created with: every bubble
//! goes to `say`, the clock to `now_ms`, and every change to how a sprite looks to
//! `sprite_changed` (see `sprite.rs`). The host drives the frame loop through the C API in `api.rs`.
//!
//! # Aliasing
//!
//! While a thread runs, the runtime functions it calls reach the `Runtime` through the raw pointer
//! every thread holds and borrow it mutably for the length of the call. So nothing may hold a
//! reference to the `Runtime` across a coroutine resume: the runtime lives behind a raw pointer
//! from [`Runtime::create`] to [`Runtime::destroy`], and the functions that resume threads
//! ([`Runtime::frame`], [`Runtime::shutdown`]) take that pointer and borrow only between resumes.
//!
//! Host callbacks follow the same rule. A callback receives the runtime pointer and may call back
//! in (the headless host asks for the target's name in `say`), so no reference to the runtime may
//! be alive while one runs either. The clock gets only the host's data, but a host may keep the
//! runtime pointer there, so the same holds for it. Everything that calls the host is a method of
//! [`Rt`], which borrows the runtime only between callbacks.

use std::ffi::c_void;
use std::ptr;
use std::rc::Rc;
use std::time::{Duration, Instant};

use corosensei::CoroutineResult;
use thatch_abi::{EFFECT_COUNT, HAT_BROADCAST, HAT_FLAG, Host, OpaqueRuntime, TAG_STR, ThreadCtx, Value};

use crate::strings::{self, Handle};
use crate::thread::{ScriptFn, Status, Thread, Yield};
use crate::value::VarSlot;

pub const FPS: f64 = 30.0;
const WORK_TIME: Duration = Duration::from_micros(25_000);

pub struct Script {
    pub hat: i64,
    pub arg: String,
    pub func: ScriptFn,
}

pub struct Target {
    pub name: String,
    pub is_stage: bool,
    pub nums: Vec<f64>,
    pub num_names: Vec<String>,
    pub anys: Vec<Value>,
    pub any_names: Vec<String>,
    pub scripts: Vec<Script>,
    pub bubble: String,
    pub bubble_token: u64,
    // How the target looks; see `sprite.rs`.
    pub x: f64,
    pub y: f64,
    pub direction: f64,
    pub size: f64,
    pub costume: usize,
    pub costumes: Vec<Rc<String>>,
    pub visible: bool,
    pub rotation_style: i64,
    pub draggable: bool,
    pub volume: f64,
    pub effects: [f64; EFFECT_COUNT],
}

pub struct Runtime {
    /// The pointer this runtime lives behind, which threads use to reach it.
    this: *mut Runtime,
    /// The host's callbacks. `data` is the host's; the runtime only passes it back.
    pub(crate) host: Host,
    pub targets: Vec<Box<Target>>,
    /// Target indices in drawing order, the stage first and the frontmost sprite last. Hats start
    /// in the reverse of this order, so the layer blocks change it.
    pub order: Vec<usize>,
    /// The mouse in Scratch coordinates, as the host last reported it.
    pub mouse_x: f64,
    pub mouse_y: f64,
    consts: Vec<Rc<String>>,
    const_ptrs: Vec<Handle>,
    pub threads: Vec<Thread>,
    /// Threads replaced by a restart. Suspended ones are resumed to finish at the end of the pass.
    dying: Vec<Thread>,
    next_thread_id: u64,
    /// Whether a block asked for a redraw in this frame, which ends the frame after the current
    /// pass as in scratch-vm. Cleared when a frame starts.
    pub redraw_requested: bool,
    /// Whether the host should draw after the current frame, and whether something asked for that
    /// between frames, which the next frame carries over (see [`Runtime::host_should_draw`]).
    draw_requested: bool,
    draw_requested_between: bool,
    in_frame: bool,
    /// Whether a program was loaded; a runtime holds one.
    pub(crate) loaded: bool,
    pub frame: u64,
    timer_start_ms: f64,
    frame_started: Instant,
    rng: u64,
    /// The message of the last error, for `thatch_runtime_last_error`.
    pub last_error: Option<String>,
}

impl Runtime {
    /// Allocates a runtime behind a raw pointer. Free it with [`Runtime::destroy`].
    pub fn create(host: Host, seed: u64) -> *mut Runtime {
        let rt = Box::into_raw(Box::new(Self {
            this: ptr::null_mut(),
            host,
            targets: Vec::new(),
            order: Vec::new(),
            mouse_x: 0.0,
            mouse_y: 0.0,
            consts: Vec::new(),
            const_ptrs: Vec::new(),
            threads: Vec::new(),
            dying: Vec::new(),
            next_thread_id: 1,
            redraw_requested: false,
            draw_requested: false,
            draw_requested_between: false,
            in_frame: false,
            loaded: false,
            frame: 0,
            timer_start_ms: 0.0,
            frame_started: Instant::now(),
            rng: seed | 1,
            last_error: None,
        }));
        // SAFETY: just allocated, and nothing else knows the pointer yet.
        unsafe { (*rt).this = rt };
        rt
    }

    /// Frees a runtime from [`Runtime::create`]. Call [`Runtime::shutdown`] first.
    ///
    /// # Safety
    /// `rt` must come from `create`, and no thread may run or be suspended any more.
    pub unsafe fn destroy(rt: *mut Runtime) {
        // SAFETY: forwarded from the caller.
        drop(unsafe { Box::from_raw(rt) });
    }

    // ---- registration -------------------------------------------------------------------------

    pub fn add_const(&mut self, s: String) {
        let rc = Rc::new(s);
        self.const_ptrs.push(strings::handle_of(&rc));
        self.consts.push(rc);
    }

    pub fn add_target(&mut self, name: String, is_stage: bool) -> usize {
        self.targets.push(Box::new(Target {
            name,
            is_stage,
            nums: Vec::new(),
            num_names: Vec::new(),
            anys: Vec::new(),
            any_names: Vec::new(),
            scripts: Vec::new(),
            bubble: String::new(),
            bubble_token: 0,
            x: 0.0,
            y: 0.0,
            direction: 90.0,
            size: 100.0,
            costume: 0,
            costumes: Vec::new(),
            visible: true,
            rotation_style: thatch_abi::ROTATE_ALL_AROUND,
            draggable: false,
            volume: 100.0,
            effects: [0.0; EFFECT_COUNT],
        }));
        let index = self.targets.len() - 1;
        // The stage is drawn first whatever order it was registered in.
        if is_stage {
            self.order.insert(0, index)
        } else {
            self.order.push(index)
        }
        index
    }

    pub fn add_costume(&mut self, target: usize, name: String) {
        self.targets[target].costumes.push(Rc::new(name));
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_sprite(
        &mut self,
        target: usize,
        x: f64,
        y: f64,
        direction: f64,
        size: f64,
        costume: i64,
        visible: bool,
        rotation_style: i64,
        draggable: bool,
        volume: f64,
    ) {
        let t = &mut self.targets[target];
        t.x = x;
        t.y = y;
        t.direction = direction;
        t.size = size;
        t.costume = usize::try_from(costume).unwrap_or(0).min(t.costumes.len().saturating_sub(1));
        t.visible = visible;
        t.rotation_style = rotation_style;
        t.draggable = draggable;
        t.volume = volume;
    }

    pub fn add_num_var(&mut self, target: usize, name: String, initial: f64) {
        let t = &mut self.targets[target];
        t.nums.push(initial);
        t.num_names.push(name);
    }

    pub fn add_any_var(&mut self, target: usize, name: String, tag: u64, bits: u64) {
        let value = if tag == TAG_STR {
            let s = self.consts.get(bits as usize).cloned().unwrap_or_default();
            Value { tag: TAG_STR, bits: strings::into_raw(s) as u64 }
        } else {
            Value { tag, bits }
        };
        let t = &mut self.targets[target];
        t.anys.push(value);
        t.any_names.push(name);
    }

    pub fn add_script(&mut self, target: usize, hat: i64, arg: String, func: ScriptFn) {
        self.targets[target].scripts.push(Script { hat, arg, func });
    }

    pub(crate) fn stage_index(&self) -> usize {
        self.targets.iter().position(|t| t.is_stage).unwrap_or(0)
    }

    /// Forgets everything a program registered, as after a failed load. No thread may exist yet.
    pub(crate) fn unload(&mut self) {
        self.release_variables();
        self.targets.clear();
        self.consts.clear();
        self.const_ptrs.clear();
    }

    /// Drops the strings the variables hold.
    fn release_variables(&mut self) {
        for t in &mut self.targets {
            for slot in &mut t.anys {
                // SAFETY: variable slots own their strings, and no thread runs while this does.
                unsafe { VarSlot::replace(ptr::addr_of_mut!(*slot), crate::value::num(0.0)) };
            }
        }
    }

    /// A uniform random number in `[0, 1)`, from a xorshift generator.
    pub fn random(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// A block asks for a redraw: the frame ends after the current pass, and the host draws.
    pub fn request_redraw(&mut self) {
        self.redraw_requested = true;
        self.host_draw();
    }

    /// Something changed what the host shows, without asking for a redraw the way scratch-vm's
    /// blocks do. The scheduler goes on; the host draws after the frame.
    pub fn host_draw(&mut self) {
        if self.in_frame {
            self.draw_requested = true;
        } else {
            self.draw_requested_between = true;
        }
    }

    /// Whether the host should draw: something asked for it during the last frame or since. A
    /// frame clears what it carried over only once it starts, so a `stop all` from the host between
    /// frames is drawn after the next one.
    pub fn host_should_draw(&self) -> bool {
        self.draw_requested || self.draw_requested_between
    }

    // ---- threads ------------------------------------------------------------------------------

    fn new_thread(&mut self, target: usize, script: usize) -> Thread {
        let id = self.next_thread_id;
        self.next_thread_id += 1;
        let stage = self.stage_index();
        let func = self.targets[target].scripts[script].func;
        let t = &mut self.targets[target];
        let nums = t.nums.as_mut_ptr();
        let anys = t.anys.as_mut_ptr();
        let s = &mut self.targets[stage];
        let ctx = ThreadCtx {
            nums,
            anys,
            stage_nums: s.nums.as_mut_ptr(),
            stage_anys: s.anys.as_mut_ptr(),
            consts: self.const_ptrs.as_ptr().cast::<*const c_void>(),
            killed: 0,
            inner: ptr::null_mut(),
        };
        Thread::new(self.this, id, target, script, func, ctx)
    }

    /// Starts every script whose hat matches, as scratch-vm's `startHats` does: targets from front
    /// to back with the stage last, scripts in file order. A thread of the same script and target
    /// that is still in the thread list, even one that finished during this pass, is replaced in
    /// place by the new one. Returns the ids of the threads started.
    pub fn start_hats(&mut self, hat: i64, arg: &str) -> Vec<u64> {
        let mut started = Vec::new();
        let order = self.order.clone();
        for target in order.into_iter().rev() {
            for script in 0..self.targets[target].scripts.len() {
                let s = &self.targets[target].scripts[script];
                if s.hat != hat {
                    continue;
                }
                let matches = match hat {
                    HAT_BROADCAST => s.arg.to_uppercase() == arg.to_uppercase(),
                    _ => true,
                };
                if !matches {
                    continue;
                }
                let thread = self.new_thread(target, script);
                started.push(thread.id);
                match self.threads.iter().position(|t| t.target == target && t.script == script) {
                    Some(i) => {
                        let mut old = std::mem::replace(&mut self.threads[i], thread);
                        old.kill();
                        self.dying.push(old);
                    }
                    None => self.threads.push(thread),
                }
            }
        }
        started
    }

    pub fn thread_status(&self, id: u64) -> Option<Status> {
        self.threads.iter().find(|t| t.id == id).map(|t| t.status)
    }

    /// Kills every thread, the part of `stop all` that does not call the host; [`Rt::stop_all`]
    /// is the rest. Returns the targets whose bubbles need clearing.
    fn kill_all(&mut self) -> Vec<usize> {
        for t in &mut self.threads {
            t.kill();
        }
        (0..self.targets.len()).filter(|i| !self.targets[*i].bubble.is_empty()).collect()
    }

    pub fn stop_others(&mut self, target: usize, except: u64) {
        for t in &mut self.threads {
            if t.target == target && t.id != except {
                t.kill();
            }
        }
    }

    // ---- scheduling ---------------------------------------------------------------------------

    /// Runs one frame.
    ///
    /// # Safety
    /// `rt` must come from [`Runtime::create`], and the caller may hold no reference to it.
    pub unsafe fn frame(rt: *mut Runtime) -> Result<(), String> {
        {
            // SAFETY: no thread runs here, and the caller holds no reference.
            let r = unsafe { &mut *rt };
            r.redraw_requested = false;
            r.draw_requested = std::mem::take(&mut r.draw_requested_between);
            r.in_frame = true;
            r.frame_started = Instant::now();
            for t in &mut r.threads {
                if t.status == Status::TickYield {
                    t.status = Status::Running;
                }
            }
        }
        // SAFETY: forwarded from the caller.
        let result = unsafe { Self::passes(rt) };
        // SAFETY: no thread runs here.
        unsafe {
            (*rt).in_frame = false;
            (*rt).frame += 1;
        }
        result
    }

    /// The passes of a frame.
    ///
    /// # Safety
    /// As [`Runtime::frame`].
    unsafe fn passes(rt: *mut Runtime) -> Result<(), String> {
        loop {
            let mut active = 0;
            let mut stopped = false;
            let mut i = 0;
            // SAFETY: as above; each borrow ends before the next step resumes a thread.
            while i < unsafe { (*rt).threads.len() } {
                let (status, skip) = {
                    // SAFETY: as above.
                    let t = unsafe { &(&(*rt).threads)[i] };
                    (t.status, t.status == Status::Done || (t.status == Status::TickYield && !t.killed))
                };
                // scratch-vm counts the thread it stepped, even when that thread replaced itself in
                // the list by restarting its own script.
                // SAFETY: forwarded from the caller.
                let status = if skip { status } else { unsafe { Self::step(rt, i)? } };
                match status {
                    Status::Running => active += 1,
                    Status::Done => stopped = true,
                    _ => {}
                }
                i += 1;
            }
            // SAFETY: no thread runs here.
            let done = unsafe {
                let r = &mut *rt;
                if stopped || r.threads.iter().any(|t| t.status == Status::Done) {
                    r.threads.retain(|t| t.status != Status::Done);
                }
                active == 0 || r.redraw_requested || r.frame_started.elapsed() >= WORK_TIME
            };
            // SAFETY: forwarded from the caller.
            unsafe { Self::finish_dying(rt) };
            if done {
                return Ok(());
            }
        }
    }

    /// Steps the thread at index `i` once and returns the status it stopped in.
    ///
    /// # Safety
    /// As [`Runtime::frame`].
    unsafe fn step(rt: *mut Runtime, i: usize) -> Result<Status, String> {
        let (id, mut coroutine) = {
            // SAFETY: no thread runs here.
            let t = unsafe { &mut (&mut (*rt).threads)[i] };
            if t.status == Status::Fresh && t.killed {
                t.status = Status::Done;
                return Ok(Status::Done);
            }
            (t.id, t.take_coroutine()?)
        };
        // No reference to the runtime is alive while the thread runs.
        let result = coroutine.resume(());
        let status = match result {
            CoroutineResult::Yield(Yield::Loop) => Status::Running,
            CoroutineResult::Yield(Yield::Wait) => Status::Waiting,
            CoroutineResult::Yield(Yield::Tick) => Status::TickYield,
            CoroutineResult::Return(()) => Status::Done,
        };
        // SAFETY: the thread has suspended or returned, so nothing else borrows the runtime.
        let r = unsafe { &mut *rt };
        // The script may have restarted its own thread, which moved the old one to `dying`.
        let slot = match r.threads.get_mut(i) {
            Some(t) if t.id == id => Some(t),
            _ => r.threads.iter_mut().chain(r.dying.iter_mut()).find(|t| t.id == id),
        };
        if let Some(t) = slot {
            t.put_coroutine(coroutine);
            t.status = status;
        }
        Ok(status)
    }

    /// Lets killed, suspended threads return so they can be dropped.
    ///
    /// # Safety
    /// As [`Runtime::frame`].
    unsafe fn finish_dying(rt: *mut Runtime) {
        loop {
            // SAFETY: no thread runs here.
            let dying = std::mem::take(unsafe { &mut (*rt).dying });
            if dying.is_empty() {
                return;
            }
            for mut t in dying {
                t.finish();
            }
        }
    }

    /// Stops everything and lets every thread finish, at the end of a run.
    ///
    /// # Safety
    /// As [`Runtime::frame`].
    pub unsafe fn shutdown(rt: *mut Runtime) {
        // SAFETY: forwarded from the caller; no thread runs here.
        // The host is going away, so it hears nothing more: no bubbles or effects are cleared.
        let threads = unsafe {
            Rt::new(rt).with(|r| {
                r.kill_all();
                std::mem::take(&mut r.threads)
            })
        };
        for mut t in threads {
            t.finish();
        }
        // SAFETY: forwarded from the caller.
        unsafe { Self::finish_dying(rt) };
    }

    /// Whether some thread could still run. A killed thread only waits for its next step to finish,
    /// so it does not count.
    pub fn has_threads(&self) -> bool {
        self.threads.iter().any(|t| !t.killed)
    }

    pub fn bubble_token(&self, target: usize) -> u64 {
        self.targets[target].bubble_token
    }
}

/// The runtime behind its raw pointer, for everything that calls the host with that pointer.
///
/// A callback may call back into the runtime through the C API, which reads it, so no reference
/// to the runtime may be alive while one runs. The methods here borrow the runtime only for short
/// stretches with [`Rt::with`] and call the host between them.
#[derive(Clone, Copy)]
pub struct Rt(*mut Runtime);

impl Rt {
    /// # Safety
    /// `rt` must come from [`Runtime::create`] and live while the handle is used, and nothing may
    /// hold a reference to the runtime meanwhile: the rule in the module documentation.
    pub unsafe fn new(rt: *mut Runtime) -> Self {
        Self(rt)
    }

    /// Borrows the runtime for `f`. `f` must not call the host with the runtime pointer (only `Rt`
    /// methods do that) or reach the runtime through another handle; the borrow ends when it
    /// returns.
    pub fn with<R>(self, f: impl FnOnce(&mut Runtime) -> R) -> R {
        // SAFETY: `new` promised a live runtime that nothing else borrows, and `f` reaches it only
        // through this borrow.
        unsafe { f(&mut *self.0) }
    }

    /// The runtime as the host sees it, for callbacks.
    pub(crate) fn opaque(self) -> *mut OpaqueRuntime {
        self.0.cast::<OpaqueRuntime>()
    }

    /// `say` and `think`. Returns the bubble token, which `say for secs` uses to clear only its own
    /// bubble.
    pub fn say(self, target: usize, text: String, think: bool) -> u64 {
        let text = truncate_utf16(text, 330);
        let (token, host) = self.with(|r| {
            let t = &mut r.targets[target];
            t.bubble_token += 1;
            t.bubble.clone_from(&text);
            let token = t.bubble_token;
            r.request_redraw();
            (token, r.host)
        });
        if let Some(say) = host.say {
            // SAFETY: the host gave this callback and its data for the runtime to call. The text is
            // a copy that outlives the call, and no reference to the runtime is alive, so the
            // callback may call back in.
            unsafe {
                say(
                    host.data,
                    self.0.cast::<OpaqueRuntime>(),
                    target as i64,
                    text.as_ptr(),
                    text.len(),
                    i8::from(think),
                )
            };
        }
        token
    }

    /// Milliseconds on the host's clock, or on the virtual clock when the host has none. The clock
    /// is a host callback, so it is read with no reference to the runtime alive.
    pub fn now_ms(self) -> f64 {
        let (now, data, frame) = self.with(|r| (r.host.now_ms, r.host.data, r.frame));
        match now {
            // SAFETY: the host gave this callback and its data for the runtime to call, and no
            // reference to the runtime is alive, so the callback may call back in.
            Some(now) => unsafe { now(data) },
            None => frame as f64 * 1000.0 / FPS,
        }
    }

    /// The `timer` reporter, in seconds.
    pub fn timer(self) -> f64 {
        let now = self.now_ms();
        self.with(|r| (now - r.timer_start_ms) / 1000.0)
    }

    pub fn reset_timer(self) {
        let now = self.now_ms();
        self.with(|r| r.timer_start_ms = now);
    }

    /// Stops every thread, as the stop sign or a `stop all` block does, clears every bubble, and
    /// clears every target's graphic effects, as scratch-vm's `stopAll` does through each target's
    /// `onStopAll`.
    pub fn stop_all(self) {
        for target in self.with(Runtime::kill_all) {
            self.say(target, String::new(), false);
        }
        for target in 0..self.with(|r| r.targets.len()) {
            self.clear_effects(target);
        }
    }

    /// The green flag: everything stops first, as scratch-vm's `greenFlag` calls `stopAll`.
    pub fn start_flag(self) {
        self.stop_all();
        self.reset_timer();
        self.with(|r| r.start_hats(HAT_FLAG, ""));
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.release_variables();
    }
}

/// Cuts a string to at most `max` UTF-16 code units, as scratch-vm's `substr(0, 330)` cuts bubbles.
/// When the cut splits a character outside the Basic Multilingual Plane, JavaScript keeps the lone
/// high surrogate, which shows as U+FFFD, so that is what is kept here.
fn truncate_utf16(text: String, max: usize) -> String {
    if text.len() <= max || text.encode_utf16().count() <= max {
        return text;
    }
    let mut units = 0;
    let mut end = text.len();
    let mut split = false;
    for (i, c) in text.char_indices() {
        if units + c.len_utf16() > max {
            end = i;
            split = units < max;
            break;
        }
        units += c.len_utf16();
    }
    let mut text = text;
    text.truncate(end);
    if split {
        text.push('\u{FFFD}');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::truncate_utf16;

    #[test]
    fn bubbles_are_cut_by_utf16_units() {
        assert_eq!(truncate_utf16("abcd".into(), 3), "abc");
        assert_eq!(truncate_utf16("ab😀".into(), 3), "ab\u{FFFD}");
        assert_eq!(truncate_utf16("ab😀".into(), 4), "ab😀");
        assert_eq!(truncate_utf16("a😀b".into(), 3), "a😀");
    }
}

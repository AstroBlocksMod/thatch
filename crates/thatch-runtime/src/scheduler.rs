//! The runtime state and the scheduler.
//!
//! A frame steps the threads in passes, as scratch-vm's sequencer does: a pass steps every thread
//! that is not done and not waiting for the next frame, in list order, including threads started
//! during the pass. Passes repeat while some stepped thread is still active, no redraw was requested
//! and less than 75 percent of the frame time has gone by. The clock that `wait` and `timer` read is
//! virtual, one thirtieth of a second per frame, so runs are repeatable; only the frame budget is
//! real time.

use std::ffi::c_void;
use std::io::Write;
use std::ptr;
use std::rc::Rc;
use std::time::{Duration, Instant};

use corosensei::CoroutineResult;
use thatch_abi::{HAT_BROADCAST, HAT_FLAG, TAG_STR, ThreadCtx, Value};

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
}

pub struct Runtime {
    pub targets: Vec<Box<Target>>,
    consts: Vec<Rc<String>>,
    const_ptrs: Vec<Handle>,
    pub threads: Vec<Thread>,
    /// Threads replaced by a restart while suspended. They are resumed to finish at the end of the
    /// pass.
    dying: Vec<Thread>,
    next_thread_id: u64,
    /// The id of the thread being stepped, or 0 between steps.
    pub current: u64,
    pub redraw_requested: bool,
    pub frame: u64,
    timer_start_ms: f64,
    frame_started: Instant,
    rng: u64,
    pub headless: bool,
    pub say_count: u64,
}

impl Runtime {
    pub fn new(seed: u64) -> Self {
        Self {
            targets: Vec::new(),
            consts: Vec::new(),
            const_ptrs: Vec::new(),
            threads: Vec::new(),
            dying: Vec::new(),
            next_thread_id: 1,
            current: 0,
            redraw_requested: false,
            frame: 0,
            timer_start_ms: 0.0,
            frame_started: Instant::now(),
            rng: seed | 1,
            headless: true,
            say_count: 0,
        }
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
        }));
        self.targets.len() - 1
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

    fn stage_index(&self) -> usize {
        self.targets.iter().position(|t| t.is_stage).unwrap_or(0)
    }

    // ---- clock --------------------------------------------------------------------------------

    /// Milliseconds on the virtual clock.
    pub fn now_ms(&self) -> f64 {
        self.frame as f64 * 1000.0 / FPS
    }

    pub fn timer(&self) -> f64 {
        (self.now_ms() - self.timer_start_ms) / 1000.0
    }

    pub fn reset_timer(&mut self) {
        self.timer_start_ms = self.now_ms();
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

    pub fn request_redraw(&mut self) {
        self.redraw_requested = true;
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
        Thread::new(ptr::addr_of_mut!(*self), id, target, script, func, ctx)
    }

    /// Starts every script whose hat matches, as scratch-vm's `startHats` does: targets from front
    /// to back with the stage last, scripts in file order, and a script already running for the same
    /// target restarted in place. Returns the ids of the threads started.
    pub fn start_hats(&mut self, hat: i64, arg: &str) -> Vec<u64> {
        let mut started = Vec::new();
        for target in (0..self.targets.len()).rev() {
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
                match self
                    .threads
                    .iter()
                    .position(|t| t.target == target && t.script == script && t.status != Status::Done)
                {
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

    pub fn stop_all(&mut self) {
        for t in &mut self.threads {
            t.kill();
        }
        for t in &mut self.targets {
            t.bubble.clear();
        }
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
    pub fn frame(&mut self) -> Result<(), String> {
        self.redraw_requested = false;
        self.frame_started = Instant::now();
        for t in &mut self.threads {
            if t.status == Status::TickYield {
                t.status = Status::Running;
            }
        }
        loop {
            let mut active = 0;
            let mut stopped = false;
            let mut i = 0;
            while i < self.threads.len() {
                let t = &self.threads[i];
                let skip = t.status == Status::Done || (t.status == Status::TickYield && !t.killed);
                if !skip {
                    self.step(i)?;
                }
                match self.threads[i].status {
                    Status::Running => active += 1,
                    Status::Done => stopped = true,
                    _ => {}
                }
                i += 1;
            }
            if stopped {
                self.threads.retain(|t| t.status != Status::Done);
            }
            self.finish_dying();
            if active == 0 || self.redraw_requested || self.frame_started.elapsed() >= WORK_TIME {
                break;
            }
        }
        self.frame += 1;
        Ok(())
    }

    /// Steps the thread at index `i` once.
    fn step(&mut self, i: usize) -> Result<(), String> {
        let id = self.threads[i].id;
        if self.threads[i].status == Status::Fresh && self.threads[i].killed {
            self.threads[i].status = Status::Done;
            return Ok(());
        }
        let mut coroutine = self.threads[i].take_coroutine()?;
        self.current = id;
        let result = coroutine.resume(());
        self.current = 0;
        let status = match result {
            CoroutineResult::Yield(Yield::Loop) => Status::Running,
            CoroutineResult::Yield(Yield::Tick) => Status::TickYield,
            CoroutineResult::Return(()) => Status::Done,
        };
        // The script may have restarted its own thread, which moved the old one to `dying`.
        let slot = match self.threads.get_mut(i) {
            Some(t) if t.id == id => Some(t),
            _ => self.dying.iter_mut().find(|t| t.id == id),
        };
        if let Some(t) = slot {
            t.put_coroutine(coroutine);
            t.status = status;
        }
        Ok(())
    }

    /// Lets killed, suspended threads return so they can be dropped.
    fn finish_dying(&mut self) {
        let dying = std::mem::take(&mut self.dying);
        for mut t in dying {
            t.finish();
        }
    }

    /// Stops everything and lets every thread finish, at the end of a run.
    pub fn shutdown(&mut self) {
        self.stop_all();
        let threads = std::mem::take(&mut self.threads);
        for mut t in threads {
            if t.suspended() {
                t.finish();
            }
        }
        self.finish_dying();
    }

    /// Whether some thread could still run.
    pub fn has_threads(&self) -> bool {
        !self.threads.is_empty()
    }

    pub fn start_flag(&mut self) {
        self.reset_timer();
        self.start_hats(HAT_FLAG, "");
    }

    // ---- looks --------------------------------------------------------------------------------

    /// `say` and `think`. Returns the bubble token, which `say for secs` uses to clear only its own
    /// bubble.
    pub fn say(&mut self, target: usize, text: String, think: bool) -> u64 {
        let text = truncate_utf16(text, 330);
        let t = &mut self.targets[target];
        t.bubble_token += 1;
        if self.headless && !text.is_empty() {
            self.say_count += 1;
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{} {}: {}", t.name, if think { "thinks" } else { "says" }, text);
        }
        t.bubble = text;
        self.redraw_requested = true;
        t.bubble_token
    }

    pub fn bubble_token(&self, target: usize) -> u64 {
        self.targets[target].bubble_token
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        for t in &mut self.targets {
            for slot in &mut t.anys {
                // SAFETY: variable slots own their strings, and nothing runs after the runtime.
                unsafe { VarSlot::replace(ptr::addr_of_mut!(*slot), crate::value::num(0.0)) };
            }
        }
    }
}

/// Cuts a string to at most `max` UTF-16 code units, as scratch-vm cuts bubbles.
fn truncate_utf16(text: String, max: usize) -> String {
    if text.encode_utf16().count() <= max {
        return text;
    }
    let mut units = 0;
    let mut end = text.len();
    for (i, c) in text.char_indices() {
        units += c.len_utf16();
        if units > max {
            end = i;
            break;
        }
    }
    let mut text = text;
    text.truncate(end);
    text
}

//! The headless host: runs a program with a virtual clock and prints what sprites say.
//!
//! It is the first implementation of the host table in `thatch_abi::Host`, and it goes through the
//! same C API (`thatch_runtime_*` in `api.rs`) a host written in C would use, so running the samples
//! exercises that surface too.

use std::ffi::{CStr, c_char, c_void};
use std::io::Write;
use std::ptr;
use std::time::Instant;

use thatch_abi::{Host, InitFn, OpaqueRuntime};

use crate::api;

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Stop after this many frames. The run also stops once no script is left.
    pub frames: Option<u64>,
    /// Print the frame count and wall time to stderr.
    pub timing: bool,
    /// The seed of `pick random`. Random by default.
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct RunReport {
    pub frames: u64,
    pub says: u64,
}

/// What the headless host keeps between callbacks.
struct Headless {
    says: u64,
}

/// The `say` callback: prints every non-empty bubble as `Name says: text`.
unsafe extern "C" fn headless_say(
    data: *mut c_void,
    rt: *mut OpaqueRuntime,
    target: i64,
    text: *const u8,
    len: usize,
    think: i8,
) {
    if len == 0 {
        return;
    }
    // SAFETY: the runtime passes back the `Headless` this host registered, a live runtime, and
    // `len` bytes of UTF-8 that outlive the call.
    unsafe {
        let host = &mut *data.cast::<Headless>();
        host.says += 1;
        let text = std::str::from_utf8_unchecked(std::slice::from_raw_parts(text, len));
        let mut name_len = 0;
        let name = api::thatch_runtime_target_name(rt, target, &raw mut name_len);
        let name = if name.is_null() {
            ""
        } else {
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(name.cast::<u8>(), name_len))
        };
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{name} {}: {text}", if think != 0 { "thinks" } else { "says" });
    }
}

/// Checks the program's ABI version, registers the program through `init` and runs it headless
/// until no script is left or the frame limit is reached.
///
/// `abi_version` is the version the program was compiled against (`thatch_abi::ABI_VERSION` at
/// its compile time); a program built for another version is refused before any of its code runs.
pub fn run(init: InitFn, abi_version: u32, options: &RunOptions) -> Result<RunReport, String> {
    let seed = options.seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
    });
    let mut headless = Headless { says: 0 };
    let host = Host { data: ptr::addr_of_mut!(headless).cast::<c_void>(), say: Some(headless_say), now_ms: None };
    // SAFETY: `host` outlives the runtime, which is created and destroyed here, and the runtime is
    // only touched between frames through the C API.
    unsafe {
        let rt = api::thatch_runtime_create(thatch_abi::ABI_VERSION as i32, &raw const host, seed);
        let result = run_program(rt, init, abi_version, options);
        api::thatch_runtime_destroy(rt);
        result.map(|frames| RunReport { frames, says: headless.says })
    }
}

/// The body of [`run`], whose result `run` returns only after destroying the runtime.
///
/// # Safety
/// `rt` must come from `thatch_runtime_create` and `init` must be a program's init function.
unsafe fn run_program(
    rt: *mut OpaqueRuntime,
    init: InitFn,
    abi_version: u32,
    options: &RunOptions,
) -> Result<u64, String> {
    // SAFETY: forwarded from the caller.
    unsafe {
        match api::thatch_runtime_load(rt, abi_version as i32, init) {
            thatch_abi::LOAD_OK => {}
            thatch_abi::LOAD_WRONG_ABI_VERSION => {
                return Err(format!(
                    "the program was compiled for ABI version {abi_version}, but this runtime has version {}",
                    thatch_abi::ABI_VERSION
                ));
            }
            thatch_abi::LOAD_NO_STAGE => return Err("the program registered no stage".to_owned()),
            other => return Err(format!("loading the program failed with code {other}")),
        }
        let started = Instant::now();
        api::thatch_runtime_green_flag(rt);
        while api::thatch_runtime_has_threads(rt) != 0 {
            if options.frames.is_some_and(|max| api::thatch_runtime_frame_count(rt) >= max) {
                break;
            }
            if api::thatch_runtime_frame(rt) != 0 {
                let mut len = 0;
                let msg = api::thatch_runtime_last_error(rt, &raw mut len);
                let msg = if msg.is_null() {
                    "unknown error".to_owned()
                } else {
                    String::from_utf8_lossy(std::slice::from_raw_parts(msg.cast::<u8>(), len)).into_owned()
                };
                return Err(msg);
            }
        }
        let frames = api::thatch_runtime_frame_count(rt);
        if options.timing {
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            eprintln!("thatch: {frames} frames in {ms:.1} ms");
        }
        Ok(frames)
    }
}

/// Parses the command line of a compiled program: `--frames N`, `--timing`, `--seed N`.
pub fn parse_args(args: &[String]) -> Result<RunOptions, String> {
    let mut options = RunOptions::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--timing" => options.timing = true,
            "--frames" | "--seed" => {
                let value = args.get(i + 1).ok_or_else(|| format!("{} needs a number", args[i]))?;
                let n: u64 = value.parse().map_err(|_| format!("{} needs a number, not {value}", args[i]))?;
                if args[i] == "--frames" {
                    options.frames = Some(n);
                } else {
                    options.seed = Some(n);
                }
                i += 1;
            }
            other => return Err(format!("unknown option {other}")),
        }
        i += 1;
    }
    Ok(options)
}

/// The entry point a compiled program's `main` calls.
///
/// # Safety
/// `argv` must point at `argc` C strings and `init` at the program's init function.
pub unsafe fn main(argc: i32, argv: *const *const c_char, init: InitFn, abi_version: u32) -> i32 {
    let mut args = Vec::new();
    for i in 1..argc.max(0) as usize {
        // SAFETY: the caller promises `argc` C strings.
        let s = unsafe { CStr::from_ptr(*argv.add(i)) };
        args.push(s.to_string_lossy().into_owned());
    }
    let options = match parse_args(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("thatch: {e}");
            return 2;
        }
    };
    match run(init, abi_version, &options) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("thatch: {e}");
            1
        }
    }
}

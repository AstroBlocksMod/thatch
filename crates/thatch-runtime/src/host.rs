//! The headless host: runs a program with a virtual clock and prints what sprites say.

use std::ffi::{CStr, c_char};
use std::time::Instant;

use crate::InitFn;
use crate::scheduler::Runtime;

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

/// Checks the program's ABI version, registers the program through `init` and runs it headless
/// until no script is left or the frame limit is reached.
///
/// `abi_version` is the version the program was compiled against (`thatch_abi::ABI_VERSION` at
/// its compile time); a program built for another version is refused before any of its code runs.
pub fn run(init: InitFn, abi_version: u32, options: &RunOptions) -> Result<RunReport, String> {
    if abi_version != thatch_abi::ABI_VERSION {
        return Err(format!(
            "the program was compiled for ABI version {abi_version}, but this runtime has version {}",
            thatch_abi::ABI_VERSION
        ));
    }
    let seed = options.seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
    });
    let rt = Runtime::create(seed);
    // SAFETY: `rt` is the runtime just created; references into it are only taken while no thread
    // runs, and the runtime is shut down and freed exactly once below.
    let result = unsafe { run_program(rt, init, options) };
    unsafe {
        Runtime::shutdown(rt);
        Runtime::destroy(rt);
    }
    result
}

/// The body of [`run`], whose result `run` returns only after shutting the runtime down.
///
/// # Safety
/// `rt` must come from [`Runtime::create`] and `init` must be a program's init function.
unsafe fn run_program(rt: *mut Runtime, init: InitFn, options: &RunOptions) -> Result<RunReport, String> {
    // SAFETY: forwarded from the caller; `init` only registers, it runs no script.
    unsafe {
        init(rt);
        if !(*rt).targets.iter().any(|t| t.is_stage) {
            return Err("the program registered no stage".to_owned());
        }
        let started = Instant::now();
        (*rt).start_flag();
        while (*rt).has_threads() {
            if options.frames.is_some_and(|max| (*rt).frame >= max) {
                break;
            }
            Runtime::frame(rt)?;
        }
        let report = RunReport { frames: (*rt).frame, says: (*rt).say_count };
        if options.timing {
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            eprintln!("thatch: {} frames in {ms:.1} ms", report.frames);
        }
        Ok(report)
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

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

/// Registers the program through `init` and runs it headless until no script is left or the frame
/// limit is reached.
pub fn run(init: InitFn, options: &RunOptions) -> Result<RunReport, String> {
    let seed = options.seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
    });
    let mut runtime = Box::new(Runtime::new(seed));
    // SAFETY: `init` is the program's registration function, compiled against this ABI.
    unsafe { init(std::ptr::addr_of_mut!(*runtime)) };
    if !runtime.targets.iter().any(|t| t.is_stage) {
        return Err("the program registered no stage".to_owned());
    }
    let started = Instant::now();
    runtime.start_flag();
    while runtime.has_threads() {
        if options.frames.is_some_and(|max| runtime.frame >= max) {
            break;
        }
        runtime.frame()?;
    }
    runtime.shutdown();
    let report = RunReport { frames: runtime.frame, says: runtime.say_count };
    if options.timing {
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        eprintln!("thatch: {} frames in {ms:.1} ms", report.frames);
    }
    Ok(report)
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
pub unsafe fn main(argc: i32, argv: *const *const c_char, init: InitFn) -> i32 {
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
    match run(init, &options) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("thatch: {e}");
            1
        }
    }
}

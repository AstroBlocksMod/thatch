//! The headless host: runs a program with a virtual clock and prints what sprites say.
//!
//! It is the first implementation of the host table in `thatch_abi::Host`, and it goes through the
//! same C API (`thatch_runtime_*` in `api.rs`) a host written in C would use, so running the samples
//! exercises that surface too. It has no window, so its input comes from a script (see
//! [`parse_input_script`]) that says which keys and mouse events happen before which frame.

use std::ffi::{CStr, c_char, c_void};
use std::io::Write;
use std::ptr;
use std::time::Instant;

use thatch_abi::{Host, InitFn, OpaqueRuntime};

use crate::api;

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Stop after this many frames. The run also stops once no script is left and no input is
    /// still to come.
    pub frames: Option<u64>,
    /// Print the frame count and wall time to stderr.
    pub timing: bool,
    /// The seed of `pick random`. Random by default.
    pub seed: Option<u64>,
    /// Input to feed the program, in frame order.
    pub input: Vec<InputEvent>,
}

/// Something the player does.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// A key goes down or up, named as the `when key pressed` menu names it.
    Key { key: String, down: bool },
    /// The mouse moves to a point in Scratch coordinates.
    MouseMove { x: f64, y: f64 },
    /// The mouse button goes down or up over the named target, or over the stage.
    MouseButton { down: bool, target: Option<String> },
}

/// An input and the frame it happens before (the first frame is 0).
#[derive(Debug, Clone, PartialEq)]
pub struct InputEvent {
    pub frame: u64,
    pub input: Input,
}

/// Parses an input script: one event per line, starting with the number of the frame it happens
/// before, blank lines and `#` comments allowed.
///
/// ```text
/// 5 key down space
/// 8 key up space
/// 12 key down left arrow
/// 20 mouse 100 -50
/// 21 mouse down
/// 25 mouse up Cat
/// ```
///
/// `mouse down` and `mouse up` take the name of the target under the pointer; without one it is
/// the stage, which is also what scratch-vm picks when it has no renderer.
///
/// Words are separated by any whitespace. A key or target name is the rest of the line, so it may
/// contain spaces (`left arrow`, `Sprite 1`).
pub fn parse_input_script(text: &str) -> Result<Vec<InputEvent>, String> {
    let mut events = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = |what: &str| format!("input line {}: {what}: {line:?}", number + 1);
        let (frame, rest) = first_word(line);
        let frame: u64 = frame.parse().map_err(|_| bad("expected a frame number"))?;
        let (kind, rest) = first_word(rest);
        let input = match kind {
            "key" => {
                let (state, key) = first_word(rest);
                let down = match state {
                    "down" => true,
                    "up" => false,
                    _ => return Err(bad("expected `key down NAME` or `key up NAME`")),
                };
                if key.is_empty() {
                    return Err(bad("expected `key down NAME` or `key up NAME`"));
                }
                Input::Key { key: key.to_owned(), down }
            }
            "mouse" => match first_word(rest) {
                (state @ ("down" | "up"), target) => {
                    let target = (!target.is_empty()).then(|| target.to_owned());
                    Input::MouseButton { down: state == "down", target }
                }
                (x, rest) => {
                    let x: f64 = x.parse().map_err(|_| bad("expected `mouse X Y`, `mouse down` or `mouse up`"))?;
                    let (y, rest) = first_word(rest);
                    let y: f64 = y.parse().map_err(|_| bad("expected `mouse X Y`"))?;
                    if !rest.is_empty() {
                        return Err(bad("expected `mouse X Y`"));
                    }
                    Input::MouseMove { x, y }
                }
            },
            _ => return Err(bad("expected key or mouse after the frame")),
        };
        events.push(InputEvent { frame, input });
    }
    events.sort_by_key(|e| e.frame);
    Ok(events)
}

/// The first whitespace-separated word of `s` and the rest after the whitespace that follows it.
fn first_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim()),
        None => (s, ""),
    }
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
    // No `sprite_changed` and no `measure`: like scratch-vm without a renderer, which is what the
    // oracle behind the samples runs, nothing is fenced and motion requests no redraw.
    let host = Host {
        data: ptr::addr_of_mut!(headless).cast::<c_void>(),
        say: Some(headless_say),
        now_ms: None,
        sprite_changed: None,
        measure: None,
    };
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
        // A target the project lacks is a mistake in the script, not a click outside the stage.
        for event in &options.input {
            if let Input::MouseButton { target: Some(name), .. } = &event.input
                && target_index(rt, Some(name)).is_none()
            {
                return Err(format!("the input script clicks {name:?}, which the project has no target named"));
            }
        }
        let started = Instant::now();
        api::thatch_runtime_green_flag(rt);
        let mut next_input = 0;
        while api::thatch_runtime_has_threads(rt) != 0 || next_input < options.input.len() {
            let frame = api::thatch_runtime_frame_count(rt);
            if options.frames.is_some_and(|max| frame >= max) {
                break;
            }
            while let Some(event) = options.input.get(next_input).filter(|e| e.frame <= frame) {
                feed(rt, &event.input);
                next_input += 1;
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

/// Hands one input event to the runtime through the C API, as a host with a window would.
///
/// # Safety
/// `rt` must come from `thatch_runtime_create`, between frames.
unsafe fn feed(rt: *mut OpaqueRuntime, input: &Input) {
    // SAFETY: forwarded from the caller.
    unsafe {
        match input {
            Input::Key { key, down } => {
                api::thatch_runtime_key(rt, key.as_ptr().cast::<c_char>(), key.len(), i8::from(*down));
            }
            Input::MouseMove { x, y } => api::thatch_runtime_mouse_move(rt, *x, *y),
            Input::MouseButton { down, target } => {
                let index = target_index(rt, target.as_deref()).unwrap_or(-1);
                // A headless run drags nothing.
                api::thatch_runtime_mouse_button(rt, i8::from(*down), index, 0);
            }
        }
    }
}

/// The target with this name, or the stage for None. Scratch keeps sprite names unique, and the
/// stage's name is never a sprite's.
///
/// # Safety
/// As [`feed`].
unsafe fn target_index(rt: *mut OpaqueRuntime, target: Option<&str>) -> Option<i64> {
    // SAFETY: forwarded from the caller.
    unsafe {
        let count = api::thatch_runtime_target_count(rt);
        (0..count).find(|i| match target {
            Some(name) => {
                let mut len = 0;
                let p = api::thatch_runtime_target_name(rt, *i, &raw mut len);
                !p.is_null() && std::slice::from_raw_parts(p.cast::<u8>(), len) == name.as_bytes()
            }
            None => api::thatch_runtime_target_is_stage(rt, *i) != 0,
        })
    }
}

/// Parses the command line of a compiled program: `--frames N`, `--timing`, `--seed N`,
/// `--input FILE`.
pub fn parse_args(args: &[String]) -> Result<RunOptions, String> {
    let mut options = RunOptions::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--timing" => options.timing = true,
            "--input" => {
                let path = args.get(i + 1).ok_or_else(|| "--input needs a file".to_owned())?;
                let text = std::fs::read_to_string(path).map_err(|e| format!("could not read {path}: {e}"))?;
                options.input = parse_input_script(&text)?;
                i += 1;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_scripts_parse() {
        let events = parse_input_script("# a comment\n\n8 key up space\n5 key down left arrow\n20 mouse 100 -50.5\n21 mouse down\n25 mouse up Cat\n")
            .unwrap();
        assert_eq!(
            events,
            [
                InputEvent { frame: 5, input: Input::Key { key: "left arrow".into(), down: true } },
                InputEvent { frame: 8, input: Input::Key { key: "space".into(), down: false } },
                InputEvent { frame: 20, input: Input::MouseMove { x: 100.0, y: -50.5 } },
                InputEvent { frame: 21, input: Input::MouseButton { down: true, target: None } },
                InputEvent { frame: 25, input: Input::MouseButton { down: false, target: Some("Cat".into()) } },
            ]
        );
        assert!(parse_input_script("x key down a").is_err());
        assert!(parse_input_script("1 key sideways a").is_err());
        assert!(parse_input_script("1 mouse 3").is_err());
        assert!(parse_input_script("1 touch 3 4").is_err());
        assert!(parse_input_script("1 key down").is_err());
        assert!(parse_input_script("1 mouse 3 4 5").is_err());
    }

    #[test]
    fn input_scripts_take_any_whitespace() {
        let events = parse_input_script("5  key\tdown  space\n20\tmouse 10  20\n25 mouse up  Sprite 1 \n").unwrap();
        assert_eq!(
            events,
            [
                InputEvent { frame: 5, input: Input::Key { key: "space".into(), down: true } },
                InputEvent { frame: 20, input: Input::MouseMove { x: 10.0, y: 20.0 } },
                InputEvent { frame: 25, input: Input::MouseButton { down: false, target: Some("Sprite 1".into()) } },
            ]
        );
    }
}

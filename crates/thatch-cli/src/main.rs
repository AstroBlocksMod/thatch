//! The `thatch` command.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, anyhow, bail};
use clap::{Parser, Subcommand};
use thatch_runtime::RunOptions;

#[derive(Parser)]
#[command(name = "thatch", version, about = "Compiles Scratch projects to native code")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Runs a project. By default it is built for the host with a window into a temporary
    /// directory, which is removed when the game ends, and started from there. With --host
    /// headless it is compiled in memory instead and prints what sprites say.
    Run {
        /// An .sb3 file or a project.json, which has no costumes and so always runs headless.
        project: PathBuf,
        /// The host to run on, as for `thatch build`.
        #[arg(long)]
        host: Option<String>,
        /// Where the host's build output is, as for `thatch build`.
        #[arg(long)]
        host_dir: Option<PathBuf>,
        /// The runtime library to link against, as for `thatch build`. Not used headless.
        #[arg(long)]
        runtime_lib: Option<PathBuf>,
        /// Stop after this many frames.
        #[arg(long)]
        frames: Option<u64>,
        /// Print the frame count and wall time to stderr. Headless only.
        #[arg(long)]
        timing: bool,
        /// The seed of `pick random`.
        #[arg(long)]
        seed: Option<u64>,
        /// A script of key and mouse events to feed the program, one per line: `FRAME key down
        /// NAME`, `FRAME key up NAME`, `FRAME mouse X Y`, `FRAME mouse down [TARGET]`, `FRAME
        /// mouse up [TARGET]`. See samples/input/input.txt.
        #[arg(long)]
        input: Option<PathBuf>,
    },
    /// Compiles a project to a game with a window (or, with --host headless, one that prints what
    /// sprites say), and writes the license notices that have to ship with it next to it, as
    /// OUTPUT.licenses.txt.
    Build {
        project: PathBuf,
        /// The executable to write (or the object file with --object).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Write an object file and do not link.
        #[arg(long)]
        object: bool,
        /// Leave out `main`, for a host that provides its own and drives the program through the C
        /// API in thatch.h. Only with --object.
        #[arg(long, requires = "object")]
        no_main: bool,
        /// The runtime library to link against. Defaults to $THATCH_RUNTIME_LIB, then
        /// libthatch_runtime.a next to this executable, then lib/libthatch_runtime.a in Thatch's
        /// data directory, where installing a host puts one. --host-dir does not change where it
        /// is looked for; pass this for a runtime library somewhere else. Its license notices are
        /// expected next to it, with the extension .licenses.txt in place of .a, where building
        /// the runtime puts them.
        #[arg(long)]
        runtime_lib: Option<PathBuf>,
        /// The host the game runs on. `scratch-everywhere` opens a window and draws the stage; it
        /// needs the project as an .sb3, links the host's adapter, and puts the host's shared
        /// library, the project file and the host's license texts next to the executable. Games
        /// built into one directory share that library, and each build replaces it, so build them
        /// all with the same host build. `headless` builds a game without a window that prints what sprites say. Without this
        /// option an .sb3 gets a window when the scratch-everywhere host is installed, and is
        /// built headless with a note when it is not; a project.json is always headless.
        #[arg(long, conflicts_with = "object")]
        host: Option<String>,
        /// Where the host's build output is: the directory with its thatch-host.txt. Defaults to
        /// $THATCH_HOST_DIR, then hosts/HOST next to this executable, then the host's build
        /// directory in the checkout this thatch was built in (so that a fresh build wins over an
        /// older install), then hosts/HOST in Thatch's data directory (where `cmake --install`
        /// puts a host). The data directory is $THATCH_HOME, else
        /// ~/Library/Application Support/thatch on macOS and $XDG_DATA_HOME/thatch (by default
        /// ~/.local/share/thatch) elsewhere.
        #[arg(long, conflicts_with = "object")]
        host_dir: Option<PathBuf>,
    },
    /// Prints the IR of a project.
    Dump { project: PathBuf },
    /// Lowers a project and reports what is not supported, without compiling it.
    Check { project: PathBuf },
}

/// The stack the compiler runs on. Lowering and code generation recurse once per nesting level, and
/// a project may nest blocks up to `thatch_scratch::MAX_NESTING` deep, which needs more than the
/// main thread's stack in a debug build. Only the pages actually used cost memory.
const COMPILER_STACK: usize = 256 << 20;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    std::thread::Builder::new()
        .name("thatch".to_owned())
        .stack_size(COMPILER_STACK)
        .spawn(move || command(cli))?
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

fn command(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Cmd::Run { project, host, host_dir, runtime_lib, frames, timing, seed, input } => {
            // --timing measures the in-memory run, so without a host asked for it means headless,
            // as `thatch run --timing` always did.
            let host = if timing && host.is_none() && host_dir.is_none() {
                None
            } else {
                choose_host(host, host_dir, &project, "runs")?
            };
            if let Some(host) = host {
                if timing {
                    bail!("--timing is only for --host {HEADLESS}");
                }
                return run_on_host(&project, host, runtime_lib, frames, seed, input);
            }
            if runtime_lib.is_some() {
                bail!(
                    "--runtime-lib is for a game that is linked; a headless run compiles in memory and links nothing"
                );
            }
            let input = match input {
                Some(path) => {
                    let text =
                        std::fs::read_to_string(&path).with_context(|| format!("could not read {}", path.display()))?;
                    thatch_runtime::parse_input_script(&text).map_err(anyhow::Error::msg)?
                }
                None => Vec::new(),
            };
            run(&project, RunOptions { frames, timing, seed, input })
        }
        Cmd::Build { project, output, object, no_main, runtime_lib, host, host_dir } => {
            // An object file is linked by whoever asked for it, so no host is looked for.
            let host = if object { None } else { choose_host(host, host_dir, &project, "is built")? };
            build(&project, output, object, !no_main, runtime_lib, host)
        }
        Cmd::Dump { project } => {
            let lowered = lower(&project)?;
            print!("{}", thatch_ir::dump(&lowered.program));
            Ok(())
        }
        Cmd::Check { project } => {
            let lowered = lower(&project)?;
            if lowered.warnings.is_empty() {
                println!("everything in {} is supported", project.display());
            }
            Ok(())
        }
    }
}

fn lower(project: &Path) -> anyhow::Result<thatch_scratch::Lowered> {
    let p = thatch_sb3::Project::read(project)?;
    let lowered = thatch_scratch::lower(&p)?;
    for w in &lowered.warnings {
        eprintln!("warning: {w}");
    }
    Ok(lowered)
}

/// The host a game runs on. A host that was asked for has to be there. Without one, an .sb3 gets
/// the default host with a window if it is installed; otherwise, and for a project.json (also one
/// in a file named .sb3), which has no costumes to draw, the game is headless, as `thatch` needs nothing else for that. The
/// fallback says so, since the player gets no window: `what` is "runs" or "is built".
fn choose_host(
    name: Option<String>,
    host_dir: Option<PathBuf>,
    project: &Path,
    what: &str,
) -> anyhow::Result<Option<Host>> {
    if let Some(name) = name {
        return resolve_host(&name, host_dir);
    }
    // A directory given for the host means the host is wanted.
    if host_dir.is_some() {
        return resolve_host(DEFAULT_HOST, host_dir);
    }
    // A bare project.json is headless whatever it is called, since it is the content that tells
    // the formats apart. A project that cannot be read is too, and reading it reports why.
    if project.extension().is_none_or(|e| !e.eq_ignore_ascii_case("sb3"))
        || thatch_sb3::is_bare_json(project).unwrap_or(true)
    {
        return Ok(None);
    }
    match find_host(DEFAULT_HOST, None) {
        Ok(host) => Ok(Some(host)),
        Err(e) if e.is::<HostNotFound>() => {
            eprintln!(
                "note: the {DEFAULT_HOST} host is not installed, so this game {what} without a window. Install it \
                 as docs/scratch-everywhere.md describes to get one, or pass --host {HEADLESS} to go without this note"
            );
            Ok(None)
        }
        // A host that is there but cannot be used is not the same as none: it needs fixing, and
        // falling back would hide that.
        Err(e) => Err(e.context(format!(
            "the {DEFAULT_HOST} host is installed but cannot be used; build it again, or pass --host {HEADLESS}"
        ))),
    }
}

/// The error for a host whose build output is nowhere to be found, which `choose_host` tells
/// apart from a host that was found but cannot be used.
#[derive(Debug)]
struct HostNotFound(String);

impl std::fmt::Display for HostNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HostNotFound {}

/// Thatch's data directory, where hosts are installed: $THATCH_HOME, else the platform's place
/// for an application's data.
fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("THATCH_HOME").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").filter(|d| !d.is_empty()).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home.map(|h| h.join("Library/Application Support/thatch"));
    }
    match std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir).join("thatch")),
        None => home.map(|h| h.join(".local/share/thatch")),
    }
}

/// The host called `name`: None for the headless one, which is built in, else its build output.
fn resolve_host(name: &str, host_dir: Option<PathBuf>) -> anyhow::Result<Option<Host>> {
    if name != HEADLESS {
        return find_host(name, host_dir).map(Some);
    }
    if host_dir.is_some() {
        bail!("--host-dir names where a host with a window is; the headless host is built in");
    }
    Ok(None)
}

/// `thatch run` on a host with a window: builds the game into a temporary directory, as `thatch
/// build` would, and runs it from there until it ends. The directory goes with it.
fn run_on_host(
    project: &Path,
    host: Host,
    runtime_lib: Option<PathBuf>,
    frames: Option<u64>,
    seed: Option<u64>,
    input: Option<PathBuf>,
) -> anyhow::Result<()> {
    // thatch handles Ctrl-C from before the temporary directory exists until the game has ended,
    // so that the directory is removed after all. An interrupt while the game is built stops
    // thatch once the build is over (a compiler driver it runs is interrupted along with it, in a
    // terminal), and the game is not started.
    let (interrupts, interrupted) = std::sync::mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = interrupts.send(());
    })
    .context("could not set up Ctrl-C handling")?;
    let dir =
        tempfile::Builder::new().prefix("thatch-run-").tempdir().context("could not create a temporary directory")?;
    let stem = project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "game".to_owned());
    let exe = dir.path().join(stem);
    let built = build(project, Some(exe.clone()), false, true, runtime_lib, Some(host));
    if interrupted.try_recv().is_ok() {
        drop(dir);
        std::process::exit(128 + 2);
    }
    built?;
    // Only what was asked for is passed on, since a host's adapter takes the options it knows.
    let mut game = Command::new(&exe);
    if let Some(frames) = frames {
        game.arg("--frames").arg(frames.to_string());
    }
    if let Some(seed) = seed {
        game.arg("--seed").arg(seed.to_string());
    }
    if let Some(input) = input {
        game.arg("--input").arg(input);
    }
    let mut game = game.spawn().with_context(|| format!("could not start {}", exe.display()))?;
    let status = wait_for_game(&mut game, &interrupted)?;
    drop(dir);
    if !status.success() {
        // The game has said what went wrong; thatch ends as it did, as a shell would show it.
        std::process::exit(exit_code(status));
    }
    Ok(())
}

/// Waits for the game `thatch run` started, through the interrupts thatch gets meanwhile.
///
/// Ctrl-C in a terminal interrupts the game and thatch alike, so the game is ending already and
/// thatch only waits. An interrupt sent to thatch alone has to end the game too, so when the game
/// is still running a moment after an interrupt, thatch ends it. It does not interrupt the game a
/// second time, which a game that is merely slow to quit would take as Ctrl-C pressed twice.
fn wait_for_game(
    game: &mut std::process::Child,
    interrupted: &std::sync::mpsc::Receiver<()>,
) -> anyhow::Result<std::process::ExitStatus> {
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::{Duration, Instant};
    const GRACE: Duration = Duration::from_secs(1);
    let mut first: Option<Instant> = None;
    let mut ended = false;
    loop {
        if let Some(status) = game.try_wait().context("could not wait for the game")? {
            return Ok(status);
        }
        match interrupted.recv_timeout(Duration::from_millis(20)) {
            Ok(()) => {
                first.get_or_insert_with(Instant::now);
            }
            Err(RecvTimeoutError::Timeout) => {}
            // The handler keeps the sender for good, so this does not happen; it would spin.
            Err(RecvTimeoutError::Disconnected) => std::thread::sleep(Duration::from_millis(20)),
        }
        if !ended && first.is_some_and(|at| at.elapsed() >= GRACE) {
            // The game has not been reaped (`try_wait` found it running), so this cannot end
            // another process that took its id.
            game.kill().context("could not end the game")?;
            ended = true;
        }
    }
}

/// The status a process ended with, as a shell gives it: its exit code, or 128 plus the signal
/// that ended it.
fn exit_code(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return 128 + signal;
    }
    status.code().unwrap_or(1)
}

fn run(project: &Path, options: RunOptions) -> anyhow::Result<()> {
    let lowered = lower(project)?;
    let compiled = thatch_codegen::compile_jit(&lowered.program, &thatch_runtime::symbols())?;
    // SAFETY: the address is the init function the compiler just produced for this ABI.
    let init: thatch_runtime::InitFn =
        unsafe { std::mem::transmute::<*const u8, thatch_runtime::InitFn>(compiled.init_address()) };
    thatch_runtime::run(init, compiled.abi_version(), &options).map_err(anyhow::Error::msg)?;
    Ok(())
}

/// The libraries a Rust static library needs from the system, as `rustc --print
/// native-static-libs` lists them for each platform.
fn system_libs() -> anyhow::Result<&'static [&'static str]> {
    Ok(match std::env::consts::OS {
        "linux" => &["-lgcc_s", "-lutil", "-lrt", "-lpthread", "-lm", "-ldl", "-lc"],
        // rustc also lists -lSystem, -lc and -lm, but cc links libSystem, which holds both of the
        // others, anyway; naming it again only makes the linker warn about a duplicate.
        "macos" => &["-liconv"],
        "freebsd" | "openbsd" | "netbsd" => &["-lexecinfo", "-lpthread", "-lm", "-lc"],
        other => bail!("linking is not supported on {other} yet; use --object and link by hand"),
    })
}

/// A host with a window, as the `thatch-host.txt` in its build output describes it. `thatch`
/// knows nothing else about a host: the file says what to link and what to put next to the game.
///
/// The file has one `key = value` per line, with `#` comments. Paths are relative to its
/// directory.
///
/// ```text
/// adapter = libthatch_host_scratch_everywhere.a
/// library = libscratch-everywhere.so
/// notices = licenses.txt
/// linker = c++
/// project = sb3
/// ```
struct Host {
    name: String,
    /// The static library with the host's `main` and its implementation of the host table.
    adapter: PathBuf,
    /// Shared libraries the adapter needs, which are linked and copied next to the executable.
    libraries: Vec<PathBuf>,
    /// The license texts the host adds to the notices.
    notices: Option<PathBuf>,
    /// The compiler driver to link with: `c++` for an adapter written in C++.
    linker: String,
    /// When the host reads the project file at run time: the extension to copy it under, next to
    /// the executable.
    project_extension: Option<String>,
}

/// The host `thatch build` builds for unless told otherwise: games get a window.
const DEFAULT_HOST: &str = "scratch-everywhere";
/// The host built into the runtime, which needs no build output of its own.
const HEADLESS: &str = "headless";
/// The compiler drivers a host's description may ask to link with.
const LINKERS: &[&str] = &["cc", "c++"];

/// A path from the environment, unless the variable is unset or empty.
fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Finds the build output of the host called `name` and reads its description.
fn find_host(name: &str, dir: Option<PathBuf>) -> anyhow::Result<Host> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("{name:?} is not the name of a host");
    }
    const MANIFEST: &str = "thatch-host.txt";
    let mut candidates = Vec::new();
    if let Some(dir) = dir {
        candidates.push(dir);
    } else if let Some(dir) = env_path("THATCH_HOST_DIR") {
        candidates.push(dir);
    } else {
        let beside = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_owned));
        if let Some(beside) = &beside {
            // A packaged thatch has its hosts next to it.
            candidates.push(beside.join("hosts").join(name));
            // In a checkout, thatch is target/<profile>/thatch, or target/<triple>/<profile>/thatch
            // when built for a target, and a host is built in hosts/<name>/build. That comes
            // before the data directory, so that a thatch built in a checkout uses the host built
            // with it rather than an older one installed from it, which may be for an older ABI.
            for checkout in beside.ancestors().skip(2).take(2) {
                candidates.push(checkout.join("hosts").join(name).join("build/lib"));
            }
        }
        // `cmake --install` puts a host in the data directory, which is how a thatch from
        // `cargo install`, which comes alone, finds one.
        if let Some(data) = data_dir() {
            candidates.push(data.join("hosts").join(name));
        }
    }
    // Only a description that is not there sends the search on. One that cannot be looked at (a
    // directory without permission to enter, say) may be the host that was meant, so it is an
    // error and not a reason to go on without it.
    let mut found = None;
    for dir in &candidates {
        let manifest = dir.join(MANIFEST);
        match std::fs::metadata(&manifest) {
            Ok(meta) if meta.is_file() => {
                found = Some(dir);
                break;
            }
            Ok(_) => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => {}
            Err(e) => return Err(anyhow::Error::new(e).context(format!("could not look at {}", manifest.display()))),
        }
    }
    let Some(dir) = found else {
        let looked: Vec<String> = candidates.iter().map(|d| d.display().to_string()).collect();
        return Err(HostNotFound(format!(
            "the host {name} was not found: no {MANIFEST} in {}. Build the host first (see the docs), \
             point --host-dir or THATCH_HOST_DIR at its build output, or build a game without a window \
             with --host {HEADLESS}",
            looked.join(" or ")
        ))
        .into());
    };
    let manifest = dir.join(MANIFEST);
    let text = std::fs::read_to_string(&manifest).with_context(|| format!("could not read {}", manifest.display()))?;
    let mut host = Host {
        name: name.to_owned(),
        adapter: PathBuf::new(),
        libraries: Vec::new(),
        notices: None,
        linker: "cc".to_owned(),
        project_extension: None,
    };
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let entry = line.split_once('=').map(|(key, value)| (key.trim(), value.trim()));
        let at = || format!("{} line {}", manifest.display(), number + 1);
        // The description is read from a directory found by path, so what it may make thatch do
        // is limited: name files of that directory only, and link with a compiler driver.
        let file = |value: &str| -> anyhow::Result<PathBuf> {
            let plain = !value.is_empty()
                && value != "."
                && value != ".."
                && !value.contains(['/', '\\'])
                && Path::new(value).file_name() == Some(std::ffi::OsStr::new(value));
            if !plain {
                bail!("{}: {value:?} is not the name of a file in the host's directory", at());
            }
            Ok(dir.join(value))
        };
        match entry {
            Some(("adapter", value)) => host.adapter = file(value)?,
            Some(("library", value)) => host.libraries.push(file(value)?),
            Some(("notices", value)) => host.notices = Some(file(value)?),
            Some(("linker", value)) if LINKERS.contains(&value) => host.linker = value.to_owned(),
            Some(("linker", value)) => bail!("{}: the linker {value:?} is not one of {}", at(), LINKERS.join(", ")),
            Some(("project", value)) if !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric()) => {
                host.project_extension = Some(value.to_owned());
            }
            _ => bail!("{}: expected adapter, library, notices, linker or project", at()),
        }
    }
    if host.adapter.as_os_str().is_empty() {
        bail!("{} names no adapter", manifest.display());
    }
    for file in std::iter::once(&host.adapter).chain(&host.libraries).chain(&host.notices) {
        if !file.is_file() {
            bail!("{} names {}, which is not there; build the host again", manifest.display(), file.display());
        }
    }
    Ok(host)
}

/// The linker option that makes an executable look for shared libraries in its own directory.
fn rpath_beside_executable() -> &'static str {
    if cfg!(target_os = "macos") { "-Wl,-rpath,@executable_path" } else { "-Wl,-rpath,$ORIGIN" }
}

/// `path` with `suffix` added to its whole file name, so that `game` and `game.exe` cannot collide.
fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(suffix);
    path.with_file_name(name)
}

fn build(
    project: &Path,
    output: Option<PathBuf>,
    object_only: bool,
    with_main: bool,
    runtime_lib: Option<PathBuf>,
    host: Option<Host>,
) -> anyhow::Result<()> {
    if let Some(host) = &host
        && let Some(extension) = &host.project_extension
        && project.extension().is_none_or(|e| !e.eq_ignore_ascii_case(extension.as_str()))
    {
        bail!(
            "the {} host reads the project's costumes when the game runs, so it needs the project as an .{extension} file, \
             not {}; build it with --host {HEADLESS} for a game without a window",
            host.name,
            project.display()
        );
    }
    let stem = project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "program".to_owned());
    let output = output.unwrap_or_else(|| PathBuf::from(if object_only { format!("{stem}.o") } else { stem.clone() }));
    // Everything is written into a fresh private directory next to the output and moved into
    // place at the end, so a failed build leaves an existing output alone and nobody can plant a
    // path for the intermediate files. Each kind of file has a directory of its own in there, so
    // that no project or output name can make two of them the same file.
    let parent = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;
    let work = tempfile::Builder::new()
        .prefix(".thatch-build-")
        .tempdir_in(parent)
        .with_context(|| format!("could not create a work directory in {}", parent.display()))?;
    let dir = |name: &str| -> anyhow::Result<PathBuf> {
        let dir = work.path().join(name);
        std::fs::create_dir(&dir).with_context(|| format!("could not create {}", dir.display()))?;
        Ok(dir)
    };
    let (source_dir, link_dir, libraries_dir) = (dir("source")?, dir("link")?, dir("libraries")?);
    let (staging, notices_dir) = (dir("ship")?, dir("notices")?);
    dir("old")?;

    // The project is read once, into the work directory, and that copy is both compiled and
    // shipped, so that a project saved again in the middle of the build cannot give the game the
    // code of one version and the costumes of another.
    let snapshot = source_dir.join(project.file_name().unwrap_or(std::ffi::OsStr::new("project")));
    std::fs::copy(project, &snapshot).with_context(|| format!("could not read {}", project.display()))?;
    // A host that reads the project file has to get the format it reads, which is told by the
    // content, not by the name: a project.json in a file named .sb3 is no archive.
    if let Some(host) = &host
        && let Some(extension) = &host.project_extension
    {
        let bare_json = thatch_sb3::is_bare_json(&snapshot)?;
        let mismatch = match extension.to_ascii_lowercase().as_str() {
            "sb3" => bare_json,
            "json" => !bare_json,
            _ => false,
        };
        if mismatch {
            let holds = if bare_json { "a bare project.json" } else { "an .sb3 archive" };
            bail!(
                "{} holds {holds}, but the {} host reads the project as an .{extension} file when the game runs; \
                 build it with --host {HEADLESS} for a game without a window",
                project.display(),
                host.name
            );
        }
    }
    let lowered = lower(&snapshot)?;
    // A host brings its own main.
    let bytes = thatch_codegen::compile_object(&lowered.program, with_main && host.is_none())?;
    let object = link_dir.join("program.o");
    std::fs::write(&object, bytes).with_context(|| format!("could not write {}", object.display()))?;
    if object_only {
        return publish(&[(object, output)], work, project, &|| Ok(()));
    }
    // The host's shared libraries are copied too, and the game is linked against the copies,
    // named after them and shipped with them, so that a host rebuilt meanwhile cannot ship a
    // library other than the one the game was linked with. They all come from the host's one
    // directory, so their file names differ.
    let libraries: Vec<PathBuf> = match &host {
        Some(host) => host
            .libraries
            .iter()
            .map(|library| {
                let copy = libraries_dir.join(library.file_name().context("a host library has no file name")?);
                std::fs::copy(library, &copy).with_context(|| format!("could not copy {}", library.display()))?;
                Ok(copy)
            })
            .collect::<anyhow::Result<_>>()?,
        None => Vec::new(),
    };
    let runtime = find_runtime_lib(runtime_lib)?;
    let mut notices = runtime_notices(&runtime)?;
    let exe = link_dir.join("program");
    let linker = host.as_ref().map_or("cc", |h| h.linker.as_str());
    let mut link = Command::new(linker);
    link.arg("-o").arg(&exe).arg(&object);
    if let Some(host) = &host {
        // The adapter comes before the runtime it calls, and the shared libraries after both.
        link.arg(&host.adapter);
    }
    link.arg(&runtime.path);
    if !libraries.is_empty() {
        for library in &libraries {
            link_shared(&mut link, library);
        }
        link.arg(rpath_beside_executable());
    }
    let status = link
        .args(system_libs()?)
        .status()
        .with_context(|| format!("could not run {linker}; a C compiler is needed to link"))?;
    if !status.success() {
        bail!("linking failed");
    }

    // What ships with the executable goes next to it, the executable last.
    let mut moves = Vec::new();
    let mut in_place = false;
    if let Some(host) = &host {
        // The shared libraries the game loads from its own directory. Shipping them as separate
        // files is also what lets a player replace one, which the LGPL asks for. A rebuild
        // replaces the copy an earlier build put there, so games built into one directory share
        // it and have to be built with the same host build. Each goes under the name the
        // executable asks the loader for, which can differ from the library's file name.
        for (library, original) in libraries.iter().zip(&host.libraries) {
            let name = load_name(library, original)?;
            let staged = staging.join(&name);
            std::fs::rename(library, &staged).with_context(|| format!("could not move {}", library.display()))?;
            moves.push((staged, output.with_file_name(&name)));
        }
        if let Some(extension) = &host.project_extension {
            let to = beside(&output, &format!(".{extension}"));
            // The project is where the game looks for it already, as when `thatch build game.sb3`
            // builds `game` next to it: shipping the copy would replace the project with itself.
            // It stays as it is, and `publish` checks that it did not change while the game was
            // built.
            in_place = same_file(project, &to);
            if !in_place {
                moves.push((snapshot.clone(), to));
            }
        }
        if let Some(path) = &host.notices {
            let text = std::fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
            notices.push_str("\n\n");
            notices.push_str(&text);
        }
    }
    // The notices: whoever ships the executable has to ship them too.
    // They have a directory of their own, so that no host library's name can be theirs.
    let notices_file = notices_dir.join("licenses.txt");
    std::fs::write(&notices_file, notices).context("could not write the license notices")?;
    moves.push((notices_file, beside(&output, ".licenses.txt")));
    moves.push((exe, output));
    // A project left in place has to be the version the game was built from once everything is
    // in place, or the build is undone: an editor saving it meanwhile does not wait for thatch.
    let unchanged = || -> anyhow::Result<()> {
        if !in_place {
            return Ok(());
        }
        let now = std::fs::read(project).with_context(|| format!("could not read {}", project.display()))?;
        if now != std::fs::read(&snapshot).context("could not read the copy of the project")? {
            bail!(
                "{} changed while the game was built, so the game's code would be from the old version; build it again",
                project.display()
            );
        }
        Ok(())
    };
    publish(&moves, work, project, &unchanged)
}

/// Whether two paths name the same existing file, however they are spelled.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        #[cfg(unix)]
        (Ok(a), Ok(b)) => {
            use std::os::unix::fs::MetadataExt;
            (a.dev(), a.ino()) == (b.dev(), b.ino())
        }
        #[cfg(not(unix))]
        (Ok(_), Ok(_)) => std::fs::canonicalize(a).ok() == std::fs::canonicalize(b).ok(),
        _ => false,
    }
}

/// The file name a game asks the loader for when it loads a host's shared library, which is the
/// name it has to have next to the game. An ELF library linked by file name is recorded under
/// its soname when it has one. A macOS library is recorded under its install name, which has to
/// be relative to the rpath (or the executable) for the library to be found next to the game.
/// Without either, it is the file name. `library` is the build's copy of the host's library
/// `original`, which the messages name.
fn load_name(library: &Path, original: &Path) -> anyhow::Result<std::ffi::OsString> {
    let file_name = library.file_name().context("a host library has no file name")?.to_owned();
    let data = std::fs::read(library).with_context(|| format!("could not read {}", original.display()))?;
    let (recorded, is_macho) =
        recorded_name(&data).with_context(|| format!("could not read {} as a shared library", original.display()))?;
    let library = original;
    let Some(recorded) = recorded else { return Ok(file_name) };
    let recorded = String::from_utf8(recorded)
        .map_err(|_| anyhow!("{} names itself with something other than text", library.display()))?;
    let name = if is_macho {
        ["@rpath/", "@executable_path/", "@loader_path/"].iter().find_map(|p| recorded.strip_prefix(p)).with_context(
            || {
                format!(
                    "{} has the install name {recorded}, so a game would load it from there and not from its own \
                     directory; the host has to build it with an install name of @rpath/{}",
                    library.display(),
                    file_name.to_string_lossy()
                )
            },
        )?
    } else {
        recorded.as_str()
    };
    if name.is_empty() || name.contains('/') {
        bail!("{} names itself {recorded}, which is not a file name next to the game", library.display());
    }
    Ok(name.into())
}

/// The name a shared library records for itself (an ELF soname, a Mach-O install name), and
/// whether it is a Mach-O library.
fn recorded_name(data: &[u8]) -> anyhow::Result<(Option<Vec<u8>>, bool)> {
    use object::FileKind;
    use object::read::elf::{ElfFile32, ElfFile64};
    use object::read::macho::{MachOFatFile32, MachOFatFile64, MachOFile32, MachOFile64};
    Ok(match FileKind::parse(data)? {
        FileKind::Elf32 => (soname(&ElfFile32::parse(data)?, data)?, false),
        FileKind::Elf64 => (soname(&ElfFile64::parse(data)?, data)?, false),
        FileKind::MachO32 => (install_name(&MachOFile32::parse(data)?)?, true),
        FileKind::MachO64 => (install_name(&MachOFile64::parse(data)?)?, true),
        FileKind::MachOFat32 => (universal_name(MachOFatFile32::parse(data)?.arches(), data)?, true),
        FileKind::MachOFat64 => (universal_name(MachOFatFile64::parse(data)?.arches(), data)?, true),
        _ => (None, false),
    })
}

/// The install name of a universal library. A game is linked against the part for its own
/// architecture and records that part's name, while the library ships under one name, so every
/// part has to give the same one.
fn universal_name(parts: &[impl object::read::macho::FatArch], data: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
    let mut names = parts.iter().map(|part| -> anyhow::Result<_> { Ok(recorded_name(part.data(data)?)?.0) });
    let first = names.next().transpose()?.flatten();
    for name in names {
        if name? != first {
            bail!("its architectures give it different install names, and it can ship under only one");
        }
    }
    Ok(first)
}

fn soname<'data, Elf: object::read::elf::FileHeader<Endian = object::Endianness>>(
    file: &object::read::elf::ElfFile<'data, Elf>,
    data: &'data [u8],
) -> object::Result<Option<Vec<u8>>> {
    let table = file.elf_section_table().dynamic_table(file.endian(), data)?;
    for entry in &table {
        if entry.tag == object::elf::DT_SONAME {
            return Ok(Some(table.string(entry)?.to_vec()));
        }
    }
    Ok(None)
}

fn install_name<Mach: object::read::macho::MachHeader<Endian = object::Endianness>>(
    file: &object::read::macho::MachOFile<'_, Mach>,
) -> object::Result<Option<Vec<u8>>> {
    let mut commands = file.macho_load_commands()?;
    while let Some(command) = commands.next()? {
        if let object::read::macho::LoadCommandVariant::IdDylib(dylib) = command.variant()? {
            return Ok(Some(command.string(file.endian(), dylib.dylib.name)?.to_vec()));
        }
    }
    Ok(None)
}

/// Links a host's shared library. On Linux by file name from its directory (`-l:name`), so that a
/// library built without a soname is still recorded by name and found next to the game through
/// the rpath, not at the path it had when the game was built. Elsewhere by path: a macOS library
/// carries the name it is loaded by in itself.
fn link_shared(link: &mut Command, library: &Path) {
    match (cfg!(target_os = "linux"), library.parent(), library.file_name()) {
        (true, Some(dir), Some(name)) => {
            link.arg("-L").arg(dir).arg(format!("-l:{}", name.to_string_lossy()));
        }
        _ => {
            link.arg(library);
        }
    }
}

/// Moves the files of a build into place, as `(from, to)` pairs in order, all into one
/// directory. Two files may not go to the same place, none where a directory is, and none over
/// the project being built. Once all are in place, `check` says whether the build still stands.
/// When a move or the check fails, the moves before it are undone and the files they replaced
/// put back, so that a failed build leaves the old output as it was.
///
/// `work` is the build's own directory next to the output. The files being replaced are kept in
/// its `old` directory until then, under names nobody else uses, and go with it. A file that
/// cannot be put back is not lost: the work directory is then kept, and the error says where.
fn publish(
    moves: &[(PathBuf, PathBuf)],
    work: tempfile::TempDir,
    project: &Path,
    check: &dyn Fn() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let Some((_, first)) = moves.first() else { return Ok(()) };
    let parent = first.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    // Builds into the same directory move their files one build at a time, so that a game's
    // executable, libraries, project and notices all come from the same build.
    let _lock = lock_directory(parent)?;
    for (i, (_, to)) in moves.iter().enumerate() {
        if moves[..i].iter().any(|(_, earlier)| earlier == to) {
            bail!("two of the files of this build would be written to {}; choose another output name", to.display());
        }
        if same_file(project, to) {
            bail!(
                "{} is the project being built, which the build would replace; choose another output name",
                to.display()
            );
        }
        if to.symlink_metadata().is_ok_and(|m| m.is_dir()) {
            bail!("{} is a directory, so the build cannot be written there; choose another output name", to.display());
        }
    }
    let old = work.path().join("old");
    // Each file a move replaces is kept first, so that it can be put back.
    let mut done: Vec<(&Path, &Path, Option<PathBuf>)> = Vec::new();
    let mut lost: Vec<String> = Vec::new();
    let mut failure = None;
    for (i, (from, to)) in moves.iter().enumerate() {
        // Where the file system does not tell names apart by case, two names of this build can
        // be one file, which only shows once the first has been put there.
        if done.iter().any(|(_, earlier, _)| same_file(earlier, to)) {
            failure = Some(anyhow!(
                "two of the files of this build would be written to {}, which the file system takes for the same name; \
                 choose another output name",
                to.display()
            ));
            break;
        }
        let kept = old.join(i.to_string());
        let had_old = match to.symlink_metadata() {
            Ok(_) => match std::fs::rename(to, &kept) {
                Ok(()) => true,
                // Without the old file kept, it could not be put back, so nothing replaces it.
                Err(e) => {
                    failure = Some(anyhow::Error::new(e).context(format!("could not move {} aside", to.display())));
                    break;
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => {
                failure = Some(anyhow::Error::new(e).context(format!("could not look at {}", to.display())));
                break;
            }
        };
        match std::fs::rename(from, to) {
            Ok(()) => done.push((from, to, had_old.then_some(kept))),
            Err(e) => {
                if had_old && std::fs::rename(&kept, to).is_err() {
                    lost.push(format!("the old {} is at {}", to.display(), kept.display()));
                }
                failure = Some(anyhow::Error::new(e).context(format!("could not write {}", to.display())));
                break;
            }
        }
    }
    if failure.is_none() {
        failure = check().err();
    }
    // Without a failure, the replaced files go with the build's directory.
    let Some(error) = failure else { return Ok(()) };
    for (from, to, kept) in done.into_iter().rev() {
        // A new file that cannot be taken back is replaced by the old one, or stays if there was
        // none, which leaves nothing lost.
        let _ = std::fs::rename(to, from);
        if let Some(kept) = kept
            && std::fs::rename(&kept, to).is_err()
        {
            lost.push(format!("the old {} is at {}", to.display(), kept.display()));
        }
    }
    if lost.is_empty() {
        return Err(error);
    }
    let _ = work.keep();
    Err(error.context(format!("the build failed and could not put back what it replaced: {}", lost.join("; "))))
}

/// Locks a directory for one build's moves, until the returned file is dropped. Where a
/// directory cannot be opened as a file (Windows), builds are not kept apart.
fn lock_directory(dir: &Path) -> anyhow::Result<Option<std::fs::File>> {
    if !cfg!(unix) {
        return Ok(None);
    }
    let file = std::fs::File::open(dir).with_context(|| format!("could not open {}", dir.display()))?;
    file.lock().with_context(|| format!("could not lock {} for this build", dir.display()))?;
    Ok(Some(file))
}

/// The runtime library to link.
struct RuntimeLib {
    path: PathBuf,
    /// Whether it is the library next to `thatch`, which was built together with it.
    beside_thatch: bool,
}

fn find_runtime_lib(given: Option<PathBuf>) -> anyhow::Result<RuntimeLib> {
    if let Some(path) = given {
        return Ok(RuntimeLib { path, beside_thatch: false });
    }
    if let Some(path) = env_path("THATCH_RUNTIME_LIB") {
        return Ok(RuntimeLib { path, beside_thatch: false });
    }
    let exe = std::env::current_exe().context("could not find the thatch executable")?;
    if let Some(path) = exe.parent().map(|d| d.join("libthatch_runtime.a")).filter(|p| p.exists()) {
        return Ok(RuntimeLib { path, beside_thatch: true });
    }
    // `cargo install` installs only the thatch executable. Installing a host also installs the
    // runtime library it was built and tested with, and its notices, into the data directory.
    if let Some(path) = data_dir().map(|d| d.join("lib/libthatch_runtime.a")).filter(|p| p.exists()) {
        return Ok(RuntimeLib { path, beside_thatch: false });
    }
    bail!(
        "libthatch_runtime.a was not found next to thatch or in Thatch's data directory. A thatch from \
         `cargo install` comes without it: install a host (see docs/scratch-everywhere.md), which installs \
         the runtime library too, pass --runtime-lib, or set THATCH_RUNTIME_LIB"
    )
}

/// The license notices of a runtime library: the texts that have to ship with every executable
/// linked against it. Building the runtime writes them next to the library, with the extension
/// `.licenses.txt` (see `crates/thatch-runtime/build.rs`). Without that file, the notices of the
/// runtime `thatch` itself was built with stand in. They are the right ones for the library next
/// to `thatch`; for any other library they may not be, so that case gets a warning.
fn runtime_notices(runtime: &RuntimeLib) -> anyhow::Result<String> {
    let beside = runtime.path.with_extension("licenses.txt");
    match std::fs::read_to_string(&beside) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if !runtime.beside_thatch {
                eprintln!(
                    "warning: {} was not found, so the license notices written next to the executable are those of \
                     the runtime thatch was built with; check them against the library you linked before shipping",
                    beside.display()
                );
            }
            Ok(thatch_runtime::LICENSE_NOTICES.to_owned())
        }
        Err(e) => Err(e).with_context(|| format!("could not read {}", beside.display())),
    }
}

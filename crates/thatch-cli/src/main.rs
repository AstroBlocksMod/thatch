//! The `thatch` command.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
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
    /// Compiles a project in memory and runs it headless, printing what sprites say.
    Run {
        /// An .sb3 file or a project.json.
        project: PathBuf,
        /// Stop after this many frames.
        #[arg(long)]
        frames: Option<u64>,
        /// Print the frame count and wall time to stderr.
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
    /// Compiles a project to a standalone executable, and writes the license notices that have to
    /// ship with it next to it, as OUTPUT.licenses.txt.
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
        /// The runtime library to link against. Defaults to libthatch_runtime.a next to this
        /// executable, or $THATCH_RUNTIME_LIB. Its license notices are expected next to it, with
        /// the extension .licenses.txt in place of .a, where building the runtime puts them.
        #[arg(long)]
        runtime_lib: Option<PathBuf>,
        /// Build the game for a host with a window instead of the headless one; `scratch-everywhere`
        /// is the one there is. The project has to be an .sb3. Links the host's adapter, and puts
        /// the host's shared library, the project file and the host's license texts next to the
        /// executable.
        #[arg(long, conflicts_with = "object")]
        host: Option<String>,
        /// Where the host's build output is: the directory with its thatch-host.txt. Defaults to
        /// $THATCH_HOST_DIR, then hosts/HOST next to this executable, then the host's build
        /// directory in a checkout of Thatch.
        #[arg(long, requires = "host")]
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
        Cmd::Run { project, frames, timing, seed, input } => {
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
            let host = host.map(|name| find_host(&name, host_dir)).transpose()?;
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
        "macos" => &["-liconv", "-lSystem", "-lc", "-lm"],
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

/// Finds the build output of the host called `name` and reads its description.
fn find_host(name: &str, dir: Option<PathBuf>) -> anyhow::Result<Host> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("{name:?} is not the name of a host");
    }
    const MANIFEST: &str = "thatch-host.txt";
    let mut candidates = Vec::new();
    if let Some(dir) = dir {
        candidates.push(dir);
    } else if let Some(dir) = std::env::var_os("THATCH_HOST_DIR") {
        candidates.push(PathBuf::from(dir));
    } else if let Some(beside) = std::env::current_exe().ok().as_deref().and_then(Path::parent) {
        // An installed thatch has its hosts next to it. In a checkout, thatch is
        // target/<profile>/thatch and a host is built in hosts/<name>/build.
        candidates.push(beside.join("hosts").join(name));
        candidates.push(beside.join("../../hosts").join(name).join("build/lib"));
    }
    let Some(dir) = candidates.iter().find(|dir| dir.join(MANIFEST).is_file()) else {
        let looked: Vec<String> = candidates.iter().map(|d| d.display().to_string()).collect();
        bail!(
            "the host {name} was not found: no {MANIFEST} in {}. Build the host first (see the docs), \
             or point --host-dir or THATCH_HOST_DIR at its build output",
            looked.join(" or ")
        );
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
        match entry {
            Some(("adapter", value)) => host.adapter = dir.join(value),
            Some(("library", value)) => host.libraries.push(dir.join(value)),
            Some(("notices", value)) => host.notices = Some(dir.join(value)),
            Some(("linker", value)) => host.linker = value.to_owned(),
            Some(("project", value)) => host.project_extension = Some(value.to_owned()),
            _ => bail!(
                "{} line {}: expected adapter, library, notices, linker or project",
                manifest.display(),
                number + 1
            ),
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
            "the {} host reads the project's costumes when the game runs, so it needs the project as an .{extension} file, not {}",
            host.name,
            project.display()
        );
    }
    let lowered = lower(project)?;
    // A host brings its own main.
    let bytes = thatch_codegen::compile_object(&lowered.program, with_main && host.is_none())?;
    let stem = project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "program".to_owned());
    let output = output.unwrap_or_else(|| PathBuf::from(if object_only { format!("{stem}.o") } else { stem.clone() }));
    // Everything is written into a fresh private directory next to the output and moved into
    // place at the end, so a failed build leaves an existing output alone and nobody can plant a
    // path for the intermediate files.
    let parent = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let work = tempfile::Builder::new()
        .prefix(".thatch-build-")
        .tempdir_in(parent)
        .with_context(|| format!("could not create a work directory in {}", parent.display()))?;
    let object = work.path().join(format!("{stem}.o"));
    std::fs::write(&object, bytes).with_context(|| format!("could not write {}", object.display()))?;
    if object_only {
        std::fs::rename(&object, &output).with_context(|| format!("could not write {}", output.display()))?;
        return Ok(());
    }
    let runtime = find_runtime_lib(runtime_lib)?;
    let mut notices = runtime_notices(&runtime)?;
    let exe = work.path().join(&stem);
    let linker = host.as_ref().map_or("cc", |h| h.linker.as_str());
    let mut link = Command::new(linker);
    link.arg("-o").arg(&exe).arg(&object);
    if let Some(host) = &host {
        // The adapter comes before the runtime it calls, and the shared libraries after both.
        link.arg(&host.adapter);
    }
    link.arg(&runtime.path);
    if let Some(host) = host.as_ref().filter(|h| !h.libraries.is_empty()) {
        link.args(&host.libraries).arg(rpath_beside_executable());
    }
    let status = link
        .args(system_libs()?)
        .status()
        .with_context(|| format!("could not run {linker}; a C compiler is needed to link"))?;
    if !status.success() {
        bail!("linking failed");
    }

    // What ships with the executable goes next to it. Everything is prepared in a directory of
    // its own in the work directory first and then moved into place, the executable last.
    let staging = work.path().join("ship");
    std::fs::create_dir(&staging).with_context(|| format!("could not create {}", staging.display()))?;
    let mut moves = Vec::new();
    if let Some(host) = &host {
        // The shared libraries the game loads from its own directory. Shipping them as separate
        // files is also what lets a player replace one, which the LGPL asks for.
        for library in &host.libraries {
            let name = library.file_name().context("a host library has no file name")?;
            let staged = staging.join(name);
            std::fs::copy(library, &staged).with_context(|| format!("could not copy {}", library.display()))?;
            moves.push((staged, output.with_file_name(name)));
        }
        if let Some(extension) = &host.project_extension {
            let staged = staging.join(format!("project.{extension}"));
            std::fs::copy(project, &staged).with_context(|| format!("could not copy {}", project.display()))?;
            moves.push((staged, beside(&output, &format!(".{extension}"))));
        }
        if let Some(path) = &host.notices {
            let text = std::fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
            notices.push_str("\n\n");
            notices.push_str(&text);
        }
    }
    // The notices: whoever ships the executable has to ship them too.
    let notices_file = staging.join("licenses.txt");
    std::fs::write(&notices_file, notices).context("could not write the license notices")?;
    moves.push((notices_file, beside(&output, ".licenses.txt")));
    moves.push((exe, output));
    for (from, to) in moves {
        std::fs::rename(&from, &to).with_context(|| format!("could not write {}", to.display()))?;
    }
    Ok(())
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
    if let Ok(p) = std::env::var("THATCH_RUNTIME_LIB") {
        return Ok(RuntimeLib { path: PathBuf::from(p), beside_thatch: false });
    }
    let exe = std::env::current_exe().context("could not find the thatch executable")?;
    let beside = exe.parent().map(|d| d.join("libthatch_runtime.a"));
    match beside {
        Some(path) if path.exists() => Ok(RuntimeLib { path, beside_thatch: true }),
        _ => bail!("libthatch_runtime.a was not found next to thatch; pass --runtime-lib or set THATCH_RUNTIME_LIB"),
    }
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

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
        Cmd::Build { project, output, object, no_main, runtime_lib } => {
            build(&project, output, object, !no_main, runtime_lib)
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

fn build(
    project: &Path,
    output: Option<PathBuf>,
    object_only: bool,
    with_main: bool,
    runtime_lib: Option<PathBuf>,
) -> anyhow::Result<()> {
    let lowered = lower(project)?;
    let bytes = thatch_codegen::compile_object(&lowered.program, with_main)?;
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
    let notices = runtime_notices(&runtime)?;
    let exe = work.path().join(&stem);
    let status = Command::new("cc")
        .arg("-o")
        .arg(&exe)
        .arg(&object)
        .arg(&runtime.path)
        .args(system_libs()?)
        .status()
        .context("could not run cc; a C compiler is needed to link")?;
    if !status.success() {
        bail!("linking failed");
    }
    // The notices go next to the executable, since whoever ships the one has to ship the other.
    let notices_file = work.path().join("licenses.txt");
    let notices_output = notices_path(&output);
    std::fs::write(&notices_file, notices).with_context(|| format!("could not write {}", notices_output.display()))?;
    std::fs::rename(&exe, &output).with_context(|| format!("could not write {}", output.display()))?;
    std::fs::rename(&notices_file, &notices_output)
        .with_context(|| format!("could not write {}", notices_output.display()))?;
    Ok(())
}

/// Where the license notices of the executable `output` go: next to it, under its whole file name
/// with `.licenses.txt` added, so that `game` and `game.exe` cannot collide.
fn notices_path(output: &Path) -> PathBuf {
    let mut name = output.file_name().unwrap_or_default().to_owned();
    name.push(".licenses.txt");
    output.with_file_name(name)
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

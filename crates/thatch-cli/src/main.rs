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
    },
    /// Compiles a project to a standalone executable.
    Build {
        project: PathBuf,
        /// The executable to write (or the object file with --object).
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Write an object file and do not link.
        #[arg(long)]
        object: bool,
        /// The runtime library to link against. Defaults to libthatch_runtime.a next to this
        /// executable, or $THATCH_RUNTIME_LIB.
        #[arg(long)]
        runtime_lib: Option<PathBuf>,
    },
    /// Prints the IR of a project.
    Dump { project: PathBuf },
    /// Lowers a project and reports what is not supported, without compiling it.
    Check { project: PathBuf },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Run { project, frames, timing, seed } => run(&project, RunOptions { frames, timing, seed }),
        Cmd::Build { project, output, object, runtime_lib } => build(&project, output, object, runtime_lib),
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
    runtime_lib: Option<PathBuf>,
) -> anyhow::Result<()> {
    let lowered = lower(project)?;
    let bytes = thatch_codegen::compile_object(&lowered.program)?;
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
    let exe = work.path().join(&stem);
    let status = Command::new("cc")
        .arg("-o")
        .arg(&exe)
        .arg(&object)
        .arg(&runtime)
        .args(system_libs()?)
        .status()
        .context("could not run cc; a C compiler is needed to link")?;
    if !status.success() {
        bail!("linking failed");
    }
    std::fs::rename(&exe, &output).with_context(|| format!("could not write {}", output.display()))?;
    Ok(())
}

fn find_runtime_lib(given: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(p) = given {
        return Ok(p);
    }
    if let Ok(p) = std::env::var("THATCH_RUNTIME_LIB") {
        return Ok(PathBuf::from(p));
    }
    let exe = std::env::current_exe().context("could not find the thatch executable")?;
    let beside = exe.parent().map(|d| d.join("libthatch_runtime.a"));
    match beside {
        Some(p) if p.exists() => Ok(p),
        _ => bail!("libthatch_runtime.a was not found next to thatch; pass --runtime-lib or set THATCH_RUNTIME_LIB"),
    }
}

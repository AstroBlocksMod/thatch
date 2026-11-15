//! End-to-end tests: every sample compiles and prints what scratch-vm prints for it.
//!
//! `samples/<name>/expected.txt` holds the output of the same project on scratch-vm (see
//! `samples/README.md`). Each sample runs three times: compiled in memory with `thatch run`, as a
//! linked executable from `thatch build`, and under the C host in `c_host/host.c`, which tests
//! `thatch.h` and the runtime's C API the way a real host uses them. A sample with an `input.txt`
//! gets it as `--input`; the C host takes no input, so those samples skip it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

fn samples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples")
}

fn sample_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(samples_dir())
        .expect("samples directory")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("project.json").exists())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no samples found");
    names
}

fn expected(name: &str) -> String {
    std::fs::read_to_string(samples_dir().join(name).join("expected.txt")).expect("expected.txt")
}

fn input_script(name: &str) -> Option<PathBuf> {
    let path = samples_dir().join(name).join("input.txt");
    path.exists().then_some(path)
}

/// The run arguments of a sample: the frame limit and its input script, if it has one.
fn run_args(name: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["--frames".into(), "3000".into()];
    if let Some(input) = input_script(name) {
        args.push("--input".into());
        args.push(input.into());
    }
    args
}

fn thatch() -> Command {
    Command::new(env!("CARGO_BIN_EXE_thatch"))
}

fn runtime_lib() -> PathBuf {
    let lib = Path::new(env!("CARGO_BIN_EXE_thatch")).with_file_name("libthatch_runtime.a");
    assert!(lib.exists(), "{} is missing; build the workspace first", lib.display());
    lib
}

/// The system libraries the runtime needs, as `thatch build` links them.
fn system_libs() -> &'static [&'static str] {
    if cfg!(target_os = "linux") {
        &["-lgcc_s", "-lutil", "-lrt", "-lpthread", "-lm", "-ldl", "-lc"]
    } else if cfg!(target_os = "macos") {
        &["-liconv", "-lSystem", "-lc", "-lm"]
    } else if cfg!(any(target_os = "freebsd", target_os = "openbsd", target_os = "netbsd")) {
        // As `thatch build` links on these systems.
        &["-lexecinfo", "-lpthread", "-lm", "-lc"]
    } else {
        &["-lpthread", "-lm", "-lc"]
    }
}

/// A directory for build output, removed when the guard drops, also when an assertion fails.
fn temp_out_dir(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(&format!("thatch-{tag}-")).tempdir().expect("temp dir")
}

fn check_output(name: &str, mode: &str, output: &std::process::Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{name} ({mode}) failed: {stderr}");
    assert!(!stderr.contains("warning:"), "{name} ({mode}) has unsupported blocks: {stderr}");
    assert_eq!(stdout, expected(name), "{name} ({mode}) printed the wrong output");
}

#[test]
fn samples_run_in_memory() {
    for name in sample_names() {
        let project = samples_dir().join(&name).join("project.json");
        let output = thatch().arg("run").arg(&project).args(run_args(&name)).output().expect("thatch runs");
        check_output(&name, "run", &output);
    }
}

#[test]
fn samples_build_to_executables() {
    let runtime_lib = runtime_lib();
    let out_dir = temp_out_dir("samples");
    for name in sample_names() {
        let project = samples_dir().join(&name).join("project.json");
        let exe = out_dir.path().join(&name);
        let build = thatch()
            .arg("build")
            .arg(&project)
            .arg("-o")
            .arg(&exe)
            .arg("--runtime-lib")
            .arg(&runtime_lib)
            .output()
            .expect("thatch builds");
        assert!(build.status.success(), "{name} did not build: {}", String::from_utf8_lossy(&build.stderr));
        let output = Command::new(&exe).args(run_args(&name)).output().expect("the executable runs");
        check_output(&name, "build", &output);
    }
}

#[test]
fn samples_run_under_a_c_host() {
    let runtime_lib = runtime_lib();
    let include = Path::new(env!("CARGO_MANIFEST_DIR")).join("../thatch-abi/include");
    let host_c = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/c_host/host.c");
    let out_dir = temp_out_dir("c-host");
    for name in sample_names() {
        if input_script(&name).is_some() {
            continue;
        }
        let project = samples_dir().join(&name).join("project.json");
        let object = out_dir.path().join(format!("{name}.o"));
        let exe = out_dir.path().join(&name);
        let build = thatch()
            .arg("build")
            .arg(&project)
            .arg("--object")
            .arg("--no-main")
            .arg("-o")
            .arg(&object)
            .output()
            .expect("thatch builds");
        assert!(build.status.success(), "{name} did not build: {}", String::from_utf8_lossy(&build.stderr));
        let link = Command::new("cc")
            .arg("-std=c99")
            .arg("-Wall")
            .arg("-Wextra")
            .arg("-Werror")
            .arg("-I")
            .arg(&include)
            .arg("-o")
            .arg(&exe)
            .arg(&host_c)
            .arg(&object)
            .arg(&runtime_lib)
            .args(system_libs())
            .output()
            .expect("cc runs");
        assert!(
            link.status.success(),
            "{name} did not link with the C host: {}",
            String::from_utf8_lossy(&link.stderr)
        );
        let output = Command::new(&exe).arg("--frames").arg("3000").output().expect("the C host runs");
        check_output(&name, "c host", &output);
    }
}

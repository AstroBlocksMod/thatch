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

/// `thatch build` for the headless host. Builds default to the host with a window, but the
/// samples are checked against scratch-vm without a renderer, and are bare project.json files.
fn headless_build() -> Command {
    let mut command = thatch();
    command.arg("build").arg("--host").arg("headless");
    command
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
        let output = thatch()
            .arg("run")
            .arg("--host")
            .arg("headless")
            .arg(&project)
            .args(run_args(&name))
            .output()
            .expect("thatch runs");
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
        let build = headless_build()
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

/// The notices a built executable has to ship are written next to it: the ones gathered when the
/// runtime library was built, which lie next to that library.
#[test]
fn a_build_writes_the_license_notices_next_to_the_executable() {
    let runtime_lib = runtime_lib();
    let gathered = std::fs::read_to_string(runtime_lib.with_extension("licenses.txt"))
        .expect("building the runtime writes its notices next to the library");
    assert!(gathered.contains("corosensei") && gathered.contains("The Rust standard library"), "{gathered}");
    // A toolchain without the standard library's notices is the one gap the file may have; the
    // runtime's own tests check what it says then.
    assert!(thatch_runtime::LICENSE_NOTICES_LACK_RUST || !gathered.contains("THIS FILE IS INCOMPLETE"), "{gathered}");

    let out_dir = temp_out_dir("notices");
    let project = samples_dir().join("hello").join("project.json");
    let exe = out_dir.path().join("hello.bin");
    let build = headless_build()
        .arg(&project)
        .arg("-o")
        .arg(&exe)
        .arg("--runtime-lib")
        .arg(&runtime_lib)
        .output()
        .expect("thatch builds");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    // The linker may have warnings of its own; thatch has none about the notices.
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(!stderr.contains("licenses.txt"), "{stderr}");
    let written = std::fs::read_to_string(out_dir.path().join("hello.bin.licenses.txt")).expect("the notices");
    assert_eq!(written, gathered);

    // A runtime library without notices next to it: the build goes through with the notices
    // thatch has built in, and says so.
    let lone = out_dir.path().join("libother.a");
    std::fs::copy(&runtime_lib, &lone).expect("the library copies");
    let exe = out_dir.path().join("lone");
    let build = headless_build()
        .arg(&project)
        .arg("-o")
        .arg(&exe)
        .arg("--runtime-lib")
        .arg(&lone)
        .output()
        .expect("thatch builds");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(stderr.contains("warning:") && stderr.contains("libother.licenses.txt"), "{stderr}");
    let written = std::fs::read_to_string(out_dir.path().join("lone.licenses.txt")).expect("the notices");
    assert_eq!(written, gathered, "thatch and the library were built together, so the notices agree");

    // An object file is not something to ship, so it gets no notices.
    let object = out_dir.path().join("hello.o");
    let build =
        thatch().arg("build").arg(&project).arg("--object").arg("-o").arg(&object).output().expect("thatch builds");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    assert!(!out_dir.path().join("hello.o.licenses.txt").exists());
}

/// An .sb3 is built for the host with a window unless asked otherwise. Where that host is not
/// installed, as for a thatch from `cargo install`, `thatch run` runs it headless and says so,
/// while asking for the host by name is an error.
#[test]
fn without_the_host_an_sb3_runs_headless_with_a_note() {
    let dir = temp_out_dir("no-host");
    let sb3 = dir.path().join("hello.sb3");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&sb3).expect("the archive is created"));
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("project.json", stored).expect("the entry starts");
    std::io::Write::write_all(&mut zip, &std::fs::read(samples_dir().join("hello/project.json")).expect("the sample"))
        .expect("the entry is written");
    zip.finish().expect("the archive is finished");
    // No host anywhere: an empty host directory, and an empty data directory.
    let nowhere = dir.path().join("nowhere");
    std::fs::create_dir(&nowhere).expect("the directory is created");
    let run = |extra: &[&str]| {
        thatch()
            .arg("run")
            .arg(&sb3)
            .args(extra)
            .env("THATCH_HOST_DIR", &nowhere)
            .env("THATCH_HOME", &nowhere)
            .output()
            .expect("thatch runs")
    };

    let output = run(&[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected("hello"));
    assert!(stderr.contains("note:") && stderr.contains("without a window"), "{stderr}");

    let quiet = run(&["--host", "headless"]);
    assert!(quiet.status.success() && quiet.stderr.is_empty(), "{}", String::from_utf8_lossy(&quiet.stderr));

    let asked = run(&["--host", "scratch-everywhere"]);
    let stderr = String::from_utf8_lossy(&asked.stderr);
    assert!(!asked.status.success() && stderr.contains("was not found"), "{stderr}");

    // A host that is there but cannot be used is an error that says why, not a reason to go
    // without a window.
    let broken = dir.path().join("broken");
    std::fs::create_dir(&broken).expect("the directory is created");
    std::fs::write(broken.join("thatch-host.txt"), "adapter = libgone.a\n").expect("a broken description");
    let output = thatch()
        .arg("run")
        .arg(&sb3)
        .env("THATCH_HOST_DIR", &broken)
        .env("THATCH_HOME", &nowhere)
        .output()
        .expect("thatch runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success() && stderr.contains("cannot be used") && stderr.contains("libgone.a"), "{stderr}");
    assert!(!stderr.contains("note:"), "{stderr}");

    // A bare project.json is headless whatever it is called, since it is no archive the host
    // could read: the broken host is not even looked at.
    let raw = dir.path().join("raw.sb3");
    std::fs::copy(samples_dir().join("hello/project.json"), &raw).expect("the project is copied");
    let output = thatch()
        .arg("run")
        .arg(&raw)
        .env("THATCH_HOST_DIR", &broken)
        .env("THATCH_HOME", &nowhere)
        .output()
        .expect("thatch runs");
    assert!(output.status.success() && output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected("hello"));
    // And an archive gets the host whatever it is called: named .json, it is looked for, and here
    // not found, which the note says.
    let archive = dir.path().join("archive.json");
    std::fs::copy(&sb3, &archive).expect("the archive is copied");
    let output = thatch()
        .arg("run")
        .arg(&archive)
        .env("THATCH_HOST_DIR", &nowhere)
        .env("THATCH_HOME", &nowhere)
        .output()
        .expect("thatch runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success() && stderr.contains("note:"), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected("hello"));

    // A host directory that cannot be looked into may hold the host that was meant, so it is an
    // error, not a reason to go without a window. (Not for root, who can look anyway.)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).expect("the directory is created");
        std::fs::write(locked.join("thatch-host.txt"), "adapter = libgone.a\n").expect("a description");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("locked");
        if std::fs::metadata(locked.join("thatch-host.txt")).is_err() {
            let output = thatch()
                .arg("run")
                .arg(&sb3)
                .env("THATCH_HOST_DIR", &locked)
                .env("THATCH_HOME", &nowhere)
                .output()
                .expect("thatch runs");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success() && stderr.contains("could not look at"), "{stderr}");
            assert!(!stderr.contains("note:"), "{stderr}");
        }
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).expect("unlocked");
    }

    // --timing measures the in-memory run, so without a host asked for, it runs headless quietly.
    let timed = run(&["--timing"]);
    let stderr = String::from_utf8_lossy(&timed.stderr);
    assert!(timed.status.success() && !stderr.contains("note:") && stderr.contains("frames in"), "{stderr}");
    // A headless run links nothing, so a runtime library given for it is a mistake worth saying.
    let linked = run(&["--host", "headless", "--runtime-lib", "libthatch_runtime.a"]);
    assert!(!linked.status.success(), "{}", String::from_utf8_lossy(&linked.stderr));
}

/// An empty variable is no setting: an empty THATCH_HOST_DIR does not make thatch look for a host
/// in the current directory and skip the others.
#[test]
fn an_empty_host_dir_variable_is_ignored() {
    let dir = temp_out_dir("empty-host-dir");
    let manifest = dir.path().join("thatch-host.txt");
    std::fs::write(&manifest, "adapter = nothing-here.a\n").expect("a decoy description");
    let output = thatch()
        .arg("build")
        .arg(samples_dir().join("hello/project.json"))
        .args(["--host", "made-up", "-o"])
        .arg(dir.path().join("game"))
        .current_dir(dir.path())
        .env("THATCH_HOST_DIR", "")
        .env("THATCH_HOME", dir.path().join("nowhere"))
        .output()
        .expect("thatch builds");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success() && stderr.contains("was not found"), "the decoy was read: {stderr}");
}

fn run_ok(command: &mut Command, what: &str) {
    let output = command.output().unwrap_or_else(|e| panic!("{what} did not start: {e}"));
    assert!(output.status.success(), "{what} failed: {}", String::from_utf8_lossy(&output.stderr));
}

/// Builds the C host of `samples_run_under_a_c_host` into `dir/libadapter.a`, the adapter of the
/// made-up hosts in these tests.
fn c_adapter(dir: &Path) {
    let include = Path::new(env!("CARGO_MANIFEST_DIR")).join("../thatch-abi/include");
    let host_c = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/c_host/host.c");
    let object = dir.join("host.o");
    run_ok(Command::new("cc").arg("-c").arg("-I").arg(&include).arg("-o").arg(&object).arg(&host_c), "cc");
    run_ok(Command::new("ar").arg("rcs").arg(dir.join("libadapter.a")).arg(&object), "ar");
}

/// Builds `source` into a shared library in `dir` that is loaded under a versioned name: its
/// file name and that name, which on Linux is its soname and on macOS its install name.
fn shared_library(dir: &Path, source: &Path, name: &str) -> (String, String) {
    let (file, loaded, option) = if cfg!(target_os = "macos") {
        let loaded = format!("lib{name}.1.dylib");
        (format!("lib{name}.dylib"), loaded.clone(), format!("-Wl,-install_name,@rpath/{loaded}"))
    } else {
        let loaded = format!("lib{name}.so.1");
        (format!("lib{name}.so"), loaded.clone(), format!("-Wl,-soname,{loaded}"))
    };
    run_ok(
        Command::new("cc").arg("-shared").arg("-fPIC").arg(option).arg("-o").arg(dir.join(&file)).arg(source),
        "cc -shared",
    );
    (file, loaded)
}

/// `thatch build --host` knows a host only through the `thatch-host.txt` in its build output:
/// which adapter to link, which shared libraries and license texts go next to the game, and
/// whether the project file does too. This host is made up on the spot: its adapter is the C host
/// of the test below, and its shared library does nothing but has to be found when the game starts.
#[test]
fn a_build_for_a_host_follows_the_hosts_description() {
    let runtime_lib = runtime_lib();
    let host_dir = temp_out_dir("host");
    let dir = host_dir.path();

    c_adapter(dir);
    let extra_c = dir.join("extra.c");
    std::fs::write(&extra_c, "int thatch_test_extra(void) { return 7; }\n").expect("the source is written");
    // The library is loaded under a versioned name, as many are, so the game only starts if
    // thatch puts it next to the game under that name and not under its file name.
    let (library, shipped) = shared_library(dir, &extra_c, "extra");
    std::fs::write(dir.join("notices.txt"), "The notices of the made-up host.\n").expect("the notices are written");
    let manifest = format!(
        "# a made-up host\nadapter = libadapter.a\nlibrary = {library}\nnotices = notices.txt\nlinker = cc\nproject = json\n"
    );
    std::fs::write(dir.join("thatch-host.txt"), &manifest).expect("the description is written");

    let out_dir = temp_out_dir("host-game");
    let project = samples_dir().join("hello").join("project.json");
    let exe = out_dir.path().join("game");
    let build = |project: &Path, host_dir: &Path| {
        thatch()
            .arg("build")
            .arg(project)
            .arg("--host")
            .arg("made-up")
            .arg("--host-dir")
            .arg(host_dir)
            .arg("--runtime-lib")
            .arg(&runtime_lib)
            .arg("-o")
            .arg(&exe)
            .output()
            .expect("thatch builds")
    };
    let built = build(&project, dir);
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));

    // The game is the adapter's main around the compiled program, and runs like the C host.
    let output = Command::new(&exe).arg("--frames").arg("3000").output().expect("the game runs");
    check_output("hello", "made-up host", &output);
    // `thatch run` on the host builds the same game in a temporary directory and runs it.
    let run = thatch()
        .arg("run")
        .arg(&project)
        .arg("--host")
        .arg("made-up")
        .arg("--host-dir")
        .arg(dir)
        .arg("--runtime-lib")
        .arg(&runtime_lib)
        .arg("--frames")
        .arg("3000")
        .output()
        .expect("thatch runs");
    check_output("hello", "thatch run on a made-up host", &run);
    // Next to it: the shared library, the project file under the game's name, and the notices of
    // the runtime with the host's after them.
    assert!(out_dir.path().join(&shipped).is_file(), "the shared library is next to the game as {shipped}");
    assert!(!out_dir.path().join(&library).exists(), "and not under its file name");
    let copied = std::fs::read(out_dir.path().join("game.json")).expect("the project file is next to the game");
    assert_eq!(copied, std::fs::read(&project).expect("the project"));
    let notices = std::fs::read_to_string(out_dir.path().join("game.licenses.txt")).expect("the notices");
    let runtime_notices = std::fs::read_to_string(runtime_lib.with_extension("licenses.txt")).expect("the runtime's");
    assert_eq!(notices, format!("{runtime_notices}\n\nThe notices of the made-up host.\n"));

    // A host that reads the project at run time needs it in the format it reads.
    std::fs::write(dir.join("thatch-host.txt"), manifest.replace("project = json", "project = sb3"))
        .expect("rewritten");
    let refused = build(&project, dir);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success() && stderr.contains("bare project.json"), "{stderr}");
    // The format is told by the content: a project.json named .sb3 is no archive either.
    let raw = out_dir.path().join("raw.sb3");
    std::fs::copy(&project, &raw).expect("the project is copied");
    let refused = build(&raw, dir);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success() && stderr.contains("bare project.json"), "{stderr}");
    std::fs::remove_file(&raw).expect("removed");
    // A host that was not built is an error that says where it was looked for.
    let missing = build(&project, &dir.join("nowhere"));
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(!missing.status.success() && stderr.contains("thatch-host.txt") && stderr.contains("nowhere"), "{stderr}");
    // So is a description that names a file that is not there.
    std::fs::write(dir.join("thatch-host.txt"), "adapter = libgone.a\n").expect("rewritten");
    let broken = build(&project, dir);
    let stderr = String::from_utf8_lossy(&broken.stderr);
    assert!(!broken.status.success() && stderr.contains("libgone.a"), "{stderr}");

    // A description may only name files of its own directory and link with a compiler driver,
    // since thatch finds it by path and runs and copies what it names.
    for (text, refused) in [
        ("adapter = libadapter.a\nnotices = ../../secret.txt\n", "secret.txt"),
        ("adapter = /etc/passwd\n", "/etc/passwd"),
        ("adapter = libadapter.a\nlinker = /bin/sh\n", "/bin/sh"),
    ] {
        std::fs::write(dir.join("thatch-host.txt"), text).expect("rewritten");
        let refused_build = build(&project, dir);
        let stderr = String::from_utf8_lossy(&refused_build.stderr);
        assert!(!refused_build.status.success() && stderr.contains(refused), "{text}: {stderr}");
    }

    // Two files of one build cannot go to the same place: here the executable is given the name
    // of the host's library. The build fails before it replaces anything.
    std::fs::write(dir.join("thatch-host.txt"), &manifest).expect("rewritten");
    let clash = out_dir.path().join(&shipped);
    // Bytes the build would not write, so that only a build that puts them back keeps them.
    let before = b"not the library a build would write".to_vec();
    std::fs::write(&clash, &before).expect("the library is replaced");
    let output = thatch()
        .arg("build")
        .arg(&project)
        .args(["--host", "made-up", "--host-dir"])
        .arg(dir)
        .arg("--runtime-lib")
        .arg(&runtime_lib)
        .arg("-o")
        .arg(&clash)
        .output()
        .expect("thatch builds");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success() && stderr.contains("two of the files"), "{stderr}");
    assert_eq!(std::fs::read(&clash).expect("still there"), before, "nothing was replaced");
    // Where the file system does not tell names apart by case (as macOS by default), a name that
    // differs from the library's only in case is the same file, and the build is undone.
    let upper = out_dir.path().join(shipped.to_uppercase());
    if upper.exists() {
        let output = thatch()
            .arg("build")
            .arg(&project)
            .args(["--host", "made-up", "--host-dir"])
            .arg(dir)
            .arg("--runtime-lib")
            .arg(&runtime_lib)
            .arg("-o")
            .arg(&upper)
            .output()
            .expect("thatch builds");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success() && stderr.contains("the same name"), "{stderr}");
        assert_eq!(std::fs::read(&clash).expect("still there"), before, "the library was put back");
    }

    let build_into = |project: &Path, out: &Path| {
        thatch()
            .arg("build")
            .arg(project)
            .args(["--host", "made-up", "--host-dir"])
            .arg(dir)
            .arg("--runtime-lib")
            .arg(&runtime_lib)
            .arg("-o")
            .arg(out)
            .output()
            .expect("thatch builds")
    };
    // Nothing is written where a directory is, and the directory is left where it was.
    let occupied = out_dir.path().join("occupied");
    std::fs::create_dir(&occupied).expect("the directory is created");
    let output = build_into(&project, &occupied);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success() && stderr.contains("is a directory"), "{stderr}");
    assert!(occupied.is_dir(), "the directory is still there");
    // The files a build replaces are kept in its own directory, which goes with it.
    let leftovers: Vec<_> = std::fs::read_dir(out_dir.path())
        .expect("the output directory")
        .map(|e| e.expect("an entry").file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".thatch") || name.ends_with(".thatch-old"))
        .collect();
    assert!(leftovers.is_empty(), "left behind: {leftovers:?}");

    // A project that is where the game looks for it already, as when `thatch build game.sb3`
    // builds `game` next to it, stays as it is.
    let own_dir = temp_out_dir("host-own-project");
    let own = own_dir.path().join("game.json");
    std::fs::copy(&project, &own).expect("the project is copied");
    let output = build_into(&own, &own_dir.path().join("game"));
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(std::fs::read(&own).expect("the project"), std::fs::read(&project).expect("the sample"));
    let ran = Command::new(own_dir.path().join("game")).arg("--frames").arg("3000").output().expect("the game runs");
    check_output("hello", "made-up host, project in place", &ran);
    // No file of the build may replace the project, which would be lost: not the executable,
    let output = build_into(&own, &own);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success() && stderr.contains("is the project being built"), "{stderr}");
    assert_eq!(std::fs::read(&own).expect("the project"), std::fs::read(&project).expect("the sample"));
    // and not a host library loaded under the project's name, while the project stays in place.
    let own_c = dir.join("own.c");
    std::fs::write(&own_c, "int thatch_test_own(void) { return 9; }\n").expect("the source is written");
    let own_option =
        if cfg!(target_os = "macos") { "-Wl,-install_name,@rpath/game.json" } else { "-Wl,-soname,game.json" };
    run_ok(
        Command::new("cc").args(["-shared", "-fPIC", own_option, "-o"]).arg(dir.join("libown.so")).arg(&own_c),
        "cc -shared",
    );
    std::fs::write(dir.join("thatch-host.txt"), format!("{manifest}library = libown.so\n")).expect("rewritten");
    let output = build_into(&own, &own_dir.path().join("game"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success() && stderr.contains("is the project being built"), "{stderr}");
    assert_eq!(std::fs::read(&own).expect("the project"), std::fs::read(&project).expect("the sample"));
    std::fs::write(dir.join("thatch-host.txt"), &manifest).expect("rewritten");

    // A host library may have any name the notices might have had in the build's own directory.
    let named_c = dir.join("named.c");
    std::fs::write(&named_c, "int thatch_test_named(void) { return 8; }\n").expect("the source is written");
    let name_option =
        if cfg!(target_os = "macos") { "-Wl,-install_name,@rpath/licenses.txt" } else { "-Wl,-soname,licenses.txt" };
    run_ok(
        Command::new("cc")
            .arg("-shared")
            .arg("-fPIC")
            .arg(name_option)
            .arg("-o")
            .arg(dir.join("licenses.txt"))
            .arg(&named_c),
        "cc -shared",
    );
    std::fs::write(dir.join("thatch-host.txt"), format!("{manifest}library = licenses.txt\n")).expect("rewritten");
    let named_dir = temp_out_dir("host-named");
    let output = build_into(&project, &named_dir.path().join("game"));
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(
        std::fs::read(named_dir.path().join("licenses.txt")).expect("the library"),
        std::fs::read(dir.join("licenses.txt")).expect("the host's"),
        "the library is shipped as it is"
    );
    let notices = std::fs::read_to_string(named_dir.path().join("game.licenses.txt")).expect("the notices");
    assert!(notices.ends_with("The notices of the made-up host.\n"), "{notices}");

    // A macOS library whose install name is a fixed path would be loaded from there, not from
    // next to the game, so the host has to be built differently.
    if cfg!(target_os = "macos") {
        run_ok(
            Command::new("cc")
                .args(["-shared", "-fPIC", "-Wl,-install_name,/usr/local/lib/libfixed.dylib", "-o"])
                .arg(dir.join("libfixed.dylib"))
                .arg(&named_c),
            "cc -shared",
        );
        std::fs::write(dir.join("thatch-host.txt"), format!("{manifest}library = libfixed.dylib\n"))
            .expect("rewritten");
        let output = build_into(&project, &named_dir.path().join("fixed"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success() && stderr.contains("@rpath/libfixed.dylib"), "{stderr}");
        // A universal library ships under one name, so each of its architectures has to give it
        // that name.
        for (arch, name) in [("arm64", "liba.dylib"), ("x86_64", "libb.dylib")] {
            run_ok(
                Command::new("cc")
                    .args(["-arch", arch, "-shared", &format!("-Wl,-install_name,@rpath/{name}"), "-o"])
                    .arg(dir.join(format!("{arch}.dylib")))
                    .arg(&named_c),
                "cc -shared",
            );
        }
        run_ok(
            Command::new("lipo")
                .arg("-create")
                .arg(dir.join("arm64.dylib"))
                .arg(dir.join("x86_64.dylib"))
                .arg("-output")
                .arg(dir.join("libfat.dylib")),
            "lipo",
        );
        std::fs::write(dir.join("thatch-host.txt"), format!("{manifest}library = libfat.dylib\n")).expect("rewritten");
        let output = build_into(&project, &named_dir.path().join("fat"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success() && stderr.contains("different install names"), "{stderr}");
    }
    std::fs::write(dir.join("thatch-host.txt"), &manifest).expect("rewritten");

    // `thatch run` on a host ends as the game did: the C host refuses arguments it does not know.
    let run = thatch()
        .arg("run")
        .arg(&project)
        .args(["--host", "made-up", "--host-dir"])
        .arg(dir)
        .arg("--runtime-lib")
        .arg(&runtime_lib)
        .args(["--seed", "1"])
        .output()
        .expect("thatch runs");
    assert_eq!(run.status.code(), Some(2), "{}", String::from_utf8_lossy(&run.stderr));
}

/// `thatch run` on a host waits for the game, so that it can remove the temporary directory it
/// built the game in. Ctrl-C in a terminal interrupts both, and the game ends as it would on its
/// own. An interrupt sent to thatch alone ends the game too.
#[cfg(unix)]
#[test]
fn thatch_run_ends_the_game_it_was_interrupted_for() {
    use std::os::unix::process::CommandExt;
    use std::time::{Duration, Instant};
    let host_dir = temp_out_dir("interrupt-host");
    c_adapter(host_dir.path());
    std::fs::write(host_dir.path().join("thatch-host.txt"), "adapter = libadapter.a\n").expect("the description");
    // A script that says it has started and then never ends, so the game runs until it is ended.
    let project = host_dir.path().join("forever.json");
    std::fs::write(
        &project,
        r#"{"targets":[{"isStage":true,"name":"Stage","variables":{},"lists":{},"broadcasts":{},"blocks":{},
            "currentCostume":0,"costumes":[],"sounds":[],"volume":100,"layerOrder":0},
            {"isStage":false,"name":"Cat","variables":{},"lists":{},"broadcasts":{},"blocks":{
            "flag":{"opcode":"event_whenflagclicked","next":"say","parent":null,"inputs":{},"fields":{},"shadow":false,"topLevel":true,"x":0,"y":0},
            "say":{"opcode":"looks_say","next":"loop","parent":"flag","inputs":{"MESSAGE":[1,[10,"started"]]},"fields":{},"shadow":false,"topLevel":false},
            "loop":{"opcode":"control_forever","next":null,"parent":"say","inputs":{},"fields":{},"shadow":false,"topLevel":false}},
            "currentCostume":0,"costumes":[],"sounds":[],"volume":100,"layerOrder":1,"visible":true,"x":0,"y":0,
            "size":100,"direction":90,"draggable":false,"rotationStyle":"all around"}],
            "monitors":[],"extensions":[],"meta":{"semver":"3.0.0"}}"#,
    )
    .expect("the project is written");

    /// thatch in a process group of its own. If the test fails while thatch has not been reaped,
    /// the group is ended with everything in it, the game included, so that no game outlives the
    /// test. Once thatch has been reaped, its id may be taken again, so nothing is sent.
    struct Group(std::process::Child);
    impl Drop for Group {
        fn drop(&mut self) {
            if let Ok(None) = self.0.try_wait() {
                let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", self.0.id())]).status();
                let _ = self.0.wait();
            }
        }
    }

    for (signal_group, expected) in [(false, 128 + 9), (true, 128 + 2)] {
        let temp = temp_out_dir("interrupt-temp");
        let mut command = thatch();
        command
            .arg("run")
            .arg(&project)
            .args(["--host", "made-up", "--host-dir"])
            .arg(host_dir.path())
            .arg("--runtime-lib")
            .arg(runtime_lib())
            .env("TMPDIR", temp.path())
            // A process group of its own, which the interrupt goes to as Ctrl-C sends it to the
            // terminal's foreground group.
            .process_group(0)
            .stdout(std::process::Stdio::piped());
        let mut thatch = Group(command.spawn().expect("thatch runs"));
        // Until the game has been built and has said that it started.
        let stdout = thatch.0.stdout.take().expect("the game's output");
        let (lines, said) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufRead::lines(std::io::BufReader::new(stdout)) {
                let _ = lines.send(line);
            }
        });
        let line = said.recv_timeout(Duration::from_secs(60)).expect("the game started");
        assert_eq!(line.expect("a line"), "Cat says: started");
        let target = if signal_group { format!("-{}", thatch.0.id()) } else { thatch.0.id().to_string() };
        run_ok(Command::new("kill").args(["-INT", "--", &target]), "kill");
        let interrupted = Instant::now();
        let status = loop {
            if let Some(status) = thatch.0.try_wait().expect("thatch is waited for") {
                break status;
            }
            assert!(interrupted.elapsed() < Duration::from_secs(20), "the game did not end");
            std::thread::sleep(Duration::from_millis(50));
        };
        // From the group, the game ends of the interrupt itself; to thatch alone, thatch ends it.
        assert_eq!(status.code(), Some(expected), "interrupting the group: {signal_group}");
        let left: Vec<_> = std::fs::read_dir(temp.path()).expect("the temporary directory").collect();
        assert!(left.is_empty(), "the game's temporary directory is left: {left:?}");
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

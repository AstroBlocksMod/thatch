//! Gathers the license notices a program built with this runtime has to ship.
//!
//! The runtime is linked into every executable Thatch builds, and so are the crates it depends on
//! and Rust's standard library. Their licenses ask for a notice in each copy, so this script
//! collects the texts once, when the runtime is built, into one file:
//!
//! - `$OUT_DIR/licenses.txt`, which the crate embeds as `thatch_runtime::LICENSE_NOTICES`, and
//! - `libthatch_runtime.licenses.txt` next to `libthatch_runtime.a`, where `thatch build` looks
//!   for the notices of the library it links.
//!
//! The list of crates comes from `cargo metadata` for the platform being built for; the texts come
//! from the sources of those crates and from the toolchain. See `build/notices.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

// The parts only the test uses are dead code here.
#[allow(dead_code)]
#[path = "build/notices.rs"]
mod notices;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("cargo sets {name} for build scripts"))
}

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=build/notices.rs");
    let manifest_dir = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env("OUT_DIR"));
    let target = env("TARGET");
    let mut missing = Vec::new();

    // Offline first: everything this build links was already downloaded for it, and a build script
    // should not reach for the network without need. `cargo metadata` also wants the manifests of
    // packages this build does not use, though, such as another crate's test dependencies, and a
    // fresh checkout that never ran the tests does not have those yet; then cargo may fetch them.
    let cargo = env("CARGO");
    let manifest = manifest_dir.join("Cargo.toml");
    let linked = notices::linked_packages(cargo.as_ref(), &manifest, Some(&target), true)
        .or_else(|_| notices::linked_packages(cargo.as_ref(), &manifest, Some(&target), false));
    let packages = match linked {
        Ok(linked) => {
            println!("cargo::rerun-if-changed={}", linked.lock_file.display());
            linked.packages
        }
        Err(e) => {
            missing
                .push(format!("the crates the runtime links could not be listed, so their notices are missing: {e}"));
            Vec::new()
        }
    };
    let rust = sysroot().and_then(|sysroot| notices::rust_notices(&sysroot, &target));
    let gathered = notices::notices(&target, &packages, rust);
    missing.extend(gathered.missing);
    let mut text = gathered.text;
    if !missing.is_empty() {
        // Whoever ships the file should not have to have seen the build's output to know.
        let mut note = String::from("THIS FILE IS INCOMPLETE. When the Thatch runtime was built:\n\n");
        for gap in &missing {
            note.push_str(&format!("  {gap}\n"));
        }
        note.push('\n');
        text.insert_str(0, &note);
    }
    for warning in missing.iter().chain(&gathered.unlisted) {
        println!("cargo::warning={warning}");
    }

    std::fs::write(out_dir.join("licenses.txt"), &text).expect("OUT_DIR is writable");
    if let Some(dir) = artifact_dir(&out_dir) {
        let name = if env("CARGO_CFG_TARGET_ENV") == "msvc" { "thatch_runtime" } else { "libthatch_runtime" };
        let path = dir.join(format!("{name}.licenses.txt"));
        if let Err(e) = std::fs::write(&path, &text) {
            println!("cargo::warning=could not write {}: {e}", path.display());
        }
    }
}

/// The sysroot of the compiler this build uses, which holds the standard library's notices.
fn sysroot() -> Result<PathBuf, String> {
    let rustc = env("RUSTC");
    let output = Command::new(&rustc)
        .args(["--print", "sysroot"])
        .output()
        .map_err(|e| format!("could not run {rustc} --print sysroot: {e}"))?;
    if !output.status.success() {
        return Err(format!("{rustc} --print sysroot failed"));
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
}

/// The directory cargo puts `libthatch_runtime.a` in, worked out from `OUT_DIR`, which is
/// `<artifact dir>/build/<package>-<hash>/out` in cargo's layout. Cargo has no official way to ask,
/// so with any other layout this is None, and `thatch build` falls back to the embedded notices.
fn artifact_dir(out_dir: &Path) -> Option<PathBuf> {
    let build = out_dir.parent()?.parent()?;
    (build.file_name()? == "build").then(|| build.parent().map(Path::to_owned))?
}

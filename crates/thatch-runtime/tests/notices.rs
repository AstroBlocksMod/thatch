//! The license notices of the runtime (`build/notices.rs`, which the build script runs).
//!
//! Whatever the runtime links ends up in every executable Thatch builds, so its dependencies are a
//! reviewed list, `LINKED_CRATES`. These tests fail when that list and the dependency tree part
//! ways: a crate was added, removed, or changed the licenses it offers.

use std::collections::BTreeSet;
use std::path::Path;

#[allow(dead_code)]
#[path = "../build/notices.rs"]
mod notices;

use notices::{ALLOWED_LICENSES, LINKED_CRATES, html_to_text, linked_packages, offers};

/// What the runtime links on every platform together.
fn linked() -> Vec<notices::Package> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    linked_packages(env!("CARGO").as_ref(), &manifest, None, false).expect("cargo metadata runs").packages
}

#[test]
fn the_list_of_linked_crates_matches_the_dependency_tree() {
    let linked: BTreeSet<String> = linked().into_iter().map(|p| p.name).collect();
    let listed: BTreeSet<String> = LINKED_CRATES.iter().map(|c| c.name.to_owned()).collect();
    let unlisted: Vec<_> = linked.difference(&listed).collect();
    let gone: Vec<_> = listed.difference(&linked).collect();
    assert!(
        unlisted.is_empty(),
        "the runtime now links {unlisted:?}. Check that each license is permissive, add each to LINKED_CRATES in \
         crates/thatch-runtime/build/notices.rs and update the list in docs/getting-started.md"
    );
    assert!(
        gone.is_empty(),
        "the runtime no longer links {gone:?}. Remove each from LINKED_CRATES in \
         crates/thatch-runtime/build/notices.rs and from the list in docs/getting-started.md"
    );
    let names: Vec<_> = LINKED_CRATES.iter().map(|c| c.name).collect();
    assert!(names.is_sorted(), "keep LINKED_CRATES sorted by name");
}

#[test]
fn every_linked_crate_is_taken_under_a_permissive_license_it_offers() {
    for package in linked() {
        let Some(listed) = LINKED_CRATES.iter().find(|c| c.name == package.name) else { continue };
        assert!(
            ALLOWED_LICENSES.contains(&listed.license),
            "{} is taken under {}, which is not a license the runtime may link",
            listed.name,
            listed.license
        );
        assert!(
            offers(&package.license, listed.license),
            "{} {} offers {:?}, not {}",
            package.name,
            package.version,
            package.license,
            listed.license
        );
        // The sources of crates for other platforms may not be on this machine.
        if package.dir.is_dir() {
            for file in listed.files {
                assert!(
                    package.dir.join(file).is_file(),
                    "{} {} has no file {file}; update its entry in LINKED_CRATES",
                    package.name,
                    package.version
                );
            }
        }
    }
}

#[test]
fn the_library_directory_is_found_in_both_cargo_layouts() {
    let dir = |p: &str| notices::artifact_dir(Path::new(p));
    // Cargo 1.99 and older, then 1.100 and newer.
    assert_eq!(dir("/t/debug/build/thatch-runtime-0123abcd/out"), Some("/t/debug".into()));
    assert_eq!(dir("/t/debug/build/thatch-runtime/0123abcd/out"), Some("/t/debug".into()));
    assert_eq!(dir("/elsewhere/out"), None);
}

#[test]
fn spdx_expressions_are_read_conservatively() {
    assert!(offers("MIT OR Apache-2.0", "MIT"));
    assert!(offers("Apache-2.0 OR BSL-1.0", "BSL-1.0"));
    assert!(offers("MIT/Apache-2.0", "Apache-2.0"));
    assert!(offers("(MIT OR Apache-2.0)", "MIT"));
    assert!(offers("MIT", "MIT"));
    assert!(!offers("MIT AND Apache-2.0", "MIT"), "both apply, so MIT alone is not on offer");
    assert!(!offers("GPL-3.0-only", "MIT"));
    assert!(!offers("Apache-2.0 WITH LLVM-exception OR MIT", "Apache-2.0"));
}

#[test]
fn the_built_notices_cover_what_this_platform_links() {
    let notices = thatch_runtime::LICENSE_NOTICES;
    // Complete, except where the toolchain came without the standard library's notices, which the
    // file then says and which is the one gap a build may have.
    let head = &notices[..notices.len().min(2000)];
    if thatch_runtime::LICENSE_NOTICES_LACK_RUST {
        assert!(head.starts_with("THIS FILE IS INCOMPLETE"), "{head}");
        let gaps = head.split("\n\n").nth(1).unwrap_or_default();
        assert_eq!(gaps.lines().count(), 1, "the standard library is the only gap: {head}");
        assert!(gaps.contains("Rust standard library"), "{head}");
    } else {
        assert!(!notices.contains("THIS FILE IS INCOMPLETE"), "{head}");
    }
    // corosensei is linked everywhere; its MIT text names its author.
    assert!(notices.contains("\ncorosensei "), "the crate has a section");
    assert!(notices.contains("Amanieu d'Antras"), "with its copyright line");
    assert!(notices.contains("Permission is hereby granted, free of charge"));
    assert!(notices.contains("ryu-js") && notices.contains("asks for no notice in a compiled program"));
    assert!(notices.contains("The Rust standard library"));
    if cfg!(windows) {
        assert!(notices.contains("\nwindows-sys "));
    } else {
        assert!(!notices.contains("\nwindows-sys "), "only what this platform links is listed");
    }
    assert!(notices.is_ascii() || !notices.contains('\u{1F4E6}'), "the package symbols of Rust's file are dropped");
}

#[test]
fn html_becomes_readable_text() {
    let html = "<html><head><title>gone</title></head><body>\n\
        <h1>Notices</h1>\n<p>Dual  licensed,\n at your <a href=\"x\">option</a>.</p>\n\
        <h3>\u{1F4E6} first-1.0.0</h3>\n<p><b>Authors:</b> A &#60;a@example.org&#62; &amp; B</p>\n\
        <details><summary><code>LICENSE-MIT</code></summary><pre>\n  MIT License\n\nCopyright &#34;A&#34;\n</pre></details>\n\
        <h3>second-2.0.0</h3>\n<details><summary><code>LICENSE</code></summary><pre>\nMIT   License\nCopyright \"A\"\n</pre></details>\n\
        </body></html>";
    assert_eq!(
        html_to_text(html),
        "Notices\n-------\n\nDual licensed, at your option.\n\n\
         first-1.0.0\n-----------\n\nAuthors: A <a@example.org> & B\n\n\
         LICENSE-MIT:\n\n  MIT License\n\nCopyright \"A\"\n\n\
         second-2.0.0\n------------\n\nLICENSE:\n\n(The same text as LICENSE-MIT of first-1.0.0, above.)\n"
    );
}

//! Snapshots of the IR every sample lowers to, so any change in lowering shows up in review.
//!
//! After an intended change, run `INSTA_UPDATE=always cargo test -p thatch-scratch` (or
//! `cargo insta review`) and commit the updated files in `tests/snapshots/`.

use std::path::Path;

#[test]
fn samples_lower_to_the_expected_ir() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples");
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .expect("the samples directory is readable")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            entry.path().join("project.json").is_file().then(|| entry.file_name().into_string().ok())?
        })
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no samples found in {}", root.display());
    for name in names {
        let project = thatch_sb3::Project::read(&root.join(&name).join("project.json"))
            .unwrap_or_else(|e| panic!("sample {name} does not load: {e}"));
        let lowered = thatch_scratch::lower(&project).unwrap_or_else(|e| panic!("sample {name} does not lower: {e}"));
        assert!(lowered.warnings.is_empty(), "sample {name} has warnings: {:?}", lowered.warnings);
        insta::assert_snapshot!(name.as_str(), thatch_ir::dump(&lowered.program));
    }
}

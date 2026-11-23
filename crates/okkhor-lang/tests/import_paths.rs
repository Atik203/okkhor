//! Path policy: relative/traversal/absolute rejection and symlink-escape
//! rejection.

mod common;

use okkhor_lang::{ImportOptions, SkipReason, error::LangError, import_fixture_dir};

use common::{synthetic_root, temp_dir, test_options, write_file};

fn manifest_with(paths: &[&str]) -> String {
    let entries: Vec<String> = paths
        .iter()
        .map(|p| format!("{{\"kind\": \"layout\", \"path\": \"{p}\"}}"))
        .collect();
    format!("{{\"schema\": 1, \"assets\": [{}]}}", entries.join(", "))
}

fn good_layout(id: &str) -> String {
    format!(
        "{{\"schema\": 1, \"id\": \"{id}\", \"label\": \"x\", \"language\": \"bangla\", \
         \"mappings\": [{{\"key\": \"a\", \"output\": \"আ\"}}]}}"
    )
}

fn import_paths(tag: &str, paths: &[&str]) -> okkhor_lang::ImportOutcome {
    let dir = temp_dir(tag);
    write_file(&dir, "manifest.json", &manifest_with(paths));
    write_file(&dir, "layouts/ok.json", &good_layout("ok"));
    import_fixture_dir(&dir, &test_options()).expect("import continues")
}

#[test]
fn dot_dot_traversal_is_rejected() {
    let outcome = import_paths("traversal", &["layouts/../manifest.json"]);
    assert_eq!(outcome.report.imported, 0);
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(
        outcome.report.unsupported[0].reason,
        SkipReason::PathRejected
    );
    assert_eq!(
        outcome.report.unsupported[0].name,
        "layouts/../manifest.json"
    );
}

#[test]
fn current_dir_component_is_rejected() {
    let outcome = import_paths("dot", &["./layouts/ok.json"]);
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(
        outcome.report.unsupported[0].reason,
        SkipReason::PathRejected
    );
}

#[test]
fn absolute_path_is_rejected() {
    let outcome = import_paths("absolute", &["/layouts/ok.json"]);
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(
        outcome.report.unsupported[0].reason,
        SkipReason::PathRejected
    );
}

#[test]
fn backslash_separator_is_rejected() {
    let outcome = import_paths("backslash", &[r"layouts\\ok.json"]);
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(
        outcome.report.unsupported[0].reason,
        SkipReason::PathRejected
    );
}

#[test]
fn missing_file_is_rejected_as_path() {
    let outcome = import_paths("missing", &["layouts/nope.json"]);
    assert_eq!(outcome.report.imported, 0);
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(
        outcome.report.unsupported[0].reason,
        SkipReason::PathRejected
    );
}

#[test]
fn valid_relative_path_still_imports() {
    let outcome = import_paths("valid-rel", &["layouts/ok.json"]);
    assert_eq!(outcome.report.imported, 1);
    assert_eq!(outcome.report.skipped, 0);
    assert!(
        outcome
            .assets
            .layouts
            .contains_key(&okkhor_lang::LayoutId::from_code("ok").unwrap())
    );
}

#[test]
fn strict_mode_escalates_path_rejection_to_unsafe_path() {
    let dir = temp_dir("strict-path");
    write_file(&dir, "manifest.json", &manifest_with(&["../evil.json"]));
    let strict = ImportOptions {
        strict: true,
        ..test_options()
    };
    match import_fixture_dir(&dir, &strict) {
        Err(LangError::UnsafePath) => {}
        other => panic!("expected UnsafePath, got {other:?}"),
    }
}

#[test]
fn unknown_kind_is_unsupported_not_fatal() {
    let dir = temp_dir("unknown-kind");
    write_file(
        &dir,
        "manifest.json",
        r#"{"schema": 1, "assets": [{"kind": "font", "path": "fonts/x.json"}]}"#,
    );
    let outcome = import_fixture_dir(&dir, &test_options()).expect("import continues");
    assert_eq!(outcome.report.imported, 0);
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(
        outcome.report.unsupported[0].reason,
        SkipReason::UnsupportedKind
    );
    let strict = ImportOptions {
        strict: true,
        ..test_options()
    };
    match import_fixture_dir(&dir, &strict) {
        Err(LangError::InvalidData { reason }) => assert_eq!(reason, "unsupported_kind"),
        other => panic!("expected InvalidData, got {other:?}"),
    }
}

/// A symlink inside the fixture root pointing outside must not escape the
/// root, even though the lexical manifest path looks innocent.
#[test]
fn symlink_escape_is_rejected() {
    let dir = temp_dir("symlink-escape");
    let outside = temp_dir("symlink-outside");
    write_file(&outside, "secret.json", &good_layout("secret"));
    let link = dir.join("layouts").join("evil.json");
    std::fs::create_dir_all(link.parent().unwrap()).expect("create dir");
    write_file(
        &dir,
        "manifest.json",
        &manifest_with(&["layouts/evil.json"]),
    );
    if let Err(error) = try_symlink(&outside.join("secret.json"), &link) {
        eprintln!("SKIP: cannot create symlink in this environment: {error}");
        return;
    }
    let outcome = import_fixture_dir(&dir, &test_options()).expect("import continues");
    assert_eq!(outcome.report.imported, 0, "symlink escape must not import");
    assert_eq!(outcome.report.skipped, 1);
    assert_eq!(
        outcome.report.unsupported[0].reason,
        SkipReason::PathRejected
    );
}

/// Sanity: the committed synthetic tree itself contains no symlinks.
#[test]
fn synthetic_tree_has_no_symlinks() {
    fn visit(dir: &std::path::Path) {
        for entry in std::fs::read_dir(dir).expect("read dir") {
            let entry = entry.expect("entry");
            let file_type = entry.file_type().expect("file type");
            assert!(
                !file_type.is_symlink(),
                "symlink: {}",
                entry.path().display()
            );
            if file_type.is_dir() {
                visit(&entry.path());
            }
        }
    }
    visit(&synthetic_root());
}

/// Portable symlink creation for the escape test.
#[cfg(windows)]
fn try_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

/// Portable symlink creation for the escape test.
#[cfg(unix)]
fn try_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

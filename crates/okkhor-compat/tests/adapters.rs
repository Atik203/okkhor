//! Contract §5.7 tests: feature gates, DLL path policy table, registry
//! lifecycle, and error-display hygiene.
//!
//! The loader seam is stubbed under `cfg(test)`: no real DLL is ever opened.
//! Synthetic files created here are nonce `.dll`-named stubs containing plain
//! text, never real library bytes. Path-table and lifecycle tests exercise
//! whichever adapters are compiled in; run with all features to cover every
//! adapter:
//!
//! ```powershell
//! cargo test -p okkhor-compat --features word-trie,sentence-trie,translator,caret,emoji-manager
//! ```
//! or per adapter, e.g. `cargo test -p okkhor-compat --features word-trie`.

use std::path::{Path, PathBuf};

use okkhor_compat::{AdapterKind, AdapterRegistry, CompatError, DllPathPolicy, ValidatedDllPath};

/// Adapters compiled in for this test run.
fn enabled_adapters() -> Vec<AdapterKind> {
    AdapterKind::ALL
        .iter()
        .copied()
        .filter(|kind| kind.is_enabled())
        .collect()
}

/// First enabled adapter, or `None` when running with default features.
fn any_enabled() -> Option<AdapterKind> {
    enabled_adapters().into_iter().next()
}

/// Unique scratch root under the OS temp dir. Callers must remove it when done.
fn scratch_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "okkhor-compat-test-{}-{}-{}",
        std::process::id(),
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ))
}

fn write_stub(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create scratch dirs");
    }
    std::fs::write(path, bytes).expect("write synthetic stub file");
}

/// Build `<root>/` containing `<dll_file_name>` with `contents`, plus a policy
/// allowlisting `<root>`.
fn rooted_fixture(
    tag: &str,
    adapter: AdapterKind,
    contents: &[u8],
) -> (PathBuf, PathBuf, DllPathPolicy) {
    let root = scratch_root(tag);
    let dll = root.join(adapter.dll_file_name());
    write_stub(&dll, contents);
    let policy = DllPathPolicy {
        allowed_roots: vec![root.clone()],
        ..DllPathPolicy::default()
    };
    (root, dll, policy)
}

fn remove_root(root: &Path) {
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn adapter_metadata_matches_contract() {
    assert_eq!(AdapterKind::ALL.len(), 5);
    assert_eq!(AdapterKind::WordTrie.feature_name(), "word-trie");
    assert_eq!(AdapterKind::SentenceTrie.feature_name(), "sentence-trie");
    assert_eq!(AdapterKind::Translator.feature_name(), "translator");
    assert_eq!(AdapterKind::Caret.feature_name(), "caret");
    assert_eq!(AdapterKind::EmojiManager.feature_name(), "emoji-manager");
    assert_eq!(AdapterKind::WordTrie.dll_file_name(), "word_trie.dll");
    assert_eq!(
        AdapterKind::SentenceTrie.dll_file_name(),
        "_sentencetrie_native.dll"
    );
    assert_eq!(AdapterKind::Translator.dll_file_name(), "translator.dll");
    assert_eq!(AdapterKind::Caret.dll_file_name(), "caret.dll");
    assert_eq!(
        AdapterKind::EmojiManager.dll_file_name(),
        "emoji_manager.dll"
    );
    // serde uses snake_case codes, matching the fixture language convention.
    for kind in AdapterKind::ALL {
        let json = serde_json::to_string(&kind).expect("serialize AdapterKind");
        let back: AdapterKind = serde_json::from_str(&json).expect("round-trip");
        assert_eq!(back, kind);
    }
    assert_eq!(
        serde_json::to_string(&AdapterKind::WordTrie).expect("serialize"),
        "\"word_trie\""
    );
    assert_eq!(
        serde_json::to_string(&AdapterKind::SentenceTrie).expect("serialize"),
        "\"sentence_trie\""
    );
    assert_eq!(
        serde_json::to_string(&AdapterKind::EmojiManager).expect("serialize"),
        "\"emoji_manager\""
    );
    // Registry slots follow declaration order; pin the invariant that
    // `AdapterKind as usize` indexes `ALL` (used by `AdapterRegistry`).
    for (index, kind) in AdapterKind::ALL.iter().enumerate() {
        assert_eq!(*kind as usize, index);
    }
}

#[test]
fn default_policy_loads_nothing() {
    let policy = DllPathPolicy::default();
    assert!(policy.allowed_roots.is_empty());
    assert_eq!(policy.max_path_bytes, 32_768);
    assert_eq!(policy.max_file_bytes, 128 * 1024 * 1024);
}

#[test]
fn disabled_adapters_reject_without_touching_filesystem() {
    // Uses a relative path that fails no filesystem check: with default
    // features the feature gate must fire before any path check.
    let policy = DllPathPolicy::default();
    let mut registry = AdapterRegistry::new(policy.clone());
    let mut saw_disabled = false;
    for adapter in AdapterKind::ALL {
        if adapter.is_enabled() {
            continue;
        }
        saw_disabled = true;
        let relative = Path::new(adapter.dll_file_name());
        assert!(
            matches!(
                ValidatedDllPath::validate(relative, adapter, &policy),
                Err(CompatError::FeatureDisabled { .. })
            ),
            "{adapter:?} must report FeatureDisabled"
        );
        assert!(
            matches!(
                registry.load(relative, adapter),
                Err(CompatError::FeatureDisabled { .. })
            ),
            "{adapter:?} load must report FeatureDisabled"
        );
        assert!(!registry.is_loaded(adapter));
    }
    if enabled_adapters().len() == AdapterKind::ALL.len() {
        assert!(!saw_disabled, "all adapters enabled: nothing to check here");
    } else {
        assert!(saw_disabled, "expected at least one disabled adapter");
    }
}

#[test]
fn all_disabled_by_default() {
    // Pins the default-feature contract: statuses report enabled=false and
    // load never touches the loader. When features are enabled this test
    // vacuously passes; per-feature runs cover the enabled side.
    let registry = AdapterRegistry::new(DllPathPolicy::default());
    for status in registry.statuses() {
        assert_eq!(status.enabled, status.kind.is_enabled());
        assert!(!status.loaded);
    }
    let kinds: Vec<AdapterKind> = registry
        .statuses()
        .iter()
        .map(|status| status.kind)
        .collect();
    assert_eq!(kinds, AdapterKind::ALL.to_vec());
}

#[test]
fn path_table_relative_rejected() {
    let Some(adapter) = any_enabled() else { return };
    let policy = DllPathPolicy::default();
    let relative = Path::new(adapter.dll_file_name());
    assert!(matches!(
        ValidatedDllPath::validate(relative, adapter, &policy),
        Err(CompatError::InvalidDllPath {
            reason: "not_absolute"
        })
    ));
}

#[test]
fn path_table_dot_component_rejected() {
    let Some(adapter) = any_enabled() else { return };
    let (root, _dll, policy) = rooted_fixture("dotdot", adapter, b"synthetic stub");
    let evil = root.join("..").join(adapter.dll_file_name());
    let result = ValidatedDllPath::validate(&evil, adapter, &policy);
    remove_root(&root);
    assert!(matches!(
        result,
        Err(CompatError::InvalidDllPath {
            reason: "dot_component"
        })
    ));
}

#[test]
fn path_table_absolute_outside_root_rejected() {
    let Some(adapter) = any_enabled() else { return };
    // allowlist = scratch root A; candidate lives in unrelated scratch root B.
    let (root_a, _dll, policy) = rooted_fixture("allow-a", adapter, b"synthetic stub");
    let root_b = scratch_root("outside-b");
    let outside = root_b.join(adapter.dll_file_name());
    write_stub(&outside, b"synthetic stub");
    let result = ValidatedDllPath::validate(&outside, adapter, &policy);
    remove_root(&root_a);
    remove_root(&root_b);
    assert!(matches!(result, Err(CompatError::PathOutsideAllowlist)));
}

#[test]
fn path_table_wrong_extension_rejected() {
    let Some(adapter) = any_enabled() else { return };
    let root = scratch_root("badext");
    let candidate = root.join("word_trie.dat");
    write_stub(&candidate, b"synthetic stub");
    let policy = DllPathPolicy {
        allowed_roots: vec![root.clone()],
        ..DllPathPolicy::default()
    };
    // Rule 4 fires before rule 5 (extension is checked first), so the
    // static reason is always `bad_extension` here and never a path leak.
    let result = ValidatedDllPath::validate(&candidate, adapter, &policy);
    remove_root(&root);
    assert!(matches!(
        result,
        Err(CompatError::InvalidDllPath {
            reason: "bad_extension"
        })
    ));
}

#[test]
fn path_table_wrong_file_name_rejected() {
    let Some(adapter) = any_enabled() else { return };
    // Pick a *different* adapter's file name so the extension is right but the
    // name is wrong.
    let other = AdapterKind::ALL
        .iter()
        .copied()
        .find(|kind| kind.dll_file_name() != adapter.dll_file_name())
        .expect("two adapters share no file name");
    let root = scratch_root("wrongname");
    let candidate = root.join(other.dll_file_name());
    write_stub(&candidate, b"synthetic stub");
    let policy = DllPathPolicy {
        allowed_roots: vec![root.clone()],
        ..DllPathPolicy::default()
    };
    let result = ValidatedDllPath::validate(&candidate, adapter, &policy);
    remove_root(&root);
    assert!(matches!(
        result,
        Err(CompatError::InvalidDllPath {
            reason: "wrong_file_name"
        })
    ));
}

#[test]
fn path_table_symlink_escape_rejected() {
    let Some(adapter) = any_enabled() else { return };
    let (root, _dll, policy) = rooted_fixture("linkroot", adapter, b"synthetic stub");
    let outside_dir = scratch_root("linkoutside");
    let outside = outside_dir.join(adapter.dll_file_name());
    write_stub(&outside, b"synthetic stub");
    let link = root.join(adapter.dll_file_name());
    // Remove the real file first so the link name is free.
    let _ = std::fs::remove_file(&link);
    let linked = create_symlink_file(&outside, &link);
    if !linked {
        eprintln!("SKIP: symlink creation not permitted on this machine");
        remove_root(&root);
        remove_root(&outside_dir);
        return;
    }
    let result = ValidatedDllPath::validate(&link, adapter, &policy);
    remove_root(&root);
    remove_root(&outside_dir);
    assert!(
        matches!(result, Err(CompatError::PathOutsideAllowlist)),
        "symlink escape must not validate, got {result:?}"
    );
}

#[cfg(windows)]
fn create_symlink_file(original: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_file(original, link).is_ok()
}

#[cfg(not(windows))]
fn create_symlink_file(original: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(original, link).is_ok()
}

#[test]
fn path_table_oversized_path_rejected() {
    let Some(adapter) = any_enabled() else { return };
    let (root, dll, mut policy) = rooted_fixture("bigpath", adapter, b"synthetic stub");
    policy.max_path_bytes = 8; // every real canonical path exceeds this
    let result = ValidatedDllPath::validate(&dll, adapter, &policy);
    remove_root(&root);
    assert!(matches!(result, Err(CompatError::BoundExceeded { .. })));
}

#[test]
fn path_table_oversized_file_rejected() {
    let Some(adapter) = any_enabled() else { return };
    let (root, dll, mut policy) = rooted_fixture("bigfile", adapter, b"0123456789");
    policy.max_file_bytes = 4; // stub is 10 bytes
    let result = ValidatedDllPath::validate(&dll, adapter, &policy);
    remove_root(&root);
    assert!(matches!(result, Err(CompatError::BoundExceeded { .. })));
}

#[test]
fn path_table_valid_canonical_path() {
    let Some(adapter) = any_enabled() else { return };
    let (root, dll, policy) = rooted_fixture("valid", adapter, b"synthetic stub");
    let validated = ValidatedDllPath::validate(&dll, adapter, &policy)
        .expect("in-allowlist canonical path must validate");
    assert_eq!(validated.adapter(), adapter);
    assert!(validated.path().is_absolute());
    // Never the caller's original string when it differs (e.g. `.` segments
    // are impossible here by rule, but canonical form is returned regardless).
    assert_eq!(
        validated.path(),
        &dll.canonicalize().expect("candidate exists")
    );
    remove_root(&root);
}

#[test]
fn error_display_never_contains_candidate_path() {
    let Some(adapter) = any_enabled() else { return };
    let (root, _dll, policy) = rooted_fixture("hygiene", adapter, b"synthetic stub");
    let outside_dir = scratch_root("hygiene-outside");
    let outside = outside_dir.join(adapter.dll_file_name());
    write_stub(&outside, b"synthetic stub");
    let errors: Vec<CompatError> = vec![
        ValidatedDllPath::validate(Path::new("relative.dll"), adapter, &policy)
            .expect_err("relative must fail"),
        ValidatedDllPath::validate(&outside, adapter, &policy).expect_err("outside root must fail"),
        AdapterRegistry::new(policy)
            .unload(adapter)
            .expect_err("not loaded"),
    ];
    remove_root(&root);
    remove_root(&outside_dir);
    for error in &errors {
        let rendered = error.to_string();
        assert!(
            !rendered.contains("okkhor-compat-test")
                && !rendered.contains("hygiene")
                && !rendered.contains(".dll"),
            "Display leaked path/text: {rendered}"
        );
        assert!(!format!("{error:?}").contains("okkhor-compat-test"));
    }
    // kind()/code() are the only logging-safe accessors.
    assert_eq!(errors[0].kind(), "compat_invalid_dll_path");
    assert_eq!(errors[1].kind(), "compat_path_outside_allowlist");
    assert_eq!(errors[2].kind(), "compat_not_loaded");
    assert!(errors.iter().all(|error| error.code().is_none()));
}

#[test]
fn registry_lifecycle_load_unload_idempotent() {
    // NOTE: this integration target links the normally-compiled library, so
    // `cfg(test)` stubbing does not apply here and `load()` on an enabled
    // adapter would invoke the real OS loader. Successful stubbed loads are
    // covered by the `cfg(test)` unit tests in `src/adapter.rs`; this test
    // only exercises loader-free paths (feature gates, `NotLoaded`, status
    // ordering), so no real DLL is ever opened.
    let enabled = enabled_adapters();
    let mut registry = AdapterRegistry::new(DllPathPolicy::default());
    for adapter in AdapterKind::ALL {
        if adapter.is_enabled() {
            assert!(!registry.is_loaded(adapter));
            // Nothing was ever loaded through this target: unload is NotLoaded.
            assert!(matches!(
                registry.unload(adapter),
                Err(CompatError::NotLoaded { .. })
            ));
        } else {
            // Disabled adapters reject before any path check or loader call,
            // without touching the filesystem (relative bogus path).
            assert!(matches!(
                registry.load(Path::new(adapter.dll_file_name()), adapter),
                Err(CompatError::FeatureDisabled { .. })
            ));
            assert!(matches!(
                registry.unload(adapter),
                Err(CompatError::NotLoaded { .. })
            ));
        }
    }
    assert_eq!(enabled.len(), enabled_adapters().len());
    let kinds: Vec<AdapterKind> = registry
        .statuses()
        .iter()
        .map(|status| status.kind)
        .collect();
    assert_eq!(kinds, AdapterKind::ALL.to_vec());
}

#[test]
fn unload_never_loaded_reports_not_loaded() {
    let mut registry = AdapterRegistry::new(DllPathPolicy::default());
    for adapter in AdapterKind::ALL {
        if adapter.is_enabled() {
            assert!(matches!(
                registry.unload(adapter),
                Err(CompatError::NotLoaded { .. })
            ));
        }
    }
}

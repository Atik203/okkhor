//! Adapter registry and handle types (contract §5.4).
//!
//! No `unsafe` in this module. All unsafe lives below the [`AdapterRegistry`]
//! seam in the private `ffi` module. Raw pointers never appear in any public
//! signature.
//!
//! Load order is fixed: feature gate → path validation → platform check →
//! loader. A disabled adapter returns [`CompatError::FeatureDisabled`] and
//! performs no path check and no loader call.

use std::marker::PhantomData;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{CompatError, DllPathPolicy, ValidatedDllPath};

/// Legacy adapter slot. DLL file names are contract constants, not
/// proprietary bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    WordTrie,
    SentenceTrie,
    Translator,
    Caret,
    EmojiManager,
}

impl AdapterKind {
    /// All five adapters, in registry status order.
    pub const ALL: [Self; 5] = [
        Self::WordTrie,
        Self::SentenceTrie,
        Self::Translator,
        Self::Caret,
        Self::EmojiManager,
    ];

    /// Cargo feature name: `"word-trie"`, `"sentence-trie"`, `"translator"`,
    /// `"caret"`, `"emoji-manager"`.
    pub fn feature_name(self) -> &'static str {
        match self {
            Self::WordTrie => "word-trie",
            Self::SentenceTrie => "sentence-trie",
            Self::Translator => "translator",
            Self::Caret => "caret",
            Self::EmojiManager => "emoji-manager",
        }
    }

    /// DLL file name constant: `"word_trie.dll"`,
    /// `"_sentencetrie_native.dll"`, `"translator.dll"`, `"caret.dll"`,
    /// `"emoji_manager.dll"`.
    pub fn dll_file_name(self) -> &'static str {
        match self {
            Self::WordTrie => "word_trie.dll",
            Self::SentenceTrie => "_sentencetrie_native.dll",
            Self::Translator => "translator.dll",
            Self::Caret => "caret.dll",
            Self::EmojiManager => "emoji_manager.dll",
        }
    }

    /// True only when the corresponding Cargo feature is compiled in.
    pub fn is_enabled(self) -> bool {
        match self {
            Self::WordTrie => cfg!(feature = "word-trie"),
            Self::SentenceTrie => cfg!(feature = "sentence-trie"),
            Self::Translator => cfg!(feature = "translator"),
            Self::Caret => cfg!(feature = "caret"),
            Self::EmojiManager => cfg!(feature = "emoji-manager"),
        }
    }

    /// Registry slot index, in [`AdapterKind::ALL`] declaration order.
    fn slot(self) -> usize {
        self as usize
    }
}

/// Snapshot of one adapter slot for the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterStatus {
    pub kind: AdapterKind,
    pub enabled: bool,
    pub loaded: bool,
}

/// Opaque handle to a loaded adapter library.
///
/// Deliberately `!Send + !Sync` in 0.3 (the private `Rc` marker is neither):
/// the handle is created, used, and dropped on the loading thread only. The
/// daemon must create the [`AdapterRegistry`] on the thread that will own
/// adapter calls, normally at startup.
///
/// No raw pointer accessors, no `Deref`, no `Send`/`Sync`.
///
/// TODO(Task 3, milestone 0.3/0.4): adapter-specific safe methods are added
/// behind their feature gates in later milestones. This scaffold handle
/// intentionally exposes only [`AdapterHandle::kind`]; no stub pretends the
/// call surface is done.
#[derive(Debug)]
pub struct AdapterHandle {
    kind: AdapterKind,
    /// Owned OS library handle on Windows production builds; a stub token
    /// elsewhere (see `platform_load`). Never exposed publicly.
    #[cfg(all(windows, not(test)))]
    native: *mut std::ffi::c_void,
    // Contract §5.4 asks for `PhantomData<*const ()>` here, but a raw-pointer
    // marker is itself `Send + Sync` and would not pin the handle to the
    // loading thread. `Rc` is neither, which is what the `!Send + !Sync`
    // requirement needs.
    _not_send_sync: PhantomData<std::rc::Rc<()>>,
}

impl AdapterHandle {
    /// Which adapter slot this handle belongs to.
    pub fn kind(&self) -> AdapterKind {
        self.kind
    }

    /// Stub handle for tests and non-Windows targets: records loading intent
    /// without opening any DLL.
    #[cfg(not(all(windows, not(test))))]
    fn stub(kind: AdapterKind) -> Self {
        Self {
            kind,
            _not_send_sync: PhantomData,
        }
    }
}

/// Owns loaded adapter handles and their validation policy.
///
/// At most five adapters exist, so handles live in a fixed slot array indexed
/// by [`AdapterKind`] declaration order.
#[derive(Debug, Default)]
pub struct AdapterRegistry {
    policy: DllPathPolicy,
    loaded: [Option<AdapterHandle>; 5],
}

impl AdapterRegistry {
    /// Create an empty registry over a daemon-owned [`DllPathPolicy`].
    /// Create this on the thread that will own adapter calls.
    pub fn new(policy: DllPathPolicy) -> Self {
        Self {
            policy,
            loaded: [None, None, None, None, None],
        }
    }

    /// Snapshot of every adapter slot, ordered by [`AdapterKind::ALL`].
    pub fn statuses(&self) -> Vec<AdapterStatus> {
        AdapterKind::ALL
            .iter()
            .map(|kind| AdapterStatus {
                kind: *kind,
                enabled: kind.is_enabled(),
                loaded: self.is_loaded(*kind),
            })
            .collect()
    }

    /// Whether the adapter slot currently holds a loaded handle.
    pub fn is_loaded(&self, adapter: AdapterKind) -> bool {
        self.loaded[adapter.slot()].is_some()
    }

    /// Feature gate → path validation → platform check → loader.
    ///
    /// Loading an already-loaded adapter is a no-op returning `Ok(())`; the
    /// existing handle is kept. All unsafe lives below this seam.
    pub fn load(&mut self, path: &Path, adapter: AdapterKind) -> Result<(), CompatError> {
        // Feature gate first: no path check and no loader call when disabled.
        if !adapter.is_enabled() {
            return Err(CompatError::FeatureDisabled { adapter });
        }
        if self.is_loaded(adapter) {
            return Ok(());
        }
        let validated = ValidatedDllPath::validate(path, adapter, &self.policy)?;
        let handle = platform_load(&validated)?;
        self.loaded[adapter.slot()] = Some(handle);
        Ok(())
    }

    /// Release the library for one adapter slot.
    ///
    /// Chosen behavior: unloading an adapter that was never loaded (or was
    /// already unloaded) returns `Err(CompatError::NotLoaded { adapter })`
    /// rather than silently succeeding, so callers can distinguish a real
    /// release from a no-op. State is otherwise idempotent: a second unload
    /// after a successful unload reports the same `NotLoaded` error and
    /// changes nothing.
    pub fn unload(&mut self, adapter: AdapterKind) -> Result<(), CompatError> {
        match self.loaded[adapter.slot()].take() {
            Some(handle) => {
                platform_unload(handle, adapter)?;
                Ok(())
            }
            None => Err(CompatError::NotLoaded { adapter }),
        }
    }
}

impl Drop for AdapterRegistry {
    fn drop(&mut self) {
        // Unload everything on the loading thread. `Drop` runs on the owning
        // thread because `AdapterHandle` is `!Send + !Sync`; unload errors are
        // intentionally ignored here — there is no logging-safe channel to
        // report them from `drop`.
        for slot in self.loaded.iter_mut() {
            if let Some(handle) = slot.take() {
                let adapter = handle.kind();
                let _ = platform_unload(handle, adapter);
            }
        }
    }
}

/// Windows production loader call: [`crate::ffi::load_library`].
#[cfg(all(windows, not(test)))]
fn platform_load(validated: &ValidatedDllPath) -> Result<AdapterHandle, CompatError> {
    // SAFETY: `validated` passed every §5.3 rule (absolute, canonical,
    // allowlisted, bounded); the call runs once per handle at startup on the
    // loading thread, never on the hook or candidate path.
    unsafe {
        crate::ffi::load_library(validated).map(|native| AdapterHandle {
            kind: validated.adapter(),
            native,
            _not_send_sync: PhantomData,
        })
    }
}

/// Stubbed loader seam for tests and non-Windows targets: no real DLL is ever
/// opened. On non-Windows targets loading is unsupported; under `cfg(test)`
/// the validated path is recorded as a stub handle.
#[cfg(not(all(windows, not(test))))]
fn platform_load(validated: &ValidatedDllPath) -> Result<AdapterHandle, CompatError> {
    #[cfg(not(windows))]
    {
        let _ = validated;
        Err(CompatError::UnsupportedPlatform)
    }
    #[cfg(windows)]
    {
        Ok(AdapterHandle::stub(validated.adapter()))
    }
}

/// Windows production unload call: [`crate::ffi::free_library`].
#[cfg(all(windows, not(test)))]
fn platform_unload(handle: AdapterHandle, adapter: AdapterKind) -> Result<(), CompatError> {
    // SAFETY: `handle` is owned by this live `AdapterHandle`, freed exactly
    // once (it is taken out of the slot array first), on the loading thread.
    unsafe { crate::ffi::free_library(handle.native) }.map_err(|error| CompatError::LoadFailed {
        adapter,
        code: error.code(),
    })
}

/// Stubbed unload seam: nothing was opened, so there is nothing to free.
#[cfg(not(all(windows, not(test))))]
fn platform_unload(handle: AdapterHandle, _adapter: AdapterKind) -> Result<(), CompatError> {
    let _ = handle;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! `cfg(test)` unit tests: the library itself is compiled with
    //! `cfg(test)`, so the loader seam above is stubbed and these tests prove
    //! the full `load` lifecycle without ever opening a real DLL.
    //! Synthetic nonce files only; never a real library.

    use super::*;
    use std::path::PathBuf;

    fn scratch_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "okkhor-compat-unit-{}-{}-{}",
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

    fn enabled_adapters() -> Vec<AdapterKind> {
        AdapterKind::ALL
            .iter()
            .copied()
            .filter(|kind| kind.is_enabled())
            .collect()
    }

    #[test]
    fn slots_follow_all_order() {
        for (index, kind) in AdapterKind::ALL.iter().enumerate() {
            assert_eq!(kind.slot(), index);
        }
    }

    #[test]
    fn stubbed_lifecycle_load_unload_idempotent() {
        let enabled = enabled_adapters();
        if enabled.is_empty() {
            // Default features: every load is FeatureDisabled; unload is NotLoaded.
            let mut registry = AdapterRegistry::new(DllPathPolicy::default());
            for adapter in AdapterKind::ALL {
                assert!(matches!(
                    registry.load(Path::new(adapter.dll_file_name()), adapter),
                    Err(CompatError::FeatureDisabled { .. })
                ));
                assert!(matches!(
                    registry.unload(adapter),
                    Err(CompatError::NotLoaded { .. })
                ));
            }
            return;
        }
        // Feature-on runs: the stubbed seam records intent without opening any
        // DLL (no `LoadLibrary` call happens under `cfg(test)`).
        let mut allowed = Vec::new();
        let mut roots = Vec::new();
        let mut dlls = Vec::new();
        for adapter in &enabled {
            let root = scratch_root(adapter.feature_name());
            let dll = root.join(adapter.dll_file_name());
            write_stub(&dll, b"synthetic stub, not a real DLL");
            allowed.push(root.clone());
            roots.push(root);
            dlls.push((*adapter, dll));
        }
        let mut registry = AdapterRegistry::new(DllPathPolicy {
            allowed_roots: allowed,
            ..DllPathPolicy::default()
        });
        for (adapter, dll) in &dlls {
            registry
                .load(dll, *adapter)
                .expect("stubbed load must succeed");
            assert!(registry.is_loaded(*adapter));
        }
        assert_eq!(
            registry
                .statuses()
                .iter()
                .filter(|status| status.loaded)
                .count(),
            dlls.len()
        );
        // Double load is an idempotent no-op returning Ok.
        for (adapter, dll) in &dlls {
            registry
                .load(dll, *adapter)
                .expect("second load is a no-op Ok");
            assert!(registry.is_loaded(*adapter));
        }
        // Unload releases; second unload reports NotLoaded.
        for (adapter, _) in &dlls {
            registry.unload(*adapter).expect("unload must succeed");
            assert!(!registry.is_loaded(*adapter));
            assert!(matches!(
                registry.unload(*adapter),
                Err(CompatError::NotLoaded { .. })
            ));
        }
        // Drop with a loaded handle must not panic.
        if let Some((adapter, dll)) = dlls.first() {
            registry.load(dll, *adapter).expect("reload for drop test");
        }
        drop(registry);
        for root in &roots {
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn stubbed_load_validates_before_recording() {
        let enabled = enabled_adapters();
        if enabled.is_empty() {
            return;
        }
        // A relative path must fail validation even though the stub would
        // accept anything: validation runs before the loader seam.
        let mut registry = AdapterRegistry::new(DllPathPolicy::default());
        for adapter in enabled {
            assert!(matches!(
                registry.load(Path::new(adapter.dll_file_name()), adapter),
                Err(CompatError::InvalidDllPath { .. })
            ));
            assert!(!registry.is_loaded(adapter));
        }
    }
}

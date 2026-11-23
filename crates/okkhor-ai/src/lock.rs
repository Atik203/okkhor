use std::sync::{Mutex, MutexGuard, PoisonError};

/// Lock a mutex, recovering the value if a previous holder panicked.
///
/// The AI core holds no user text in a way that a poisoned guard could corrupt,
/// so recovery keeps the worker responsive instead of propagating a panic.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

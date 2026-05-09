use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Per-root cancellation tokens shared between `scan_cancellable` (the worker
/// thread that polls `Arc<AtomicBool>::load`) and the `cancel_scan` Tauri
/// command (which calls `store(true)`).
///
/// The map is keyed by `root.id`. There is at most one in-flight cancellation
/// token per root — re-registering replaces any previous token, which is fine
/// because callers only register before launching a worker for that root.
pub struct ScanCancellations {
    flags: Mutex<HashMap<i64, Arc<AtomicBool>>>,
}

impl Default for ScanCancellations {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanCancellations {
    pub fn new() -> Self {
        Self {
            flags: Mutex::new(HashMap::new()),
        }
    }

    /// Register a fresh cancellation token for `root_id` and return it.
    /// The returned `Arc` should be moved into the scan worker thread.
    pub fn register(&self, root_id: i64) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        let mut g = self.flags.lock().unwrap_or_else(|e| e.into_inner());
        g.insert(root_id, flag.clone());
        flag
    }

    /// Race-free check-and-insert. Returns `Some(token)` when no scan was
    /// previously tracked for this root and we just claimed the slot, `None`
    /// when a scan is already in flight.
    ///
    /// Why: `is_active()` followed by `register()` is a TOCTOU race — two
    /// auto-scan callers could both observe "not active" and then both call
    /// `register()`, which silently replaces the first token and orphans
    /// that worker's cancel signal. `try_register` collapses both steps into
    /// a single critical section so duplicates are impossible.
    pub fn try_register(&self, root_id: i64) -> Option<Arc<AtomicBool>> {
        let mut g = self.flags.lock().unwrap_or_else(|e| e.into_inner());
        if g.contains_key(&root_id) {
            return None;
        }
        let flag = Arc::new(AtomicBool::new(false));
        g.insert(root_id, flag.clone());
        Some(flag)
    }

    /// Signal cancellation for `root_id`. Returns `true` when a token existed
    /// and was set, `false` when no scan was tracked.
    pub fn cancel(&self, root_id: i64) -> bool {
        let g = self.flags.lock().unwrap_or_else(|e| e.into_inner());
        match g.get(&root_id) {
            Some(flag) => {
                flag.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// Drop the cancellation token after the worker has finished. Idempotent.
    pub fn clear(&self, root_id: i64) {
        let mut g = self.flags.lock().unwrap_or_else(|e| e.into_inner());
        g.remove(&root_id);
    }

    /// Returns `true` when a scan worker is currently registered for this
    /// root id. Used by `start_scan_all_roots` to skip roots that already
    /// have an in-flight scan instead of racing two workers on the same DB.
    pub fn is_active(&self, root_id: i64) -> bool {
        let g = self.flags.lock().unwrap_or_else(|e| e.into_inner());
        g.contains_key(&root_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_returns_unset_token() {
        let regs = ScanCancellations::new();
        let token = regs.register(1);
        assert!(!token.load(Ordering::Relaxed));
    }

    #[test]
    fn cancel_sets_the_token_for_that_root() {
        let regs = ScanCancellations::new();
        let token = regs.register(7);
        assert!(regs.cancel(7));
        assert!(token.load(Ordering::Relaxed));
    }

    #[test]
    fn cancel_for_unknown_root_returns_false() {
        let regs = ScanCancellations::new();
        assert!(!regs.cancel(999));
    }

    #[test]
    fn cancel_does_not_affect_other_roots() {
        let regs = ScanCancellations::new();
        let a = regs.register(1);
        let b = regs.register(2);
        regs.cancel(1);
        assert!(a.load(Ordering::Relaxed));
        assert!(!b.load(Ordering::Relaxed));
    }

    #[test]
    fn clear_removes_token() {
        let regs = ScanCancellations::new();
        regs.register(1);
        regs.clear(1);
        assert!(!regs.cancel(1));
    }

    #[test]
    fn is_active_tracks_register_and_clear() {
        let regs = ScanCancellations::new();
        assert!(!regs.is_active(5));
        regs.register(5);
        assert!(regs.is_active(5));
        regs.clear(5);
        assert!(!regs.is_active(5));
    }

    #[test]
    fn try_register_succeeds_when_no_prior_token() {
        let regs = ScanCancellations::new();
        let token = regs.try_register(11).expect("first claim must succeed");
        assert!(!token.load(Ordering::Relaxed));
        assert!(regs.is_active(11));
    }

    #[test]
    fn try_register_returns_none_when_already_active() {
        let regs = ScanCancellations::new();
        let _ = regs.try_register(11).unwrap();
        assert!(
            regs.try_register(11).is_none(),
            "a second claim while one is in flight must fail"
        );
    }

    #[test]
    fn try_register_succeeds_again_after_clear() {
        let regs = ScanCancellations::new();
        let _ = regs.try_register(11).unwrap();
        regs.clear(11);
        assert!(regs.try_register(11).is_some());
    }
}

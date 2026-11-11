//! Progress reporting and cancellation for long-running operations.
//!
//! The UI creates a [`Progress`], hands a clone to the background task, and reads it every
//! frame. The task reports what it is doing with [`Progress::set`] and calls
//! [`Progress::check`] at safe points; after the user presses Cancel, `check` returns a
//! [`Cancelled`] error and the task stops before writing its output.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Error returned when the user cancelled the operation.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cancelled")
    }
}

impl std::error::Error for Cancelled {}

const UNKNOWN: u32 = u32::MAX;

#[derive(Default)]
struct Inner {
    /// Fraction done × 10 000, or `UNKNOWN` (initialised lazily, see `fraction`).
    done: AtomicU32,
    known: AtomicBool,
    cancel: AtomicBool,
    stage: Mutex<String>,
}

#[derive(Clone, Default)]
pub struct Progress(Arc<Inner>);

impl Progress {
    /// Report the current stage and how far along the whole operation is (0.0 – 1.0).
    pub fn set(&self, fraction: f32, stage: impl Into<String>) {
        self.0.done.store(
            (fraction.clamp(0.0, 1.0) * 10_000.0) as u32,
            Ordering::Relaxed,
        );
        self.0.known.store(true, Ordering::Relaxed);
        if let Ok(mut s) = self.0.stage.lock() {
            *s = stage.into();
        }
    }

    /// Report progress through `n` equal steps inside the range `from..to`.
    pub fn step(&self, from: f32, to: f32, i: usize, n: usize, stage: impl Into<String>) {
        let t = if n == 0 { 1.0 } else { i as f32 / n as f32 };
        self.set(from + (to - from) * t, stage);
    }

    /// Fraction done, or `None` before the first report.
    pub fn fraction(&self) -> Option<f32> {
        let v = self.0.done.load(Ordering::Relaxed);
        (self.0.known.load(Ordering::Relaxed) && v != UNKNOWN).then(|| v as f32 / 10_000.0)
    }

    pub fn stage(&self) -> String {
        self.0.stage.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn cancel(&self) {
        self.0.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancel.load(Ordering::Relaxed)
    }

    /// Stop here if the user pressed Cancel.
    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Whether an error (possibly wrapped in context) means "the user cancelled".
pub fn is_cancelled(e: &anyhow::Error) -> bool {
    e.downcast_ref::<Cancelled>().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_and_cancel() {
        let p = Progress::default();
        assert_eq!(p.fraction(), None);
        p.step(0.2, 0.6, 1, 4, "Working");
        assert!((p.fraction().unwrap() - 0.3).abs() < 1e-3);
        assert_eq!(p.stage(), "Working");
        assert!(p.check().is_ok());
        p.clone().cancel();
        let err: anyhow::Error =
            anyhow::Error::from(p.check().unwrap_err()).context("while saving");
        assert!(is_cancelled(&err));
    }
}

//! Cooperative interruption of synchronous background analysis.
//!
//! A checkpoint unwinds to `run`, so a nested type lookup cannot turn an
//! interrupted body into an empty type and cache it. Existing panic fences
//! must pass this private payload through. Ordinary panics keep their usual
//! handling, and cancellation does not invoke the panic hook.

use std::any::Any;
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Why a background analysis stopped before producing a complete result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cancelled {
    Shutdown,
    Deadline,
}

/// A shared deadline and shutdown signal, checked within each worker's walk.
pub(crate) struct AnalysisCancellation {
    shutdown: Arc<AtomicBool>,
    deadline: Instant,
    /// A controlled clock lets tests expire during a chosen analysis stage.
    #[cfg(test)]
    pub(crate) clock: Option<Box<dyn Fn() -> Instant + Send + Sync>>,
}

thread_local! {
    static ACTIVE: RefCell<Option<Arc<AnalysisCancellation>>> = const { RefCell::new(None) };
}

struct Restore(Option<Arc<AnalysisCancellation>>);

impl Drop for Restore {
    fn drop(&mut self) {
        ACTIVE.with(|active| *active.borrow_mut() = self.0.take());
    }
}

impl AnalysisCancellation {
    /// Share the server's shutdown flag and one deadline across file workers.
    pub(crate) fn new(shutdown: Arc<AtomicBool>, deadline: Instant) -> Self {
        Self {
            shutdown,
            deadline,
            #[cfg(test)]
            clock: None,
        }
    }

    /// Check the deadline and shutdown flag without unwinding.
    pub(crate) fn reason(&self) -> Option<Cancelled> {
        #[cfg(test)]
        let now = self
            .clock
            .as_ref()
            .map_or_else(Instant::now, |clock| clock());
        #[cfg(not(test))]
        let now = Instant::now();
        if self.shutdown.load(Ordering::Acquire) {
            Some(Cancelled::Shutdown)
        } else if now >= self.deadline {
            Some(Cancelled::Deadline)
        } else {
            None
        }
    }

    /// Run one synchronous operation, returning no partial result on cancellation.
    /// Cache guards belong inside `operation`, so unwinding clears its temporary
    /// scopes and inference state before the caller handles `Err`.
    pub(crate) fn run<T>(self: &Arc<Self>, operation: impl FnOnce() -> T) -> Result<T, Cancelled> {
        let _restore = Restore(ACTIVE.with(|active| active.borrow_mut().replace(Arc::clone(self))));
        match catch_unwind(AssertUnwindSafe(|| {
            checkpoint();
            operation()
        })) {
            Ok(value) => Ok(value),
            Err(payload) => match payload.downcast_ref::<Cancelled>() {
                Some(reason) => Err(*reason),
                None => resume_unwind(payload),
            },
        }
    }
}

/// Stop at a safe boundary before more analysis or cache publication.
/// With no background operation active, this is just a thread-local check.
#[inline]
pub(crate) fn checkpoint() {
    let reason = ACTIVE.with(|active| active.borrow().as_ref().and_then(|active| active.reason()));
    if let Some(reason) = reason {
        resume_unwind(Box::new(reason));
    }
}

/// Keep cancellation out of error-recovery paths that return default values.
pub(crate) fn rethrow_if_cancelled(payload: &(dyn Any + Send)) {
    if let Some(reason) = payload.downcast_ref::<Cancelled>() {
        resume_unwind(Box::new(*reason));
    }
}

#[cfg(test)]
mod tests;

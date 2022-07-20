mod aliasing;
mod cache_padded;
pub mod loom;

pub use aliasing::*;
pub use cache_padded::*;

use self::loom::sync::{Mutex, MutexGuard, PoisonError};

#[inline(always)]
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    if cfg!(debug_assertions) {
        mutex
            .lock()
            .expect("Internal mutex(es) should never be poisoned")
    } else {
        // At the moment this has the same asm as calling unwrap_unchecked
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cold]
#[inline]
#[allow(dead_code)]
pub(crate) fn cold() {}

#[cfg(feature = "nightly")]
pub(crate) use ::core::intrinsics::{likely, unlikely};

#[cfg(not(feature = "nightly"))]
#[inline]
pub(crate) fn likely(b: bool) -> bool {
    if !b {
        cold();
    }
    b
}

#[cfg(not(feature = "nightly"))]
#[inline]
pub(crate) fn unlikely(b: bool) -> bool {
    if b {
        cold();
    }
    b
}

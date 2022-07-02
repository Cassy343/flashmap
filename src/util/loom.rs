#[cfg(loom)]
pub use loom::*;

#[cfg(not(loom))]
pub use std::{hint, sync, thread};

#[cfg(not(loom))]
pub mod cell {
    pub use std::cell::Cell;
    use std::cell::UnsafeCell as StdUnsafeCell;

    #[repr(transparent)]
    pub struct UnsafeCell<T: ?Sized> {
        inner: StdUnsafeCell<T>,
    }

    impl<T> UnsafeCell<T> {
        #[inline(always)]
        pub fn new(value: T) -> Self {
            Self {
                inner: StdUnsafeCell::new(value),
            }
        }
    }

    impl<T: ?Sized> UnsafeCell<T> {
        #[inline(always)]
        pub fn with<F, R>(&self, f: F) -> R
        where
            F: FnOnce(*const T) -> R,
        {
            f(self.inner.get())
        }

        #[inline(always)]
        pub fn with_mut<F, R>(&self, f: F) -> R
        where
            F: FnOnce(*mut T) -> R,
        {
            f(self.inner.get())
        }
    }
}

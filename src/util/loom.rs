#[cfg(loom)]
pub use loom::{hint, thread};

#[cfg(not(loom))]
pub use std::{hint, sync, thread};

#[cfg(loom)]
pub mod sync {
    pub use loom::sync::*;
    pub use std::sync::PoisonError;
}

#[cfg(loom)]
pub mod cell {
    pub use loom::cell::Cell;
    use std::{
        marker::PhantomData,
        ops::{Deref, DerefMut},
    };

    pub struct MutPtr<'a, T: ?Sized> {
        mut_ptr: loom::cell::MutPtr<T>,
        _lifetime: PhantomData<&'a ()>,
    }

    impl<'a, T> Deref for MutPtr<'a, T> {
        type Target = T;

        #[inline(always)]
        fn deref(&self) -> &Self::Target {
            unsafe { self.mut_ptr.deref() }
        }
    }

    impl<'a, T> DerefMut for MutPtr<'a, T> {
        #[inline(always)]
        fn deref_mut(&mut self) -> &mut Self::Target {
            unsafe { self.mut_ptr.deref() }
        }
    }

    #[repr(transparent)]
    pub struct UnsafeCell<T: ?Sized> {
        inner: loom::cell::UnsafeCell<T>,
    }

    impl<T> UnsafeCell<T> {
        #[inline(always)]
        pub fn new(value: T) -> Self {
            Self {
                inner: loom::cell::UnsafeCell::new(value),
            }
        }

        #[inline(always)]
        pub fn into_inner(self) -> T {
            self.inner.into_inner()
        }
    }

    impl<T: ?Sized> UnsafeCell<T> {
        #[inline(always)]
        pub fn get_mut(&mut self) -> MutPtr<'_, T> {
            MutPtr {
                mut_ptr: self.inner.get_mut(),
                _lifetime: PhantomData,
            }
        }

        #[inline(always)]
        pub fn with<F, R>(&self, f: F) -> R
        where
            F: FnOnce(*const T) -> R,
        {
            self.inner.with(f)
        }

        #[inline(always)]
        pub fn with_mut<F, R>(&self, f: F) -> R
        where
            F: FnOnce(*mut T) -> R,
        {
            self.inner.with_mut(f)
        }
    }
}

#[cfg(not(loom))]
pub mod cell {
    pub use std::cell::Cell;
    use std::{
        cell::UnsafeCell as StdUnsafeCell,
        ops::{Deref, DerefMut},
    };

    pub struct MutPtr<'a, T: ?Sized> {
        mut_ptr: &'a mut T,
    }

    impl<'a, T> Deref for MutPtr<'a, T> {
        type Target = T;

        #[inline(always)]
        fn deref(&self) -> &Self::Target {
            self.mut_ptr
        }
    }

    impl<'a, T> DerefMut for MutPtr<'a, T> {
        #[inline(always)]
        fn deref_mut(&mut self) -> &mut Self::Target {
            self.mut_ptr
        }
    }

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

        #[inline(always)]
        pub fn into_inner(self) -> T {
            self.inner.into_inner()
        }
    }

    impl<T: ?Sized> UnsafeCell<T> {
        #[inline(always)]
        pub fn get_mut(&mut self) -> MutPtr<'_, T> {
            MutPtr {
                mut_ptr: self.inner.get_mut(),
            }
        }

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

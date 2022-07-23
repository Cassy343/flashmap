#![allow(dead_code)]

#[cfg(loom)]
pub use loom::*;
#[cfg(not(loom))]
pub use std::{sync, thread};

pub use track_access::*;

#[allow(clippy::borrowed_box)]
pub fn dderef<T>(x: &Box<T>) -> &T {
    x
}

pub fn maybe_loom_model<F>(test: F)
where
    F: Fn() + Send + Sync + 'static,
{
    #[cfg(loom)]
    loom::model(test);
    #[cfg(not(loom))]
    test();
}

#[cfg(loom)]
mod track_access {
    use loom::{alloc::Track, cell::UnsafeCell};
    use std::{
        borrow::Borrow,
        hash::{Hash, Hasher},
    };

    use flashmap::TrustedHashEq;

    pub struct TrackAccess<T>(Track<Box<UnsafeCell<T>>>);

    unsafe impl<T> TrustedHashEq for TrackAccess<T> where Self: Hash + Eq {}

    impl<T> TrackAccess<T> {
        pub fn new(val: T) -> Self {
            Self(Track::new(Box::new(UnsafeCell::new(val))))
        }

        pub fn get(&self) -> &T {
            self.0.get_ref().with(|ptr| unsafe { &*ptr })
        }

        pub fn get_mut(&mut self) -> &mut T {
            self.0.get_ref().with_mut(|ptr| unsafe { &mut *ptr })
        }
    }

    unsafe impl<T: Sync> Sync for TrackAccess<T> {}

    impl<T: PartialEq> PartialEq for TrackAccess<T> {
        fn eq(&self, other: &Self) -> bool {
            PartialEq::eq(self.get(), other.get())
        }
    }

    impl<T: Eq> Eq for TrackAccess<T> {}

    impl<T: Hash> Hash for TrackAccess<T> {
        fn hash<H: Hasher>(&self, state: &mut H) {
            Hash::hash(self.get(), state)
        }
    }

    impl<T> Borrow<T> for TrackAccess<T> {
        fn borrow(&self) -> &T {
            self.0.get_ref().with(|ptr| unsafe { &*ptr })
        }
    }
}

#[cfg(not(loom))]
mod track_access {
    use std::borrow::Borrow;
    use std::hash::Hash;

    use flashmap::TrustedHashEq;

    // The intent is that the tests are run with miri which will do the tracking through the box
    #[derive(PartialEq, Eq, Hash)]
    pub struct TrackAccess<T>(Box<T>);

    unsafe impl<T> TrustedHashEq for TrackAccess<T> where Self: Hash + Eq {}

    impl<T> TrackAccess<T> {
        pub fn new(val: T) -> Self {
            Self(Box::new(val))
        }

        pub fn get(&self) -> &T {
            &self.0
        }

        pub fn get_mut(&mut self) -> &mut T {
            &mut self.0
        }
    }

    impl<T> Borrow<T> for TrackAccess<T> {
        fn borrow(&self) -> &T {
            &self.0
        }
    }
}

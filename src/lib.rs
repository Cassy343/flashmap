#![cfg_attr(feature = "nightly", feature(core_intrinsics))]
#![deny(rust_2018_idioms, unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

pub mod algorithm;
mod core;
mod read;
mod util;
mod view;
mod write;

pub use read::*;
pub(crate) use util::loom;
pub use util::{deterministic::*, Alias};
pub use view::View;
pub use write::*;

use self::core::Core;
use std::{
    collections::hash_map::RandomState,
    fmt::{self, Debug, Formatter},
    hash::{BuildHasher, Hash},
};

pub(crate) type Map<K, V, S = RandomState> = hashbrown::HashMap<Alias<K>, Alias<V>, S>;

/// Creates a new map with a [`RandomState`](std::collections::hash_map::RandomState) hasher.
///
/// If you wish to specify additional parameters, see [`with_capacity`](crate::with_capacity),
/// [`with_hasher`](crate::with_hasher), and [`Builder`](crate::Builder).
pub fn new<K, V>() -> (WriteHandle<K, V>, ReadHandle<K, V>)
where
    K: TrustedHashEq,
{
    Builder::new().build()
}

/// Creates a new map with the specified initial capacity and a
/// [`RandomState`](std::collections::hash_map::RandomState) hasher.
///
/// If you wish to specify additional parameters, see [`Builder`](crate::Builder).
pub fn with_capacity<K, V>(capacity: usize) -> (WriteHandle<K, V>, ReadHandle<K, V>)
where
    K: TrustedHashEq,
{
    Builder::new().with_capacity(capacity).build()
}

/// Creates a new map with the specified hasher.
///
/// If you wish to specify additional parameters, see [`Builder`](crate::Builder).
///
/// # Safety
///
/// The given hasher builder must produce a deterministic hasher. In other words, the built hasher
/// must always produce the same hash given the same input and initial state.
pub unsafe fn with_hasher<K, V, S>(hasher: S) -> (WriteHandle<K, V, S>, ReadHandle<K, V, S>)
where
    K: TrustedHashEq,
    S: Clone + BuildHasher,
{
    unsafe { Builder::new().with_hasher(hasher).build() }
}

/// A builder for a map.
///
/// This builder allows you to specify an initial capacity and a hasher, and provides more
/// flexibility in how that hasher can be constructed.
#[derive(Clone, Copy)]
pub struct Builder<S = RandomState> {
    capacity: usize,
    hasher: HasherGen<S>,
}

impl<S> Debug for Builder<S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Builder")
            .field("capacity", &self.capacity)
            .field("hasher", &std::any::type_name::<S>())
            .finish()
    }
}

impl Builder<RandomState> {
    /// Creates a new builder with a [`RandomState`](std::collections::hash_map::RandomState)
    /// hasher, and an initial capacity of zero.
    pub fn new() -> Self {
        Self {
            capacity: 0,
            hasher: HasherGen::MakeBoth(|| {
                let hasher = RandomState::default();
                (hasher.clone(), hasher)
            }),
        }
    }
}

impl Default for Builder<RandomState> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Builder<S> {
    /// Sets the initial capacity of the map. If not specified, the default is 0.
    pub fn with_capacity(self, capacity: usize) -> Self {
        Self {
            capacity,
            hasher: self.hasher,
        }
    }

    /// Sets the hasher for the underlying map. The provided hasher must implement `Clone` due to
    /// the implementation details of this crate.
    ///
    /// # Safety
    ///
    /// See [`crate::with_hasher`](crate::with_hasher).
    pub unsafe fn with_hasher<H>(self, hasher: H) -> Builder<H>
    where
        H: Clone + BuildHasher,
    {
        Builder {
            capacity: self.capacity,
            hasher: HasherGen::Clone(hasher, H::clone),
        }
    }

    /// Sets the hasher for the underlying map. Similar to
    /// [`with_hasher`](crate::Builder::with_hasher), but instead of using a concrete hasher
    /// builder, the provided function will be called as many times as necessary to initialize
    /// the underlying map.
    ///
    /// # Safety
    ///
    /// See [`crate::with_hasher`](crate::with_hasher).
    pub unsafe fn with_hasher_generator<H>(self, gen: fn() -> H) -> Builder<H>
    where
        H: BuildHasher,
    {
        Builder {
            capacity: self.capacity,
            hasher: HasherGen::Generate(gen),
        }
    }

    /// Consumes the builder and returns a write handle and read handle to the map.
    ///
    /// # Examples
    ///
    /// ```
    /// # use flashmap::Builder;
    /// // Use type inference to determine the key and value types
    /// let (mut write, read) = Builder::new().build();
    ///
    /// write.guard().insert(10u32, 20u32);
    ///
    /// // Or specify them explicitly
    /// let (write, read) = Builder::new().build::<String, String>();
    /// ```
    pub fn build<K, V>(self) -> (WriteHandle<K, V, S>, ReadHandle<K, V, S>)
    where
        K: TrustedHashEq,
        S: BuildHasher,
    {
        unsafe { self.build_assert_trusted() }
    }

    /// Consumes the builder and returns a write handle and read handle to the map.
    ///
    /// # Safety
    ///
    /// The implementations of `Hash` and `Eq` for the key type **must** be deterministic. See
    /// [`TrustedHashEq`](crate::TrustedHashEq) for details.
    pub unsafe fn build_assert_trusted<K, V>(self) -> (WriteHandle<K, V, S>, ReadHandle<K, V, S>)
    where
        K: Hash + Eq,
        S: BuildHasher,
    {
        unsafe { Core::build_map(self.into_args()) }
    }

    pub(crate) fn into_args(self) -> BuilderArgs<S> {
        let (h1, h2) = self.hasher.generate();
        BuilderArgs {
            capacity: self.capacity,
            h1,
            h2,
        }
    }
}

#[derive(Clone, Copy)]
enum HasherGen<S> {
    Generate(fn() -> S),
    MakeBoth(fn() -> (S, S)),
    Clone(S, fn(&S) -> S),
}

impl<S> HasherGen<S> {
    fn generate(self) -> (S, S) {
        match self {
            Self::Generate(gen) => (gen(), gen()),
            Self::MakeBoth(make_both) => make_both(),
            Self::Clone(hasher, clone) => (clone(&hasher), hasher),
        }
    }
}

pub(crate) struct BuilderArgs<S> {
    pub capacity: usize,
    pub h1: S,
    pub h2: S,
}

/// ```compile_fail
/// fn assert_send<T: Send>() {}
/// use flashmap::*;
/// assert_send::<Evicted<'_, (), ()>>();
/// ```
///
/// ```compile_fail
/// fn assert_send<T: Send>() {}
/// use flashmap::*;
/// assert_send::<Alias<std::cell::Cell<()>>>();
/// ```
#[allow(dead_code)]
struct NotSendTypes;

/// ```compile_fail
/// fn assert_sync<T: Sync>() {}
/// use flashmap::*;
/// assert_sync::<Evicted<'_, (), ()>>();
/// ```
///
/// ```compile_fail
/// fn assert_sync<T: Sync>() {}
/// use flashmap::*;
/// assert_sync::<Alias<std::sync::MutexGuard<'_, ()>>>();
/// ```
#[allow(dead_code)]
struct NotSyncTypes;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::hash_map::DefaultHasher, marker::PhantomData};

    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    #[derive(PartialEq, Eq, Hash)]
    struct SendOnly(PhantomData<*const u8>);

    unsafe impl Send for SendOnly {}

    #[derive(PartialEq, Eq, Hash)]
    struct SyncOnly(PhantomData<*const u8>);

    unsafe impl Sync for SyncOnly {}

    #[derive(PartialEq, Eq, Hash)]
    struct SendSync;

    impl BuildHasher for SendSync {
        type Hasher = DefaultHasher;

        fn build_hasher(&self) -> Self::Hasher {
            unimplemented!()
        }
    }

    #[test]
    fn send_types() {
        assert_send::<ReadHandle<SendSync, SendSync, SendSync>>();
        assert_send::<WriteHandle<SendSync, SendSync, SendSync>>();
        assert_send::<View<ReadGuard<'_, SendSync, SendSync, SendSync>>>();
        assert_send::<Leaked<SendOnly>>();
    }

    #[test]
    fn sync_types() {
        assert_sync::<ReadHandle<SendSync, SendSync, SendSync>>();
        assert_sync::<View<ReadGuard<'_, SendSync, SendSync, SendSync>>>();
        assert_sync::<Leaked<SyncOnly>>();
    }
}

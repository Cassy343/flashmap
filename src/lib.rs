use std::{
    fmt::{self, Debug, Formatter},
    hash::BuildHasher,
};

use aliasing::{MaybeAliased, ReadSafe};
use handle::Handle;
use hashbrown::hash_map::DefaultHashBuilder;

mod aliasing;
mod cache_padded;
mod handle;
mod loom;
pub mod read;
mod write;

pub use read::*;
pub use write::*;

pub type Map<K, V, S = DefaultHashBuilder> =
    hashbrown::HashMap<MaybeAliased<K, ReadSafe>, MaybeAliased<V>, S>;

pub fn new<K, V>() -> (WriteHandle<K, V>, ReadHandle<K, V>) {
    Options::new().build()
}

pub fn with_capacity<K, V>(capacity: usize) -> (WriteHandle<K, V>, ReadHandle<K, V>) {
    Options::new().with_capacity(capacity).build()
}

pub fn with_hasher<K, V, S>(hasher: S) -> (WriteHandle<K, V, S>, ReadHandle<K, V, S>)
where
    S: Clone + BuildHasher,
{
    Options::new().with_hasher(hasher).build()
}

#[derive(Clone, Copy)]
pub struct Options<S = DefaultHashBuilder> {
    capacity: usize,
    hasher: HasherGen<S>,
}

impl<S> Debug for Options<S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Options")
            .field("capacity", &self.capacity)
            .field("hasher", &std::any::type_name::<S>())
            .finish()
    }
}

impl Options<DefaultHashBuilder> {
    pub fn new() -> Self {
        Self {
            capacity: 0,
            hasher: HasherGen::MakeAndClone(|| {
                let hasher = DefaultHashBuilder::default();
                (hasher.clone(), hasher)
            }),
        }
    }
}

impl<S> Options<S> {
    pub fn with_capacity(self, capacity: usize) -> Self {
        Self {
            capacity,
            hasher: self.hasher,
        }
    }

    pub fn with_hasher<H>(self, hasher: H) -> Options<H>
    where
        H: Clone + BuildHasher,
    {
        Options {
            capacity: self.capacity,
            hasher: HasherGen::Clone(hasher, H::clone),
        }
    }

    pub fn with_hasher_fn<H>(self, make: fn() -> H) -> Options<H>
    where
        H: BuildHasher,
    {
        Options {
            capacity: self.capacity,
            hasher: HasherGen::Make(make),
        }
    }

    pub fn build<K, V>(self) -> (WriteHandle<K, V, S>, ReadHandle<K, V, S>) {
        Handle::new(self)
    }

    pub(crate) fn into_args(self) -> (usize, S, S) {
        let (h1, h2) = self.hasher.generate();
        (self.capacity, h1, h2)
    }
}

#[derive(Clone, Copy)]
enum HasherGen<S> {
    Make(fn() -> S),
    Clone(S, fn(&S) -> S),
    MakeAndClone(fn() -> (S, S)),
}

impl<S> HasherGen<S> {
    fn generate(self) -> (S, S) {
        match self {
            Self::Make(make) => (make(), make()),
            Self::Clone(hasher, clone) => (clone(&hasher), hasher),
            Self::MakeAndClone(make_and_clone) => make_and_clone(),
        }
    }
}

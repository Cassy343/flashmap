#![deny(rust_2018_idioms, unsafe_op_in_unsafe_fn)]
#![warn(missing_docs, missing_debug_implementations)]
#![doc = include_str!("../README.md")]

mod core;
mod read;
mod util;
mod write;

pub use read::*;
pub(crate) use util::loom;
pub use util::Alias;
pub use write::*;

use std::{
    collections::hash_map::RandomState,
    fmt::{self, Debug, Formatter},
    hash::{BuildHasher, Hash},
};

pub(crate) type Map<K, V, S = RandomState> = hashbrown::HashMap<Alias<K>, Alias<V>, S>;

pub fn new<K, V>() -> (WriteHandle<K, V>, ReadHandle<K, V>)
where
    K: Eq + Hash,
{
    Builder::new().build()
}

pub fn with_capacity<K, V>(capacity: usize) -> (WriteHandle<K, V>, ReadHandle<K, V>)
where
    K: Eq + Hash,
{
    Builder::new().with_capacity(capacity).build()
}

pub fn with_hasher<K, V, S>(hasher: S) -> (WriteHandle<K, V, S>, ReadHandle<K, V, S>)
where
    K: Eq + Hash,
    S: Clone + BuildHasher,
{
    Builder::new().with_hasher(hasher).build()
}

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
    pub fn new() -> Self {
        Self {
            capacity: 0,
            hasher: HasherGen::MakeAndClone(|| {
                let hasher = RandomState::default();
                (hasher.clone(), hasher)
            }),
        }
    }
}

impl<S> Builder<S> {
    pub fn with_capacity(self, capacity: usize) -> Self {
        Self {
            capacity,
            hasher: self.hasher,
        }
    }

    pub fn with_hasher<H>(self, hasher: H) -> Builder<H>
    where
        H: Clone + BuildHasher,
    {
        Builder {
            capacity: self.capacity,
            hasher: HasherGen::Clone(hasher, H::clone),
        }
    }

    pub fn with_hasher_fn<H>(self, make: fn() -> H) -> Builder<H>
    where
        H: BuildHasher,
    {
        Builder {
            capacity: self.capacity,
            hasher: HasherGen::Make(make),
        }
    }

    pub fn build<K, V>(self) -> (WriteHandle<K, V, S>, ReadHandle<K, V, S>)
    where
        K: Eq + Hash,
        S: BuildHasher,
    {
        core::Handle::new(self)
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

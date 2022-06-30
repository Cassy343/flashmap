use std::{
    borrow::Borrow,
    hash::{BuildHasher, Hash},
    ops::Deref,
    ptr::NonNull,
};

use hashbrown::hash_map::DefaultHashBuilder;

use crate::{
    aliasing::Alias,
    handle::{Handle, MapAccess, MapIndex, ReaderStatus, RefCount},
    loom::cell::UnsafeCell,
    loom::sync::Arc,
    Map,
};

pub struct ReadHandle<K, V, S = DefaultHashBuilder> {
    inner: Arc<Handle<K, V, S>>,
    map_access: MapAccess<K, V, S>,
    refcount: NonNull<RefCount>,
    refcount_key: usize,
}

unsafe impl<K, V, S> Send for ReadHandle<K, V, S> where Arc<Map<K, V, S>>: Send {}

impl<K, V, S> ReadHandle<K, V, S> {
    pub fn new(
        inner: Arc<Handle<K, V, S>>,
        map_access: MapAccess<K, V, S>,
        refcount: NonNull<RefCount>,
        refcount_key: usize,
    ) -> Self {
        Self {
            refcount,
            map_access,
            inner,
            refcount_key,
        }
    }

    #[inline]
    pub fn guard(&self) -> ReadGuard<'_, K, V, S> {
        let map_index = unsafe { self.refcount.as_ref().increment() };

        ReadGuard {
            handle: self,
            map: unsafe { self.map_access.get(map_index) },
            map_index,
        }
    }
}

impl<K, V, S> Clone for ReadHandle<K, V, S> {
    fn clone(&self) -> Self {
        Handle::new_reader(Arc::clone(&self.inner))
    }
}

impl<K, V, S> Drop for ReadHandle<K, V, S> {
    fn drop(&mut self) {
        unsafe {
            self.inner.release_refcount(self.refcount_key);
        }
    }
}

pub struct ReadGuard<'a, K, V, S> {
    handle: &'a ReadHandle<K, V, S>,
    map: &'a UnsafeCell<Map<K, V, S>>,
    map_index: MapIndex,
}

impl<'a, K, V, S: BuildHasher> ReadGuard<'a, K, V, S> {
    #[inline]
    pub fn contains_key<Q: ?Sized>(&self, key: &Q) -> bool
    where
        Alias<K>: Borrow<Q> + Eq + Hash,
        Q: Hash + Eq,
    {
        self.map.with(|ptr| unsafe { &*ptr }.contains_key(key))
    }

    #[inline]
    pub fn get<Q: ?Sized>(&self, key: &Q) -> Option<&V>
    where
        Alias<K>: Borrow<Q> + Eq + Hash,
        Q: Hash + Eq,
    {
        self.map
            .with(|ptr| unsafe { &*ptr }.get(key).map(|value| &**value))
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.map.with(|ptr| {
            unsafe { &*ptr }
                .iter()
                .map(|(key, value)| (&**key, &**value))
        })
    }

    #[inline]
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.map
            .with(|ptr| unsafe { &*ptr }.keys().map(Deref::deref))
    }

    #[inline]
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.map
            .with(|ptr| unsafe { &*ptr }.values().map(Deref::deref))
    }
}

impl<'a, K, V, S> Drop for ReadGuard<'a, K, V, S> {
    fn drop(&mut self) {
        let refcount = unsafe { self.handle.refcount.as_ref() };
        if refcount.decrement(self.map_index) == ReaderStatus::Residual {
            unsafe { self.handle.inner.release_residual() };
        }
    }
}

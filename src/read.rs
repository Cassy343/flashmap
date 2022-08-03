use crate::{
    core::{Core, MapIndex, RefCount, SharedMapAccess},
    loom::cell::UnsafeCell,
    loom::sync::Arc,
    util::unlikely,
    view::sealed::ReadAccess,
    Map, View,
};
use std::hash::{BuildHasher, Hash};
use std::{collections::hash_map::RandomState, ptr::NonNull};

/// A read handle for the map.
///
/// This type allows for the creation of [`ReadGuard`s](crate::ReadGuard), which provide immutable
/// access to the underlying data.
pub struct ReadHandle<K, V, S = RandomState>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    core: Arc<Core<K, V, S>>,
    map_access: SharedMapAccess<K, V, S>,
    refcount: NonNull<RefCount>,
    refcount_key: usize,
}

unsafe impl<K, V, S> Send for ReadHandle<K, V, S>
where
    K: Send + Sync + Eq + Hash,
    V: Send + Sync,
    S: Send + Sync + BuildHasher,
{
}
unsafe impl<K, V, S> Sync for ReadHandle<K, V, S>
where
    K: Send + Sync + Eq + Hash,
    V: Send + Sync,
    S: Send + Sync + BuildHasher,
{
}

impl<K, V, S> ReadHandle<K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    pub(crate) fn new(
        core: Arc<Core<K, V, S>>,
        map_access: SharedMapAccess<K, V, S>,
        refcount: NonNull<RefCount>,
        refcount_key: usize,
    ) -> Self {
        Self {
            refcount,
            map_access,
            core,
            refcount_key,
        }
    }

    /// Creates a new [`ReadGuard`](crate::ReadGuard) wrapped in a [`View`](crate::View), allowing
    /// safe access to the map.
    ///
    /// # Examples
    ///
    /// ```
    /// # use flashmap;
    /// let (write, read) = flashmap::new::<u32, u32>();
    ///
    /// let guard = read.guard();
    ///
    /// // The map should be empty since we added nothing to it.
    /// assert!(guard.is_empty());
    ///
    /// // Maybe do some more work with the guard
    ///
    /// // The guard is released when dropped (you don't have to drop it explicitly)
    /// drop(guard);
    /// ```
    ///
    /// In order to see the most recent updates from the writer, a new guard needs to be created:
    /// ```
    /// # use flashmap;
    /// let (mut write, read) = flashmap::new::<String, String>();
    ///
    /// let guard = read.guard();
    ///
    /// // This key is not in the map yet
    /// assert!(!guard.contains_key("ferris"));
    ///
    /// write.guard().insert("ferris".to_owned(), "crab".to_owned());
    ///
    /// // Since we're still using the same guard, the write isn't visible to us yet
    /// assert!(!guard.contains_key("ferris"));
    ///
    /// // Drop the old guard and get a new one
    /// drop(guard);
    /// let guard = read.guard();
    ///
    /// // The write is now visible
    /// assert_eq!(guard.get("ferris").unwrap(), "crab");
    /// ```
    #[inline]
    pub fn guard(&self) -> View<ReadGuard<'_, K, V, S>> {
        let map_index = unsafe { self.refcount.as_ref() }.increment();

        View::new(ReadGuard {
            handle: self,
            map: unsafe { self.map_access.get(map_index) },
            map_index,
        })
    }
}

impl<K, V, S> Clone for ReadHandle<K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    fn clone(&self) -> Self {
        Core::new_reader(Arc::clone(&self.core))
    }
}

impl<K, V, S> Drop for ReadHandle<K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    fn drop(&mut self) {
        unsafe { self.core.release_refcount(self.refcount_key) };
    }
}

/// Provides immutable access to the map, and prevents entries from being dropped.
///
/// This guard provides a snapshot view of the map at a particular point in time. A new guard must
/// be created in order to see updates from the writer. See
/// [`ReadHandle::guard`](crate::ReadHandle::guard) for examples. See [`View`](crate::View) for
/// additional examples and the public API to interact with the underlying map.
pub struct ReadGuard<'guard, K, V, S = RandomState>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    handle: &'guard ReadHandle<K, V, S>,
    map: &'guard UnsafeCell<Map<K, V, S>>,
    map_index: MapIndex,
}

unsafe impl<K, V, S> Send for ReadGuard<'_, K, V, S>
where
    K: Send + Sync + Eq + Hash,
    V: Send + Sync,
    S: Send + Sync + BuildHasher,
{
}
unsafe impl<K, V, S> Sync for ReadGuard<'_, K, V, S>
where
    K: Send + Sync + Eq + Hash,
    V: Send + Sync,
    S: Send + Sync + BuildHasher,
{
}

impl<'guard, K, V, S> ReadAccess for ReadGuard<'guard, K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    type Map = Map<K, V, S>;

    #[inline]
    fn with_map<'read, F, R>(&'read self, op: F) -> R
    where
        F: FnOnce(&'read Self::Map) -> R,
    {
        self.map.with(|ptr| op(unsafe { &*ptr }))
    }
}

impl<'guard, K, V, S> Drop for ReadGuard<'guard, K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    #[inline]
    fn drop(&mut self) {
        let current_reader_map = unsafe { self.handle.refcount.as_ref() }.decrement();

        if unlikely(current_reader_map != self.map_index) {
            unsafe { self.handle.core.release_residual() };
        }
    }
}

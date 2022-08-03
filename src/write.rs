use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hash},
    mem,
    num::NonZeroUsize,
    ops::Deref,
    sync::atomic::{AtomicUsize, Ordering},
};

use hashbrown::hash_map::RawEntryMut;

use crate::{
    core::Core,
    loom::cell::UnsafeCell,
    loom::sync::Arc,
    util::{Alias, BorrowHelper},
    view::sealed::ReadAccess,
    Map, View,
};

static NEXT_WRITER_UID: AtomicUsize = AtomicUsize::new(1);
const LEAKED_VALUE_MISMATCH: &str = "Leaked value is not from this map";

#[repr(transparent)]
#[derive(PartialEq, Eq, Clone, Copy)]
struct WriterUid(NonZeroUsize);

impl WriterUid {
    fn next() -> Self {
        Self(
            NonZeroUsize::new(NEXT_WRITER_UID.fetch_add(1, Ordering::Relaxed))
                .expect("Maximum number of maps exceeded for this architecture"),
        )
    }
}

/// A write handle to the underlying map.
///
/// This type allows for the creation of [`WriteGuard`s](crate::WriteGuard) which allow for
/// mutation of the underlying map.
pub struct WriteHandle<K, V, S = RandomState>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    core: Arc<Core<K, V, S>>,
    operations: UnsafeCell<Vec<Operation<K, V>>>,
    uid: WriterUid,
}

unsafe impl<K, V, S> Send for WriteHandle<K, V, S>
where
    K: Send + Sync + Hash + Eq,
    V: Send + Sync,
    S: Send + Sync + BuildHasher,
{
}

impl<K, V, S> WriteHandle<K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    pub(crate) unsafe fn new(core: Arc<Core<K, V, S>>) -> Self {
        Self {
            core,
            operations: UnsafeCell::new(Vec::new()),
            uid: WriterUid::next(),
        }
    }

    /// Blocks the calling thread until all readers see the same version of the map.
    ///
    /// If all readers already see the same version of the map (or if there are no active readers)
    /// then this function is a no-op.
    ///
    /// This function is meant for advanced use only. See
    /// `Leaked::`[`into_inner`](crate::Leaked::into_inner) for an example use-case.
    #[inline]
    pub fn synchronize(&self) {
        self.core.synchronize();
    }

    /// Creates a new [`WriteGuard`](crate::WriteGuard) wrapped in a [`View`](crate::View),
    /// allowing for safe read and write access to the map.
    ///
    /// # Examples
    ///
    /// ```
    /// # use flashmap;
    /// let (mut write, read) = flashmap::new::<String, String>();
    ///
    /// let mut guard = write.guard();
    ///
    /// // Insert a few values
    /// guard.insert("apple".to_owned(), "red".to_owned());
    /// guard.insert("banana".to_owned(), "yellow".to_owned());
    ///
    /// // Remove a value
    /// assert_eq!(&*guard.remove("apple".to_owned()).unwrap(), "red");
    ///
    /// // Publishing makes all previous changes visible to new readers. Dropping the
    /// // guard has the same effect.
    /// guard.publish();
    /// ```
    ///
    /// Unlike a read guard, when reading through a write guard, all changes will be immediately
    /// visible.
    /// ```
    /// # use flashmap;
    /// let (mut write, read) = flashmap::new::<String, String>();
    ///
    /// let mut guard = write.guard();
    ///
    /// // Our insert is immediately visible to us
    /// guard.insert("apple".to_owned(), "red".to_owned());
    /// assert_eq!(guard.get("apple").unwrap(), "red");
    /// assert!(!guard.contains_key("banana"));
    ///
    /// guard.insert("banana".to_owned(), "yellow".to_owned());
    /// assert_eq!(guard.get("banana").unwrap(), "yellow");
    ///
    /// // Likewise, removes (and all other operations) are immediately visible
    /// assert_eq!(&*guard.remove("apple".to_owned()).unwrap(), "red");
    /// assert!(!guard.contains_key("apple"));
    /// ```
    pub fn guard(&mut self) -> View<WriteGuard<'_, K, V, S>> {
        self.synchronize();
        unsafe { self.guard_unchecked() }
    }

    unsafe fn guard_unchecked(&mut self) -> View<WriteGuard<'_, K, V, S>> {
        let map = self.core.writer_map();
        map.with_mut(|map_ptr| {
            self.operations.with_mut(|ops_ptr| {
                let operations = unsafe { &mut *ops_ptr };
                unsafe { Self::flush_operations(operations, &mut *map_ptr) };
                operations.shrink_to(64);
            });
        });

        View::new(WriteGuard {
            map,
            handle: self,
            handle_uid: self.uid,
        })
    }

    /// Reclaims a leaked value, providing ownership of the underlying value.
    ///
    /// # Panics
    ///
    /// Panics if the leaked value provided came from a different map then the one this handle is
    /// associated with.
    ///
    /// # Examples
    ///
    /// ```
    /// use flashmap::{self, Evicted};
    ///
    /// let (mut write, read) = flashmap::new::<String, String>();
    ///
    /// write.guard().insert("ferris".to_owned(), "crab".to_owned());
    ///
    /// // ~~ stuff happens ~~
    ///
    /// let leaked = write.guard().remove("ferris".to_owned())
    ///     .map(Evicted::leak)
    ///     .unwrap();
    ///
    /// let value = write.reclaim_one(leaked);
    /// assert_eq!(value, "crab");
    /// ```
    #[inline]
    pub fn reclaim_one(&self, leaked: Leaked<V>) -> V {
        (self.reclaimer())(leaked)
    }

    /// Returns a function which can safely reclaim leaked values. This is useful for reclaiming
    /// multiple leaked values while only performign the necessary synchronization once.
    ///
    /// # Panics
    ///
    /// The **returned function** will panic if given a leaked value not from the map this handle
    /// is associated with. This function itself will not panic.
    ///
    /// # Examples
    ///
    /// ```
    /// use flashmap::{self, Evicted};
    ///
    /// let (mut write, read) = flashmap::new::<u32, String>();
    ///
    /// let mut guard = write.guard();
    /// guard.insert(0xFF0000, "red".to_owned());
    /// guard.insert(0x00FF00, "green".to_owned());
    /// guard.insert(0x0000FF, "blue".to_owned());
    /// guard.publish();
    ///
    /// // ~~ stuff happens ~~
    ///
    /// let mut guard = write.guard();
    /// let colors = [0xFF0000, 0x00FF00, 0x0000FF].map(|hex| {
    ///     guard.remove(hex).map(Evicted::leak).unwrap()
    /// });
    /// guard.publish();
    ///
    /// let [red, green, blue] = colors.map(write.reclaimer());
    ///
    /// assert_eq!(red, "red");
    /// assert_eq!(green, "green");
    /// assert_eq!(blue, "blue");
    /// ```
    #[inline]
    pub fn reclaimer(&self) -> impl Fn(Leaked<V>) -> V + '_ {
        self.synchronize();
        unsafe { self.reclaimer_unchecked() }
    }

    unsafe fn reclaimer_unchecked(&self) -> impl Fn(Leaked<V>) -> V + '_ {
        let uid = self.uid;
        move |leaked| {
            assert!(uid == leaked.handle_uid, "{LEAKED_VALUE_MISMATCH}");
            unsafe { Alias::into_owned(leaked.value) }
        }
    }

    #[cfg(feature = "async")]
    pub fn into_async(self) -> AsyncWriteHandle<K, V, S> {
        AsyncWriteHandle { inner: self }
    }

    #[inline]
    pub(crate) unsafe fn flush_operations(
        operations: &mut Vec<Operation<K, V>>,
        map: &mut Map<K, V, S>,
    ) {
        // We do unchecked ops in here since this function benches pretty hot when doing a lot
        // of writing

        for Operation {
            raw: mut operation,
            leaky,
        } in operations.drain(..)
        {
            match operation {
                RawOperation::InsertUnique(key, value) => {
                    map.insert_unique_unchecked(key, value);
                }
                RawOperation::Replace(ref key, value) => {
                    let slot =
                        unsafe { map.get_mut(BorrowHelper::new_ref(key)).unwrap_unchecked() };
                    if !leaky {
                        unsafe { Alias::drop(slot) };
                    }
                    *slot = value;
                }
                RawOperation::Remove(ref key) => {
                    let (mut k, mut v) = unsafe {
                        map.remove_entry(BorrowHelper::new_ref(key))
                            .unwrap_unchecked()
                    };
                    unsafe { Alias::drop(&mut k) };
                    if !leaky {
                        unsafe { Alias::drop(&mut v) };
                    }
                }
                RawOperation::Drop(ref mut value) => unsafe { Alias::drop(value) },
            }
        }
    }
}

impl<K, V, S> Drop for WriteHandle<K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    fn drop(&mut self) {
        let operations =
            mem::replace(&mut self.operations, UnsafeCell::new(Vec::new())).into_inner();
        unsafe { self.core.replay_on_drop(operations) };
    }
}

/// Provides mutable access to the underlying map, and publishes all changes to new readers when
/// dropped.
///
/// See [`WriteHandle::guard`](crate::WriteHandle::guard) for examples. See [`View`](crate::View)
/// for additional examples and the public API to interact with the underlying map.
pub struct WriteGuard<'guard, K: Eq + Hash, V, S: BuildHasher = RandomState> {
    map: &'guard UnsafeCell<Map<K, V, S>>,
    handle: &'guard WriteHandle<K, V, S>,
    handle_uid: WriterUid,
}

impl<'guard, K, V, S> ReadAccess for WriteGuard<'guard, K, V, S>
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
        self.map.with(|map_ptr| op(unsafe { &*map_ptr }))
    }
}

impl<'guard, K, V, S> WriteGuard<'guard, K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    #[inline]
    fn with_map_mut<'write, F, R>(&'write mut self, op: F) -> R
    where
        F: FnOnce(&'write mut Map<K, V, S>, &'write mut Vec<Operation<K, V>>) -> R,
    {
        self.map.with_mut(|map_ptr| {
            self.handle
                .operations
                .with_mut(|ops_ptr| unsafe { op(&mut *map_ptr, &mut *ops_ptr) })
        })
    }

    #[inline]
    pub(crate) fn insert<'ret>(&mut self, key: K, value: V) -> Option<Evicted<'ret, K, V>>
    where
        'guard: 'ret,
    {
        let value = Alias::new(value);

        let evicted = self.with_map_mut(|map, operations| {
            match map.raw_entry_mut().from_key(BorrowHelper::new_ref(&key)) {
                RawEntryMut::Vacant(entry) => {
                    let key = Alias::new(key);
                    entry.insert(unsafe { Alias::copy(&key) }, unsafe { Alias::copy(&value) });
                    operations.push(Operation::new(RawOperation::InsertUnique(key, value)));
                    None
                }
                RawEntryMut::Occupied(mut entry) => {
                    let old = mem::replace(entry.get_mut(), unsafe { Alias::copy(&value) });
                    operations.push(Operation::new(RawOperation::Replace(key, value)));
                    Some(old)
                }
            }
        });

        evicted.map(|alias| unsafe { Evicted::new(self, alias) })
    }

    #[inline]
    pub(crate) fn replace<'ret, F>(&mut self, key: K, op: F) -> Option<Evicted<'ret, K, V>>
    where
        F: FnOnce(&V) -> V,
        'guard: 'ret,
    {
        let evicted =
            self.with_map_mut(
                |map, operations| match map.get_mut(BorrowHelper::new_ref(&key)) {
                    Some(value) => {
                        let new_value = Alias::new(op(&**value));
                        operations.push(Operation::new(RawOperation::Replace(key, unsafe {
                            Alias::copy(&new_value)
                        })));
                        let old_value = mem::replace(value, new_value);
                        Some(old_value)
                    }
                    None => None,
                },
            );

        evicted.map(|value| unsafe { Evicted::new(self, value) })
    }

    #[inline]
    pub(crate) fn remove<'ret>(&mut self, key: K) -> Option<Evicted<'ret, K, V>>
    where
        'guard: 'ret,
    {
        let evicted = self.with_map_mut(|map, operations| {
            let removed = map.remove(BorrowHelper::new_ref(&key));

            if removed.is_some() {
                operations.push(Operation::new(RawOperation::Remove(key)));
            }

            removed
        });

        evicted.map(|value| unsafe { Evicted::new(self, value) })
    }

    #[inline]
    pub(crate) fn drop_lazily(&self, leaked: Leaked<V>) {
        assert!(
            self.handle_uid == leaked.handle_uid,
            "{LEAKED_VALUE_MISMATCH}"
        );
        self.handle.operations.with_mut(|ops_ptr| {
            unsafe { &mut *ops_ptr }.push(Operation::new(RawOperation::Drop(Leaked::into_inner(
                leaked,
            ))));
        });
    }

    #[inline]
    pub(crate) fn publish(self) {
        // publishing logic happens on drop
        drop(self);
    }
}

impl<'guard, K, V, S> Drop for WriteGuard<'guard, K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    fn drop(&mut self) {
        unsafe { self.handle.core.publish() };
    }
}

pub(crate) struct Operation<K, V> {
    raw: RawOperation<K, V>,
    leaky: bool,
}

impl<K, V> Operation<K, V> {
    #[inline]
    fn new(raw: RawOperation<K, V>) -> Self {
        Self { raw, leaky: false }
    }

    #[inline]
    fn make_leaky(&mut self) {
        self.leaky = true;
    }
}

enum RawOperation<K, V> {
    InsertUnique(Alias<K>, Alias<V>),
    Replace(K, Alias<V>),
    Remove(K),
    Drop(Alias<V>),
}

/// A value which was evicted from a map.
///
/// Due to the nature of concurrent data structures, memory often cannot be reclaimed the instant a
/// writer decides it no longer needs to be used. This goes for `flashmap` as well. When a value is
/// removed from the map, an `Evicted<'a, V>` is returned. This type only guarantees that the value
/// is valid for reads for the duration of `'a`, which will never outlive the guard which is
/// protecting the value. To use the evicted value after the associated guard is dropped, it must
/// be [`leak`](crate::Evicted::leak)ed, at which point the programmer is responsible for dropping
/// or claiming ownership of the value. If an evicted value is not leaked, then it will be dropped
/// at some unspecified point after (or while) the guard is dropped when it is safe to do so.
///
/// # Inspecting an evicted value
///
/// `Evicted` implements [`Deref`](std::ops::Deref), so you can get immutable access to the
/// underlying value.
///
/// ```
/// use flashmap::{self, Evicted};
///
/// let (mut write, read) = flashmap::new::<u32, u32>();
/// let mut guard = write.guard();
///
/// // Insert a key-value pair
/// guard.insert(0, 0);
///
/// // Evict the entry and its value
/// let removed: Evicted<'_, u32, u32> = guard.remove(0).unwrap();
///
/// // Inspect the evicted value by dereferencing it
/// assert_eq!(*removed, 0);
/// ```
///
/// # Leaking
///
/// To use an evicted value beyond the lifetime of the guard which provides it, you must leak the
/// value. This also means that you're responsible for manually dropping it. See
/// [`leak`](crate::Evicted::leak) and [`Leaked`](crate::Leaked) for more information.
pub struct Evicted<'a, K, V> {
    leaked: Leaked<V>,
    operations: &'a UnsafeCell<Vec<Operation<K, V>>>,
    operation: usize,
}

impl<'a, K, V> Evicted<'a, K, V> {
    #[inline]
    unsafe fn new<S>(guard: &WriteGuard<'a, K, V, S>, value: Alias<V>) -> Self
    where
        K: Eq + Hash,
        S: BuildHasher,
    {
        let operations = &guard.handle.operations;
        let operation = operations.with(|ops_ptr| unsafe { &*ops_ptr }.len() - 1);

        Self {
            leaked: Leaked {
                value,
                handle_uid: guard.handle_uid,
            },
            operations,
            operation,
        }
    }

    /// Leaks the contained value, extending its lifetime until it is manually converted into an
    /// owned value or dropped.
    ///
    /// The primary means for safely turning a leaked value into an owned value are through the
    /// [`reclaim_one`](crate::WriteHandle::reclaim_one) and
    /// [`reclaimer`](crate::WriteHandle::reclaimer) methods. For dropping a leaked value, you can
    /// use the [`drop_lazily`](crate::View::drop_lazily) method. For more advanced use, see the
    /// [`Leaked`](crate::Leaked) type and its associated [`into_inner`](crate::Leaked::into_inner)
    /// method.
    ///
    /// # Examples
    ///
    /// ```
    /// use flashmap::{self, Evicted, Leaked};
    ///
    /// let (mut write, read) = flashmap::new::<u32, String>();
    /// let mut guard = write.guard();
    ///
    /// // Insert a couple values
    /// guard.insert(1, "a".to_owned());
    /// guard.insert(2, "b".to_owned());
    ///
    /// // Evict those values
    /// let a = guard.remove(1).map(Evicted::leak).unwrap();
    /// let b = guard.remove(2).map(Evicted::leak).unwrap();
    ///
    /// guard.publish();
    ///
    /// // Reclaim one
    /// let a = write.reclaim_one(a);
    /// assert_eq!(a, "a");
    ///
    /// // Lazily drop another
    /// write.guard().drop_lazily(b);
    /// ```
    pub fn leak(evicted: Self) -> Leaked<V> {
        evicted
            .operations
            .with_mut(|ptr| unsafe { (*ptr).get_unchecked_mut(evicted.operation) }.make_leaky());

        evicted.leaked
    }
}

impl<K, V> Deref for Evicted<'_, K, V> {
    type Target = V;

    fn deref(&self) -> &Self::Target {
        &self.leaked
    }
}

/// A leaked value from the map.
///
/// Similar to [`Evicted`](crate::Evicted), this type implements [`Deref`](std::ops::Deref),
/// allowing for immutable access to the underlying value.
///
/// This type behaves similarly to [`ManuallyDrop`](std::mem::ManuallyDrop) in that the underlying
/// value is not dropped if the wrapper is dropped. See [`leak`](crate::Evicted::leak) for how to
/// safely drop or take ownership of a leaked value. See [`into_inner`](crate::Leaked::into_inner)
/// for details on how to unsafely take ownership of a leaked value.
#[must_use = "Not using a leaked value may cause a memory leak"]
pub struct Leaked<V> {
    value: Alias<V>,
    handle_uid: WriterUid,
}

unsafe impl<V> Send for Leaked<V> where V: Send {}
unsafe impl<V> Sync for Leaked<V> where V: Sync {}

impl<V> Leaked<V> {
    /// Consumes this leaked value, providing the inner aliased value. Note that the aliased value
    /// must be manually dropped via `Alias::`[`drop`](crate::Alias::drop), or converted into an
    /// owned value via `Alias::`[`into_owned`](crate::Alias::into_owned).
    ///
    /// # Examples
    ///
    /// ```
    /// use flashmap::{self, Alias, Evicted, Leaked};
    ///
    /// let (mut write, read) = flashmap::new::<u32, Box<u32>>();
    ///
    /// write.guard().insert(10, Box::new(20));
    ///
    /// // Remove and leak the previously inserted value
    /// let leaked: Leaked<Box<u32>> = write.guard()
    ///     .remove(10)
    ///     .map(Evicted::leak)
    ///     .unwrap();
    ///
    /// // Extract the inner aliased value
    /// let inner: Alias<Box<u32>> = Leaked::into_inner(leaked);
    ///
    /// // Wait until no more readers can access the aliased value
    /// write.synchronize();
    ///
    /// // Safety: we called `synchronize` on the write handle of the map the aliased
    /// // value came from, so we are guaranteed that we are the only ones accessing the
    /// // aliased value from this point forward.
    /// let value = unsafe { Alias::into_owned(inner) };
    ///
    /// assert_eq!(*value, 20);
    /// ```
    #[must_use = "Not using an aliased value may cause a memory leak"]
    pub fn into_inner(leaked: Self) -> Alias<V> {
        leaked.value
    }
}

impl<V> Deref for Leaked<V> {
    type Target = V;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

#[cfg(feature = "async")]
pub use async_handle::*;

#[cfg(feature = "async")]
mod async_handle {
    use super::*;
    use crate::Synchronize;

    pub struct AsyncWriteHandle<K, V, S = RandomState>
    where
        K: Hash + Eq,
        S: BuildHasher,
    {
        pub(super) inner: WriteHandle<K, V, S>,
    }

    impl<K, V, S> AsyncWriteHandle<K, V, S>
    where
        K: Hash + Eq,
        S: BuildHasher,
    {
        #[inline]
        pub fn synchronize(&self) -> Synchronize<'_> {
            self.inner.core.synchronize_fut()
        }

        #[inline]
        pub async fn guard(&mut self) -> View<WriteGuard<'_, K, V, S>> {
            self.synchronize().await;
            unsafe { self.inner.guard_unchecked() }
        }

        #[inline]
        pub async fn reclaim_one(&self, leaked: Leaked<V>) -> V {
            (self.reclaimer().await)(leaked)
        }

        #[inline]
        pub async fn reclaimer(&self) -> impl Fn(Leaked<V>) -> V + '_ {
            self.synchronize().await;
            unsafe { self.inner.reclaimer_unchecked() }
        }

        pub fn into_sync(self) -> WriteHandle<K, V, S> {
            self.inner
        }
    }
}

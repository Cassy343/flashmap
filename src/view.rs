use std::borrow::Borrow;
use std::hash::{BuildHasher, Hash};
use std::ops::Deref;

use crate::util::BorrowHelper;
use crate::{Evicted, Leaked, Map, WriteGuard};

pub(crate) mod sealed {
    pub trait ReadAccess {
        type Map;

        fn with_map<'read, F, R>(&'read self, op: F) -> R
        where
            F: FnOnce(&'read Self::Map) -> R;
    }
}

/// Wraps a guard and provides a view into the map based on that guard.
///
/// This type is the proxy through which all read and write operations are performed on the map.
pub struct View<G> {
    guard: G,
}

impl<G> View<G> {
    #[inline]
    pub(crate) fn new(guard: G) -> Self {
        Self { guard }
    }
}

impl<K, V, S, G> View<G>
where
    G: sealed::ReadAccess<Map = Map<K, V, S>>,
    S: BuildHasher,
{
    /// Returns whether or not the underlying map is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// # use flashmap;
    /// let (mut write, read) = flashmap::new::<u32, u32>();
    ///
    /// // The map is empty
    /// assert!(read.guard().is_empty());
    ///
    /// // Add something to the map
    /// write.guard().insert(10, 20);
    ///
    /// // The map is no longer empty
    /// assert!(!read.guard().is_empty());
    /// ```
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.guard.with_map(Map::is_empty)
    }

    /// Returns the length of the map.
    ///
    /// Note that this just returns the length of the snapshot this guard is viewing. This value
    /// should not be relied upon for program correctness.
    ///
    /// # Examples
    ///
    /// ```
    /// # use flashmap;
    /// let (mut write, read) = flashmap::new::<u32, u32>();
    ///
    /// // There are no entries in the map currently
    /// assert_eq!(read.guard().len(), 0);
    ///
    /// // Add two entires
    /// let mut write_guard = write.guard();
    /// write_guard.insert(1, 42);
    /// write_guard.insert(2, 43);
    /// write_guard.publish();
    ///
    /// // Now new read guards will see those entries
    /// assert_eq!(read.guard().len(), 2);
    /// ```
    #[inline]
    pub fn len(&self) -> usize {
        self.guard.with_map(Map::len)
    }

    /// Returns whether or not the map contains the given key.
    ///
    /// # Examples
    ///
    /// ```
    /// # use flashmap;
    /// let (mut write, read) = flashmap::new::<u32, u32>();
    ///
    /// write.guard().insert(0, 0);
    ///
    /// let guard = read.guard();
    /// assert!(guard.contains_key(&0));
    /// assert!(!guard.contains_key(&1));
    /// ```
    #[inline]
    pub fn contains_key<Q: ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q> + Eq + Hash,
        Q: Hash + Eq,
    {
        self.guard
            .with_map(|map| map.contains_key(BorrowHelper::new_ref(key)))
    }

    /// Returns a reference to the value corresponding to the key.
    ///
    /// # Examples
    ///
    /// ```
    /// # use flashmap;
    /// let (mut write, read) = flashmap::new::<String, String>();
    ///
    /// write.guard().insert("apples".to_owned(), "oranges".to_owned());
    ///
    /// let guard = read.guard();
    /// assert_eq!(guard.get("apples").unwrap(), "oranges");
    /// assert!(guard.get("bananas").is_none());
    /// ```
    #[inline]
    pub fn get<Q: ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q> + Eq + Hash,
        Q: Hash + Eq,
    {
        self.guard
            .with_map(|map| map.get(BorrowHelper::new_ref(key)).map(Deref::deref))
    }

    /// An iterator visiting all key-value pairs in arbitrary order.
    ///
    /// # Examples
    ///
    /// ```
    /// # use flashmap;
    /// let (mut write, read) = flashmap::new::<i8, i8>();
    ///
    /// let mut guard = write.guard();
    /// guard.insert(3, 5);
    /// guard.insert(5, 7);
    /// guard.insert(7, 11);
    /// guard.publish();
    ///
    /// let mut result = 0i8;
    /// for (&key, &value) in read.guard().iter() {
    ///     result += key * value;
    /// }
    ///
    /// // 3*5 + 5*7 + 7*11 == 127 == i8::MAX
    /// assert_eq!(result, i8::MAX);
    /// ```
    #[inline]
    pub fn iter<'read>(&'read self) -> impl Iterator<Item = (&K, &V)> + '_
    where
        (K, V): 'read,
    {
        self.guard
            .with_map(|map| map.iter().map(|(key, value)| (&**key, &**value)))
    }

    /// An iterator visiting all keys in arbitrary order.
    ///
    /// # Examples
    ///
    /// ```
    /// # use flashmap;
    /// let (mut write, read) = flashmap::new::<u32, String>();
    ///
    /// let mut guard = write.guard();
    /// guard.insert(1, "one".to_owned());
    /// guard.insert(10, "ten".to_owned());
    /// guard.insert(100, "one hundred".to_owned());
    /// guard.publish();
    ///
    /// let mut result = 0u32;
    /// for &key in read.guard().keys() {
    ///     result += key;
    /// }
    ///
    /// // 1 + 10 + 100 == 111
    /// assert_eq!(result, 111);
    /// ```
    #[inline]
    pub fn keys<'read>(&'read self) -> impl Iterator<Item = &K> + '_
    where
        (K, V): 'read,
    {
        self.guard.with_map(|map| map.keys().map(Deref::deref))
    }

    /// An iterator visiting all values in arbitrary order.
    ///
    /// # Examples
    ///
    /// ```
    /// # use flashmap;
    /// let (mut write, read) = flashmap::new::<String, u32>();
    ///
    /// let mut guard = write.guard();
    /// guard.insert("one".to_owned(), 1);
    /// guard.insert("ten".to_owned(), 10);
    /// guard.insert("one hundred".to_owned(), 100);
    /// guard.publish();
    ///
    /// let mut result = 0u32;
    /// for &key in read.guard().values() {
    ///     result += key;
    /// }
    ///
    /// // 1 + 10 + 100 == 111
    /// assert_eq!(result, 111);
    /// ```
    #[inline]
    pub fn values<'read>(&'read self) -> impl Iterator<Item = &V> + '_
    where
        (K, V): 'read,
    {
        self.guard.with_map(|map| map.values().map(Deref::deref))
    }
}

// TODO: It would probably be nicer if the write functionality got abstracted out into traits, but
// that is a massive headache I don't want to deal with, so we're doing this for now.
impl<'guard, K, V, S> View<WriteGuard<'guard, K, V, S>>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    /// Inserts a key-value pair into the map.
    ///
    /// If the map did not have this key present, then `None` is returned. If it did, then the
    /// evicted value is returned. See [`Evicted`](crate::Evicted) for details.
    ///
    /// # Examples
    ///
    /// ```
    /// # use flashmap;
    /// let (mut write, read) = flashmap::new::<u32, String>();
    /// let mut guard = write.guard();
    ///
    /// assert!(guard.insert(17, "seven teen".to_owned()).is_none());
    /// assert_eq!(&*guard.insert(17, "seventeen".to_owned()).unwrap(), "seven teen");
    /// ```
    #[inline]
    pub fn insert<'ret>(&mut self, key: K, value: V) -> Option<Evicted<'ret, K, V>>
    where
        'guard: 'ret,
    {
        self.guard.insert(key, value)
    }

    /// Replaces the value associated with the given key according to the provided function.
    ///
    /// If the key is not present, then the function is not called, and `None` is returned. If the
    /// key is present, then the function is called with the current value provided as the argument,
    /// the value in the map is replaced, and the evicted value is returned. See
    /// [`Evicted`](crate::Evicted) for details.
    ///
    /// # Examples
    ///
    /// ```
    /// # use flashmap;
    /// let (mut write, read) = flashmap::new::<u32, String>();
    /// let mut guard = write.guard();
    ///
    /// guard.insert(1, "a".to_owned());
    ///
    /// // The key 0 is not in the map, so nothing changes
    /// assert!(guard.replace(0, |_| String::new()).is_none());
    /// assert_eq!(guard.get(&1).unwrap(), "a");
    ///
    /// // The key 1 is in the map, so the closure gets called
    /// let evicted = guard.replace(1, |old| {
    ///     assert_eq!(old, "a");
    ///     "b".to_owned()
    /// }).unwrap();
    ///
    /// // We evicted the old value "a"
    /// assert_eq!(&*evicted, "a");
    ///
    /// // And now "b" is in the map
    /// assert_eq!(guard.get(&1).unwrap(), "b");
    /// ```
    #[inline]
    pub fn replace<'ret, F>(&mut self, key: K, op: F) -> Option<Evicted<'ret, K, V>>
    where
        F: FnOnce(&V) -> V,
        'guard: 'ret,
    {
        self.guard.replace(key, op)
    }

    /// Removes a key from the map, returning the value at the key if the key was previously in the
    /// map. See [`Evicted`](crate::Evicted) for details on accessing the removed value.
    ///
    /// # Examples
    ///
    /// ```
    /// # use flashmap;
    /// let (mut write, read) = flashmap::new::<u32, String>();
    /// let mut guard = write.guard();
    ///
    /// guard.insert(0, "a".to_owned());
    ///
    /// assert_eq!(&*guard.remove(0).unwrap(), "a");
    /// assert!(guard.remove(0).is_none());
    /// assert!(guard.remove(1).is_none());
    /// ```
    #[inline]
    pub fn remove<'ret>(&mut self, key: K) -> Option<Evicted<'ret, K, V>>
    where
        'guard: 'ret,
    {
        self.guard.remove(key)
    }

    /// Takes ownership of a leaked value and drops the inner value when it is safe to do so.
    ///
    /// There are no guarantees regarding when the leaked value will be dropped. It is only
    /// guaranteed that it will eventually be dropped provided that no handles or guards
    /// associated with this map are leaked or forgotten.
    ///
    /// # Panics
    ///
    /// Panics if the provided leaked value came from a different map then the one this guard is
    /// associated with. Note that it is **not** required that this method is called with the same
    /// guard that created the leaked value.
    ///
    /// # Examples
    ///
    /// ```
    /// # use flashmap::{self, Evicted};
    /// let (mut write, read) = flashmap::new::<String, String>();
    ///
    /// write.guard().insert("ferris".to_owned(), "crab".to_owned());
    ///
    /// // ~~ stuff happens ~~
    ///
    /// let mut guard = write.guard();
    ///
    /// // Remove the value and leak it
    /// let leaked = guard.remove("ferris".to_owned())
    ///     .map(Evicted::leak)
    ///     .unwrap();
    /// assert_eq!(&*leaked, "crab");
    ///
    /// // We decide we don't need to keep the leaked value around, so we tell the guard
    /// // to drop it when it is safe to do so
    /// guard.drop_lazily(leaked);
    ///
    /// guard.publish();
    /// ```
    #[inline]
    pub fn drop_lazily(&self, leaked: Leaked<V>) {
        self.guard.drop_lazily(leaked)
    }

    /// Consumes this view and its guard, publishing all previous changes to the map.
    ///
    /// This has the same effect as dropping the view. Note that the changes will only be visible
    /// through newly created read or write guards.
    #[inline]
    pub fn publish(self) {
        self.guard.publish()
    }
}

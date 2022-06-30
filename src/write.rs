use std::hash::{BuildHasher, Hash};

use hashbrown::hash_map::{DefaultHashBuilder, RawEntryMut};

use crate::{aliasing::Alias, handle::Handle, loom::cell::UnsafeCell, loom::sync::Arc, Map};

pub struct WriteHandle<K, V, S = DefaultHashBuilder>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    inner: Arc<Handle<K, V, S>>,
    operations: Vec<Operation<K, V>>,
}

unsafe impl<K, V, S> Send for WriteHandle<K, V, S>
where
    Arc<Map<K, V, S>>: Send,
    K: Eq + Hash,
    S: BuildHasher,
{
}

impl<K, V, S> WriteHandle<K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    pub fn new(inner: Arc<Handle<K, V, S>>) -> Self {
        Self {
            inner,
            operations: Vec::new(),
        }
    }

    #[inline]
    pub fn guard(&mut self) -> WriteGuard<'_, K, V, S> {
        let map = unsafe { self.inner.start_write() };
        map.with_mut(|ptr| unsafe { self.flush_operations(&mut *ptr) });

        WriteGuard { handle: self, map }
    }

    #[inline]
    unsafe fn flush_operations(&mut self, map: &mut Map<K, V, S>) {
        for operation in self.operations.drain(..) {
            match operation {
                Operation::InsertUnique(key, value) => {
                    map.insert_unique_unchecked(key, value);
                }
                Operation::Replace(key, value) => {
                    let slot = map.get_mut(&key).unwrap_unchecked();
                    Alias::drop(slot);
                    *slot = value;
                }
                Operation::Remove(key) => {
                    Alias::drop(&mut map.remove(&key).unwrap_unchecked());
                }
            }
        }

        self.operations.shrink_to(64);
    }

    #[inline]
    unsafe fn drop_operations(&mut self, map: &mut Map<K, V, S>) {
        for operation in self.operations.drain(..) {
            match operation {
                Operation::InsertUnique(_key, _value) => (),
                Operation::Replace(key, _value) => {
                    Alias::drop(map.get_mut(&key).unwrap_unchecked());
                }
                Operation::Remove(key) => {
                    Alias::drop(map.get_mut(&key).unwrap_unchecked());
                }
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
        let map = unsafe { self.inner.start_write() };
        map.with_mut(|ptr| unsafe { self.drop_operations(&mut *ptr) });
    }
}

// TODO: remove this code smell
pub struct WriteGuard<'a, K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    handle: &'a mut WriteHandle<K, V, S>,
    map: &'a UnsafeCell<Map<K, V, S>>,
}

impl<'a, K, V, S> WriteGuard<'a, K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    #[inline]
    pub fn insert(&mut self, key: K, value: V) -> InsertionResult {
        let value = Alias::new(value);

        self.map.with_mut(|ptr| unsafe {
            match (&mut *ptr).raw_entry_mut().from_key(&key) {
                RawEntryMut::Vacant(entry) => {
                    let key = Alias::new(key);
                    entry.insert(Alias::copy(&key), Alias::copy(&value));
                    self.handle
                        .operations
                        .push(Operation::InsertUnique(key, value));
                    InsertionResult::Inserted
                }
                RawEntryMut::Occupied(mut entry) => {
                    *entry.get_mut() = value;
                    InsertionResult::ReplacedOccupier
                }
            }
        })
    }

    #[inline]
    pub fn rcu<F>(&mut self, key: K, rcu: F) -> bool
    where
        F: FnOnce(&V) -> V,
    {
        self.map
            .with_mut(|ptr| match unsafe { &mut *ptr }.get_mut(&key) {
                Some(value) => {
                    let new_value = Alias::new(rcu(&**value));
                    self.handle
                        .operations
                        .push(Operation::Replace(key, unsafe { Alias::copy(&new_value) }));
                    *value = new_value;
                    true
                }
                None => false,
            })
    }

    #[inline]
    pub fn remove(&mut self, key: K) -> RemovalResult {
        let removed = self
            .map
            .with_mut(|ptr| unsafe { &mut *ptr }.remove(&key))
            .is_some();

        if removed {
            self.handle.operations.push(Operation::Remove(key));
            RemovalResult::Removed
        } else {
            RemovalResult::NotFound
        }
    }
}

impl<'a, K, V, S> Drop for WriteGuard<'a, K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    fn drop(&mut self) {
        unsafe { self.handle.inner.finish_write() };
    }
}

enum Operation<K, V> {
    InsertUnique(Alias<K>, Alias<V>),
    Replace(K, Alias<V>),
    Remove(K),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum InsertionResult {
    Inserted,
    ReplacedOccupier,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RemovalResult {
    Removed,
    NotFound,
}

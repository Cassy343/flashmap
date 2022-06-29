use std::{
    hash::{BuildHasher, Hash},
    mem,
};

use hashbrown::hash_map::DefaultHashBuilder;

use crate::{
    aliasing::{MaybeAliased, ReadSafe},
    handle::Handle,
    loom::cell::UnsafeCell,
    loom::sync::Arc,
    Map,
};

pub struct WriteHandle<K, V, S = DefaultHashBuilder> {
    inner: Arc<Handle<K, V, S>>,
    operations: Vec<Operation<K, V>>,
}

unsafe impl<K, V, S> Send for WriteHandle<K, V, S> where Arc<Map<K, V, S>>: Send {}

impl<K, V, S> WriteHandle<K, V, S> {
    pub fn new(inner: Arc<Handle<K, V, S>>) -> Self {
        Self {
            inner,
            operations: Vec::new(),
        }
    }
}

impl<K, V, S> WriteHandle<K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    #[inline]
    pub fn guard(&mut self) -> WriteGuard<'_, K, V, S> {
        let map = unsafe { self.inner.start_write() };
        map.with_mut(|ptr| self.flush_operations(unsafe { &mut *ptr }));

        WriteGuard { handle: self, map }
    }

    #[inline]
    fn flush_operations(&mut self, map: &mut Map<K, V, S>) {
        for operation in self.operations.drain(..) {
            match operation {
                Operation::Insert(key, value) => unsafe {
                    *map.get_mut(&key).unwrap_unchecked() = value;
                },
                Operation::InsertUnique(key, value) => {
                    map.insert_unique_unchecked(key, value);
                }
                Operation::Replace(key, value) => unsafe {
                    *map.get_mut(&key).unwrap_unchecked() = value;
                },
                Operation::Remove(key) => unsafe {
                    map.remove(&key).unwrap_unchecked();
                },
            }
        }

        self.operations.shrink_to(64);
    }
}

// TODO: remove this code smell
pub struct WriteGuard<'a, K, V, S> {
    handle: &'a mut WriteHandle<K, V, S>,
    map: &'a UnsafeCell<Map<K, V, S>>,
}

impl<'a, K, V, S> WriteGuard<'a, K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    #[inline]
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let key = unsafe { MaybeAliased::new_read_safe(key) };
        let value = MaybeAliased::new(value);

        let res = self
            .map
            .with_mut(|ptr| unsafe { (&mut *ptr).insert(key.alias(), value.alias()) });

        if res.is_some() {
            self.handle.operations.push(Operation::Insert(key, value));
        } else {
            self.handle
                .operations
                .push(Operation::InsertUnique(key, value))
        }

        res.map(|value| unsafe { value.into_owned() })
    }

    #[inline]
    pub fn rcu<F>(&mut self, key: K, rcu: F) -> Option<V>
    where
        F: FnOnce(&V) -> V,
    {
        self.map.with_mut(|ptr| {
            match unsafe {
                (&mut *ptr).get_mut(MaybeAliased::<_, ReadSafe>::new_ref_read_safe(&key))
            } {
                Some(value) => {
                    let new_value = MaybeAliased::new(rcu(unsafe { value.get() }));
                    self.handle
                        .operations
                        .push(Operation::Replace(key, unsafe { new_value.alias() }));
                    Some(unsafe { mem::replace(value, new_value).into_owned() })
                }
                None => None,
            }
        })
    }

    #[inline]
    pub fn remove(&mut self, key: K) -> Option<V> {
        let res = self
            .map
            .with_mut(|ptr| unsafe { (&mut *ptr).remove(MaybeAliased::new_ref_read_safe(&key)) });

        if res.is_some() {
            self.handle.operations.push(Operation::Remove(key));
        }

        res.map(|value| unsafe { value.into_owned() })
    }
}

impl<'a, K, V, S> Drop for WriteGuard<'a, K, V, S> {
    fn drop(&mut self) {
        unsafe { self.handle.inner.finish_write() };
    }
}

enum Operation<K, V> {
    Insert(MaybeAliased<K, ReadSafe>, MaybeAliased<V>),
    InsertUnique(MaybeAliased<K, ReadSafe>, MaybeAliased<V>),
    Replace(K, MaybeAliased<V>),
    Remove(K),
}

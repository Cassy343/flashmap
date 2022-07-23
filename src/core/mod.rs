mod refcount;
mod store;

pub use refcount::*;
pub use store::*;

use hashbrown::hash_map::DefaultHashBuilder;
use slab::Slab;

use crate::{
    loom::{
        cell::{Cell, UnsafeCell},
        sync::{
            atomic::{fence, AtomicIsize, Ordering},
            Arc, Mutex,
        },
        thread::{self, Thread},
    },
    util::{likely, lock, Alias},
};
use crate::{util::CachePadded, Map, ReadHandle, WriteHandle};
use crate::{Builder, BuilderArgs};
use std::hash::{BuildHasher, Hash};
use std::marker::PhantomData;
use std::process::abort;
use std::ptr::{self, NonNull};

pub struct Core<K, V, S = DefaultHashBuilder> {
    residual: AtomicIsize,
    // All readers need to be dropped before we're dropped, so we don't need to worry about
    // freeing any refcounts.
    refcounts: Mutex<Slab<NonNull<RefCount>>>,
    writer_thread: UnsafeCell<Option<Thread>>,
    writer_map: Cell<MapIndex>,
    maps: OwnedMapAccess<K, V, S>,
    // TODO: figure out if core can implement send or sync
    _not_send_sync: PhantomData<*const u8>,
}

impl<K, V, S> Core<K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    pub unsafe fn build_map(options: Builder<S>) -> (WriteHandle<K, V, S>, ReadHandle<K, V, S>) {
        let BuilderArgs { capacity, h1, h2 } = options.into_args();

        let maps = Box::new([
            CachePadded::new(UnsafeCell::new(Map::with_capacity_and_hasher(capacity, h1))),
            CachePadded::new(UnsafeCell::new(Map::with_capacity_and_hasher(capacity, h2))),
        ]);

        #[cfg(not(miri))]
        let init_refcount_capacity = num_cpus::get();

        #[cfg(miri)]
        let init_refcount_capacity = 1;

        let me = Arc::new(Self {
            residual: AtomicIsize::new(0),
            refcounts: Mutex::new(Slab::with_capacity(init_refcount_capacity)),
            writer_thread: UnsafeCell::new(None),
            writer_map: Cell::new(MapIndex::Second),
            maps: OwnedMapAccess::new(maps),
            _not_send_sync: PhantomData,
        });

        let write_handle = WriteHandle::new(Arc::clone(&me));
        let read_handle = Self::new_reader(me);

        (write_handle, read_handle)
    }
}

impl<K, V, S> Core<K, V, S> {
    pub fn new_reader(me: Arc<Self>) -> ReadHandle<K, V, S> {
        let mut guard = lock(&me.refcounts);
        let refcount = RefCount::new(me.writer_map.get().other());
        let refcount = NonNull::new(Box::into_raw(Box::new(refcount))).unwrap();
        let key = guard.insert(refcount);
        drop(guard);

        let map_access = me.maps.share();
        ReadHandle::new(me, map_access, refcount, key)
    }

    pub unsafe fn release_refcount(&self, key: usize) {
        let refcount = lock(&self.refcounts).remove(key);

        drop(unsafe { Box::from_raw(refcount.as_ptr()) });
    }

    #[inline]
    pub unsafe fn release_residual(&self) {
        let last_residual = self.residual.fetch_sub(1, Ordering::AcqRel);

        // If we were not the last residual reader, we do nothing.
        if last_residual != isize::MIN + 1 {
            return;
        }

        self.residual.store(0, Ordering::Release);

        // Since we were the last reader, and the writer was waiting on us, it's our job to wake it
        // up.
        self.writer_thread.with(|ptr| match unsafe { &*ptr } {
            Some(thread) => thread.unpark(),
            // This branch is entirely unreachable (assuming this library is coded correctly),
            // however I'd like to keep the additional code around reading as small as possible,
            // so in release mode we currently do nothing on this branch.
            None => {
                #[cfg(debug_assertions)]
                {
                    unreachable!("WAITING_ON_READERS state observed when writer_thread is None");
                }

                #[cfg(not(debug_assertions))]
                {
                    crate::util::cold();
                }
            }
        });
    }

    #[inline]
    pub fn synchronize(&self) {
        let residual = self.residual.load(Ordering::Acquire);

        if residual != 0 {
            let current = Some(thread::current());
            let old = self
                .writer_thread
                .with_mut(|ptr| unsafe { ptr::replace(ptr, current) });
            drop(old);

            let latest_residual = self.residual.fetch_add(isize::MIN, Ordering::AcqRel);

            if likely(latest_residual != 0) {
                loop {
                    // Wait for the next writable map to become available
                    thread::park();

                    let residual = self.residual.load(Ordering::Acquire);
                    if likely(residual == 0) {
                        break;
                    } else {
                        debug_assert!(residual < 0);
                    }
                }
            } else {
                self.residual.store(0, Ordering::Release);
            }
        }
    }

    #[inline]
    pub fn writer_map(&self) -> &UnsafeCell<Map<K, V, S>> {
        self.maps.get(self.writer_map.get())
    }

    #[inline]
    pub unsafe fn finish_write(&self) {
        debug_assert_eq!(self.residual.load(Ordering::Relaxed), 0);

        let guard = lock(&self.refcounts);

        // This needs to be within the mutex
        self.writer_map.set(self.writer_map.get().other());

        fence(Ordering::Release);

        let mut initial_residual = 0isize;

        // Clippy doesn't like that we're iterating over something in a mutex apparently
        #[allow(clippy::significant_drop_in_scrutinee)]
        for (_, refcount) in guard.iter() {
            let refcount = unsafe { refcount.as_ref() };

            // Because the highest bit is used in the refcount, this cast will not be lossy
            initial_residual += refcount.swap_maps() as isize;

            // If we overflowed, then abort.
            if initial_residual < 0 {
                abort();
            }
        }

        fence(Ordering::Acquire);

        drop(guard);

        self.residual.fetch_add(initial_residual, Ordering::AcqRel);
    }
}

impl<K, V, S> Drop for Core<K, V, S> {
    fn drop(&mut self) {
        let reader_map_index = self.writer_map.get().other();
        self.maps.get(reader_map_index).with_mut(|ptr| unsafe {
            (*ptr).drain().for_each(|(ref mut key, ref mut value)| {
                Alias::drop(key);
                Alias::drop(value);
            });
        });
    }
}

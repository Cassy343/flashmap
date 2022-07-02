use hashbrown::hash_map::DefaultHashBuilder;
use slab::Slab;

use crate::Builder;
use crate::{
    core::MapIndex,
    loom::{
        cell::{Cell, UnsafeCell},
        sync::{
            atomic::{fence, AtomicU8, AtomicUsize, Ordering},
            Arc, Mutex,
        },
        thread::{self, Thread},
    },
    util::Alias,
};
use crate::{util::CachePadded, Map, ReadHandle, WriteHandle};
use std::hash::{BuildHasher, Hash};
use std::ptr::{self, NonNull};
use std::process::abort;

use super::{OwnedMapAccess, RefCount};

const WRITABLE: u8 = 0;
const NOT_WRITABLE: u8 = 1;
const WAITING_ON_READERS: u8 = 2;

pub struct Handle<K, V, S = DefaultHashBuilder> {
    residual: AtomicUsize,
    // All readers need to be dropped before we're dropped, so we don't need to worry about
    // freeing any refcounts.
    refcounts: Mutex<Slab<NonNull<RefCount>>>,
    writer_thread: UnsafeCell<Option<Thread>>,
    writer_state: AtomicU8,
    writer_map: Cell<MapIndex>,
    maps: OwnedMapAccess<K, V, S>,
}

impl<K, V, S> Handle<K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    pub fn new(options: Builder<S>) -> (WriteHandle<K, V, S>, ReadHandle<K, V, S>) {
        let (capacity, h1, h2) = options.into_args();

        let maps = Box::new([
            CachePadded::new(UnsafeCell::new(Map::with_capacity_and_hasher(capacity, h1))),
            CachePadded::new(UnsafeCell::new(Map::with_capacity_and_hasher(capacity, h2))),
        ]);

        #[cfg(not(miri))]
        let init_refcount_capacity = num_cpus::get();

        #[cfg(miri)]
        let init_refcount_capacity = 0;

        let me = Arc::new(Self {
            residual: AtomicUsize::new(0),
            refcounts: Mutex::new(Slab::with_capacity(init_refcount_capacity)),
            writer_thread: UnsafeCell::new(None),
            writer_state: AtomicU8::new(WRITABLE),
            writer_map: Cell::new(MapIndex::Second),
            maps: OwnedMapAccess::new(maps),
        });

        let write_handle = WriteHandle::new(Arc::clone(&me));
        let read_handle = Self::new_reader(me);

        (write_handle, read_handle)
    }
}

impl<K, V, S> Handle<K, V, S> {
    #[inline]
    pub fn new_reader(me: Arc<Self>) -> ReadHandle<K, V, S> {
        let mut guard = me.refcounts.lock().unwrap();
        let refcount = RefCount::new(me.writer_map.get().other());
        let refcount = NonNull::new(Box::into_raw(Box::new(refcount))).unwrap();
        let key = guard.insert(refcount);
        drop(guard);

        let map_access = me.maps.share();
        ReadHandle::new(me, map_access, refcount, key)
    }

    #[inline]
    pub fn start_read(refcount: &RefCount) -> MapIndex {
        refcount.increment()
    }

    #[inline]
    pub fn finish_read(refcount: &RefCount, map_index: MapIndex) -> ReaderStatus {
        if refcount.decrement() == map_index {
            ReaderStatus::Normal
        } else {
            ReaderStatus::Residual
        }
    }

    #[inline]
    pub unsafe fn release_refcount(&self, key: usize) {
        drop(Box::from_raw(
            self.refcounts.lock().unwrap().remove(key).as_ptr(),
        ));
    }

    #[inline]
    pub unsafe fn release_residual(&self) {
        // TODO: why does loom fail if either of these are anything weaker than AcqRel?

        if self.residual.fetch_sub(1, Ordering::AcqRel) == 1 {
            if self.writer_state.swap(WRITABLE, Ordering::AcqRel) == WAITING_ON_READERS {
                self.writer_thread.with(|ptr| {
                    (*ptr).as_ref().map(Thread::unpark);
                });
            }
        }
    }

    // TODO: remove this code smell
    #[inline]
    pub unsafe fn start_write<'w>(&self) -> &'w UnsafeCell<Map<K, V, S>> {
        match self.writer_state.load(Ordering::Acquire) {
            WRITABLE => (),
            NOT_WRITABLE => {
                self.writer_thread
                    .with_mut(|ptr| drop(ptr::replace(ptr, Some(thread::current()))));

                let exchange_result = self.writer_state.compare_exchange(
                    NOT_WRITABLE,
                    WAITING_ON_READERS,
                    Ordering::Release,
                    Ordering::Relaxed,
                );
                
                debug_assert!(matches!(exchange_result, Ok(NOT_WRITABLE) | Err(WRITABLE)));
            }
            WAITING_ON_READERS => {
                #[cfg(debug_assertions)]
                {
                    panic!("Concurrent calls to start_write")
                }

                // This branch could only ever be taken if our internal implementation is wrong,
                // so no need to keep the debug info around in release builds
                #[cfg(not(debug_assertions))]
                {
                    abort()
                }
            },
            _ => {
                // We never store any other value in this atomic, so this branch *really* should
                // not be reachable
                abort();
            },
        };

        // Wait for the current write map to become available
        while self.writer_state.load(Ordering::Acquire) != WRITABLE {
            thread::park();
        }

        &*(self.maps.get(self.writer_map.get()) as *const _)
    }

    #[inline]
    pub unsafe fn finish_write(&self) {
        debug_assert_eq!(self.residual.load(Ordering::Relaxed), 0);

        self.writer_state.store(NOT_WRITABLE, Ordering::Relaxed);

        fence(Ordering::Release);

        // Acquire
        let guard = self.refcounts.lock().unwrap();

        let residual = guard
            .iter()
            .map(|(_, refcount)| refcount.as_ref().swap_maps())
            .sum::<usize>();

        // This needs to be within the mutex
        self.writer_map.set(self.writer_map.get().other());

        // Release
        drop(guard);

        fence(Ordering::Acquire);

        let residual = residual.wrapping_add(self.residual.fetch_add(residual, Ordering::AcqRel));
        if residual == 0 {
            self.writer_state.store(WRITABLE, Ordering::Release);
        }
    }
}

impl<K, V, S> Drop for Handle<K, V, S> {
    fn drop(&mut self) {
        let reader_map_index = self.writer_map.get().other();
        self.maps.get(reader_map_index).with_mut(|ptr| unsafe {
            (&mut *ptr)
                .drain()
                .for_each(|(ref mut key, ref mut value)| {
                    Alias::drop(key);
                    Alias::drop(value);
                });
        });
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReaderStatus {
    Normal,
    Residual,
}

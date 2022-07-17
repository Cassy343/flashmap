use hashbrown::hash_map::DefaultHashBuilder;
use slab::Slab;

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
use crate::{Builder, BuilderArgs};
use std::hash::{BuildHasher, Hash};
use std::marker::PhantomData;
use std::ptr::{self, NonNull};

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
    _not_send_sync: PhantomData<*const u8>,
}

impl<K, V, S> Handle<K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    pub fn new(options: Builder<S>) -> (WriteHandle<K, V, S>, ReadHandle<K, V, S>) {
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
            residual: AtomicUsize::new(0),
            refcounts: Mutex::new(Slab::with_capacity(init_refcount_capacity)),
            writer_thread: UnsafeCell::new(None),
            writer_state: AtomicU8::new(WRITABLE),
            writer_map: Cell::new(MapIndex::Second),
            maps: OwnedMapAccess::new(maps),
            _not_send_sync: PhantomData,
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
    pub unsafe fn release_refcount(&self, key: usize) {
        let refcount = self.refcounts.lock().unwrap().remove(key);

        drop(unsafe { Box::from_raw(refcount.as_ptr()) });
    }

    #[inline]
    pub unsafe fn release_residual(&self) {
        // TODO: why does loom fail if either of these are anything weaker than AcqRel?

        if self.residual.fetch_sub(1, Ordering::AcqRel) == 1 {
            if self.writer_state.swap(WRITABLE, Ordering::AcqRel) == WAITING_ON_READERS {
                self.writer_thread.with(|ptr| {
                    unsafe { &*ptr }.as_ref().map(Thread::unpark);
                });
            }
        }
    }

    #[inline]
    pub fn synchronize(&self) {
        let writer_state = self.writer_state.load(Ordering::Acquire);

        if writer_state == NOT_WRITABLE {
            self.writer_thread
                .with_mut(|ptr| drop(unsafe { ptr::replace(ptr, Some(thread::current())) }));

            let exchange_result = self.writer_state.compare_exchange(
                NOT_WRITABLE,
                WAITING_ON_READERS,
                Ordering::AcqRel,
                Ordering::Acquire,
            );

            if exchange_result == Ok(NOT_WRITABLE) {
                loop {
                    // Wait for the next writable map to become available
                    thread::park();

                    if self.writer_state.load(Ordering::Acquire) == WRITABLE {
                        break;
                    }
                }
            } else {
                debug_assert_eq!(exchange_result, Err(WRITABLE));
            }
        } else {
            debug_assert_eq!(writer_state, WRITABLE);
        }
    }

    #[inline]
    pub fn writer_map(&self) -> &UnsafeCell<Map<K, V, S>> {
        self.maps.get(self.writer_map.get())
    }

    #[inline]
    pub unsafe fn finish_write(&self) {
        debug_assert_eq!(self.residual.load(Ordering::Relaxed), 0);

        self.writer_state.store(NOT_WRITABLE, Ordering::Relaxed);

        fence(Ordering::Release);

        // Acquire
        let guard = self.refcounts.lock().unwrap();

        let initial_residual = guard
            .iter()
            .map(|(_, refcount)| unsafe { refcount.as_ref() }.swap_maps())
            .sum::<usize>();

        // This needs to be within the mutex
        self.writer_map.set(self.writer_map.get().other());

        // Release
        drop(guard);

        fence(Ordering::Acquire);

        let latest_residual = self.residual.fetch_add(initial_residual, Ordering::AcqRel);
        let residual = initial_residual.wrapping_add(latest_residual);
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

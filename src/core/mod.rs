mod refcount;
mod store;

pub use refcount::*;
pub use store::*;

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
    Operation,
};
use crate::{util::CachePadded, BuilderArgs, Map, ReadHandle, WriteHandle};
use std::marker::PhantomData;
use std::process::abort;
use std::ptr::NonNull;
use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hash},
};

#[cfg(feature = "async")]
use atomic_waker::AtomicWaker;
#[cfg(feature = "async")]
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
};

pub struct Core<K, V, S = RandomState>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    residual: AtomicIsize,
    // All readers need to be dropped before we're dropped, so we don't need to worry about
    // freeing any refcounts.
    refcounts: Mutex<Slab<NonNull<RefCount>>>,
    parker: UnsafeCell<Parker>,
    writer_map: Cell<MapIndex>,
    maps: OwnedMapAccess<K, V, S>,
    replay_on_drop: UnsafeCell<Vec<Operation<K, V>>>,
    _not_sync: PhantomData<*const u8>,
}

unsafe impl<K, V, S> Send for Core<K, V, S>
where
    K: Hash + Eq,
    Alias<K>: Send,
    Alias<V>: Send,
    S: BuildHasher + Send,
{
}

impl<K, V, S> Core<K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    pub(crate) unsafe fn build_map(
        args: BuilderArgs<S>,
    ) -> (WriteHandle<K, V, S>, ReadHandle<K, V, S>) {
        let BuilderArgs { capacity, h1, h2 } = args;

        let maps = Box::new([
            CachePadded::new(UnsafeCell::new(Map::with_capacity_and_hasher(capacity, h1))),
            CachePadded::new(UnsafeCell::new(Map::with_capacity_and_hasher(capacity, h2))),
        ]);

        let init_refcount_capacity = if cfg!(not(miri)) { num_cpus::get() } else { 1 };

        let me = Arc::new(Self {
            residual: AtomicIsize::new(0),
            refcounts: Mutex::new(Slab::with_capacity(init_refcount_capacity)),
            parker: UnsafeCell::new(Parker::new()),
            writer_map: Cell::new(MapIndex::Second),
            maps: OwnedMapAccess::new(maps),
            replay_on_drop: UnsafeCell::new(Vec::new()),
            _not_sync: PhantomData,
        });

        let write_handle = unsafe { WriteHandle::new(Arc::clone(&me)) };
        let read_handle = Self::new_reader(me);

        (write_handle, read_handle)
    }

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

        // If we were not the last residual reader, or the writer is not currently waiting for the
        // last reader, we do nothing.
        if last_residual != isize::MIN + 1 {
            return;
        }

        self.residual.store(0, Ordering::Release);

        // Since we were the last reader, and the writer was waiting on us, it's our job to wake it
        // up.
        self.parker.with(|ptr| unsafe { &*ptr }.unpark());
    }

    #[inline]
    pub fn synchronize(&self) {
        let residual = self.residual.load(Ordering::Acquire);

        if residual != 0 {
            let current = thread::current();
            self.parker
                .with_mut(|ptr| unsafe { &mut *ptr }.write_sync(current));

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

    #[cfg(feature = "async")]
    #[inline]
    pub fn synchronize_fut(&self) -> Synchronize<'_> {
        Synchronize::new(&self.residual, &self.parker)
    }

    #[inline]
    pub fn writer_map(&self) -> &UnsafeCell<Map<K, V, S>> {
        self.maps.get(self.writer_map.get())
    }

    #[inline]
    pub unsafe fn publish(&self) {
        debug_assert_eq!(self.residual.load(Ordering::Relaxed), 0);

        fence(Ordering::Release);

        let guard = lock(&self.refcounts);

        // This needs to be within the mutex
        self.writer_map.set(self.writer_map.get().other());

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

        drop(guard);

        self.residual.fetch_add(initial_residual, Ordering::Relaxed);

        fence(Ordering::Acquire);
    }

    pub(crate) unsafe fn replay_on_drop(&self, operations: Vec<Operation<K, V>>) {
        self.replay_on_drop
            .with_mut(|ptr| unsafe { *ptr = operations });
    }
}

impl<K, V, S> Drop for Core<K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    fn drop(&mut self) {
        let writer_map_index = self.writer_map.get();

        self.maps.get(writer_map_index).with_mut(|ptr| unsafe {
            WriteHandle::flush_operations(&mut *self.replay_on_drop.get_mut(), &mut *ptr)
        });

        let reader_map_index = writer_map_index.other();
        self.maps.get(reader_map_index).with_mut(|ptr| unsafe {
            (*ptr).drain().for_each(|(ref mut key, ref mut value)| {
                Alias::drop(key);
                Alias::drop(value);
            });
        });
    }
}

enum Parker {
    Sync(Thread),
    #[cfg(feature = "async")]
    Async(AtomicWaker),
    #[cfg(not(feature = "async"))]
    None,
}

impl Parker {
    fn new() -> Self {
        #[cfg(feature = "async")]
        {
            Self::Async(AtomicWaker::new())
        }

        #[cfg(not(feature = "async"))]
        {
            Self::None
        }
    }

    #[inline]
    fn write_sync(&mut self, thread: Thread) {
        *self = Self::Sync(thread);
    }

    #[cfg(feature = "async")]
    #[inline]
    fn write_async(&mut self, waker: &Waker) {
        match self {
            Self::Async(atomic_waker) => atomic_waker.register(waker),
            _ => {
                crate::util::cold();

                let atomic_waker = AtomicWaker::new();
                atomic_waker.register(waker);
                *self = Self::Async(atomic_waker);
            }
        }
    }

    #[cfg(feature = "async")]
    #[inline]
    fn update_async(&self, waker: &Waker) {
        match self {
            Self::Async(atomic_waker) => atomic_waker.register(waker),
            _ => unreachable!("Attempted to update waker without one already in place"),
        }
    }

    #[inline]
    fn unpark(&self) {
        match self {
            Self::Sync(thread) => thread.unpark(),
            #[cfg(feature = "async")]
            Self::Async(waker) => waker.wake(),
            // This branch is entirely unreachable (assuming this library is coded correctly),
            // however I'd like to keep the additional code around reading as small as possible,
            // so in release mode we currently do nothing on this branch.
            #[cfg(not(feature = "async"))]
            Self::None => {
                #[cfg(debug_assertions)]
                {
                    unreachable!("Writer is waiting on readers but parker is None");
                }

                #[cfg(not(debug_assertions))]
                {
                    crate::util::cold();
                }
            }
        }
    }
}

#[cfg(feature = "async")]
pub use async_synchronize::*;

#[cfg(feature = "async")]
mod async_synchronize {
    use super::*;

    pub struct Synchronize<'a> {
        residual: &'a AtomicIsize,
        parker: &'a UnsafeCell<Parker>,
        waiting: bool,
    }

    impl<'a> Synchronize<'a> {
        #[inline]
        pub(super) fn new(residual: &'a AtomicIsize, parker: &'a UnsafeCell<Parker>) -> Self {
            Self {
                residual,
                parker,
                waiting: false,
            }
        }
    }

    impl Future for Synchronize<'_> {
        type Output = ();

        #[inline]
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if self.waiting {
                self.parker
                    .with(|ptr| unsafe { &*ptr }.update_async(cx.waker()));

                let residual = self.residual.load(Ordering::Acquire);

                if likely(residual == 0) {
                    Poll::Ready(())
                } else {
                    debug_assert!(residual < 0);
                    Poll::Pending
                }
            } else {
                let residual = self.residual.load(Ordering::Acquire);

                if residual != 0 {
                    self.parker
                        .with_mut(|ptr| unsafe { &mut *ptr }.write_async(cx.waker()));

                    let latest_residual = self.residual.fetch_add(isize::MIN, Ordering::AcqRel);

                    if likely(latest_residual != 0) {
                        self.as_mut().waiting = true;
                        return Poll::Pending;
                    } else {
                        self.residual.store(0, Ordering::Release);
                    }
                }

                Poll::Ready(())
            }
        }
    }
}

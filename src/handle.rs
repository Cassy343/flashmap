use hashbrown::hash_map::DefaultHashBuilder;
use slab::Slab;

use crate::loom::{
    cell::{Cell, UnsafeCell},
    sync::{
        atomic::{AtomicU8, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, Thread},
};
use crate::Options;
use crate::{cache_padded::CachePadded, Map, ReadHandle, WriteHandle};
use std::mem;
use std::process::abort;
use std::ptr::{self, NonNull};

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

impl<K, V, S> Handle<K, V, S> {
    pub fn new(options: Options<S>) -> (WriteHandle<K, V, S>, ReadHandle<K, V, S>) {
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
    pub unsafe fn release_refcount(&self, key: usize) {
        drop(Box::from_raw(
            self.refcounts.lock().unwrap().remove(key).as_ptr(),
        ));
    }

    #[inline]
    pub unsafe fn release_residual(&self) {
        // TODO: why does loom fail if this is anything less than AcqRel?
        if self.residual.fetch_sub(1, Ordering::AcqRel) == 1 {
            if self.writer_state.swap(WRITABLE, Ordering::AcqRel) == WAITING_ON_READERS {
                self.writer_thread.with(|ptr| {
                    (*ptr).as_ref().map(|thread| thread.unpark());
                });
            }
        }
    }

    #[inline]
    pub unsafe fn start_write<'w>(&self) -> &'w UnsafeCell<Map<K, V, S>> {
        match self.writer_state.load(Ordering::Acquire) {
            WRITABLE => (),
            NOT_WRITABLE => {
                self.writer_thread
                    .with_mut(|ptr| ptr::write(ptr, Some(thread::current())));

                match self.writer_state.compare_exchange(
                    NOT_WRITABLE,
                    WAITING_ON_READERS,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) {
                    Ok(NOT_WRITABLE) | Err(WRITABLE) => (),
                    _ => unreachable!(),
                }
            }
            WAITING_ON_READERS => panic!("Concurrent calls to start_write"),
            _ => unreachable!(),
        };

        // Wait for the current write map to become available
        while self.writer_state.load(Ordering::Acquire) != WRITABLE {
            thread::park();
        }

        &*(self.maps.maps()[self.writer_map.get() as usize] as *const _)
    }

    #[inline]
    pub unsafe fn finish_write(&self) {
        self.writer_state.store(NOT_WRITABLE, Ordering::Relaxed);

        let mut residual = 0;

        // Acquire
        let guard = self.refcounts.lock().unwrap();

        // debug_assert_eq!(self.residual.load(Ordering::SeqCst), 0);

        for refcount in guard.iter().map(|(_, refcount)| refcount.as_ref()) {
            residual += refcount.swap_maps();
        }

        // This needs to be within the mutex
        self.writer_map.set(self.writer_map.get().other());

        // Release
        drop(guard);

        residual = residual.wrapping_add(self.residual.fetch_add(residual, Ordering::AcqRel));
        if residual == 0 {
            self.writer_state.store(WRITABLE, Ordering::Release);
        }
    }
}

impl<K, V, S> Drop for Handle<K, V, S> {
    fn drop(&mut self) {
        let reader_map_index = self.writer_map.get().other();
        unsafe {
            self.maps.access.get(reader_map_index).with_mut(|ptr| {
                (&mut *ptr).drain().for_each(|(key, value)| {
                    key.drop();
                    value.drop();
                });
            });
        }
    }
}

pub struct RefCount {
    value: CachePadded<AtomicUsize>,
}

impl RefCount {
    const MAP_INDEX_FLAG: usize = 1usize << (usize::BITS - 1);
    const COUNT_MASK: usize = (1usize << (usize::BITS - 2)) - 1;

    fn new(read_index: MapIndex) -> Self {
        Self {
            value: CachePadded::new(AtomicUsize::new((read_index as usize) << (usize::BITS - 1))),
        }
    }

    #[inline]
    fn check_overflow(value: usize) {
        if Self::to_refcount(value) == Self::COUNT_MASK {
            abort();
        }
    }

    #[inline]
    fn to_refcount(value: usize) -> usize {
        value & Self::COUNT_MASK
    }

    #[inline]
    fn to_map_index(value: usize) -> MapIndex {
        unsafe { MapIndex::from_usize_unchecked(value >> (usize::BITS - 1)) }
    }

    #[inline]
    pub fn increment(&self) -> MapIndex {
        let old = self.value.fetch_add(1, Ordering::Acquire);
        Self::check_overflow(old);
        Self::to_map_index(old)
    }

    #[inline]
    pub fn decrement(&self, map_index: MapIndex) -> ReaderStatus {
        let old_value = self.value.fetch_sub(1, Ordering::Release);

        if Self::to_map_index(old_value) != map_index {
            ReaderStatus::Residual
        } else {
            ReaderStatus::Normal
        }
    }

    #[inline]
    unsafe fn swap_maps(&self) -> usize {
        let old_value = self.value.fetch_add(Self::MAP_INDEX_FLAG, Ordering::AcqRel);
        Self::to_refcount(old_value)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReaderStatus {
    Normal,
    Residual,
}

#[repr(usize)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapIndex {
    First = 0,
    Second = 1,
}

impl MapIndex {
    #[inline]
    pub unsafe fn from_usize_unchecked(index: usize) -> Self {
        mem::transmute(index)
    }

    #[inline]
    pub fn other(self) -> Self {
        match self {
            Self::First => Self::Second,
            Self::Second => Self::First,
        }
    }
}

struct OwnedMapAccess<K, V, S> {
    access: MapAccess<K, V, S>,
}

impl<K, V, S> OwnedMapAccess<K, V, S> {
    fn new(boxed: Box<[CachePadded<UnsafeCell<Map<K, V, S>>>; 2]>) -> Self {
        Self {
            access: MapAccess::new(NonNull::new(Box::into_raw(boxed)).unwrap()),
        }
    }

    #[inline]
    fn maps(&self) -> [&UnsafeCell<Map<K, V, S>>; 2] {
        [0, 1].map(|index| unsafe { &*self.access.maps.as_ref()[index] })
    }

    #[inline]
    fn share(&self) -> MapAccess<K, V, S> {
        self.access.clone()
    }
}

impl<K, V, S> Drop for OwnedMapAccess<K, V, S> {
    fn drop(&mut self) {
        unsafe {
            drop(Box::from_raw(self.access.maps.as_ptr()));
        }
    }
}

pub struct MapAccess<K, V, S> {
    maps: NonNull<[CachePadded<UnsafeCell<Map<K, V, S>>>; 2]>,
}

impl<K, V, S> MapAccess<K, V, S> {
    fn new(maps: NonNull<[CachePadded<UnsafeCell<Map<K, V, S>>>; 2]>) -> Self {
        Self { maps }
    }

    #[inline]
    pub unsafe fn get(&self, map_index: MapIndex) -> &UnsafeCell<Map<K, V, S>> {
        &*self.maps.as_ref()[map_index as usize]
    }

    fn clone(&self) -> Self {
        Self { maps: self.maps }
    }
}

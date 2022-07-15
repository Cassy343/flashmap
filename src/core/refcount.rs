use crate::loom::sync::atomic::{AtomicUsize, Ordering};
use crate::util::CachePadded;
use std::process::abort;

use super::MapIndex;

pub struct RefCount {
    value: CachePadded<AtomicUsize>,
}

impl RefCount {
    const MAP_INDEX_FLAG: usize = 1usize << (usize::BITS - 1);
    const COUNT_MASK: usize = (1usize << (usize::BITS - 2)) - 1;

    pub(super) fn new(read_index: MapIndex) -> Self {
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
    pub(super) fn increment(&self) -> MapIndex {
        let old_value = self.value.fetch_add(1, Ordering::Acquire);
        Self::check_overflow(old_value);
        Self::to_map_index(old_value)
    }

    #[inline]
    pub(super) fn decrement(&self) -> MapIndex {
        let old_value = self.value.fetch_sub(1, Ordering::Release);
        Self::to_map_index(old_value)
    }

    #[inline]
    pub(super) fn swap_maps(&self) -> usize {
        let old_value = self
            .value
            .fetch_add(Self::MAP_INDEX_FLAG, Ordering::Relaxed);
        Self::to_refcount(old_value)
    }
}

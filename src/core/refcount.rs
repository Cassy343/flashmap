use crate::loom::sync::atomic::{AtomicUsize, Ordering};
use crate::util::CachePadded;
use std::process::abort;
use std::ptr;

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
    fn to_map_index(value: usize) -> MapIndex {
        unsafe { MapIndex::from_usize_unchecked(value >> (usize::BITS - 1)) }
    }

    #[inline]
    pub(super) fn increment(&self) -> MapIndex {
        let old_value = self.value.fetch_add(1, Ordering::Acquire);

        static ABORT_WRAPPER_FN: fn() -> MapIndex = || abort();

        #[cold]
        #[inline(never)]
        fn abort_helper() -> MapIndex {
            let fn_ptr = unsafe { ptr::read_volatile(&ABORT_WRAPPER_FN) };
            (fn_ptr)()
        }

        // This checks if we overflowed from bits 0-61 into bit 62 from the previous increment.
        // This condition yields slightly better asm than `value & Self::COUNT_MASK ==
        // Self::COUNT_MASK`, which checks if the increment we just performed overflowed. Either
        // way is sufficient for soundness.
        if old_value & (Self::COUNT_MASK + 1) > 0 {
            // We do this abort_helper nonsense because without it unnecessary stack operations
            // are inserted into the generated assembly for this function. This is because
            // 1. rustc turns on trap-unreachable, so llvm uses call instead of jmp for call abort
            // 2. llvm adjusts the stack pointer using push, rather than sub
            return abort_helper();
        }

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
        // Remove the bit specifying the map, leaving just the actual count
        old_value & Self::COUNT_MASK
    }
}

use crate::loom::cell::UnsafeCell;
use crate::util::CachePadded;
use crate::Map;
use std::{marker::PhantomData, mem, ptr::NonNull};

#[repr(usize)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapIndex {
    First = 0,
    Second = 1,
}

impl MapIndex {
    #[inline]
    pub unsafe fn from_usize_unchecked(index: usize) -> MapIndex {
        // For some reason the LLVM is dumb and doing a match here produces shitty asm
        unsafe { mem::transmute(index) }
    }

    #[inline]
    pub fn other(self) -> Self {
        match self {
            Self::First => Self::Second,
            Self::Second => Self::First,
        }
    }
}

pub struct OwnedMapAccess<K, V, S> {
    access: MapAccess<K, V, S>,
    _dropck: PhantomData<[CachePadded<UnsafeCell<Map<K, V, S>>>; 2]>,
}

impl<K, V, S> OwnedMapAccess<K, V, S> {
    pub fn new(boxed: Box<[CachePadded<UnsafeCell<Map<K, V, S>>>; 2]>) -> Self {
        Self {
            access: MapAccess::new(NonNull::new(Box::into_raw(boxed)).unwrap()),
            _dropck: PhantomData,
        }
    }

    #[inline]
    pub fn get(&self, map_index: MapIndex) -> &UnsafeCell<Map<K, V, S>> {
        unsafe { self.access.get(map_index) }
    }

    #[inline]
    pub fn share(&self) -> MapAccess<K, V, S> {
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
        let maps = unsafe { &*self.maps.as_ref() };
        &maps[map_index as usize]
    }

    fn clone(&self) -> Self {
        Self { maps: self.maps }
    }
}

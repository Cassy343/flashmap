Documentation for the algorithm implemented by this crate.

There is no code or public API surface in this file, just documentation for the underlying
algorithm.

# General Design

The purpose behind this concurrent map implementation is to minimize the overhead for reading as
much as possible. This presents obvious challenges since readers cannot access the map while it is
being written to. We solve this problem by keeping two copies of the underlying map - one for the
writer to modify, and one for the readers to read from. When the writer wants to publish its
modifications, we atomically swap the two maps such that new readers see the writer's changes. We
then re-apply those changes to the old map when it is safe to do so.

This approach is already implemented by the crate `evmap`, however performance suffers due to
high latency writes and a shared atomic pointer to the readable map. `flashmap`'s implementation
of this approach ensures the readers access no shared state with each other (other than the actual
map) in their critical path, and moreover it ensures that sufficiently infrequent writes are
effectively wait-free.

The trade-offs are that this map is eventually consistent, so existing readers can still access
stale data, and there is only a single writer, so concurrent modification is not supported.

# Map State

This section briefly describes where state lives in this data structure.

## Shared State

The two maps are stored in a length-2 array in memory. Currently the backing map implementation
is provided by `hashbrown`, but this might be made generic in the future. The pointer to this array
is shared with all readers and the writer.

Readers - or more specifically `ReadGuard`s - are tracked via atomic reference
counting. Each read handle has its own individual refcount, but these refcounts are shared with
the writer.

Because of the design of this map, we are able to have the writer block when it cannot make
progress, and be unparked when the last reader gets out of its way. Thus the shared state of the
map includes the necessarry bookkeeping to accomplish this, and will be explained in greater detail
later.

## Reader State

Currently, readers store no additional state other than what is described above. Each read handle
does hold onto a `refcount_key` which is used to deallocate that handle's refcount on drop, but
this state is immutable.

## Writer State

Writers, in addition to an `Arc` of the aforementioned shared state, must store an operation log
of changes. These changes are re-applied to the second map at the beginning of the next write.

# The Algorithm

There are multiple components to this algorithm, each of which is explained individually below.

## Ref Counts

A core component of this algorithm is reference counting. Each read handle has its own reference
count (refcount) which it uses to track the number of outstanding guards which need to be dropped.
In addition to the number of outstanding guards, the refcount is also used to store the current
readable map (the map which new read guards should point to). Since there are only two maps, this
information is stored in the high bit of the refcount, and the actual guard count is stored in the
lower `usize::BITS - 2` bits (we don't use a bit so we can check for overflow).

A refcount has three principle operations: increment, decrement, and swap maps. Part of the
implementation is shown below:
```rust,ignore
pub struct RefCount {
    value: CachePadded<AtomicUsize>,
}

impl RefCount {
    /* The implementation of some helpers has been omitted for brevity */

    pub fn increment(&self) -> MapIndex {
        let old_value = self.value.fetch_add(1, Ordering::Acquire);
        Self::check_overflow(old_value);
        Self::to_map_index(old_value)
    }

    pub fn decrement(&self) -> MapIndex {
        let old_value = self.value.fetch_sub(1, Ordering::Release);
        Self::to_map_index(old_value)
    }

    pub fn swap_maps(&self) -> usize {
        let old_value = self
            .value
            .fetch_add(Self::MAP_INDEX_FLAG, Ordering::Relaxed);
        Self::to_refcount(old_value)
    }
}
```
`increment` and `decrement` both change the refcount as expected, but also make use of the return
value of the `fetch*` operations. The old value of the refcount (before the RMW) will, in its high
bit, tell us which map the writer expects us to be reading from. This is useful especially in
`increment`, since the reader can guard its access to the map *and* know which map to read from in
a single atomic operation. Why we also need the map index from `decrement` will be explained later.

The `fetch_add` in `swap_maps` has the same effect as toggling the high bit. This could be done
through `fetch_xor` but I've seen that compiled to a CAS loop on most architectures. The return
value is the number of active guards at the time of swapping the maps. This quantity will be
referred to as "residual" since it's the number of residual readers still accessing the old map.

## Creating and Dropping Read Handles

Each read handle has its own refcount, and all the refcounts for every read handle are stored in
a single contiguous array (currently backed by `slab`). This array is wrapped in a `Mutex`, so
any access to the entire array involves acquiring a lock. Specifically, when a read handle is
constructed, a new refcount is allocated on the heap, the refcount array is locked, and a pointer
to that refcount is added to the array. Similarly, when a read handle is dropped, the array is
locked, the pointer is removed, and the refcount deallocated.

## The Read Algorithm

The read algorithm is described below:
1. `increment` the refcount.
2. Take the returned `MapIndex`, convert it to an offset in the lengt-2 map array, and get a
   reference to the map to read from. Store the map index in the read guard.
3. When the read guard is dropped, `decrement` the refcount. If the returned map index matches
   the one stored on the guard, we are done. If they do not match, proceed to 4.
4. Since the map indexes do not match, this is a residual guard. Call `release_residual` to
   signal that we are done reading.
5. If we see from the atomic decrement in `release_residual` that we were the last residual
   reader, wake the writer (this portion of the algorithm will be explained later).

`release_residual` is a function that operates on an atomic counter called `residual` which tracks
the number of read guards blocking the writer from making progress. This quantity will be explained
with the write algorithm.

## Parking/Unparking the Writer

Parking and unparking the writer is also managed through the `residual` quantity. The residual
count is stored in the lower 63 bits (or however many for the given architecture), and the highest
bit signals whether or not the writer is waiting to be unparked.

When the writer prepares to park, it sets the high bit of the residual count to 1. If the return
value of this RMW operation signals that no residual readers were left, then the count is set to
zero and the writer does not park. Otherwise, it parks.

When the last residual reader decides to unpark the writer, it will atomically set the residual
count to 0 and then `unpark` the writing thread.

## The Write Algorithm

The write algorithm is split into two parts: `synchronize` + start write, and `publish`. When
a new write guard is created, `start_write` is called, and when that guard is dropped
`publish` is called.

`synchronize` + start write:
1. If there are no residual readers, we are done.
2. If there are residual readers, then park as described above.
3. Once unblocked, obtain a reference to the writable map.
4. Apply changes from previous write.

After these steps but before `publish`, the changes to the writable map are made and stored in
the operation log.

`publish`:
1. Acquire the lock on the refcount array.
2. Swap out the value of the field storing the writable map with the old map.
3. Call `swap_maps` on each refcount in the array, and (non-atomically) accumulate a sum of the
   returned residual count. Call this sum `initial_residual`.
4. Release the lock on the refcount array.
5. Atomically add `initial_residual` to the actual `residual` counter the readers will be
   decrementing. If there were no residual readers, or they finished reading while performing 3,
   then the residual count is now 0, signaling that we don't need to park on the next call to
   `synchronize`.

Note that read handles are not swapped to the new map at the same time, this is done one-by-one.

An important invariant of this algorithm is that `residual == 0` whenever `publish` is called.
That way, either the writing thread will see `residual == 0` after swapping all the maps, or one
of the residual readers will see `residual & isize::MAX == 1` as the old value when it performs its
atomic decrement. In either case, this provides a definite signal that there are no more readers
lingering on the old map.

# Recap

Creating a read guard corresponds to an atomic increment, and dropping a read guard corresponds to
an atomic decrement, and at most one reader will execute the logic to unpark the writer. So overall
the readers' critical path is wait-free if this crate is compiled on an architecture which supports
wait-free atomic fetch-adds, and even if not, or the unparking logic is engaged, those operations
are still fairly cheap compared to hash map operations.

Creating a write guard involves executing the `synchronize` + start write algorithm and then
flushing the operations from the previous write. When the write guard is dropped, the maps are
swapped, but no modifications to either map is made until the next creation of the write guard.
This means that the maps (after the first write) are always out of sync technically, but this is by
design. If we assume that there is some non-negligible amount of time between writes, and that
readers don't have large critical sections, then that means the writer will almost never have to
wait! The time between writes gives the residual readers a chance to move to the new map, and thus
when the writer goes to write again, it won't have to wait for the readers.
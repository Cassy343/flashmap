A lock-free, partially wait-free, eventually consistent, concurrent hashmap.

This map implementation allows reads to always be wait-free, and almost as cheap as reading from an
`Arc<HashMap<K, V>>`. Moreover, writes (when executed from a single thread only) will effectively
be wait-free if performed sufficiently infrequently, and readers do not hold onto guards for
extended periods of time.

The trade-offs for extremely cheap reads are that a write can only be exectued from one thread at a
time, and eventual consistency. In other words, when a write is performed, all reading threads will
only observe the write once they complete their last read and begin a new one.

# How is `flashmap` different?

The underlying algorithm used here is, in principle, the same as that used by
[`evmap`](https://crates.io/crates/evmap). However the implementation of that algorithm has been
modified to **significantly** improve reader performance, at the cost of some necessary API
changes and a different performance profile for the writer. More information on the implementation
details of the algorithm can be found in the `algorithm` module.

# When to use `flashmap`

`flashmap` is optimized for read-heavy to almost-read-only workloads where a single writer is
acceptable. Good use-cases include:
- High frequency reads with occational insertion/removal
- High frequency modification of existing entries with low contention via interior mutability with
  occasional insertion/removal
- High frequency reads with another thread executing a moderate write workload

Situations when **not** to use `flashamp` include:
- Frequent, small writes which cannot be batched
- Concurrent write access from multiple threads

# Performance

TODO: compile some benchmarks
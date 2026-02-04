use std::hash::Hash;

/// A marker trait asserting that a type has a deterministic [`Hash`](std::hash::Hash) and
/// [`Eq`](std::cmp::Eq) implementation.
///
/// This trait is implemented for all standard library types which have deterministic `Hash` and
/// `Eq` implementations.
///
/// # Safety
///
/// A deterministic `Hash` implementation guarantees that if a value is held constant, then the
/// hash of the value will also remain constant if the initial state of the provided hasher is also
/// held constant, and the hasher is itself deterministic.
///
/// A deterministic `Eq` implementation guarantees that if a value is held constant, then the
/// result of comparing it to another constant will not change.
pub unsafe trait TrustedHashEq: Hash + Eq {}

// This massive glut of impls was lifted from `evmap`:
// https://github.com/jonhoo/evmap/blob/0daf488a76f9a2f271e0aab75e84cc65661df195/src/stable_hash_eq.rs

macro_rules! trusted_hash_eq {
    ($(
        $({$($a:lifetime),*$(,)?$($T:ident$(:?$Sized:ident)?),*$(,)?}
        $({$($manual_bounds:tt)*})?)? $Type:ty,
    )*) => {
        trusted_hash_eq!{#
            $(
                $({$($a)*$($T$(:?$Sized$Sized)?)*})? $($({where $($manual_bounds)*})?
                {
                    where $(
                        $T: TrustedHashEq,
                    )*
                })?
                $Type,
            )*
        }
    };
    (#$(
        $({$($a:lifetime)*$($T:ident$(:?Sized$Sized:ident)?)*}
        {$($where_bounds:tt)*}$({$($_t:tt)*})?)? $Type:ty,
    )*) => {
        $(
            unsafe impl$(<$($a,)*$($T$(:?$Sized)?,)*>)? TrustedHashEq for $Type
            $($($where_bounds)*)? {}
        )*
    };
}

use std::{
    any::TypeId,
    borrow::Cow,
    cmp::{self, Reverse},
    collections::{BTreeMap, BTreeSet, LinkedList, VecDeque},
    convert::Infallible,
    ffi::{CStr, CString, OsStr, OsString},
    fmt,
    fs::FileType,
    io::ErrorKind,
    marker::{PhantomData, PhantomPinned},
    mem::{Discriminant, ManuallyDrop},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    num::{
        NonZeroI128, NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI8, NonZeroIsize, NonZeroU128,
        NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU8, NonZeroUsize, Wrapping,
    },
    ops::{Bound, Range, RangeFrom, RangeFull, RangeInclusive, RangeTo, RangeToInclusive},
    path::{Component, Path, PathBuf, Prefix, PrefixComponent},
    ptr::NonNull,
    rc::Rc,
    sync::{atomic, Arc},
    task::Poll,
    thread::ThreadId,
    time::{Duration, Instant, SystemTime},
};

trusted_hash_eq! {
    cmp::Ordering,
    Infallible,
    ErrorKind,
    IpAddr,
    SocketAddr,
    atomic::Ordering,
    bool, char,
    i8, i16, i32, i64, i128,
    isize,
    str,
    u8, u16, u32, u64, u128,
    (),
    usize,
    TypeId,
    CStr,
    CString,
    OsStr,
    OsString,
    fmt::Error,
    FileType,
    PhantomPinned,
    Ipv4Addr,
    Ipv6Addr,
    SocketAddrV4,
    SocketAddrV6,
    NonZeroI8, NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI128, NonZeroIsize,
    NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU128, NonZeroUsize,
    RangeFull,
    Path,
    PathBuf,
    String,
    ThreadId,
    Duration,
    Instant,
    SystemTime,
    {'a} PrefixComponent<'a>,
    {'a} Cow<'a, str>,
    {'a} Cow<'a, CStr>,
    {'a} Cow<'a, OsStr>,
    {'a} Cow<'a, Path>,
    {'a, T}{T: Clone + TrustedHashEq} Cow<'a, [T]>,
    {'a, T}{T: Clone + TrustedHashEq} Cow<'a, T>,
    {'a, T: ?Sized} &'a T,
    {'a, T: ?Sized} &'a mut T,
    {'a} Component<'a>,
    {'a} Prefix<'a>,
    {T} VecDeque<T>,
    {A} (A,),
    {A, B} (A, B),
    {A, B, C} (A, B, C),
    {A, B, C, D} (A, B, C, D),
    {A, B, C, D, E} (A, B, C, D, E),
    {A, B, C, D, E, F} (A, B, C, D, E, F),
    {A, B, C, D, E, F, G} (A, B, C, D, E, F, G),
    {A, B, C, D, E, F, G, H} (A, B, C, D, E, F, G, H),
    {A, B, C, D, E, F, G, H, I} (A, B, C, D, E, F, G, H, I),
    {A, B, C, D, E, F, G, H, I, J} (A, B, C, D, E, F, G, H, I, J),
    {A, B, C, D, E, F, G, H, I, J, K} (A, B, C, D, E, F, G, H, I, J, K),
    {A, B, C, D, E, F, G, H, I, J, K, L} (A, B, C, D, E, F, G, H, I, J, K, L),
    {Idx} Range<Idx>,
    {Idx} RangeFrom<Idx>,
    {Idx} RangeInclusive<Idx>,
    {Idx} RangeTo<Idx>,
    {Idx} RangeToInclusive<Idx>,
    {K, V} BTreeMap<K, V>,
}

macro_rules! trusted_hash_eq_fn {
    ($({$($($A:ident),+)?})*) => {
        trusted_hash_eq!{
            $(
                {Ret$(, $($A),+)?}{} fn($($($A),+)?) -> Ret,
                {Ret$(, $($A),+)?}{} extern "C" fn($($($A),+)?) -> Ret,
                $({Ret, $($A),+}{} extern "C" fn($($A),+, ...) -> Ret,)?
                {Ret$(, $($A),+)?}{} unsafe fn($($($A),+)?) -> Ret,
                {Ret$(, $($A),+)?}{} unsafe extern "C" fn($($($A),+)?) -> Ret,
                $({Ret, $($A),+}{} unsafe extern "C" fn($($A),+, ...) -> Ret,)?
            )*
        }
    };
}

trusted_hash_eq_fn! {
    {}
    {A}
    {A, B}
    {A, B, C}
    {A, B, C, D}
    {A, B, C, D, E}
    {A, B, C, D, E, F}
    {A, B, C, D, E, F, G}
    {A, B, C, D, E, F, G, H}
    {A, B, C, D, E, F, G, H, I}
    {A, B, C, D, E, F, G, H, I, J}
    {A, B, C, D, E, F, G, H, I, J, K}
    {A, B, C, D, E, F, G, H, I, J, K, L}
}

trusted_hash_eq! {
    {T} Bound<T>,
    {T} Option<T>,
    {T} Poll<T>,
    {T: ?Sized}{} *const T,
    {T: ?Sized}{} *mut T,
    {T} [T],
    {T: ?Sized} Box<T>,
    {T} Reverse<T>,
    {T} BTreeSet<T>,
    {T} LinkedList<T>,
    {T: ?Sized}{} PhantomData<T>,
    {T}{} Discriminant<T>,
    {T} ManuallyDrop<T>,
    {T} Wrapping<T>,
    {T: ?Sized}{} NonNull<T>,
    {T: ?Sized} Rc<T>,
    {T: ?Sized} Arc<T>,
    {T} Vec<T>,
    {T, E} Result<T, E>,
}

unsafe impl<T, const N: usize> TrustedHashEq for [T; N] where T: TrustedHashEq {}

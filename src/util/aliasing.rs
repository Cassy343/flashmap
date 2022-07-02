use std::{
    borrow::Borrow,
    ffi::{CStr, CString, OsStr, OsString},
    fmt::{self, Debug, Display, Formatter},
    hash::{Hash, Hasher},
    marker::PhantomData,
    mem::MaybeUninit,
    ops::Deref,
    path::{Path, PathBuf},
    ptr,
};

/// Allows for aliasing typically non-aliasable types without undefined behavior or SB violations.
///
/// This type is very similar to [`ManuallyDrop`](std::mem::ManuallyDrop) in that the underlying
/// value will be leaked without manual intervention, but additionally this type allows aliasing
/// of the inner value through the [`copy`](crate::Alias::copy) method.
/// 
/// # Alias Families
/// 
/// When describing the safety contracts of this type, it is useful to have a notion of "all
/// aliases that refer to the same data." We'll call that collection of alises an "alias family."
/// If an alias `b` is created by copying an alias `a` via [`Alias::copy`](crate::Alias::copy),
/// then `b` is a member of the same alias family as `a`. However if the value being aliased
/// implements `Clone`, then a deep copy of the underlying data can be performed via
/// [`Alias::clone`](crate::Alias::clone). If instead `b` were created by cloning `a`, then `b`
/// would **not** be in the same alias family as `a`, rather it would be the sole member of its
/// own unique alias family.
/// 
/// # Safety
/// 
/// This type and its associated operations are sound because it wraps the aliased value in
/// [`MaybeUninit`](std::mem::MaybeUninit), which causes the compiler to no longer assume any
/// pointers contained within are unique or dereferenceable. Most of the APIs of this type are
/// `unsafe`, but notably `Alias<T>` has a [`Deref`](std::ops::Deref) implementation for `T` which
/// is guaranteed to be safe. The only reason this guarantee can be made is because `Alias<T>`
/// will only ever safely give out shared references to the inner `T`, and all operations which
/// require mutable access or ownership of the underlying value are unsafe. If you plan to use
/// this type, please carefully read the safety comments for its associated methods since the
/// unsafe contracts often ask the caller to assert facts about the program which cannot easily be
/// verified locally.
/// 
/// # Thread Safety
/// 
/// `Alias<T>` is `Send` if and only if `T: Send + Sync`, and similarly `Alias<T>` is `Sync` if and
/// only if `T: Send + Sync`. Clearly if `T: !Send`, then `Alias<T>` cannot be `Send`, however if
/// `Alias<T>` were `Send` when `T: Send + !Sync`, then one could construct an `Alias<Box<T>>`,
/// copy it, and send it to another thread, and obtain a shared reference to the inner `T`,
/// violating the fact that `T: !Sync`.
/// 
/// `T` must also be `Send` in order for `Alias<T>` to be `Sync` due to the following scenario:
/// ```compile_fail
/// # use flashmap::Alias;
/// use std::{marker::PhantomData, thread};
/// 
/// struct NotSend(PhantomData<*const ()>);
/// unsafe impl Sync for NotSend {}
/// 
/// let dont_send = NotSend(PhantomData);
/// let alias = Alias::new(dont_send);
/// let alias_ref: &'static Alias<NotSend> = Box::leak(Box::new(alias));
/// 
/// thread::spawn(move || {
///     let alias_copy = unsafe { Alias::copy(alias_ref) };
///     let dont_send = unsafe { Alias::into_owned(alias_copy) };
///     // Ownership of `dont_send` has been safely obtained in another thread.
/// });
/// ```
/// According to the safety contracts of `copy` and `into_owned`, this program is safe. However,
/// this program should fail to compile since we've transferred ownership of `dont_send` to
/// another thread. Hence, for `Alias<T>` to be `Sync`, `T` must be `Send`.
/// 
/// # Examples
/// 
/// You can alias mutable references through this type without undefined behavior:
/// ```
/// # use flashmap::Alias;
/// let mut x = 10i32;
/// 
/// // Store a mutable reference to `x` in an alias
/// let a: Alias<&mut i32> = Alias::new(&mut x);
/// // Make a copy to alias the underlying pointer
/// // Safety: the value being aliased (in this case the mutable reference to `x`) is
/// // not currently be modified.
/// let b: Alias<&mut i32> = unsafe { Alias::copy(&a) };
/// 
/// // Same value
/// assert_eq!(**a, **b);
/// // Same pointer
/// assert_eq!(*a as *const i32, *b as *const i32);
/// 
/// // Convert an alias back into an owned value
/// // Safety: no alias in the same alias family as b is accessed beyond this point
/// let x_mut: &mut i32 = unsafe { Alias::into_owned(b) };
/// 
/// *x_mut += 1;
/// assert_eq!(x, 11);
/// ```
/// 
/// Similarly, you can alias boxes and other pointer types to avoid making deep copies. However,
/// the aliased value will need to be manually dropped.
/// ```
/// # use flashmap::Alias;
/// let mut boks = Alias::new(Box::new(42i32));
/// // Safety: the value being aliased is not currently being modified
/// let another_boks = unsafe { Alias::copy(&boks) };
/// 
/// assert_eq!(**boks, **another_boks);
/// 
/// // Safety: no alias in the same alias family as boks is accessed beyond this point
/// unsafe { Alias::drop(&mut boks); }
/// ```
#[repr(transparent)]
pub struct Alias<T> {
    value: MaybeUninit<T>,
    _not_send_sync: PhantomData<*const ()>,
}

// See the Thread Safety section in the documentation of Alias
unsafe impl<T> Send for Alias<T> where T: Send + Sync {}
unsafe impl<T> Sync for Alias<T> where T: Send + Sync {}

impl<T> Alias<T> {
    /// Takes ownership of the given value and returns an alias of that value. The alias must be
    /// manually dropped after calling this function, else the inner value will be leaked.
    /// 
    /// Note that the alias returned is conceptually associated with a new, unique alias family
    /// in which it is the only member.
    /// 
    /// # Examples
    /// 
    /// ```
    /// # use flashmap::Alias;
    /// let alias = Alias::new(5i32);
    /// assert_eq!(*alias, 5);
    /// // Since i32 is Copy there's no need to drop it
    /// ```
    #[inline]
    pub const fn new(val: T) -> Self {
        Self {
            value: MaybeUninit::new(val),
            _not_send_sync: PhantomData,
        }
    }

    /// Performs a deep clone of the underlying value, and returns an alias of the cloned value.
    /// 
    /// Similar to [`new`](crate::Alias::new), the returned value is conceptually part of a new
    /// alias family in which it is the only member.
    /// 
    /// # Examples
    /// 
    /// Cloning an aliased string:
    /// ```
    /// # use flashmap::Alias;
    /// let mut a = Alias::new("foo".to_owned());
    /// let mut b = Alias::clone(&a);
    /// 
    /// // Equivalent values
    /// assert_eq!(a, b);
    /// // Different objects in memory
    /// assert_ne!(a.as_ptr(), b.as_ptr());
    /// 
    /// // Ensure we don't leak memory
    /// unsafe {
    ///     Alias::drop(&mut a);
    ///     Alias::drop(&mut b);
    /// }
    /// ```
    #[inline]
    pub fn clone(other: &Self) -> Self
    where
        T: Clone,
    {
        Self::new(T::clone(&**other))
    }

    /// Create a copy of the given alias.
    ///
    /// This function performs a shallow copy. So, for example, if you `copy` an
    /// `Alias<Box<String>>`, then only the 8 bytes (or however many for your architecture)
    /// constituting the pointer to the `String` will be copied, and the actual data in the string
    /// will not be copied or read.
    /// 
    /// The returned alias is conceptually a member of the same alias family as the argument
    /// provided.
    ///
    /// # Safety
    ///
    /// The caller must assert that the `T` being aliased is safe to read. An example of when
    /// this is **not** safe is shown below:
    ///
    /// ```no_run
    /// # use flashmap::Alias;
    /// use std::{sync::Mutex, thread};
    ///
    /// // Mutexes allow for interior mutability, in other words you can modify the value
    /// // within a mutex through an immutable reference to that mutex
    /// let aliased_mutex = Alias::new(Mutex::new(0i32));
    /// // Obtain an immutable reference to the aliased mutex
    /// let x: &'static Alias<Mutex<_>> = Box::leak(Box::new(aliased_mutex));
    ///
    /// thread::spawn(move || {
    ///     // Modify the value within the mutex in parallel with the execution of the
    ///     // spawning thread
    ///     *x.lock().unwrap() = 42;
    /// });
    ///
    /// // !!!!! UNDEFINED BEHAVIOR !!!!!
    /// // Copying the alias does a shallow copy of the underlying mutex, which includes
    /// // copying (and thus reading) the integer being modified in this example.
    /// // This is a concurrent read+write data race.
    /// let y = unsafe { Alias::copy(x) };
    /// ```
    ///
    /// The reason that copying `x` is unsound here is because the data it points to could be
    /// concurrently modified by the spawned thread. `Alias` induces no indirection, and neither
    /// does `Mutex`, so the reference stored in `x` points to the actual bytes of the integer
    /// being modified, hence when we copy the alias into `y`, we read those bytes while they are
    /// being modified, causing a data race.
    /// 
    /// # Examples
    /// 
    /// Aliasing a `String`:
    /// ```
    /// # use flashmap::Alias;
    /// let mut a = Alias::new("foo".to_owned());
    /// // Safety: the value `a` is aliasing is not being concurrently modified
    /// let b = unsafe { Alias::copy(&a) };
    /// 
    /// // Equivalent values
    /// assert_eq!(a, b);
    /// // Same object in memory
    /// assert_eq!(a.as_ptr(), b.as_ptr());
    /// 
    /// // Ensure we don't leak memory
    /// unsafe {
    ///     // We only need to drop one of the aliases since they both alias the same
    ///     // location in memory
    ///     Alias::drop(&mut a);
    /// }
    /// ```
    #[inline]
    pub unsafe fn copy(other: &Self) -> Self {
        Self {
            value: unsafe { ptr::read(&other.value) },
            _not_send_sync: PhantomData,
        }
    }

    /// Converts an alias of a value into an owned value.
    /// 
    /// # Safety
    /// 
    /// The caller must assert that no alias within the same alias family as the argument is
    /// accessed during, or at any point after this function is called. Note that implicitly
    /// dropping an `Alias<T>` does **not** count as an access since the `Drop` implementation for
    /// `Alias<T>` is a no-op and does not access the underlying data.
    /// 
    /// The following example shows an **incorrect** use of `into_owned`, resulting in undefined
    /// behavior:
    /// ```no_run
    /// # use flashmap::Alias;
    /// let a = Alias::new(Box::new(10i32));
    /// // Safety: the data aliased by `a` is not currently being modified
    /// let b = unsafe { Alias::copy(&a) };
    /// 
    /// // !!!!! UNDEFINED BEHAVIOR !!!!!
    /// // `b` is in the same alias family as `a`, and `a` is accessed after this
    /// // function call.
    /// let boks = unsafe { Alias::into_owned(b) };
    /// drop(boks);
    /// 
    /// // Alias guarantees that calling `deref` is always safe, so although the actual
    /// // operation (use after free) which immediately causes UB occurs here, this is
    /// // due to the violation of the unsafe contract on `into_owned` above.
    /// assert_eq!(**a, 10);
    /// ```
    /// 
    /// # Examples
    /// 
    /// ```
    /// # use flashmap::Alias;
    /// let a = Alias::new("foo".to_owned());
    /// // Safety: the data aliased by `a` is not currently being modified
    /// let b = unsafe { Alias::copy(&a) };
    /// 
    /// // Safety: `a` is the only other member of `b`'s alias family, and is not accessed
    /// // after this point
    /// let string = unsafe { Alias::into_owned(b) };
    /// 
    /// assert_eq!(string, "foo");
    /// 
    /// // Calling Drop::drop on an Alias<T> does not count as an access
    /// drop(a);
    /// ```
    #[inline]
    pub const unsafe fn into_owned(alias: Self) -> T {
        unsafe { alias.value.assume_init() }
    }

    /// Converts an alias of a value into an owned value.
    /// 
    /// # Safety
    /// 
    /// This function has the same safety requirements as [`into_owned`](crate::Alias::into_owned).
    /// 
    /// The following example shows an **incorrect** use of `drop`, resulting in undefined
    /// behavior:
    /// ```no_run
    /// # use flashmap::Alias;
    /// let a = Alias::new(Box::new(10i32));
    /// // Safety: the data aliased by `a` is not currently being modified
    /// let mut b = unsafe { Alias::copy(&a) };
    /// 
    /// // !!!!! UNDEFINED BEHAVIOR !!!!!
    /// // `b` is in the same alias family as `a`, and `a` is accessed after this
    /// // function call.
    /// unsafe { Alias::drop(&mut b); }
    /// 
    /// // Alias guarantees that calling `deref` is always safe, so although the actual
    /// // operation (use after free) which immediately causes UB occurs here, this is
    /// // due to the violation of the unsafe contract on `drop` above.
    /// assert_eq!(**a, 10);
    /// ```
    /// 
    /// # Examples
    /// 
    /// ```
    /// # use flashmap::Alias;
    /// let a = Alias::new("foo".to_owned());
    /// // Safety: the data aliased by `a` is not currently being modified
    /// let mut b = unsafe { Alias::copy(&a) };
    /// 
    /// // Safety: neither `a` or `b` are accessed after this point
    /// unsafe { Alias::drop(&mut b) };
    /// 
    /// // Calling Drop::drop on an Alias<T> does not count as an access
    /// drop(a);
    /// ```
    #[inline]
    pub unsafe fn drop(alias: &mut Self) {
        unsafe { alias.value.assume_init_drop() }
    }
}

impl<T> Deref for Alias<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        // Safety: the caller has asserted that this method will never be called after into_owned
        // or drop by either never calling those methods or abiding to their safety contracts, so
        // it is safe to give out a shared reference to the underlying value here.
        unsafe { self.value.assume_init_ref() }
    }
}

impl<T: Debug> Debug for Alias<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(&**self, f)
    }
}

impl<T: Display> Display for Alias<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&**self, f)
    }
}

impl<T: PartialEq> PartialEq for Alias<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        PartialEq::eq(&**self, &**other)
    }
}

impl<T: Eq> Eq for Alias<T> {}

impl<T: Hash> Hash for Alias<T> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state)
    }
}

macro_rules! impl_borrow {
    ($( ($T:ty, $U:ty) ),*) => {
        $(
            impl Borrow<$U> for Alias<$T>
            where
                $T: Borrow<$U>,
            {
                #[inline]
                fn borrow(&self) -> &$U {
                    (**self).borrow()
                }
            }
        )*
    };
}

impl_borrow! {
    (String, str),
    (PathBuf, Path),
    (OsString, OsStr),
    (CString, CStr)
}

impl<T: ?Sized> Borrow<T> for Alias<Box<T>> {
    #[inline]
    fn borrow(&self) -> &T {
        (**self).borrow()
    }
}

impl<T: ?Sized> Borrow<T> for Alias<std::sync::Arc<T>> {
    #[inline]
    fn borrow(&self) -> &T {
        (**self).borrow()
    }
}

impl<T> Borrow<T> for Alias<T> {
    #[inline]
    fn borrow(&self) -> &T {
        &**self
    }
}

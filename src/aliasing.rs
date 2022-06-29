use std::{
    borrow::Borrow,
    ffi::{CStr, CString, OsStr, OsString},
    hash::{Hash, Hasher},
    marker::PhantomData,
    mem::MaybeUninit,
    path::{Path, PathBuf},
    ptr,
};

#[repr(transparent)]
pub struct MaybeAliased<T, S = Unsafe> {
    value: MaybeUninit<T>,
    _safety: PhantomData<S>,
    _not_send_sync: PhantomData<*const ()>,
}

unsafe impl<T, S> Send for MaybeAliased<T, S> where T: Send + Sync {}
unsafe impl<T, S> Sync for MaybeAliased<T, S> where T: Sync {}

impl<T> MaybeAliased<T, Unsafe> {
    #[inline]
    pub const fn new(val: T) -> Self {
        Self {
            value: MaybeUninit::new(val),
            _safety: PhantomData,
            _not_send_sync: PhantomData,
        }
    }

    #[inline]
    pub const fn new_ref(val: &T) -> &Self {
        unsafe { &*(val as *const _ as *const Self) }
    }

    #[inline]
    pub unsafe fn get(&self) -> &T {
        self.value.assume_init_ref()
    }
}

impl<T> MaybeAliased<T, ReadSafe> {
    #[inline]
    pub const unsafe fn new_read_safe(val: T) -> Self {
        Self {
            value: MaybeUninit::new(val),
            _safety: PhantomData,
            _not_send_sync: PhantomData,
        }
    }

    #[inline]
    pub const unsafe fn new_ref_read_safe(val: &T) -> &Self {
        &*(val as *const _ as *const Self)
    }

    #[inline]
    pub fn safe_get(&self) -> &T {
        unsafe { self.value.assume_init_ref() }
    }
}

impl<T, S> MaybeAliased<T, S> {
    #[inline]
    pub unsafe fn alias(&self) -> Self {
        Self {
            value: ptr::read(&self.value),
            _safety: PhantomData,
            _not_send_sync: PhantomData,
        }
    }

    #[inline]
    pub unsafe fn into_owned(self) -> T {
        self.value.assume_init()
    }

    #[inline]
    pub unsafe fn drop(self) {
        drop(self.into_owned())
    }
}

pub enum Unsafe {}
pub enum ReadSafe {}

impl<T: PartialEq> PartialEq for MaybeAliased<T, ReadSafe> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.safe_get().eq(other.safe_get())
    }
}

impl<T: Eq> Eq for MaybeAliased<T, ReadSafe> {}

impl<T: Hash> Hash for MaybeAliased<T, ReadSafe> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.safe_get().hash(state)
    }
}

macro_rules! impl_borrow {
    ($( ($T:ty, $U:ty) ),*) => {
        $(
            impl Borrow<$U> for MaybeAliased<$T, ReadSafe>
            where
                $T: Borrow<$U>,
            {
                #[inline]
                fn borrow(&self) -> &$U {
                    self.safe_get().borrow()
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

impl<T> Borrow<T> for MaybeAliased<Box<T>, ReadSafe> {
    #[inline]
    fn borrow(&self) -> &T {
        self.safe_get().borrow()
    }
}

impl<T> Borrow<T> for MaybeAliased<std::sync::Arc<T>, ReadSafe> {
    #[inline]
    fn borrow(&self) -> &T {
        self.safe_get().borrow()
    }
}

impl<T> Borrow<T> for MaybeAliased<T, ReadSafe> {
    #[inline]
    fn borrow(&self) -> &T {
        self.safe_get()
    }
}

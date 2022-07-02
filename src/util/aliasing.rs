use std::{
    borrow::Borrow,
    ffi::{CStr, CString, OsStr, OsString},
    hash::{Hash, Hasher},
    marker::PhantomData,
    mem::MaybeUninit,
    ops::Deref,
    path::{Path, PathBuf},
    ptr,
};

#[repr(transparent)]
pub struct Alias<T> {
    value: MaybeUninit<T>,
    _not_send_sync: PhantomData<*const ()>,
}

unsafe impl<T> Send for Alias<T> where T: Send + Sync {}
unsafe impl<T> Sync for Alias<T> where T: Send + Sync {}

impl<T> Alias<T> {
    #[inline]
    pub const fn new(val: T) -> Self {
        Self {
            value: MaybeUninit::new(val),
            _not_send_sync: PhantomData,
        }
    }

    #[inline]
    pub unsafe fn copy(other: &Self) -> Self {
        Self {
            value: ptr::read(&other.value),
            _not_send_sync: PhantomData,
        }
    }

    #[inline]
    pub const unsafe fn into_owned(alias: Self) -> T {
        alias.value.assume_init()
    }

    #[inline]
    pub unsafe fn drop(alias: &mut Self) {
        alias.value.assume_init_drop()
    }
}

impl<T> Deref for Alias<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { self.value.assume_init_ref() }
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

impl<T> Borrow<T> for Alias<Box<T>> {
    #[inline]
    fn borrow(&self) -> &T {
        (**self).borrow()
    }
}

impl<T> Borrow<T> for Alias<std::sync::Arc<T>> {
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

#[cfg(loom)]
pub use loom::*;

#[cfg(not(loom))]
pub use std::{sync, thread};

pub fn maybe_loom_model<F>(test: F)
where
    F: Fn() + Send + Sync + 'static,
{
    #[cfg(loom)]
    loom::model(test);
    #[cfg(not(loom))]
    test();
}

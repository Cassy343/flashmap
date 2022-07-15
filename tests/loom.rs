mod util;

use flashmap::{Evicted, ReadHandle};
use util::{thread, TrackAccess};

trait BoolExt {
    fn implies(self, other: Self) -> Self;
}

impl BoolExt for bool {
    fn implies(self, other: Self) -> Self {
        !self || other
    }
}

#[test]
pub fn only_readers() {
    util::maybe_loom_model(|| {
        let (mut write, read) = flashmap::new::<TrackAccess<u32>, TrackAccess<u32>>();

        let mut guard = write.guard();
        guard.insert(TrackAccess::new(1), TrackAccess::new(3));
        guard.insert(TrackAccess::new(2), TrackAccess::new(8));
        guard.insert(TrackAccess::new(3), TrackAccess::new(13));
        drop(guard);
        drop(write);

        fn test_read(read: ReadHandle<TrackAccess<u32>, TrackAccess<u32>>) {
            let guard = read.guard();
            let result = *guard.get(&3).unwrap().get()
                - *guard.get(&1).unwrap().get()
                - *guard.get(&2).unwrap().get();
            drop(guard);
            assert_eq!(result, 2);
        }

        thread::spawn({
            let read = read.clone();
            move || test_read(read)
        });

        thread::spawn(move || test_read(read));
    });
}

#[test]
pub fn reader_and_writer() {
    util::maybe_loom_model(|| {
        let (mut write, read) = flashmap::new::<TrackAccess<u32>, TrackAccess<u32>>();

        let t1 = thread::spawn(move || {
            write
                .guard()
                .insert(TrackAccess::new(10), TrackAccess::new(20));
        });

        let t2 = thread::spawn(move || {
            let res = read.guard().get(&10).map(|x| *x.get());
            assert!(matches!(res, Some(20) | None));
            read
        });

        t1.join().unwrap();
        let read = t2.join().unwrap();

        assert_eq!(read.guard().get(&10).map(|x| *x.get()).unwrap(), 20);
    });
}

#[test]
pub fn many_writes() {
    util::maybe_loom_model(|| {
        let (mut write, read) = flashmap::new::<TrackAccess<u32>, TrackAccess<u32>>();

        let t1 = thread::spawn(move || {
            write
                .guard()
                .insert(TrackAccess::new(10), TrackAccess::new(20));
            write
                .guard()
                .insert(TrackAccess::new(20), TrackAccess::new(40));
        });

        let t2 = thread::spawn(move || {
            let x = read.guard().get(&20).map(|x| *x.get());
            assert!(matches!(x, Some(40) | None));
            let y = read.guard().get(&10).map(|x| *x.get());
            assert!(matches!(y, Some(20) | None));
            assert!(x.is_some().implies(y.is_some()));
        });

        t1.join().unwrap();
        t2.join().unwrap();
    });
}

#[test]
pub fn many_different_writes() {
    util::maybe_loom_model(|| {
        let (mut write, read) = flashmap::new::<TrackAccess<u32>, TrackAccess<u32>>();

        write
            .guard()
            .insert(TrackAccess::new(10), TrackAccess::new(20));

        let t1 = thread::spawn(move || {
            write
                .guard()
                .insert(TrackAccess::new(20), TrackAccess::new(40));

            assert!(write
                .guard()
                .replace(TrackAccess::new(20), |x| TrackAccess::new(*x.get() + 5))
                .is_some());

            assert!(write.guard().remove(TrackAccess::new(10)).is_some());
        });

        let t2 = thread::spawn(move || {
            let x = read.guard().get(&10).map(|x| *x.get());
            assert!(matches!(x, Some(20) | None));

            let y = read.guard().get(&20).map(|x| *x.get());
            assert!(matches!(y, Some(40) | Some(45) | None));

            assert!(matches!(y, Some(40) | None).implies(x.is_some()));
            assert!(x.is_none().implies(y == Some(45)));
        });

        t1.join().unwrap();
        t2.join().unwrap();
    });
}

#[test]
pub fn complex_read_many_writes() {
    util::maybe_loom_model(|| {
        let (mut write, read) = flashmap::new::<TrackAccess<u32>, TrackAccess<u32>>();

        let t1 = thread::spawn(move || {
            write
                .guard()
                .insert(TrackAccess::new(10), TrackAccess::new(20));
            write
                .guard()
                .insert(TrackAccess::new(20), TrackAccess::new(40));
        });

        let t2 = thread::spawn(move || {
            let guard1 = read.guard();

            let x = guard1.get(&20).map(|x| *x.get());
            assert!(matches!(x, Some(40) | None));

            let guard2 = read.guard();

            let y = guard2.get(&10).map(|x| *x.get());
            assert!(matches!(y, Some(20) | None));
            assert!(x.is_some().implies(y.is_some()));

            drop(guard2);

            assert!(matches!(guard1.get(&20).map(|x| *x.get()), Some(40) | None));

            drop(guard1);
        });

        t1.join().unwrap();
        t2.join().unwrap();
    });
}

#[test]
pub fn many_handles() {
    util::maybe_loom_model(|| {
        let (mut write, read) = flashmap::new::<TrackAccess<u32>, TrackAccess<u32>>();

        let t1 = thread::spawn(move || {
            write
                .guard()
                .insert(TrackAccess::new(10), TrackAccess::new(20));
            write
        });

        let t2 = thread::spawn(move || {
            let guard1 = read.guard();

            let read2 = read.clone();
            let guard2 = read2.guard();

            let x = guard1.get(&20).map(|x| *x.get());
            assert!(matches!(x, Some(40) | None));
            drop(guard1);
            drop(read);

            let y = guard2.get(&10).map(|x| *x.get());
            assert!(matches!(y, Some(20) | None));
            assert!(x.is_some().implies(y.is_some()));
            drop(guard2);
            drop(read2);
        });

        let mut write = t1.join().unwrap();
        write
            .guard()
            .insert(TrackAccess::new(20), TrackAccess::new(40));
        drop(write);

        t2.join().unwrap();
    });
}

#[test]
pub fn evicted_and_leaked_values() {
    util::maybe_loom_model(|| {
        let (mut write, read) = flashmap::new::<TrackAccess<u32>, TrackAccess<u32>>();

        let mut guard = write.guard();
        guard.insert(TrackAccess::new(10), TrackAccess::new(20));
        guard.insert(TrackAccess::new(20), TrackAccess::new(40));
        guard.insert(TrackAccess::new(40), TrackAccess::new(80));
        drop(guard);

        let t1 = thread::spawn(move || {
            let mut guard = write.guard();
            let twenty = guard.remove(TrackAccess::new(10)).unwrap();
            let forty = guard
                .replace(TrackAccess::new(20), |val| TrackAccess::new(val.get() + 5))
                .unwrap();
            let twenty = Evicted::leak(twenty);
            assert_eq!(*forty.get(), 40);
            let eighty = guard
                .insert(TrackAccess::new(40), TrackAccess::new(90))
                .map(Evicted::leak)
                .unwrap();
            drop(guard);

            let mut twenty = write.reclaim_one(twenty);
            assert_eq!(*twenty.get_mut(), 20);
            assert_eq!(*eighty.get(), 80);

            write.guard().drop_lazily(eighty);
        });

        let t2 = thread::spawn(move || {
            let twenty = read.guard().get(&10).map(|x| *x.get());
            let forty = read.guard().get(&20).map(|x| *x.get());
            let eighty = read.guard().get(&40).map(|x| *x.get());

            assert!(twenty
                .is_none()
                .implies(forty == Some(45) && eighty == Some(90)));
            assert!((forty == Some(45)).implies(eighty == Some(90)));
        });

        t1.join().unwrap();
        t2.join().unwrap();
    });
}

// WARNING: this test takes about 20 minutes to run on my AMD 9 Ryzen 5900X with the
// loomtest-fast profile
#[test]
#[cfg(long_test)]
pub fn many_reads_many_writes() {
    util::maybe_loom_model(|| {
        let (mut write, read) = flashmap::new::<u32, u32>();

        let t1 = thread::spawn(move || {
            write.guard().insert(10, 20);
            write.guard().insert(20, 40);
        });

        let t2 = thread::spawn({
            let read = read.clone();
            move || {
                assert!(matches!(read.guard().get(&10).copied(), Some(20) | None));
            }
        });

        let t3 = thread::spawn(move || {
            assert!(matches!(read.guard().get(&20).copied(), Some(40) | None));
        });

        t1.join().unwrap();
        t2.join().unwrap();
        t3.join().unwrap();
    });
}

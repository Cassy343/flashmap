mod util;

use flashmap::{ReadHandle, RemovalResult};
use util::thread;

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
        let (mut write, read) = flashmap::new::<String, String>();

        let mut guard = write.guard();
        guard.insert("a".to_owned(), "foo".to_owned());
        guard.insert("b".to_owned(), "fizzbuzz".to_owned());
        guard.insert("c".to_owned(), "Hello, world!".to_owned());
        drop(guard);
        drop(write);

        fn test_read(read: ReadHandle<String, String>) {
            let guard = read.guard();
            let result = guard.get("c").unwrap().len()
                - guard.get("a").unwrap().len()
                - guard.get("b").unwrap().len();
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
        let (mut write, read) = flashmap::new::<u32, Box<u32>>();

        let t1 = thread::spawn(move || {
            write.guard().insert(10, Box::new(20));
        });

        let t2 = thread::spawn(move || {
            let res = read.guard().get(&10).map(|x| **x);
            assert!(matches!(res, Some(20) | None));
            read
        });

        t1.join().unwrap();
        let read = t2.join().unwrap();

        assert_eq!(read.guard().get(&10).map(|x| **x).unwrap(), 20);
    });
}

#[test]
pub fn many_writes() {
    util::maybe_loom_model(|| {
        let (mut write, read) = flashmap::new::<u32, Box<u32>>();

        let t1 = thread::spawn(move || {
            write.guard().insert(10, Box::new(20));
            write.guard().insert(20, Box::new(40));
        });

        let t2 = thread::spawn(move || {
            let x = read.guard().get(&20).map(|x| **x);
            assert!(matches!(x, Some(40) | None));
            let y = read.guard().get(&10).map(|x| **x);
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
        let (mut write, read) = flashmap::new::<u32, Box<u32>>();

        write.guard().insert(10, Box::new(20));

        let t1 = thread::spawn(move || {
            write.guard().insert(20, Box::new(40));

            assert!(write.guard().rcu(20, |x| Box::new(**x + 5)));

            assert_eq!(write.guard().remove(10), RemovalResult::Removed);
        });

        let t2 = thread::spawn(move || {
            let x = read.guard().get(&10).map(|x| **x);
            assert!(matches!(x, Some(20) | None));

            let y = read.guard().get(&20).map(|x| **x);
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
        let (mut write, read) = flashmap::new::<u32, Box<u32>>();

        let t1 = thread::spawn(move || {
            write.guard().insert(10, Box::new(20));
            write.guard().insert(20, Box::new(40));
        });

        let t2 = thread::spawn(move || {
            let guard1 = read.guard();

            let x = guard1.get(&20).map(|x| **x);
            assert!(matches!(x, Some(40) | None));

            let guard2 = read.guard();

            let y = guard2.get(&10).map(|x| **x);
            assert!(matches!(y, Some(20) | None));
            assert!(x.is_some().implies(y.is_some()));

            drop(guard2);

            assert!(matches!(guard1.get(&20).map(|x| **x), Some(40) | None));

            drop(guard1);
        });

        t1.join().unwrap();
        t2.join().unwrap();
    });
}

#[test]
pub fn many_handles() {
    util::maybe_loom_model(|| {
        let (mut write, read) = flashmap::new::<u32, Box<u32>>();

        let t1 = thread::spawn(move || {
            write.guard().insert(10, Box::new(20));
            write
        });

        let t2 = thread::spawn(move || {
            let guard1 = read.guard();

            let read2 = read.clone();
            let guard2 = read2.guard();

            let x = guard1.get(&20).map(|x| **x);
            assert!(matches!(x, Some(40) | None));
            drop(guard1);
            drop(read);

            let y = guard2.get(&10).map(|x| **x);
            assert!(matches!(y, Some(20) | None));
            assert!(x.is_some().implies(y.is_some()));
            drop(guard2);
            drop(read2);
        });

        let mut write = t1.join().unwrap();
        write.guard().insert(20, Box::new(40));
        drop(write);

        t2.join().unwrap();
    });
}

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

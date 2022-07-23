//! These tests focus on creating interesting interleavings of borrows and lifetimes.

mod util;

use flashmap::Evicted;
use std::thread;
use util::dderef;

#[test]
pub fn nested_readers() {
    let (mut write, read) = flashmap::new::<Box<u32>, Box<u32>>();

    let t1 = thread::spawn(move || {
        write.guard().insert(Box::new(10), Box::new(20));
        write.guard().insert(Box::new(20), Box::new(40));
    });

    let t2 = thread::spawn(move || {
        let outer = read.guard();
        let forty = outer.get(&20);
        let middle = read.guard();
        assert!(matches!(forty.map(dderef), Some(40) | None));
        let inner = read.guard();
        let twenty = middle.get(&10);
        assert!(matches!(forty.map(dderef), Some(40) | None));
        drop(outer);
        assert!(matches!(twenty.map(dderef), Some(20) | None));
        assert_eq!(inner.get(&10), twenty);
        drop(inner);
        assert!(matches!(twenty.map(dderef), Some(20) | None));
        drop(middle);
    });

    t1.join().unwrap();
    t2.join().unwrap();
}

#[test]
pub fn reclamation() {
    let (mut write, read) = flashmap::new::<Box<u32>, Box<u32>>();

    let mut guard = write.guard();
    guard.insert(Box::new(10), Box::new(20));
    guard.insert(Box::new(20), Box::new(40));
    guard.insert(Box::new(40), Box::new(80));
    drop(guard);

    let mut guard = write.guard();
    let twenty = guard.remove(Box::new(10)).unwrap();
    let forty = guard.replace(Box::new(20), |_| Box::new(50)).unwrap();

    let leaked_twenty = Evicted::leak(twenty);
    let leaked_forty = Evicted::leak(forty);

    let eighty = guard.insert(Box::new(40), Box::new(90)).unwrap();
    let leaked_eighty = Evicted::leak(eighty);
    drop(guard);

    let reclaimer = write.reclaimer();
    let twenty = reclaimer(leaked_twenty);
    let forty = reclaimer(leaked_forty);
    drop(reclaimer);

    assert_eq!(*twenty * 2, *forty);
    assert_eq!(**leaked_eighty, 80);

    write.guard().drop_lazily(leaked_eighty);

    drop(write);

    let guard = read.guard();
    assert!(guard.get(&10).is_none());
    assert_eq!(**guard.get(&20).unwrap(), 50);
    assert_eq!(**guard.get(&40).unwrap(), 90);

    drop(guard);
    drop(read);
}

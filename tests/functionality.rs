//! Tests the functionality of the map. IE, these test focus on making sure map operations like
//! insert/remove etc. work as expected.
//!
//! Many of these tests are modified versions of the unit tests here:
//! https://github.com/rust-lang/hashbrown/blob/83a2a98131212f5999479b885188b85c0c8a79d6/src/map.rs

mod util;

use flashmap::Evicted;
use util::dderef;

#[test]
pub fn insert() {
    let (mut write, read) = flashmap::new::<Box<u32>, Box<u32>>();
    assert_eq!(read.guard().len(), 0);
    assert!(write.guard().insert(Box::new(1), Box::new(2)).is_none());
    assert_eq!(read.guard().len(), 1);
    assert!(write.guard().insert(Box::new(2), Box::new(4)).is_none());
    assert_eq!(read.guard().len(), 2);
    assert_eq!(**read.guard().get(&1).unwrap(), 2);
    assert_eq!(**write.guard().get(&2).unwrap(), 4);
    assert_eq!(**write.guard().insert(Box::new(1), Box::new(3)).unwrap(), 2);
    assert_eq!(read.guard().len(), 2);
    assert_eq!(**write.guard().get(&1).unwrap(), 3);
    assert_eq!(**read.guard().get(&1).unwrap(), 3);
}

#[test]
fn test_empty_remove() {
    let (mut write, _read) = flashmap::new::<u32, u32>();
    assert!(write.guard().remove(0).is_none());
}

#[test]
fn test_empty_iter() {
    let (mut _write, read) = flashmap::new::<u32, u32>();
    assert!(read.guard().iter().next().is_none());
    assert!(read.guard().keys().next().is_none());
    assert!(read.guard().values().next().is_none());
    assert!(read.guard().is_empty());
}

#[test]
fn test_lots_of_insertions() {
    let (mut write, read) = flashmap::new::<Box<i32>, Box<i32>>();

    const MIN: i32 = 0;
    const MID: i32 = 10;
    const MAX: i32 = 20;

    // Try this a few times to make sure we never screw up the map's internal state.
    for _ in 0..10 {
        assert!(read.guard().is_empty());

        for i in MIN..MID {
            assert!(write.guard().insert(Box::new(i), Box::new(i)).is_none());

            for j in MIN..=i {
                assert_eq!(read.guard().get(&j).map(dderef), Some(&j));
            }

            for j in i + 1..MID {
                assert_eq!(read.guard().get(&j).map(dderef), None);
            }
        }

        for i in MID..MAX {
            assert!(!read.guard().contains_key(&i));
        }

        // remove forwards
        for i in MIN..MID {
            assert!(write.guard().remove(Box::new(i)).is_some());

            for j in MIN..=i {
                assert!(!read.guard().contains_key(&j));
            }

            for j in i + 1..MID {
                assert!(read.guard().contains_key(&j));
            }
        }

        for i in MIN..MID {
            assert!(!read.guard().contains_key(&i));
        }

        for i in MIN..MID {
            assert!(write.guard().insert(Box::new(i), Box::new(i)).is_none());
        }

        // remove backwards
        for i in (MIN..MID).rev() {
            assert!(write.guard().remove(Box::new(i)).is_some());

            for j in i..MID {
                assert!(!read.guard().contains_key(&j));
            }

            for j in MIN..i {
                assert!(read.guard().contains_key(&j));
            }
        }
    }
}

#[test]
fn test_replace() {
    let (mut write, read) = flashmap::new::<Box<i32>, Box<i32>>();
    let mut guard = write.guard();
    assert!(guard.insert(Box::new(1), Box::new(12)).is_none());
    assert!(guard.insert(Box::new(2), Box::new(8)).is_none());
    assert!(guard.insert(Box::new(5), Box::new(14)).is_none());
    drop(guard);
    let new = 100;
    write
        .guard()
        .replace(Box::new(5), |_| Box::new(new))
        .unwrap();
    assert_eq!(read.guard().get(&5).map(dderef), Some(&new));
}

#[test]
fn test_insert_overwrite() {
    let (mut write, read) = flashmap::new::<Box<i32>, Box<i32>>();
    assert!(write.guard().insert(Box::new(1), Box::new(2)).is_none());
    assert_eq!(**read.guard().get(&1).unwrap(), 2);
    assert!(write.guard().insert(Box::new(1), Box::new(3)).is_some());
    assert_eq!(**read.guard().get(&1).unwrap(), 3);
}

#[test]
fn test_insert_conflicts() {
    let (mut write, read) = flashmap::new::<Box<i32>, Box<i32>>();
    let mut guard = write.guard();
    assert!(guard.insert(Box::new(1), Box::new(2)).is_none());
    assert!(guard.insert(Box::new(5), Box::new(3)).is_none());
    assert!(guard.insert(Box::new(9), Box::new(4)).is_none());
    guard.publish();
    assert_eq!(**read.guard().get(&9).unwrap(), 4);
    assert_eq!(**read.guard().get(&5).unwrap(), 3);
    assert_eq!(**read.guard().get(&1).unwrap(), 2);
}

#[test]
fn test_conflict_remove() {
    let (mut write, read) = flashmap::new::<Box<i32>, Box<i32>>();
    assert!(write.guard().insert(Box::new(1), Box::new(2)).is_none());
    assert_eq!(**read.guard().get(&1).unwrap(), 2);
    assert!(write.guard().insert(Box::new(5), Box::new(3)).is_none());
    assert_eq!(**read.guard().get(&1).unwrap(), 2);
    assert_eq!(**read.guard().get(&5).unwrap(), 3);
    assert!(write.guard().insert(Box::new(9), Box::new(4)).is_none());
    assert_eq!(**read.guard().get(&1).unwrap(), 2);
    assert_eq!(**read.guard().get(&5).unwrap(), 3);
    assert_eq!(**read.guard().get(&9).unwrap(), 4);
    assert!(write.guard().remove(Box::new(1)).is_some());
    assert_eq!(**read.guard().get(&9).unwrap(), 4);
    assert_eq!(**read.guard().get(&5).unwrap(), 3);
}

#[test]
fn test_is_empty() {
    let (mut write, read) = flashmap::new::<Box<i32>, Box<i32>>();
    assert!(write.guard().insert(Box::new(1), Box::new(2)).is_none());
    assert!(!read.guard().is_empty());
    assert!(write.guard().remove(Box::new(1)).is_some());
    assert!(write.guard().is_empty() && read.guard().is_empty());
}

#[test]
fn test_remove() {
    let (mut write, _read) = flashmap::new::<Box<i32>, Box<i32>>();
    write.guard().insert(Box::new(1), Box::new(2));
    assert_eq!(**write.guard().remove(Box::new(1)).unwrap(), 2);
    assert!(write.guard().remove(Box::new(1)).is_none());
}

#[test]
fn test_iterate() {
    let (mut write, read) = flashmap::new::<Box<i32>, Box<i32>>();
    let mut guard = write.guard();
    for i in 0..32 {
        assert!(guard.insert(Box::new(i), Box::new(i * 2)).is_none());
    }
    drop(guard);
    assert_eq!(read.guard().len(), 32);

    let mut observed: u32 = 0;

    for (k, v) in read.guard().iter() {
        assert_eq!(**v, **k * 2);
        observed |= 1 << **k;
    }
    assert_eq!(observed, 0xFFFF_FFFF);
}

#[test]
fn test_keys() {
    let (mut write, read) = flashmap::new::<Box<i32>, Box<char>>();

    let mut guard = write.guard();
    guard.insert(Box::new(1), Box::new('a'));
    guard.insert(Box::new(2), Box::new('b'));
    guard.insert(Box::new(3), Box::new('c'));
    drop(guard);

    let keys: Vec<_> = read.guard().keys().map(|k| **k).collect();
    assert_eq!(keys.len(), 3);
    assert!(keys.contains(&1));
    assert!(keys.contains(&2));
    assert!(keys.contains(&3));
}

#[test]
fn test_values() {
    let (mut write, read) = flashmap::new::<Box<i32>, Box<char>>();

    let mut guard = write.guard();
    guard.insert(Box::new(1), Box::new('a'));
    guard.insert(Box::new(2), Box::new('b'));
    guard.insert(Box::new(3), Box::new('c'));
    drop(guard);

    let values: Vec<_> = read.guard().values().map(|v| **v).collect();
    assert_eq!(values.len(), 3);
    assert!(values.contains(&'a'));
    assert!(values.contains(&'b'));
    assert!(values.contains(&'c'));
}

#[test]
fn complex_oplog() {
    let (mut write, read) = flashmap::new::<Box<i32>, Box<i32>>();

    let mut guard = write.guard();
    guard.insert(Box::new(10), Box::new(20));
    guard.insert(Box::new(20), Box::new(40));
    guard.insert(Box::new(40), Box::new(80));
    guard.insert(Box::new(10), Box::new(10));
    guard.replace(Box::new(20), |_| Box::new(20));
    guard.remove(Box::new(20));
    guard.remove(Box::new(40));
    guard.insert(Box::new(40), Box::new(100));
    drop(guard);

    let guard = read.guard();
    assert_eq!(**guard.get(&10).unwrap(), 10);
    assert!(guard.get(&20).is_none());
    assert_eq!(**guard.get(&40).unwrap(), 100);
}

#[test]
#[should_panic]
#[cfg(not(miri))] // This test leaks memory, but that's expected
fn invalid_reclamation() {
    let (mut w1, _r1) = flashmap::new::<Box<i32>, Box<i32>>();
    let (w2, _r2) = flashmap::new::<Box<i32>, Box<i32>>();

    let mut guard = w1.guard();
    guard.insert(Box::new(1), Box::new(1));
    let leaked = guard.remove(Box::new(1)).map(Evicted::leak).unwrap();
    drop(guard);
    w2.reclaim_one(leaked);
}

#[test]
#[should_panic]
#[cfg(not(miri))] // This test leaks memory, but that's expected
fn invalid_lazy_drop() {
    let (mut w1, _r1) = flashmap::new::<Box<i32>, Box<i32>>();
    let (mut w2, _r2) = flashmap::new::<Box<i32>, Box<i32>>();

    let mut guard = w1.guard();
    guard.insert(Box::new(1), Box::new(1));
    let leaked = guard.remove(Box::new(1)).map(Evicted::leak).unwrap();
    w2.guard().drop_lazily(leaked);
    drop(guard);
}

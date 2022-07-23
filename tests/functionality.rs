//! Tests the functionality of the map. IE, these test focus on making sure map operations like
//! insert/remove etc. work as expected.

mod util;

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

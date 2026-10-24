//! Results that depend only on their input, kept for reuse.
//!
//! An analysis runs four presets on copies of one document. Three decode the same images, and all
//! four rebuild and recompress the same fonts, with identical results. Here each such result is
//! worked out once: the others wait for it instead of repeating the work. Outputs are
//! byte-for-byte what they would be without this.
//!
//! Keys are two 64-bit hashes of the input and its length, with keys chosen at random when the
//! process starts, so a crafted file cannot make two inputs collide. A memo has a memory cap (the
//! oldest results go first), and a result can be dropped once it has been taken a set number of
//! times: an analysis's decoded images are needed by its three image presets and then by nobody.

use std::collections::{BTreeMap, VecDeque};
use std::hash::{BuildHasher, Hasher, RandomState};
use std::sync::{Arc, Mutex, OnceLock};

pub type Key = (u64, u64, usize);

fn seeds() -> &'static (RandomState, RandomState) {
    static SEEDS: OnceLock<(RandomState, RandomState)> = OnceLock::new();
    SEEDS.get_or_init(|| (RandomState::new(), RandomState::new()))
}

/// The key for a result that depends on these parts, in this order.
pub fn key(parts: &[&[u8]]) -> Key {
    let (a, b) = seeds();
    let (mut ha, mut hb) = (a.build_hasher(), b.build_hasher());
    let mut len = 0;
    for p in parts {
        ha.write_usize(p.len());
        ha.write(p);
        hb.write_usize(p.len());
        hb.write(p);
        len += p.len();
    }
    (ha.finish(), hb.finish(), len)
}

pub struct Memo<V> {
    state: Mutex<State<V>>,
    cap: usize,
    /// Times a result is taken before it is dropped; 0 keeps it until the cap pushes it out.
    uses: usize,
}

type Cell<V> = Arc<OnceLock<Option<V>>>;

struct Entry<V> {
    cell: Cell<V>,
    taken: usize,
    /// What it costs to keep, once worked out.
    cost: usize,
}

struct State<V> {
    entries: BTreeMap<Key, Entry<V>>,
    order: VecDeque<Key>,
    bytes: usize,
}

impl<V: Clone> Memo<V> {
    pub const fn new(cap: usize, uses: usize) -> Self {
        Memo { state: Mutex::new(State { entries: BTreeMap::new(), order: VecDeque::new(), bytes: 0 }), cap, uses }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State<V>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The result for `key`: remembered, being worked out by another thread (then this one
    /// waits), or worked out here with `work`. `size` is what a result costs to keep.
    pub fn get_or(&self, key: Key, size: impl Fn(&V) -> usize, work: impl FnOnce() -> Option<V>) -> Option<V> {
        let cell = {
            let mut s = self.lock();
            let e = s.entries.entry(key).or_insert_with(|| Entry { cell: Arc::new(OnceLock::new()), taken: 0, cost: 0 });
            e.taken += 1;
            e.cell.clone()
        };
        let mut computed = false;
        let value = cell
            .get_or_init(|| {
                computed = true;
                work()
            })
            .clone();
        let mut s = self.lock();
        if computed {
            let cost = value.as_ref().map(&size).unwrap_or(0) + 64;
            if let Some(e) = s.entries.get_mut(&key) {
                e.cost = cost;
            }
            s.bytes += cost;
            s.order.push_back(key);
        }
        // Drop it when everyone who needs it has it, then the oldest while over the cap.
        let done = self.uses > 0 && s.entries.get(&key).is_some_and(|e| e.taken >= self.uses);
        if done {
            s.remove(&key);
        }
        while s.bytes > self.cap {
            let Some(old) = s.order.front().copied() else { break };
            s.remove(&old);
        }
        value
    }
}

impl<V> State<V> {
    fn remove(&mut self, key: &Key) {
        if let Some(e) = self.entries.remove(key) {
            self.bytes -= e.cost;
            if let Some(i) = self.order.iter().position(|k| k == key) {
                self.order.remove(i);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn works_out_once_and_keeps_within_its_cap() {
        let memo: Memo<Vec<u8>> = Memo::new(1000, 0);
        let runs = AtomicUsize::new(0);
        let work = || {
            runs.fetch_add(1, Ordering::Relaxed);
            Some(vec![7u8; 300])
        };
        let k = key(&[b"font", b"abc"]);
        assert_eq!(memo.get_or(k, Vec::len, work).unwrap().len(), 300);
        assert_eq!(memo.get_or(k, Vec::len, work).unwrap().len(), 300);
        assert_eq!(runs.load(Ordering::Relaxed), 1);
        // Different parts, or the same bytes split differently, are different keys.
        assert_ne!(key(&[b"font", b"abc"]), key(&[b"fontabc"]));
        assert_ne!(key(&[b"font", b"abc"]), key(&[b"font", b"abd"]));
        // Filling past the cap drops the oldest.
        for i in 0..5u8 {
            memo.get_or(key(&[&[i]]), Vec::len, work);
        }
        assert!(memo.lock().bytes <= 1000);
        assert!(!memo.lock().entries.contains_key(&k));
    }

    #[test]
    fn a_result_is_dropped_once_everyone_has_it() {
        let memo: Memo<Vec<u8>> = Memo::new(1 << 20, 3);
        let k = key(&[b"image"]);
        for _ in 0..2 {
            memo.get_or(k, Vec::len, || Some(vec![0; 500]));
        }
        assert!(memo.lock().bytes > 0, "kept while one more run needs it");
        memo.get_or(k, Vec::len, || Some(vec![0; 500]));
        assert_eq!(memo.lock().bytes, 0, "dropped after the third");
        assert!(memo.lock().entries.is_empty() && memo.lock().order.is_empty());
        // With no room at all, nothing is kept, and the result still comes back.
        let none: Memo<Vec<u8>> = Memo::new(0, 3);
        assert_eq!(none.get_or(k, Vec::len, || Some(vec![1; 10])), Some(vec![1; 10]));
        assert_eq!(none.lock().bytes, 0);
    }

    #[test]
    fn threads_asking_at_once_share_one_result() {
        let memo: Memo<u32> = Memo::new(1 << 20, 0);
        let runs = AtomicUsize::new(0);
        let k = key(&[b"image"]);
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    memo.get_or(k, |_| 4, || {
                        runs.fetch_add(1, Ordering::Relaxed);
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        Some(42)
                    })
                });
            }
        });
        assert_eq!(runs.load(Ordering::Relaxed), 1);
    }
}

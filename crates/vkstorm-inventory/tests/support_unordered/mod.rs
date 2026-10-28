//! The iteration order of the viewer's `std::unordered_map<LLUUID, T>`
//! (`LLInventoryModel`'s `mCategoryMap` and `mItemMap` since upstream
//! 28fee038e0), for the test harnesses that stand in for the C++ model.
//!
//! The crate takes map order from its caller; in the replay tests the
//! harness is that caller, so it reproduces what the C++ container would
//! yield after the same inserts and erases. The order depends on
//! `std::hash<LLUUID>` (`llcommon/lluuid.h`) and on the standard library's
//! table:
//!
//! - **MSVC** (`<xhash>`, VS 2026 14.51): one doubly linked list of all
//!   elements plus, per bucket, the first and last element of its run;
//!   a power-of-two bucket count starting at 8, bucket = hash & mask. A new
//!   key goes before the first element of its bucket's run (`_Find_last`
//!   finds no equal key and returns the bucket's low end), or at the list's
//!   end when its bucket is empty. Before an insert that would exceed load
//!   factor 1.0 the table grows 8x while under 512 buckets, else to the
//!   next power of two; the rehash (`_Forced_rehash`) walks the list in
//!   order and moves each element before the first already-placed element
//!   of its new bucket, or leaves it in place when that bucket is still
//!   empty.
//! - **libstdc++**: not reproduced yet (prime bucket counts, a singly
//!   linked list with buckets pointing at the node before their first
//!   element). [`Library::Libstdcxx`] panics until it is written and checked
//!   against a Linux recording.

#![allow(dead_code)]

use std::collections::HashMap;

pub type Uuid = [u8; 16];

/// The standard library whose container order to reproduce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Library {
    Msvc,
    Libstdcxx,
}

impl Library {
    /// The library of this build, which the viewer beside it also uses.
    pub fn native() -> Library {
        if cfg!(target_env = "msvc") { Library::Msvc } else { Library::Libstdcxx }
    }
}

/// `std::hash<LLUUID>`: the golden-ratio hash with avalanche mixing over
/// the two little-endian 64-bit halves.
pub fn uuid_hash(id: &Uuid) -> u64 {
    let mut h: u64 = 0;
    for i in (0..16).step_by(8) {
        let chunk = u64::from_le_bytes(id[i..i + 8].try_into().unwrap());
        h ^= chunk.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ (h >> 31) ^ (h >> 47);
    }
    h
}

/// The key order of a `std::unordered_map<LLUUID, T>` under inserts and
/// erases, kept the way MSVC keeps it: a doubly linked list (node 0 is the
/// head sentinel) and each bucket's first and last node.
#[derive(Clone, Debug)]
pub struct UnorderedOrder {
    library: Library,
    ids: Vec<Uuid>,
    prev: Vec<usize>,
    next: Vec<usize>,
    hashes: Vec<u64>,
    free: Vec<usize>,
    index: HashMap<Uuid, usize>,
    /// (first, last) node of each bucket; `HEAD` when empty.
    buckets: Vec<(usize, usize)>,
}

const HEAD: usize = 0;

impl UnorderedOrder {
    pub fn new(library: Library) -> UnorderedOrder {
        if library == Library::Libstdcxx {
            panic!("the libstdc++ unordered_map order is not reproduced yet (support_unordered)");
        }
        UnorderedOrder {
            library,
            ids: vec![[0; 16]],
            prev: vec![HEAD],
            next: vec![HEAD],
            hashes: vec![0],
            free: Vec::new(),
            index: HashMap::new(),
            buckets: vec![(HEAD, HEAD); 8],
        }
    }

    pub fn native() -> UnorderedOrder {
        UnorderedOrder::new(Library::native())
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn contains(&self, id: &Uuid) -> bool {
        self.index.contains_key(id)
    }

    fn mask(&self) -> u64 {
        self.buckets.len() as u64 - 1
    }

    fn unlink(&mut self, n: usize) {
        let (p, x) = (self.prev[n], self.next[n]);
        self.next[p] = x;
        self.prev[x] = p;
    }

    fn link_before(&mut self, n: usize, before: usize) {
        let after = self.prev[before];
        self.prev[n] = after;
        self.next[n] = before;
        self.next[after] = n;
        self.prev[before] = n;
    }

    /// `map[id]` / `try_emplace`: a new key is placed; an existing one keeps
    /// its place.
    pub fn insert(&mut self, id: Uuid) {
        if self.contains(&id) {
            return;
        }
        let new_size = self.len() + 1;
        let buckets = self.buckets.len() as u64;
        if (new_size as f32) / (buckets as f32) > 1.0 {
            let required = ((new_size as f32).ceil() as u64).max(8);
            let grown = if buckets >= required {
                buckets
            } else if buckets < 512 && buckets * 8 >= required {
                buckets * 8
            } else {
                required
            };
            self.rehash(grown.next_power_of_two() as usize);
        }
        let h = uuid_hash(&id);
        let n = match self.free.pop() {
            Some(n) => {
                self.ids[n] = id;
                self.hashes[n] = h;
                n
            }
            None => {
                self.ids.push(id);
                self.prev.push(HEAD);
                self.next.push(HEAD);
                self.hashes.push(h);
                self.ids.len() - 1
            }
        };
        self.index.insert(id, n);
        let b = (h & self.mask()) as usize;
        let (lo, _) = self.buckets[b];
        if lo == HEAD {
            self.link_before(n, HEAD);
            self.buckets[b] = (n, n);
        } else {
            self.link_before(n, lo);
            self.buckets[b].0 = n;
        }
    }

    /// `erase(id)`: the others keep their order.
    pub fn erase(&mut self, id: &Uuid) {
        let Some(n) = self.index.remove(id) else { return };
        let b = (self.hashes[n] & self.mask()) as usize;
        let (lo, hi) = self.buckets[b];
        if lo == n && hi == n {
            self.buckets[b] = (HEAD, HEAD);
        } else if lo == n {
            self.buckets[b].0 = self.next[n];
        } else if hi == n {
            self.buckets[b].1 = self.prev[n];
        }
        self.unlink(n);
        self.free.push(n);
    }

    /// `clear()`: MSVC keeps the bucket count.
    pub fn clear(&mut self) {
        for k in self.order() {
            self.erase(&k);
        }
    }

    /// The keys in iteration order.
    pub fn order(&self) -> Vec<Uuid> {
        let mut out = Vec::with_capacity(self.len());
        let mut n = self.next[HEAD];
        while n != HEAD {
            out.push(self.ids[n]);
            n = self.next[n];
        }
        out
    }

    /// `_Forced_rehash`: walks the list in order; each element moves before
    /// the first already-placed element of its new bucket, or stays where it
    /// is when that bucket is still empty.
    fn rehash(&mut self, buckets: usize) {
        self.buckets = vec![(HEAD, HEAD); buckets];
        let mask = buckets as u64 - 1;
        let mut n = self.next[HEAD];
        while n != HEAD {
            let following = self.next[n];
            let b = (self.hashes[n] & mask) as usize;
            let (lo, _) = self.buckets[b];
            if lo == HEAD {
                self.buckets[b] = (n, n);
            } else {
                self.unlink(n);
                self.link_before(n, lo);
                self.buckets[b].0 = n;
            }
            n = following;
        }
    }
}

impl Default for UnorderedOrder {
    /// The order of this build's standard library.
    fn default() -> Self {
        UnorderedOrder::native()
    }
}

/// A key-to-value map that also keeps the C++ container's order.
#[derive(Clone, Debug)]
pub struct OrderedMap<V> {
    pub order: UnorderedOrder,
    pub values: HashMap<Uuid, V>,
}

impl<V> OrderedMap<V> {
    pub fn new(library: Library) -> Self {
        OrderedMap { order: UnorderedOrder::new(library), values: HashMap::new() }
    }

    pub fn insert(&mut self, id: Uuid, v: V) {
        self.order.insert(id);
        self.values.insert(id, v);
    }

    pub fn remove(&mut self, id: &Uuid) -> Option<V> {
        self.order.erase(id);
        self.values.remove(id)
    }

    pub fn keys(&self) -> Vec<Uuid> {
        self.order.order()
    }
}

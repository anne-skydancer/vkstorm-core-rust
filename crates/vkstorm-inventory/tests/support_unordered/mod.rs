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
//! - **libstdc++** (`bits/hashtable.h`, `src/c++11/hashtable_c++0x.cc`,
//!   `src/shared/hashtable-aux.cc`; the same in GCC 14 and 15): one singly
//!   linked list of all elements, each bucket's run contiguous, and
//!   bucket = hash % bucket count. A new key goes before the first element
//!   of its bucket's run (`_M_insert_bucket_begin`), or at the front of the
//!   list when its bucket is empty. The table starts with one bucket; the
//!   bucket counts come from `_Prime_rehash_policy` (`_M_need_rehash`,
//!   `_M_next_bkt`, `__prime_list`, load factor 1.0): 13 at the first
//!   insert, then the first listed prime at least twice the count once the
//!   size would exceed it. The rehash (`_M_rehash` for unique keys) walks
//!   the list in order and places each element by the same rule in the new
//!   table. `clear()` keeps the bucket count and the policy's state.

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
        if cfg!(target_env = "msvc") {
            Library::Msvc
        } else {
            Library::Libstdcxx
        }
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
    /// libstdc++: `_Prime_rehash_policy::_M_next_resize`.
    next_resize: u64,
}

const HEAD: usize = 0;

impl UnorderedOrder {
    pub fn new(library: Library) -> UnorderedOrder {
        let buckets = match library {
            Library::Msvc => 8,
            Library::Libstdcxx => 1,
        };
        UnorderedOrder {
            library,
            ids: vec![[0; 16]],
            prev: vec![HEAD],
            next: vec![HEAD],
            hashes: vec![0],
            free: Vec::new(),
            index: HashMap::new(),
            buckets: vec![(HEAD, HEAD); buckets],
            next_resize: 0,
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

    fn bucket(&self, hash: u64) -> usize {
        Self::bucket_of(self.library, hash, self.buckets.len())
    }

    fn bucket_of(library: Library, hash: u64, buckets: usize) -> usize {
        match library {
            Library::Msvc => (hash & (buckets as u64 - 1)) as usize,
            Library::Libstdcxx => (hash % buckets as u64) as usize,
        }
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
        match self.library {
            Library::Msvc => {
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
            }
            Library::Libstdcxx => {
                if let Some(buckets) =
                    self.gnu_need_rehash(self.buckets.len() as u64, self.len() as u64, 1)
                {
                    self.rehash(buckets as usize);
                }
            }
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
        self.place(n);
    }

    /// Links node `n` (not linked) into the list by its library's rule:
    /// before the first element of its bucket, else (an empty bucket) at the
    /// list's end on MSVC and at its front on libstdc++.
    fn place(&mut self, n: usize) {
        let b = self.bucket(self.hashes[n]);
        let (lo, _) = self.buckets[b];
        if lo == HEAD {
            let at = match self.library {
                Library::Msvc => HEAD,
                Library::Libstdcxx => self.next[HEAD],
            };
            self.link_before(n, at);
            self.buckets[b] = (n, n);
        } else {
            self.link_before(n, lo);
            self.buckets[b].0 = n;
        }
    }

    /// `erase(id)`: the others keep their order.
    pub fn erase(&mut self, id: &Uuid) {
        let Some(n) = self.index.remove(id) else {
            return;
        };
        let b = self.bucket(self.hashes[n]);
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

    /// `clear()`: both keep the bucket count (libstdc++ also its rehash
    /// policy's state).
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

    /// MSVC `_Forced_rehash`: walks the list in order; each element moves
    /// before the first already-placed element of its new bucket, or stays
    /// where it is when that bucket is still empty. libstdc++ `_M_rehash`:
    /// walks the list in order and places each element in the new, empty
    /// list by the insert rule.
    fn rehash(&mut self, buckets: usize) {
        self.buckets = vec![(HEAD, HEAD); buckets];
        if self.library == Library::Libstdcxx {
            let mut nodes = Vec::with_capacity(self.len());
            let mut n = self.next[HEAD];
            while n != HEAD {
                nodes.push(n);
                n = self.next[n];
            }
            self.next[HEAD] = HEAD;
            self.prev[HEAD] = HEAD;
            for n in nodes {
                self.place(n);
            }
            return;
        }
        let mut n = self.next[HEAD];
        while n != HEAD {
            let following = self.next[n];
            let b = Self::bucket_of(self.library, self.hashes[n], buckets);
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

impl UnorderedOrder {
    /// `_Prime_rehash_policy::_M_need_rehash` (max load factor 1.0): the new
    /// bucket count when inserting `n_ins` elements into `n_elt` in `n_bkt`
    /// buckets needs a rehash.
    fn gnu_need_rehash(&mut self, n_bkt: u64, n_elt: u64, n_ins: u64) -> Option<u64> {
        if n_elt + n_ins <= self.next_resize {
            return None;
        }
        // A policy that has allocated nothing yet starts at 11.
        let min_bkts = (n_elt + n_ins).max(if self.next_resize != 0 { 0 } else { 11 }) as f64;
        if min_bkts >= n_bkt as f64 {
            return Some(self.gnu_next_bkt((min_bkts.floor() as u64 + 1).max(n_bkt * 2)));
        }
        self.next_resize = n_bkt;
        None
    }

    /// `_Prime_rehash_policy::_M_next_bkt` (max load factor 1.0).
    fn gnu_next_bkt(&mut self, n: u64) -> u64 {
        const FAST_BKT: [u8; 14] = [2, 2, 2, 3, 5, 5, 7, 7, 11, 11, 11, 11, 13, 13];
        if n < FAST_BKT.len() as u64 {
            if n == 0 {
                return 1;
            }
            self.next_resize = FAST_BKT[n as usize] as u64;
            return FAST_BKT[n as usize] as u64;
        }
        // lower_bound over [__prime_list + 6, __last_prime), __last_prime
        // being the entry before the sentinel.
        let last = GNU_PRIME_LIST.len() - 2;
        let i = 6 + GNU_PRIME_LIST[6..last].partition_point(|&p| p < n);
        self.next_resize = if i == last {
            u64::MAX
        } else {
            GNU_PRIME_LIST[i]
        };
        GNU_PRIME_LIST[i]
    }
}

/// libstdc++'s `std::__detail::__prime_list` for a 64-bit `unsigned long`
/// (`src/shared/hashtable-aux.cc`: 256 + 48 entries and the sentinel).
const GNU_PRIME_LIST: [u64; 305] = [
    2,
    3,
    5,
    7,
    11,
    13,
    17,
    19,
    23,
    29,
    31,
    37,
    41,
    43,
    47,
    53,
    59,
    61,
    67,
    71,
    73,
    79,
    83,
    89,
    97,
    103,
    109,
    113,
    127,
    137,
    139,
    149,
    157,
    167,
    179,
    193,
    199,
    211,
    227,
    241,
    257,
    277,
    293,
    313,
    337,
    359,
    383,
    409,
    439,
    467,
    503,
    541,
    577,
    619,
    661,
    709,
    761,
    823,
    887,
    953,
    1031,
    1109,
    1193,
    1289,
    1381,
    1493,
    1613,
    1741,
    1879,
    2029,
    2179,
    2357,
    2549,
    2753,
    2971,
    3209,
    3469,
    3739,
    4027,
    4349,
    4703,
    5087,
    5503,
    5953,
    6427,
    6949,
    7517,
    8123,
    8783,
    9497,
    10273,
    11113,
    12011,
    12983,
    14033,
    15173,
    16411,
    17749,
    19183,
    20753,
    22447,
    24281,
    26267,
    28411,
    30727,
    33223,
    35933,
    38873,
    42043,
    45481,
    49201,
    53201,
    57557,
    62233,
    67307,
    72817,
    78779,
    85229,
    92203,
    99733,
    107897,
    116731,
    126271,
    136607,
    147793,
    159871,
    172933,
    187091,
    202409,
    218971,
    236897,
    256279,
    277261,
    299951,
    324503,
    351061,
    379787,
    410857,
    444487,
    480881,
    520241,
    562841,
    608903,
    658753,
    712697,
    771049,
    834181,
    902483,
    976369,
    1056323,
    1142821,
    1236397,
    1337629,
    1447153,
    1565659,
    1693859,
    1832561,
    1982627,
    2144977,
    2320627,
    2510653,
    2716249,
    2938679,
    3179303,
    3439651,
    3721303,
    4026031,
    4355707,
    4712381,
    5098259,
    5515729,
    5967347,
    6456007,
    6984629,
    7556579,
    8175383,
    8844859,
    9569143,
    10352717,
    11200489,
    12117689,
    13109983,
    14183539,
    15345007,
    16601593,
    17961079,
    19431899,
    21023161,
    22744717,
    24607243,
    26622317,
    28802401,
    31160981,
    33712729,
    36473443,
    39460231,
    42691603,
    46187573,
    49969847,
    54061849,
    58488943,
    63278561,
    68460391,
    74066549,
    80131819,
    86693767,
    93793069,
    101473717,
    109783337,
    118773397,
    128499677,
    139022417,
    150406843,
    162723577,
    176048909,
    190465427,
    206062531,
    222936881,
    241193053,
    260944219,
    282312799,
    305431229,
    330442829,
    357502601,
    386778277,
    418451333,
    452718089,
    489790921,
    529899637,
    573292817,
    620239453,
    671030513,
    725980837,
    785430967,
    849749479,
    919334987,
    994618837,
    1076067617,
    1164186217,
    1259520799,
    1362662261,
    1474249943,
    1594975441,
    1725587117,
    1866894511,
    2019773507,
    2185171673,
    2364114217,
    2557710269,
    2767159799,
    2993761039,
    3238918481,
    3504151727,
    3791104843,
    4101556399,
    4294967291,
    6442450933,
    8589934583,
    12884901857,
    17179869143,
    25769803693,
    34359738337,
    51539607367,
    68719476731,
    103079215087,
    137438953447,
    206158430123,
    274877906899,
    412316860387,
    549755813881,
    824633720731,
    1099511627689,
    1649267441579,
    2199023255531,
    3298534883309,
    4398046511093,
    6597069766607,
    8796093022151,
    13194139533241,
    17592186044399,
    26388279066581,
    35184372088777,
    52776558133177,
    70368744177643,
    105553116266399,
    140737488355213,
    211106232532861,
    281474976710597,
    562949953421231,
    1125899906842597,
    2251799813685119,
    4503599627370449,
    9007199254740881,
    18014398509481951,
    36028797018963913,
    72057594037927931,
    144115188075855859,
    288230376151711717,
    576460752303423433,
    1152921504606846883,
    2305843009213693951,
    4611686018427387847,
    9223372036854775783,
    18446744073709551557,
    18446744073709551557,
];

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
        OrderedMap {
            order: UnorderedOrder::new(library),
            values: HashMap::new(),
        }
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

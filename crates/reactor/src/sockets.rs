//! Opt-in sole registry metadata owner. Socket identities stay Native-owned.
//! Qualified Apple order and unqualified GNU12 candidate order use one node store.
//! Native Linux ON stays refused until actual platform qualification.
use crate::owner::{publish as publish_owner, release, try_owner};
#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
use std::collections::HashMap;
#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
use std::hash::{BuildHasherDefault, DefaultHasher};
use std::ptr;

const NONE: usize = u32::MAX as usize;
fn link(value: u32) -> usize {
    value as usize
}
#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
type Index = HashMap<i32, usize, BuildHasherDefault<DefaultHasher>>;
struct Node {
    key: i32,
    token: usize,
    next: u32,
    previous: u32,
    generation: u64,
    live: bool,
}
pub struct ReactorSocketStore<const GNU_CANDIDATE: bool> {
    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    index: Index,
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    count: usize,
    nodes: Vec<Node>,
    head: usize,
    tail: usize,
    free: usize,
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    buckets: Vec<usize>,
}
pub type ReactorSockets = ReactorSocketStore<{ cfg!(target_os = "linux") }>;
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct ReactorSocketEntry {
    pub fd: i32,
    pub token: usize,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ReactorSocketCursor {
    pub current: usize,
    pub generation: u64,
    pub started: u32,
    pub done: u32,
}
pub struct ReactorSocketSnapshot {
    entries: Vec<ReactorSocketEntry>,
}
impl<const GNU_CANDIDATE: bool> ReactorSocketStore<GNU_CANDIDATE> {
    fn new() -> Self {
        Self {
            #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
            index: Index::default(),
            #[cfg(any(target_vendor = "apple", target_os = "linux"))]
            count: 0,
            nodes: Vec::new(),
            head: NONE,
            tail: NONE,
            free: NONE,
            #[cfg(any(target_vendor = "apple", target_os = "linux"))]
            buckets: Vec::new(),
        }
    }
    fn len(&self) -> usize {
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        {
            self.count
        }
        #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
        {
            self.index.len()
        }
    }
    fn find(&self, fd: i32) -> Option<usize> {
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        {
            if self.buckets.is_empty() {
                return None;
            }
            let bucket = fd as usize % self.buckets.len();
            let mut id = self.buckets[bucket];
            while id != NONE && self.nodes[id].key as usize % self.buckets.len() == bucket {
                if self.nodes[id].key == fd {
                    return Some(id);
                }
                id = link(self.nodes[id].next);
            }
            None
        }
        #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
        {
            self.index.get(&fd).copied()
        }
    }
    fn put(&mut self, fd: i32, token: usize) -> Result<(), i32> {
        if let Some(id) = self.find(fd) {
            self.nodes[id].token = token;
            return Ok(());
        }
        #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
        self.index.try_reserve(1).map_err(|_| -2)?;
        let id = if self.free == NONE {
            // A real socket registry cannot exhaust the u32 slot domain.
            if self.nodes.len() == u32::MAX as usize {
                return Err(-2);
            }
            self.nodes.try_reserve(1).map_err(|_| -2)?;
            self.nodes.len()
        } else {
            self.free
        };
        let generation = if id == self.nodes.len() {
            1
        } else {
            self.nodes[id].generation.checked_add(1).ok_or(-1)?
        };
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        self.prepare_order()?;
        let node = Node {
            key: fd,
            token,
            next: u32::MAX,
            previous: self.tail as u32,
            generation,
            live: true,
        };
        if id == self.nodes.len() {
            self.nodes.push(node);
        } else {
            self.free = link(self.nodes[id].next);
            self.nodes[id] = node;
        }
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        self.link_order(id);
        #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
        {
            if self.tail == NONE {
                self.head = id;
            } else {
                self.nodes[self.tail].next = id as u32;
            }
            self.tail = id;
        }
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        {
            self.count += 1;
        }
        #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
        self.index.insert(fd, id);
        Ok(())
    }
    fn erase(&mut self, fd: i32) {
        let Some(id) = self.find(fd) else {
            return;
        };
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        {
            self.count -= 1;
        }
        #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
        self.index.remove(&fd);
        let (previous, next) = (link(self.nodes[id].previous), link(self.nodes[id].next));
        #[cfg(any(target_vendor = "apple", target_os = "linux"))]
        {
            let bucket = fd as usize % self.buckets.len();
            if self.buckets[bucket] == id {
                self.buckets[bucket] = if next != NONE
                    && self.nodes[next].key as usize % self.buckets.len() == bucket
                {
                    next
                } else {
                    NONE
                };
            }
        }
        if previous == NONE {
            self.head = next;
        } else {
            self.nodes[previous].next = next as u32;
        }
        if next == NONE {
            self.tail = previous;
        } else {
            self.nodes[next].previous = previous as u32;
        }
        self.nodes[id].live = false;
        self.nodes[id].token = 0;
        self.nodes[id].next = self.free as u32;
        self.free = id;
    }
    fn next(&self, cursor: &mut ReactorSocketCursor) -> Result<Option<ReactorSocketEntry>, i32> {
        if cursor.done != 0 {
            return Ok(None);
        }
        let id = if cursor.started == 0 {
            self.head
        } else {
            let Some(current) = self.nodes.get(cursor.current) else {
                return Err(-1);
            };
            // Current-node deletion invalidates a Native iterator too. Never follow a reused slot.
            if !current.live || current.generation != cursor.generation {
                return Err(-1);
            }
            link(current.next)
        };
        if id == NONE {
            return finished(cursor);
        }
        let node = &self.nodes[id];
        cursor.current = id;
        cursor.generation = node.generation;
        cursor.started = 1;
        Ok(Some(ReactorSocketEntry {
            fd: node.key,
            token: node.token,
        }))
    }
}

#[cold]
fn finished(cursor: &mut ReactorSocketCursor) -> Result<Option<ReactorSocketEntry>, i32> {
    cursor.done = 1;
    Ok(None)
}

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
impl<const GNU_CANDIDATE: bool> ReactorSocketStore<GNU_CANDIDATE> {
    // Source-bound libc++210106 ABI1 unique-map order. Bucket heads are subordinate
    // Rust node ordinals; no Native container, iterator or pointer layout is used.
    fn rehash_order(&mut self, count: usize) -> Result<(), i32> {
        let mut buckets = Vec::new();
        buckets.try_reserve_exact(count).map_err(|_| -2)?;
        buckets.resize(count, NONE);
        if GNU_CANDIDATE {
            // GNU unique rehash rebuilds every old node through bucket-head
            // insertion, including the first old contiguous run.
            let mut current = self.head;
            self.head = NONE;
            self.tail = NONE;
            while current != NONE {
                let next = link(self.nodes[current].next);
                Self::link_into(
                    &mut self.nodes,
                    &mut self.head,
                    &mut self.tail,
                    &mut buckets,
                    current,
                );
                current = next;
            }
        } else if self.head != NONE {
            let mut previous = self.head;
            let mut previous_bucket = self.nodes[previous].key as usize % count;
            buckets[previous_bucket] = previous;
            loop {
                let current = link(self.nodes[previous].next);
                if current == NONE {
                    break;
                }
                let bucket = self.nodes[current].key as usize % count;
                if bucket == previous_bucket {
                    previous = current;
                } else if buckets[bucket] == NONE {
                    buckets[bucket] = current;
                    previous = current;
                    previous_bucket = bucket;
                } else {
                    // A later run joins its earlier bucket at that bucket's head.
                    let next = link(self.nodes[current].next);
                    self.nodes[previous].next = next as u32;
                    if next == NONE {
                        self.tail = previous;
                    } else {
                        self.nodes[next].previous = previous as u32;
                    }
                    let head = buckets[bucket];
                    let before = link(self.nodes[head].previous);
                    self.nodes[current].next = head as u32;
                    self.nodes[current].previous = before as u32;
                    self.nodes[head].previous = current as u32;
                    if before == NONE {
                        self.head = current;
                    } else {
                        self.nodes[before].next = current as u32;
                    }
                    buckets[bucket] = current;
                }
            }
        }
        self.buckets = buckets;
        Ok(())
    }
    fn prepare_order(&mut self) -> Result<(), i32> {
        let count = self.len().checked_add(1).ok_or(-2)?;
        let buckets = self.buckets.len();
        if GNU_CANDIDATE {
            // Candidate default _Prime_rehash_policy, load factor1. There is
            // no public reserve/rehash entrypoint on this selected registry.
            // With no reserve/load-factor mutation, the cached threshold is
            // determined by the current bucket policy. Preserve GNU's double
            // conversion rather than assuming threshold == bucket count.
            let threshold = if buckets == 0 {
                0
            } else {
                gnu_resize_threshold(buckets)
            };
            if count > threshold {
                let minimum = count.max(if threshold == 0 { 11 } else { 0 }) as f64;
                if minimum >= buckets.max(1) as f64 {
                    let requested =
                        ((minimum.floor() + 1.0) as usize).max(buckets.max(1).wrapping_mul(2));
                    self.rehash_order(gnu_bucket_count(requested)?)?;
                }
            }
            return Ok(());
        }
        // Native comparison promotes its size to float at max_load_factor1.
        if buckets == 0 || count as f32 > buckets as f32 {
            let growth = buckets
                .checked_mul(2)
                .and_then(|n| {
                    n.checked_add(usize::from(!(buckets > 2 && buckets.is_power_of_two())))
                })
                .ok_or(-2)?;
            let requested = growth.max((count as f32).ceil() as usize);
            let size = if requested == 1 {
                2
            } else if requested.is_power_of_two() {
                requested
            } else {
                next_prime(requested)?
            };
            self.rehash_order(size)?;
        }
        Ok(())
    }
    fn link_order(&mut self, id: usize) {
        Self::link_into(
            &mut self.nodes,
            &mut self.head,
            &mut self.tail,
            &mut self.buckets,
            id,
        );
    }
    fn link_into(
        nodes: &mut [Node],
        head: &mut usize,
        tail: &mut usize,
        buckets: &mut [usize],
        id: usize,
    ) {
        let bucket = nodes[id].key as usize % buckets.len();
        let old_head = buckets[bucket];
        let next = if old_head == NONE { *head } else { old_head };
        let previous = if old_head == NONE {
            NONE
        } else {
            link(nodes[old_head].previous)
        };
        nodes[id].next = next as u32;
        nodes[id].previous = previous as u32;
        if previous == NONE {
            *head = id;
        } else {
            nodes[previous].next = id as u32;
        }
        if next == NONE {
            *tail = id;
        } else {
            nodes[next].previous = id as u32;
        }
        buckets[bucket] = id;
    }
    fn snapshot_entries(&self) -> Result<Vec<ReactorSocketEntry>, i32> {
        let mut entries = Vec::new();
        entries.try_reserve_exact(self.len()).map_err(|_| -2)?;
        let copied;
        let traversal = if GNU_CANDIDATE {
            // GNU _M_assign clones source next-links without reinsertion.
            self
        } else {
            copied = self.copy_apple_order()?;
            &copied
        };
        let mut cursor = ReactorSocketCursor::default();
        while let Some(entry) = traversal.next(&mut cursor)? {
            entries.push(entry);
        }
        Ok(entries)
    }
    fn copy_apple_order(&self) -> Result<Self, i32> {
        // Native map copy starts empty with the source bucket count and reinserts
        // source traversal. It is not a byte/sequence clone of the live registry.
        let mut copy = Self::new();
        copy.nodes.try_reserve_exact(self.len()).map_err(|_| -2)?;
        copy.rehash_order(self.buckets.len())?;
        let mut id = self.head;
        while id != NONE {
            let node = &self.nodes[id];
            copy.put(node.key, node.token)?;
            id = link(node.next);
        }
        Ok(copy)
    }
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn next_prime(requested: usize) -> Result<usize, i32> {
    let mut candidate = requested | 1;
    loop {
        let mut divisor = 3;
        while divisor <= candidate / divisor && !candidate.is_multiple_of(divisor) {
            divisor += 2;
        }
        if divisor > candidate / divisor {
            return Ok(candidate);
        }
        candidate = candidate.checked_add(2).ok_or(-2)?;
    }
}

// Numerical bucket-policy metadata from GCC12.2.0 src/shared/hashtable-aux.cc.
// Official source candidate SHA68fe03dda8fe9d33ec38b535cd9a750119da0da461b6040117fc114674ce893e.
// This table does not qualify an installed Linux library or enable Native ON.
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn gnu_bucket_count(requested: usize) -> Result<usize, i32> {
    const PRIMES: &[u64] = &[
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
    ];
    let requested = u64::try_from(requested).map_err(|_| -2)?;
    let index = PRIMES.partition_point(|&value| value < requested);
    // Unknown wider-than-table domain refuses; no capacity/failure probe.
    usize::try_from(*PRIMES.get(index).unwrap_or(&PRIMES[PRIMES.len() - 1])).map_err(|_| -2)
}
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
fn gnu_resize_threshold(buckets: usize) -> usize {
    if buckets as u64 == 18446744073709551557 {
        usize::MAX
    } else {
        (buckets as f64).floor() as usize
    }
}

unsafe fn publish(slot: &mut *mut ReactorSockets, fd: i32, token: usize) -> i32 {
    unsafe {
        publish_owner(slot, ReactorSockets::new, |owner| {
            owner.put(fd, token)?;
            Ok((0, true))
        })
    }
    .unwrap_or_else(|status| status)
}
/// Publish/replace an identity, including null. No Socket effect or dereference.
/// # Safety
/// Slot is live/aligned/exclusive; its null/live owner has no concurrent access.
/// All outputs in this module are aligned, writable and disjoint from every input/owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_sockets_put(
    slot: *mut *mut ReactorSockets,
    fd: i32,
    token: usize,
) -> i32 {
    let Some(slot) = (unsafe { slot.as_mut() }) else {
        return -1;
    };
    unsafe { publish(slot, fd, token) }
}
/// Original operator[] semantics: missing key publishes null and changes count.
/// # Safety
/// Same exclusive owner/slot contract as put; output is writable/disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_sockets_get(
    slot: *mut *mut ReactorSockets,
    fd: i32,
    output: *mut usize,
) -> i32 {
    let (Some(slot), Some(output)) = ((unsafe { slot.as_mut() }), (unsafe { output.as_mut() }))
    else {
        return -1;
    };
    *output = 0;
    if let Some(owner) = unsafe { slot.as_ref() }
        && let Some(id) = owner.find(fd)
    {
        *output = owner.nodes[id].token;
        return 0;
    }
    unsafe { publish(slot, fd, 0) }
}
/// # Safety
/// Owner is null/live/readable, no concurrent mutation. Rust never reads a Socket.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_sockets_contains(
    owner: *const ReactorSockets,
    fd: i32,
) -> u32 {
    u32::from(unsafe { owner.as_ref() }.is_some_and(|owner| owner.find(fd).is_some()))
}
/// # Safety
/// Owner is null/live/readable, no concurrent mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_sockets_len(owner: *const ReactorSockets) -> usize {
    unsafe { owner.as_ref() }.map_or(0, |owner| owner.len())
}
/// # Safety
/// Owner is null/live/exclusive. Erases metadata only; never drops the token.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_sockets_erase(owner: *mut ReactorSockets, fd: i32) {
    if let Some(owner) = unsafe { owner.as_mut() } {
        owner.erase(fd);
    }
}
/// Return1 copied entry,0 end,-1 invalid cursor. Resume freshly after Native effects.
/// # Safety
/// Null/live readable owner; exclusive initialized cursor and writable output,
/// mutually disjoint. No concurrent mutation. No Rust borrow survives return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_sockets_next(
    owner: *const ReactorSockets,
    cursor: *mut ReactorSocketCursor,
    output: *mut ReactorSocketEntry,
) -> i32 {
    let (Some(cursor), Some(output)) = ((unsafe { cursor.as_mut() }), (unsafe { output.as_mut() }))
    else {
        return -1;
    };
    next_entry(unsafe { owner.as_ref() }, cursor, output)
}

pub(crate) fn next_entry(
    owner: Option<&ReactorSockets>,
    cursor: &mut ReactorSocketCursor,
    output: &mut ReactorSocketEntry,
) -> i32 {
    *output = ReactorSocketEntry::default();
    if cursor.started > 1 || cursor.done > 1 {
        return -1;
    }
    let Some(owner) = owner else {
        cursor.done = 1;
        return 0;
    };
    match owner.next(cursor) {
        Ok(Some(entry)) => {
            *output = entry;
            1
        }
        Ok(None) => 0,
        Err(status) => status,
    }
}
/// Capture independent key/token membership, without retaining Socket ownership.
/// Apple policy rebuilds copied-map order; fallback insertion policy stays unqualified.
/// # Safety
/// Null/live readable owner, no concurrent mutation; output writable/disjoint,
/// initially null and not holding an existing snapshot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_sockets_snapshot(
    owner: *const ReactorSockets,
    output: *mut *mut ReactorSocketSnapshot,
) -> i32 {
    let Some(output) = (unsafe { output.as_mut() }) else {
        return -1;
    };
    *output = ptr::null_mut();
    let Some(owner) = (unsafe { owner.as_ref() }) else {
        return 0;
    };
    if owner.len() == 0 {
        return 0;
    }
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    let entries = match owner.snapshot_entries() {
        Ok(entries) => entries,
        Err(status) => return status,
    };
    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    let entries = {
        let mut entries = Vec::new();
        if entries.try_reserve_exact(owner.len()).is_err() {
            return -2;
        }
        let mut cursor = ReactorSocketCursor::default();
        loop {
            match owner.next(&mut cursor) {
                Ok(Some(entry)) => entries.push(entry),
                Ok(None) => break,
                Err(status) => return status,
            }
        }
        entries
    };
    let Some(snapshot) = try_owner(|| ReactorSocketSnapshot { entries }) else {
        return -2;
    };
    *output = Box::into_raw(snapshot);
    0
}
/// # Safety
/// Snapshot is null/live/readable, output writable/disjoint. Snapshot is immutable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_sockets_snapshot_entry(
    owner: *const ReactorSocketSnapshot,
    index: usize,
    output: *mut ReactorSocketEntry,
) -> i32 {
    let Some(output) = (unsafe { output.as_mut() }) else {
        return -1;
    };
    snapshot_entry(unsafe { owner.as_ref() }, index, output)
}

pub(crate) fn snapshot_entry(
    owner: Option<&ReactorSocketSnapshot>,
    index: usize,
    output: &mut ReactorSocketEntry,
) -> i32 {
    *output = ReactorSocketEntry::default();
    match owner.and_then(|owner| owner.entries.get(index)) {
        Some(entry) => {
            *output = *entry;
            1
        }
        None => 0,
    }
}
/// # Safety
/// Snapshot is null/live/readable, not concurrently freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_sockets_snapshot_len(
    owner: *const ReactorSocketSnapshot,
) -> usize {
    unsafe { owner.as_ref() }.map_or(0, |owner| owner.entries.len())
}
/// # Safety
/// Slot is exclusive/aligned; null/live owner is not referenced by another call.
/// Frees only Rust metadata. No Native token or callback is dropped.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_sockets_free(slot: *mut *mut ReactorSockets) {
    if let Some(slot) = unsafe { slot.as_mut() } {
        unsafe {
            release(*slot);
        }
        *slot = ptr::null_mut();
    }
}
/// # Safety
/// Null/live snapshot is exclusively owned and no longer used by any cursor.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_sockets_snapshot_free(owner: *mut ReactorSocketSnapshot) {
    unsafe {
        release(owner);
    }
}
/// Source-bound ordering policy identity: Apple libc++210106 ABI1=1, fallback=0.
/// Native capability construction rejects a library whose policy does not match.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_sockets_policy() -> u32 {
    u32::from(cfg!(target_vendor = "apple"))
}
/// Return target C layout facts; unknown selectors return0.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_sockets_layout(kind: u32, field: u32) -> usize {
    match (kind, field) {
        (0, 0) => size_of::<ReactorSocketEntry>(),
        (0, 1) => align_of::<ReactorSocketEntry>(),
        (0, 2) => std::mem::offset_of!(ReactorSocketEntry, fd),
        (0, 3) => std::mem::offset_of!(ReactorSocketEntry, token),
        (1, 0) => size_of::<ReactorSocketCursor>(),
        (1, 1) => align_of::<ReactorSocketCursor>(),
        (1, 2) => std::mem::offset_of!(ReactorSocketCursor, current),
        (1, 3) => std::mem::offset_of!(ReactorSocketCursor, generation),
        (1, 4) => std::mem::offset_of!(ReactorSocketCursor, started),
        (1, 5) => std::mem::offset_of!(ReactorSocketCursor, done),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn membership_null_miss_replace_delete_reuse_and_bounded_metadata() {
        let mut owner = ptr::null_mut();
        let mut token = 99;
        unsafe {
            assert_eq!(swr_v1_reactor_sockets_len(owner), 0);
            assert_eq!(swr_v1_reactor_sockets_get(&mut owner, -7, &mut token), 0);
            assert_eq!(token, 0);
            assert_eq!(swr_v1_reactor_sockets_len(owner), 1);
            assert_eq!(swr_v1_reactor_sockets_contains(owner, -7), 1);
            for key in 0..65 {
                assert_eq!(
                    swr_v1_reactor_sockets_put(&mut owner, key, (key + 100) as usize),
                    0
                );
            }
            assert_eq!(swr_v1_reactor_sockets_put(&mut owner, 1, 777), 0);
            assert_eq!(swr_v1_reactor_sockets_get(&mut owner, 1, &mut token), 0);
            assert_eq!(token, 777);
            for _ in 0..256 {
                swr_v1_reactor_sockets_erase(owner, 1);
                assert_eq!(swr_v1_reactor_sockets_put(&mut owner, 1, 777), 0);
            }
            assert_eq!((*owner).nodes.len(), 66);
            swr_v1_reactor_sockets_erase(owner, 333);
            assert_eq!(swr_v1_reactor_sockets_len(owner), 66);
            swr_v1_reactor_sockets_free(&mut owner);
            assert!(owner.is_null());
        }
    }
    #[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
    #[test]
    fn live_cursor_fresh_values_next_links_and_independent_snapshot() {
        let mut owner = ptr::null_mut();
        let mut snapshot = ptr::null_mut();
        let mut cursor = ReactorSocketCursor::default();
        let mut entry = ReactorSocketEntry::default();
        unsafe {
            for key in [4, 8, 12] {
                assert_eq!(
                    swr_v1_reactor_sockets_put(&mut owner, key, key as usize + 100),
                    0
                );
            }
            assert_eq!(swr_v1_reactor_sockets_snapshot(owner, &mut snapshot), 0);
            assert_eq!(
                swr_v1_reactor_sockets_next(owner, &mut cursor, &mut entry),
                1
            );
            assert_eq!(entry.fd, 4);
            // Inert metadata changes only, no callback/effect/lifetime probe.
            swr_v1_reactor_sockets_erase(owner, 8);
            assert_eq!(swr_v1_reactor_sockets_put(&mut owner, 12, 555), 0);
            assert_eq!(swr_v1_reactor_sockets_put(&mut owner, 16, 666), 0);
            assert_eq!(
                swr_v1_reactor_sockets_next(owner, &mut cursor, &mut entry),
                1
            );
            assert_eq!(entry, ReactorSocketEntry { fd: 12, token: 555 });
            assert_eq!(
                swr_v1_reactor_sockets_next(owner, &mut cursor, &mut entry),
                1
            );
            assert_eq!(entry.fd, 16);
            assert_eq!(
                swr_v1_reactor_sockets_next(owner, &mut cursor, &mut entry),
                0
            );
            assert_eq!(swr_v1_reactor_sockets_snapshot_len(snapshot), 3);
            assert_eq!(
                swr_v1_reactor_sockets_snapshot_entry(snapshot, 1, &mut entry),
                1
            );
            assert_eq!(entry, ReactorSocketEntry { fd: 8, token: 108 });
            swr_v1_reactor_sockets_snapshot_free(snapshot);
            swr_v1_reactor_sockets_free(&mut owner);
        }
    }
    #[cfg(target_vendor = "apple")]
    #[test]
    fn libcxx_live_cursor_and_independently_reinserted_snapshot() {
        let mut owner = ReactorSockets::new();
        for key in [4, 8, 12] {
            owner.put(key, key as usize + 100).unwrap();
        }
        let snapshot = owner.copy_apple_order().unwrap();
        let mut cursor = ReactorSocketCursor::default();
        assert_eq!(owner.next(&mut cursor).unwrap().unwrap().fd, 12);
        owner.erase(8);
        owner.put(4, 555).unwrap();
        owner.put(16, 666).unwrap();
        assert_eq!(
            owner.next(&mut cursor).unwrap().unwrap(),
            ReactorSocketEntry { fd: 4, token: 555 }
        );
        assert!(owner.next(&mut cursor).unwrap().is_none());
        let mut cursor = ReactorSocketCursor::default();
        assert_eq!(snapshot.next(&mut cursor).unwrap().unwrap().fd, 4);
        assert_eq!(
            snapshot.next(&mut cursor).unwrap().unwrap(),
            ReactorSocketEntry { fd: 8, token: 108 }
        );
        assert_eq!(snapshot.next(&mut cursor).unwrap().unwrap().fd, 12);
    }
    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn gnu_candidate_source_growth_and_copy_without_native_qualification() {
        let mut owner = ReactorSocketStore::<true>::new();
        for key in 0..14 {
            owner.put(key, key as usize + 100).unwrap();
            assert_eq!(owner.buckets.len(), if key < 13 { 13 } else { 29 });
        }
        let expected: Vec<_> = [13].into_iter().chain(0..13).collect();
        let snapshot = owner.snapshot_entries().unwrap();
        assert_eq!(
            snapshot.iter().map(|entry| entry.fd).collect::<Vec<_>>(),
            expected
        );
        owner.erase(5);
        owner.put(0, 900).unwrap();
        owner.put(-13, 0).unwrap();
        assert_eq!(
            snapshot.iter().find(|entry| entry.fd == 0).unwrap().token,
            100
        );
        assert!(snapshot.iter().any(|entry| entry.fd == 5));
        assert_eq!(owner.nodes[owner.find(0).unwrap()].token, 900);
        assert_eq!(owner.nodes[owner.find(-13).unwrap()].token, 0);
        let mut sizes = Vec::new();
        let mut growth = ReactorSocketStore::<true>::new();
        for key in 0..80 {
            growth.put(key, key as usize).unwrap();
            if sizes.last().copied() != Some(growth.buckets.len()) {
                sizes.push(growth.buckets.len());
            }
        }
        assert_eq!(sizes, [13, 29, 59, 127]);
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux"))]
    #[test]
    fn gnu_candidate_bucket_collisions_signed_keys_erase_and_reinsert() {
        let mut owner = ReactorSocketStore::<true>::new();
        for key in [0, 13, 26] {
            owner.put(key, key as usize).unwrap();
        }
        assert_eq!(
            owner
                .snapshot_entries()
                .unwrap()
                .iter()
                .map(|entry| entry.fd)
                .collect::<Vec<_>>(),
            [26, 13, 0]
        );
        owner.erase(13);
        owner.put(13, 7).unwrap();
        assert_eq!(
            owner
                .snapshot_entries()
                .unwrap()
                .iter()
                .map(|entry| entry.fd)
                .collect::<Vec<_>>(),
            [13, 26, 0]
        );
        for key in [i32::MIN, -1, i32::MAX] {
            owner.put(key, 9).unwrap();
            assert_eq!(owner.nodes[owner.find(key).unwrap()].token, 9);
        }
        for key in [0, 13, 26, i32::MIN, -1, i32::MAX] {
            owner.erase(key);
        }
        assert_eq!(owner.len(), 0);
        assert_eq!(owner.buckets.len(), 13);
        owner.put(55, 88).unwrap();
        assert_eq!(owner.len(), 1);
        assert_eq!(owner.buckets.len(), 13);
    }
}

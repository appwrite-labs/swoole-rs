//! Authoritative ID/token registry. Tokens are never interpreted as objects.
//! Apple libc++210106 ABI1 long-key bucket traversal is source-bound; unknown
//! Native policies remain activation-blocked. Bucket heads and stable entries
//! provide both lookup and traversal, without a second hashing/allocation index.
//! Stable owned entries let Native cursors return between all effects: no Rust
//! iterator/reference is retained over a Native callback or Zend boundary.
use core::ffi::{c_long, c_void};
use core::ptr;

#[derive(Clone, Copy)]
struct Entry {
    cid: c_long,
    token: usize,
    previous: *mut Entry,
    next: *mut Entry,
}
/// Exact compiler-policy capsule, selected by Native at lazy owner creation.
/// GNU is a captured-source candidate, never an installed-image capability.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CoroutineRegistryPolicy {
    pub family: u32,
    pub version: u32,
    pub abi: u32,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Policy {
    Apple,
    Gnu,
}
impl Policy {
    fn decode(value: CoroutineRegistryPolicy) -> Option<Self> {
        if core::mem::size_of::<c_long>() != 8 || core::mem::size_of::<usize>() != 8 {
            return None;
        }
        match (value.family, value.version, value.abi) {
            (1, 210106, 1) => Some(Self::Apple),
            (2, 120200, 1) => Some(Self::Gnu),
            _ => None,
        }
    }
    fn legacy() -> Self {
        // Frozen initial-null legacy export behavior, not Native qualification.
        if cfg!(all(target_os = "linux", target_pointer_width = "64")) {
            Self::Gnu
        } else {
            Self::Apple
        }
    }
}
struct Registry {
    pool: crate::storage::Pool<Entry>,
    count: usize,
    first: *mut Entry,
    buckets: Vec<*mut Entry>,
    policy: Policy,
    next_resize: usize,
}
impl Registry {
    fn new(policy: Policy) -> Self {
        Self {
            pool: Default::default(),
            count: 0,
            first: ptr::null_mut(),
            buckets: Vec::new(),
            policy,
            next_resize: 0,
        }
    }
}
impl Default for Registry {
    fn default() -> Self {
        Self::new(Policy::legacy())
    }
}
fn try_box<T>(value: T) -> Result<Box<T>, ()> {
    // Both concrete types are non-zero-sized. Allocation failure returns to
    // Native before Native raises; no infallible Box allocation is hidden here.
    unsafe {
        let pointer = std::alloc::alloc(std::alloc::Layout::new::<T>()).cast::<T>();
        if pointer.is_null() {
            return Err(());
        }
        pointer.write(value);
        Ok(Box::from_raw(pointer))
    }
}
impl Registry {
    fn find(&self, cid: c_long) -> *mut Entry {
        if self.buckets.is_empty() {
            return ptr::null_mut();
        }
        let bucket = cid as usize % self.buckets.len();
        let mut entry = self.buckets[bucket];
        unsafe {
            while !entry.is_null() && (*entry).cid as usize % self.buckets.len() == bucket {
                if (*entry).cid == cid {
                    return entry;
                }
                entry = (*entry).next;
            }
        }
        ptr::null_mut()
    }
    fn slot(&mut self, cid: c_long) -> Result<*mut Entry, ()> {
        let found = self.find(cid);
        if !found.is_null() {
            return Ok(found);
        }
        let entry = self
            .pool
            .insert(Entry {
                cid,
                token: 0,
                previous: ptr::null_mut(),
                next: ptr::null_mut(),
            })
            .ok_or(())?;
        if self.prepare_order().is_err() {
            unsafe { self.pool.remove(entry) };
            return Err(());
        }
        let raw = entry.as_ptr();
        self.link_order(raw);
        self.count += 1;
        Ok(raw)
    }
    fn insert(&mut self, cid: c_long, token: usize) -> Result<(), ()> {
        let entry = self.slot(cid)?;
        // Whole ordinary operator[] assignment: a miss owns a null token
        // before assignment; a hit keeps its token until this one write.
        unsafe { (*entry).token = token };
        Ok(())
    }
    fn remove(&mut self, cid: c_long) -> bool {
        let pointer = self.find(cid);
        if pointer.is_null() {
            return false;
        }
        unsafe {
            let entry = *pointer;
            let bucket = cid as usize % self.buckets.len();
            if self.buckets[bucket] == pointer {
                self.buckets[bucket] = if !entry.next.is_null()
                    && (*entry.next).cid as usize % self.buckets.len() == bucket
                {
                    entry.next
                } else {
                    ptr::null_mut()
                };
            }
            if entry.previous.is_null() {
                self.first = entry.next;
            } else {
                (*entry.previous).next = entry.next;
            }
            if !entry.next.is_null() {
                (*entry.next).previous = entry.previous;
            }
            self.count -= 1;
            self.pool.remove(core::ptr::NonNull::new_unchecked(pointer));
        }
        true
    }
}

impl Registry {
    // Source-bound unique-map order from libc++210106 __hash_table/hash<long>.
    // Rust owns bucket heads and stable entries; no Native map/iterator exists.
    fn rehash_order(&mut self, count: usize) -> Result<(), ()> {
        let mut buckets = Vec::new();
        buckets.try_reserve_exact(count).map_err(|_| ())?;
        buckets.resize(count, ptr::null_mut());
        unsafe {
            if !self.first.is_null() {
                let mut previous = self.first;
                let mut previous_bucket = (*previous).cid as usize % count;
                buckets[previous_bucket] = previous;
                loop {
                    let current = (*previous).next;
                    if current.is_null() {
                        break;
                    }
                    let bucket = (*current).cid as usize % count;
                    if bucket == previous_bucket {
                        previous = current;
                    } else if buckets[bucket].is_null() {
                        buckets[bucket] = current;
                        previous = current;
                        previous_bucket = bucket;
                    } else {
                        // Merge the later bucket run at its earlier run's head.
                        let next = (*current).next;
                        (*previous).next = next;
                        if !next.is_null() {
                            (*next).previous = previous;
                        }
                        let head = buckets[bucket];
                        let before = (*head).previous;
                        (*current).next = head;
                        (*current).previous = before;
                        (*head).previous = current;
                        if before.is_null() {
                            self.first = current;
                        } else {
                            (*before).next = current;
                        }
                        buckets[bucket] = current;
                    }
                }
            }
        }
        self.buckets = buckets;
        Ok(())
    }
    fn prepare_order(&mut self) -> Result<(), ()> {
        match self.policy {
            Policy::Apple => self.prepare_apple_order(),
            Policy::Gnu => self.prepare_gnu_order(),
        }
    }
    fn prepare_apple_order(&mut self) -> Result<(), ()> {
        let count = self.count.checked_add(1).ok_or(())?;
        let buckets = self.buckets.len();
        // Original max_load_factor1 comparison and requested growth use float.
        if buckets == 0 || count as f32 > buckets as f32 {
            let growth = buckets
                .checked_mul(2)
                .and_then(|n| {
                    n.checked_add(usize::from(!(buckets > 2 && buckets.is_power_of_two())))
                })
                .ok_or(())?;
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
    // GCC12 _Prime_rehash_policy and unique-key _M_rehash_aux candidate.
    // Official package/release bodies are captured, but NOT attested to our
    // Linux image. The Native selected header continues to refuse Linux ON.
    fn prepare_gnu_order(&mut self) -> Result<(), ()> {
        let count = self.count.checked_add(1).ok_or(())?;
        if count <= self.next_resize {
            return Ok(());
        }
        let buckets = self.buckets.len().max(1); // Native embedded initial bucket.
        let minimum = count.max(if self.next_resize == 0 { 11 } else { 0 }) as f64;
        if minimum < buckets as f64 {
            self.next_resize = (buckets as f64).floor() as usize; // Native fixed load1.
            return Ok(());
        }
        let requested = (minimum.floor() as usize)
            .checked_add(1)
            .ok_or(())?
            .max(buckets.checked_mul(2).ok_or(())?);
        let (size, next_resize) = gnu_next_buckets(requested);
        let mut replacement = Vec::new();
        replacement.try_reserve_exact(size).map_err(|_| ())?;
        replacement.resize(size, ptr::null_mut());
        // Allocate before publication. Failed growth leaves policy/list intact.
        let previous_buckets = core::mem::replace(&mut self.buckets, replacement);
        let mut cursor = self.first;
        self.first = ptr::null_mut();
        while !cursor.is_null() {
            let next = unsafe { (*cursor).next };
            // GNU visits every old node, including contiguous bucket runs.
            // Reinserting at each new bucket head reverses those runs.
            self.link_order(cursor);
            cursor = next;
        }
        drop(previous_buckets);
        self.next_resize = next_resize;
        Ok(())
    }
    fn link_order(&mut self, entry: *mut Entry) {
        unsafe {
            let bucket = (*entry).cid as usize % self.buckets.len();
            let head = self.buckets[bucket];
            let next = if head.is_null() { self.first } else { head };
            let previous = if head.is_null() {
                ptr::null_mut()
            } else {
                (*head).previous
            };
            (*entry).next = next;
            (*entry).previous = previous;
            if previous.is_null() {
                self.first = entry;
            } else {
                (*previous).next = entry;
            }
            if !next.is_null() {
                (*next).previous = entry;
            }
            self.buckets[bucket] = entry;
        }
    }
}
fn next_prime(requested: usize) -> Result<usize, ()> {
    let mut candidate = requested | 1;
    loop {
        let mut divisor = 3;
        while divisor <= candidate / divisor && !candidate.is_multiple_of(divisor) {
            divisor += 2;
        }
        if divisor > candidate / divisor {
            return Ok(candidate);
        }
        candidate = candidate.checked_add(2).ok_or(())?;
    }
}

fn gnu_next_buckets(requested: usize) -> (usize, usize) {
    if requested == 0 {
        return (1, 0);
    } // Native embedded first bucket, no admission yet.
    // Release table includes a duplicated sentinel; only the last real prime is
    // used here. lower_bound excludes it, so saturation still returns it.
    let last = GNU_PRIMES.len() - 1;
    let index = GNU_PRIMES[..last].partition_point(|&p| p < requested as u64);
    let size = GNU_PRIMES[index] as usize;
    (
        size,
        if index == last {
            usize::MAX
        } else {
            (size as f64).floor() as usize
        },
    )
}
// Source data: GCC12.2.0 hashtable-aux.cc, 64-bit unsigned-long prime list.
// SHA256 68fe03dda8fe9d33ec38b535cd9a750119da0da461b6040117fc114674ce893e.
#[rustfmt::skip]
const GNU_PRIMES: &[u64] = &[
    2, 3, 5, 7, 11, 13,
    17, 19, 23, 29, 31, 37,
    41, 43, 47, 53, 59, 61,
    67, 71, 73, 79, 83, 89,
    97, 103, 109, 113, 127, 137,
    139, 149, 157, 167, 179, 193,
    199, 211, 227, 241, 257, 277,
    293, 313, 337, 359, 383, 409,
    439, 467, 503, 541, 577, 619,
    661, 709, 761, 823, 887, 953,
    1031, 1109, 1193, 1289, 1381, 1493,
    1613, 1741, 1879, 2029, 2179, 2357,
    2549, 2753, 2971, 3209, 3469, 3739,
    4027, 4349, 4703, 5087, 5503, 5953,
    6427, 6949, 7517, 8123, 8783, 9497,
    10273, 11113, 12011, 12983, 14033, 15173,
    16411, 17749, 19183, 20753, 22447, 24281,
    26267, 28411, 30727, 33223, 35933, 38873,
    42043, 45481, 49201, 53201, 57557, 62233,
    67307, 72817, 78779, 85229, 92203, 99733,
    107897, 116731, 126271, 136607, 147793, 159871,
    172933, 187091, 202409, 218971, 236897, 256279,
    277261, 299951, 324503, 351061, 379787, 410857,
    444487, 480881, 520241, 562841, 608903, 658753,
    712697, 771049, 834181, 902483, 976369, 1056323,
    1142821, 1236397, 1337629, 1447153, 1565659, 1693859,
    1832561, 1982627, 2144977, 2320627, 2510653, 2716249,
    2938679, 3179303, 3439651, 3721303, 4026031, 4355707,
    4712381, 5098259, 5515729, 5967347, 6456007, 6984629,
    7556579, 8175383, 8844859, 9569143, 10352717, 11200489,
    12117689, 13109983, 14183539, 15345007, 16601593, 17961079,
    19431899, 21023161, 22744717, 24607243, 26622317, 28802401,
    31160981, 33712729, 36473443, 39460231, 42691603, 46187573,
    49969847, 54061849, 58488943, 63278561, 68460391, 74066549,
    80131819, 86693767, 93793069, 101473717, 109783337, 118773397,
    128499677, 139022417, 150406843, 162723577, 176048909, 190465427,
    206062531, 222936881, 241193053, 260944219, 282312799, 305431229,
    330442829, 357502601, 386778277, 418451333, 452718089, 489790921,
    529899637, 573292817, 620239453, 671030513, 725980837, 785430967,
    849749479, 919334987, 994618837, 1076067617, 1164186217, 1259520799,
    1362662261, 1474249943, 1594975441, 1725587117, 1866894511, 2019773507,
    2185171673, 2364114217, 2557710269, 2767159799, 2993761039, 3238918481,
    3504151727, 3791104843, 4101556399, 4294967291, 6442450933, 8589934583,
    12884901857, 17179869143, 25769803693, 34359738337, 51539607367, 68719476731,
    103079215087, 137438953447, 206158430123, 274877906899, 412316860387, 549755813881,
    824633720731, 1099511627689, 1649267441579, 2199023255531, 3298534883309, 4398046511093,
    6597069766607, 8796093022151, 13194139533241, 17592186044399, 26388279066581, 35184372088777,
    52776558133177, 70368744177643, 105553116266399, 140737488355213, 211106232532861, 281474976710597,
    562949953421231, 1125899906842597, 2251799813685119, 4503599627370449, 9007199254740881, 18014398509481951,
    36028797018963913, 72057594037927931, 144115188075855859, 288230376151711717, 576460752303423433, 1152921504606846883,
    2305843009213693951, 4611686018427387847, 9223372036854775783, 18446744073709551557,
];

impl Registry {
    fn clear(&mut self) {
        let mut cursor = self.first;
        while !cursor.is_null() {
            // Metadata only: opaque Native tokens have no Rust destructor.
            unsafe {
                let next = (*cursor).next;
                self.pool.remove(core::ptr::NonNull::new_unchecked(cursor));
                cursor = next;
            }
        }
        self.first = ptr::null_mut();
        self.count = 0;
        self.buckets.fill(ptr::null_mut());
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        self.clear();
    }
}

/// # Safety
/// `owner` is a live aligned Native handle slot, initially null or created here,
/// serialized as the original map. Tokens are opaque values with Native custody.
/// Returns -3 on fallible allocation refusal, with no Rust exception/panic.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_coroutine_registry_insert(
    owner: *mut *mut c_void,
    cid: c_long,
    token: usize,
) -> i32 {
    unsafe { insert(owner, cid, token) }
}

pub(crate) unsafe fn insert(owner: *mut *mut c_void, cid: c_long, token: usize) -> i32 {
    unsafe { insert_with_policy(owner, cid, token, None) }
}

/// # Safety
/// Same serialized owner/token contract as legacy insert. Explicit policy must
/// match an existing owner's captured policy. Unknown/mismatched capsules return
/// -4 without mutation or allocation. Allocation refusal remains -3.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_coroutine_registry_selected_insert(
    owner: *mut *mut c_void,
    cid: c_long,
    token: usize,
    policy: CoroutineRegistryPolicy,
) -> i32 {
    unsafe { selected_insert(owner, cid, token, policy) }
}

pub(crate) unsafe fn selected_insert(
    owner: *mut *mut c_void,
    cid: c_long,
    token: usize,
    policy: CoroutineRegistryPolicy,
) -> i32 {
    let Some(policy) = Policy::decode(policy) else {
        return -4;
    };
    unsafe { insert_with_policy(owner, cid, token, Some(policy)) }
}

unsafe fn insert_with_policy(
    owner: *mut *mut c_void,
    cid: c_long,
    token: usize,
    policy: Option<Policy>,
) -> i32 {
    unsafe {
        if (*owner).is_null() {
            // Policy is captured at this original lazy allocation point only.
            let Ok(mut registry) = try_box(Registry::new(policy.unwrap_or_else(Policy::legacy)))
            else {
                return -3;
            };
            if registry.insert(cid, token).is_err() {
                return -3;
            }
            *owner = Box::into_raw(registry).cast();
        } else {
            let registry = &mut *(*owner).cast::<Registry>();
            if policy.is_some_and(|value| value != registry.policy) {
                return -4;
            }
            if registry.insert(cid, token).is_err() {
                return -3;
            }
        }
    }
    0
}
/// # Safety
/// A null or live owner created here; serialized with every map/cursor operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_coroutine_registry_remove(owner: *mut c_void, cid: c_long) {
    unsafe { erase(owner, cid) };
}

pub(crate) unsafe fn erase(owner: *mut c_void, cid: c_long) -> usize {
    if owner.is_null() {
        0
    } else {
        usize::from(unsafe { (&mut *owner.cast::<Registry>()).remove(cid) })
    }
}

/// # Safety
/// Null or a live serialized owner; Native still owns every token pointee.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_coroutine_registry_erase(owner: *mut c_void, cid: c_long) -> usize {
    unsafe { erase(owner, cid) }
}

/// Whole direct token lookup, preserving absent/stored-null public readout.
/// # Safety
/// Null or a live readable owner without racing mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_coroutine_registry_lookup(
    owner: *const c_void,
    cid: c_long,
) -> usize {
    if owner.is_null() {
        return 0;
    }
    let found = unsafe { (&*owner.cast::<Registry>()).find(cid) };
    if found.is_null() {
        0
    } else {
        unsafe { (*found).token }
    }
}
/// # Safety
/// A null or live owner created here, without racing access. The returned cursor
/// is invalidated only by removing that entry or destroying the owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_coroutine_registry_find(
    owner: *const c_void,
    cid: c_long,
) -> *const c_void {
    if owner.is_null() {
        return ptr::null();
    }
    unsafe { (&*owner.cast::<Registry>()).find(cid).cast() }
}
/// # Safety
/// A null or live owner created here, without racing access.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_coroutine_registry_count(owner: *const c_void) -> usize {
    unsafe { count(owner) }
}

pub(crate) unsafe fn count(owner: *const c_void) -> usize {
    if owner.is_null() {
        0
    } else {
        unsafe { (&*owner.cast::<Registry>()).count }
    }
}
/// # Safety
/// A null or live owner created here, without racing access.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_coroutine_registry_first(owner: *const c_void) -> *const c_void {
    if owner.is_null() {
        ptr::null()
    } else {
        unsafe { (&*owner.cast::<Registry>()).first.cast() }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RegistryItem {
    pub cid: c_long,
    pub token: usize,
    pub next: *const c_void,
}
/// # Safety
/// Cursor is null or a still-live entry returned by this registry. No racing
/// mutation. No payload token is dereferenced or interpreted by Rust.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_coroutine_registry_item(cursor: *const c_void) -> RegistryItem {
    if cursor.is_null() {
        return RegistryItem {
            cid: 0,
            token: 0,
            next: ptr::null(),
        };
    }
    unsafe {
        let entry = &*cursor.cast::<Entry>();
        RegistryItem {
            cid: entry.cid,
            token: entry.token,
            next: entry.next.cast(),
        }
    }
}
/// # Safety
/// Null or a live uniquely owned registry, serialized with every map operation.
/// All entry cursors are invalidated. Native tokens are not destroyed. Bucket
/// allocation and owner identity remain live, matching Native clear semantics.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_coroutine_registry_clear(owner: *mut c_void) {
    if !owner.is_null() {
        unsafe { (&mut *owner.cast::<Registry>()).clear() };
    }
}
/// # Safety
/// The slot is live and owns its null or uniquely allocated handle. No cursor or
/// operation can outlive this destruction. Native object tokens are not destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_coroutine_registry_drop(owner: *mut *mut c_void) {
    unsafe {
        if !(*owner).is_null() {
            drop(Box::from_raw((*owner).cast::<Registry>()));
            *owner = ptr::null_mut();
        }
    }
}

#[repr(C)]
pub struct RegistryPrintCursor {
    pub entry: *const c_void,
    pub phase: u32,
}
#[repr(C)]
pub struct RegistryPrintAction {
    pub action: u32,
    pub cid: c_long,
    pub token: usize,
    pub label: *const core::ffi::c_char,
}

/// Complete owner traversal and status/label/output admission. Native returns
/// the selected opaque token as an actual typed object pointer; Rust never
/// reconstructs foreign pointers from the frozen integer-token representation.
/// # Safety
/// Owner is null or a live serialized Registry. Cursor is initialized caller-
/// local {null,0} storage. A selected entry remains live through output and the
/// following increment, matching the original iterator domain. At phase1 only,
/// object is the exact Native pointer projected from the requested token, with
/// initialized aligned CoroutineStatus at status_offset within that allocation;
/// byte arithmetic is representable/in bounds. All objects use the same exact
/// Native-derived offset. No object/state/next read is prefetched over Native
/// output; no Rust borrow/frame survives output, abort or other effects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_coroutine_registry_print_step(
    owner: *const c_void,
    cursor: *mut RegistryPrintCursor,
    object: *const c_void,
    status_offset: usize,
) -> RegistryPrintAction {
    let c = unsafe { &mut *cursor };
    let mut result = RegistryPrintAction {
        action: 0,
        cid: 0,
        token: 0,
        label: ptr::null(),
    };
    let first = c.phase == 0;
    let entry = match c.phase {
        0 => {
            if owner.is_null() {
                ptr::null_mut()
            } else {
                unsafe { (*owner.cast::<Registry>()).first }
            }
        }
        1 => c.entry.cast_mut().cast::<Entry>(),
        2 => unsafe { (*c.entry.cast::<Entry>()).next },
        _ => return result,
    };
    if entry.is_null() {
        c.phase = 3;
        return result;
    }
    if c.phase != 1 {
        c.entry = entry.cast();
        c.phase = 1;
        result.action = if first { 1 } else { 2 }; // Native object/first offset projection.
        result.token = unsafe { (*entry).token };
        return result;
    }
    let status = unsafe {
        object
            .cast::<u8>()
            .add(status_offset)
            .cast::<crate::CoroutineStatus>()
            .read()
    };
    let state = crate::status::read(&status).state;
    let label: &[u8] = match state {
        0 => b"[INIT]\0",
        1 => b"[WAITING]\0",
        2 => b"[RUNNING]\0",
        3 => b"[END]\0",
        _ => {
            result.action = 4;
            c.phase = 3;
            return result;
        } // Native abort, never invoked here.
    };
    result.action = 3;
    result.cid = unsafe { (*entry).cid };
    result.label = label.as_ptr().cast();
    c.phase = 2;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn whole_print_requests_native_objects_and_refreshes_status_after_output() {
        #[repr(C)]
        struct Foreign {
            prefix: usize,
            status: crate::CoroutineStatus,
            tail: usize,
        }
        let mut records: [Foreign; 4] = core::array::from_fn(|n| Foreign {
            prefix: n + 10,
            status: crate::CoroutineStatus {
                state: n as i32,
                code: 0,
            },
            tail: n + 20,
        });
        let mut owner = ptr::null_mut();
        unsafe {
            for (n, record) in records.iter().enumerate() {
                assert_eq!(
                    insert(&mut owner, n as c_long, ptr::from_ref(record).addr()),
                    0
                );
            }
            let mut cursor = RegistryPrintCursor {
                entry: ptr::null(),
                phase: 0,
            };
            let mut object = ptr::null();
            let mut index = 0;
            let mut seen = 0;
            loop {
                let step = swr_v1_coroutine_registry_print_step(
                    owner,
                    &mut cursor,
                    object,
                    core::mem::offset_of!(Foreign, status),
                );
                match step.action {
                    0 => break,
                    1 | 2 => {
                        index = records
                            .iter()
                            .position(|record| ptr::from_ref(record).addr() == step.token)
                            .unwrap();
                        object = ptr::from_ref(&records[index]).cast();
                    }
                    3 => {
                        assert_eq!(step.cid, index as c_long);
                        let text = std::ffi::CStr::from_ptr(step.label).to_str().unwrap();
                        assert_eq!(
                            text,
                            match records[index].status.state {
                                0 => "[INIT]",
                                1 => "[WAITING]",
                                2 => "[RUNNING]",
                                _ => "[END]",
                            }
                        );
                        // Primitive metadata-only output barrier; future status
                        // must be observed on its own later step, not prefetched.
                        for item in &mut records {
                            item.status.state = 2;
                        }
                        seen += 1;
                    }
                    _ => panic!("unexpected output descriptor"),
                }
            }
            assert_eq!(seen, 4);
            assert_eq!(
                swr_v1_coroutine_registry_print_step(owner, &mut cursor, ptr::null(), usize::MAX)
                    .action,
                0
            );
            swr_v1_coroutine_registry_drop(&mut owner);
            let mut empty = RegistryPrintCursor {
                entry: ptr::null(),
                phase: 0,
            };
            assert_eq!(
                swr_v1_coroutine_registry_print_step(
                    ptr::null(),
                    &mut empty,
                    ptr::null(),
                    usize::MAX
                )
                .action,
                0
            );
        }
    }
    #[test]
    fn stable_cursors_survive_growth_other_removal_and_replacement() {
        unsafe {
            let mut owner = ptr::null_mut();
            assert_eq!(swr_v1_coroutine_registry_count(owner), 0);
            assert_eq!(swr_v1_coroutine_registry_insert(&mut owner, -7, 0), 0);
            let original = swr_v1_coroutine_registry_find(owner, -7);
            for id in 1..4096 {
                assert_eq!(
                    swr_v1_coroutine_registry_insert(&mut owner, id, id as usize * 13),
                    0
                );
            }
            assert_eq!(swr_v1_coroutine_registry_find(owner, -7), original);
            swr_v1_coroutine_registry_remove(owner, 97);
            swr_v1_coroutine_registry_remove(owner, -123);
            assert_eq!(swr_v1_coroutine_registry_insert(&mut owner, -7, 991), 0);
            assert_eq!(swr_v1_coroutine_registry_find(owner, -7), original);
            assert_eq!(swr_v1_coroutine_registry_item(original).token, 991);
            let mut seen = std::collections::BTreeSet::new();
            let mut cursor = swr_v1_coroutine_registry_first(owner);
            while !cursor.is_null() {
                let item = swr_v1_coroutine_registry_item(cursor);
                assert!(seen.insert(item.cid));
                cursor = item.next;
            }
            assert_eq!(seen.len(), swr_v1_coroutine_registry_count(owner));
            assert!(!seen.contains(&97));
            swr_v1_coroutine_registry_drop(&mut owner);
            assert!(owner.is_null());
            swr_v1_coroutine_registry_drop(&mut owner);
        }
    }
    // Candidate source contract only, not actual Native Linux qualification.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn gnu_candidate_growth_runs_clear_and_long_boundaries() {
        let mut registry = Registry {
            pool: Default::default(),
            count: 0,
            first: ptr::null_mut(),
            buckets: Vec::new(),
            next_resize: 0,
            policy: Policy::Gnu,
        };
        for cid in [0, 377, 754] {
            registry.insert(cid, cid as usize).unwrap();
        }
        for cid in 1..=11 {
            registry.insert(cid, cid as usize).unwrap();
        }
        assert_eq!(registry.buckets.len(), 29);
        assert_eq!(registry.next_resize, 29);
        let mut cursor = registry.first;
        let mut observed = Vec::new();
        while !cursor.is_null() {
            unsafe {
                observed.push((*cursor).cid);
                cursor = (*cursor).next;
            }
        }
        // GCC unique rehash reverses each old bucket run, unlike libc++.
        assert_eq!(observed, [11, 0, 377, 754, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let stable = registry.find(377);
        for cid in [c_long::MIN, c_long::MAX, -1, -377] {
            registry.insert(cid, cid as usize).unwrap();
            assert_eq!(unsafe { (*registry.find(cid)).token }, cid as usize);
        }
        registry.insert(377, 99).unwrap();
        assert_eq!(registry.find(377), stable);
        registry.remove(0);
        assert!(registry.find(0).is_null());
        let buckets = registry.buckets.as_ptr();
        registry.clear();
        assert_eq!(registry.count, 0);
        assert_eq!(registry.buckets.as_ptr(), buckets);
        assert_eq!(registry.next_resize, 29);
        registry.insert(-9, 7).unwrap();
        assert_eq!(registry.buckets.len(), 29);
        assert_eq!(registry.next_resize, 29);
        assert_eq!(unsafe { (*registry.find(-9)).token }, 7);
    }
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn gnu_candidate_prime_growth_thresholds_and_saturation() {
        assert_eq!(gnu_next_buckets(0), (1, 0)); // Embedded initial bucket; first admission requests12.
        assert_eq!(gnu_next_buckets(12), (13, 13));
        assert_eq!(gnu_next_buckets(26), (29, 29));
        assert_eq!(gnu_next_buckets(58), (59, 59));
        assert_eq!(
            gnu_next_buckets(9_223_372_036_854_775_783),
            (9_223_372_036_854_775_783, 9_223_372_036_854_775_808)
        );
        for pair in GNU_PRIMES.windows(2) {
            let requested = pair[0] as usize + 1;
            assert_eq!(gnu_next_buckets(requested).0, pair[1] as usize);
        }
        assert_eq!(
            gnu_next_buckets(usize::MAX),
            (18_446_744_073_709_551_557, usize::MAX)
        );
        let mut registry = Registry {
            pool: Default::default(),
            count: 0,
            first: ptr::null_mut(),
            buckets: Vec::new(),
            next_resize: 0,
            policy: Policy::Gnu,
        };
        let expected = [
            (1, 13),
            (14, 29),
            (30, 59),
            (60, 127),
            (128, 257),
            (258, 541),
            (542, 1109),
        ];
        let mut next = 0;
        for cid in 0..600 {
            registry.insert(cid, cid as usize).unwrap();
            if next < expected.len() && registry.count == expected[next].0 {
                assert_eq!(registry.buckets.len(), expected[next].1);
                assert_eq!(registry.next_resize, expected[next].1);
                next += 1;
            }
        }
        assert_eq!(next, expected.len());
    }
    #[test]
    fn clear_retains_owner_buckets_and_independent_members() {
        unsafe {
            swr_v1_coroutine_registry_clear(ptr::null_mut());
            let mut owner = ptr::null_mut();
            let mut independent = ptr::null_mut();
            for id in 0..100 {
                assert_eq!(
                    swr_v1_coroutine_registry_insert(&mut owner, id, id as usize),
                    0
                );
            }
            assert_eq!(swr_v1_coroutine_registry_insert(&mut independent, -1, 0), 0);
            let original_owner = owner;
            let buckets = (&*owner.cast::<Registry>()).buckets.as_ptr();
            let bucket_count = (&*owner.cast::<Registry>()).buckets.len();
            for _ in 0..2 {
                swr_v1_coroutine_registry_clear(owner);
                assert_eq!(owner, original_owner);
                assert_eq!(swr_v1_coroutine_registry_count(owner), 0);
                assert!(swr_v1_coroutine_registry_first(owner).is_null());
                let registry = &*owner.cast::<Registry>();
                assert_eq!(registry.buckets.as_ptr(), buckets);
                assert_eq!(registry.buckets.len(), bucket_count);
                assert!(registry.buckets.iter().all(|entry| entry.is_null()));
                assert_eq!(swr_v1_coroutine_registry_insert(&mut owner, -9, 7), 0);
                assert_eq!(swr_v1_coroutine_registry_count(owner), 1);
            }
            assert_eq!(swr_v1_coroutine_registry_count(independent), 1);
            swr_v1_coroutine_registry_drop(&mut owner);
            swr_v1_coroutine_registry_drop(&mut independent);
        }
    }
    #[test]
    fn unlink_front_middle_tail_and_independent_owners() {
        unsafe {
            let mut owner = ptr::null_mut();
            let mut independent = ptr::null_mut();
            for id in 1..=5 {
                assert_eq!(
                    swr_v1_coroutine_registry_insert(&mut owner, id, id as usize),
                    0
                );
            }
            assert_eq!(swr_v1_coroutine_registry_insert(&mut independent, 1, 99), 0);
            for id in [5, 3, 1, 4, 2] {
                swr_v1_coroutine_registry_remove(owner, id);
                assert!(swr_v1_coroutine_registry_find(owner, id).is_null());
            }
            assert!(swr_v1_coroutine_registry_first(owner).is_null());
            assert_eq!(swr_v1_coroutine_registry_count(owner), 0);
            assert_eq!(
                swr_v1_coroutine_registry_item(swr_v1_coroutine_registry_find(independent, 1))
                    .token,
                99
            );
            swr_v1_coroutine_registry_drop(&mut owner);
            swr_v1_coroutine_registry_drop(&mut independent);
        }
    }
}

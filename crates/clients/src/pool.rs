//! Sole private persistent-Client container. Tokens are never dereferenced/dropped.
use std::alloc::{Layout, alloc, dealloc};
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::hash::{BuildHasherDefault, DefaultHasher};

#[cfg(test)]
std::thread_local! {
    // Logical fallible allocation sites only; not a claim about malloc topology.
    static ALLOCATION_BUDGET: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

#[inline]
fn allocation_allowed() -> bool {
    #[cfg(test)]
    {
        ALLOCATION_BUDGET.with(|budget| match budget.get() {
            None => true,
            Some(0) => false,
            Some(left) => {
                budget.set(Some(left - 1));
                true
            }
        })
    }
    #[cfg(not(test))]
    {
        true
    }
}

#[cfg(test)]
#[unsafe(no_mangle)]
pub extern "C" fn swr_test_clients_pool_allocation_budget(budget: isize) {
    ALLOCATION_BUDGET.with(|value| value.set(usize::try_from(budget).ok()));
}

type Queues = HashMap<Vec<u8>, VecDeque<*mut c_void>, BuildHasherDefault<DefaultHasher>>;
struct Pool {
    queues: Queues,
}
const OK: u32 = 0;
const OUT_OF_MEMORY: u32 = 1;
const INVALID_ARGUMENT: u32 = 2;

unsafe fn key<'a>(data: *const u8, length: usize) -> Option<&'a [u8]> {
    if length == 0 {
        Some(&[])
    } else if data.is_null() || length > isize::MAX as usize {
        None
    } else {
        // SAFETY: `data` is non-null and `length <= isize::MAX` (checked above); the caller lends `length` initialized bytes for this call.
        Some(unsafe { std::slice::from_raw_parts(data, length) })
    }
}

/// Creates only the opaque owner; bucket/key/queue allocations remain lazy and
/// fallible. No native effects, entropy, user callbacks or Client destruction.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_pool_create() -> *mut c_void {
    if !allocation_allowed() {
        return std::ptr::null_mut();
    }
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "alloc honours Layout::new::<Pool>()"
    )]
    // SAFETY: `Pool` is not zero-sized; null is handled below.
    let memory = unsafe { alloc(Layout::new::<Pool>()) }.cast::<Pool>();
    if !memory.is_null() {
        // SAFETY: `memory` is a fresh, non-null allocation with `Pool`'s layout.
        unsafe {
            memory.write(Pool {
                queues: HashMap::with_hasher(BuildHasherDefault::default()),
            });
        }
    }
    memory.cast()
}

/// # Safety
/// Handle is null or a live unique owner returned by create, with no active calls.
/// Drops Rust container buffers only. Opaque Client tokens never run destructors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_pool_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        let pool = handle.cast::<Pool>();
        // SAFETY: a live, uniquely owned pool from `swr_v1_clients_pool_create`.
        unsafe { std::ptr::drop_in_place(pool) };
        // SAFETY: allocated with this layout by create; its value was just dropped.
        unsafe { dealloc(pool.cast(), Layout::new::<Pool>()) };
    }
}

/// # Safety
/// Handle is live with exclusive access. Key bytes are initialized and independent
/// of pool storage. Token is nonnull and remains under native Client custody.
/// 0 success;1 fallible allocation failed;2 invalid argument. New empty entry is
/// published before queue growth, preserving its logical failure prefix.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_pool_push(
    handle: *mut c_void,
    data: *const u8,
    length: usize,
    token: *mut c_void,
) -> u32 {
    // SAFETY: key bytes are initialized for `length` (null only when empty) for this call.
    let Some(key) = (unsafe { key(data, length) }) else {
        return INVALID_ARGUMENT;
    };
    if handle.is_null() || token.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: non-null (checked above); a live pool from `swr_v1_clients_pool_create`, exclusively accessed.
    let pool = unsafe { &mut *handle.cast::<Pool>() };
    if !pool.queues.contains_key(key) {
        let mut owned = Vec::new();
        if !key.is_empty() && (!allocation_allowed() || owned.try_reserve_exact(key.len()).is_err())
        {
            return OUT_OF_MEMORY;
        }
        owned.extend_from_slice(key);
        if pool.queues.len() == pool.queues.capacity()
            && (!allocation_allowed() || pool.queues.try_reserve(1).is_err())
        {
            return OUT_OF_MEMORY;
        }
        pool.queues.insert(owned, VecDeque::new());
    }
    let Some(queue) = pool.queues.get_mut(key) else {
        return INVALID_ARGUMENT;
    };
    if queue.len() == queue.capacity() && (!allocation_allowed() || queue.try_reserve(1).is_err()) {
        return OUT_OF_MEMORY;
    }
    queue.push_back(token);
    OK
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolState {
    pub kind: u32,
    pub length: usize,
}

/// # Safety
/// Handle is null or live, key initialized and independent, no concurrent writes.
/// Returns0 absent,1 present-empty,2 present-nonempty; allocates nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_pool_state(
    handle: *const c_void,
    data: *const u8,
    length: usize,
) -> PoolState {
    // SAFETY: key bytes are initialized for `length` (null only when empty) for this call.
    let Some(key) = (unsafe { key(data, length) }) else {
        return PoolState::default();
    };
    if handle.is_null() {
        return PoolState::default();
    }
    // SAFETY: non-null (checked above); a live pool from `swr_v1_clients_pool_create`.
    match unsafe { &*handle.cast::<Pool>() }.queues.get(key) {
        None => PoolState::default(),
        Some(queue) => PoolState {
            kind: if queue.is_empty() { 1 } else { 2 },
            length: queue.len(),
        },
    }
}

/// # Safety
/// Handle is null or live with exclusive access; key is initialized/independent.
/// Returns the FIFO native token or null, retaining the now-empty entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_pool_take(
    handle: *mut c_void,
    data: *const u8,
    length: usize,
) -> *mut c_void {
    // SAFETY: key bytes are initialized for `length` (null only when empty) for this call.
    let Some(key) = (unsafe { key(data, length) }) else {
        return std::ptr::null_mut();
    };
    if handle.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: non-null (checked above); a live pool from `swr_v1_clients_pool_create`, exclusively accessed.
    unsafe { &mut *handle.cast::<Pool>() }
        .queues
        .get_mut(key)
        .and_then(VecDeque::pop_front)
        .unwrap_or(std::ptr::null_mut())
}

/// # Safety
/// Handle is null or live with exclusive access; key is initialized/independent.
/// Removes only present-empty entries; never closes/destroys native tokens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_pool_remove_empty(
    handle: *mut c_void,
    data: *const u8,
    length: usize,
) -> u8 {
    // SAFETY: key bytes are initialized for `length` (null only when empty) for this call.
    let Some(key) = (unsafe { key(data, length) }) else {
        return 0;
    };
    if handle.is_null() {
        return 0;
    }
    // SAFETY: non-null (checked above); a live pool from `swr_v1_clients_pool_create`, exclusively accessed.
    let pool = unsafe { &mut *handle.cast::<Pool>() };
    if pool.queues.get(key).is_some_and(VecDeque::is_empty) {
        pool.queues.remove(key);
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::BTreeMap;

    struct Witness(*mut c_void);
    impl Witness {
        fn push(&mut self, key: &[u8], token: *mut c_void) -> u32 {
            if self.0.is_null() {
                self.0 = swr_v1_clients_pool_create();
                if self.0.is_null() {
                    return OUT_OF_MEMORY;
                }
            }
            unsafe { swr_v1_clients_pool_push(self.0, key.as_ptr(), key.len(), token) }
        }
        fn state(&self, key: &[u8]) -> PoolState {
            unsafe { swr_v1_clients_pool_state(self.0, key.as_ptr(), key.len()) }
        }
        fn take(&mut self, key: &[u8]) -> *mut c_void {
            unsafe { swr_v1_clients_pool_take(self.0, key.as_ptr(), key.len()) }
        }
        fn remove_empty(&mut self, key: &[u8]) -> u8 {
            unsafe { swr_v1_clients_pool_remove_empty(self.0, key.as_ptr(), key.len()) }
        }
    }
    impl Drop for Witness {
        fn drop(&mut self) {
            unsafe { swr_v1_clients_pool_destroy(self.0) };
            swr_test_clients_pool_allocation_budget(-1);
        }
    }

    #[test]
    fn fifo_binary_keys_empty_prefix_and_allocation_failures() {
        swr_test_clients_pool_allocation_budget(0);
        assert!(swr_v1_clients_pool_create().is_null());
        swr_test_clients_pool_allocation_budget(-1);
        let mut pool = Witness(swr_v1_clients_pool_create());
        assert!(!pool.0.is_null());
        let key = b"pool\0binary";
        let first = std::ptr::without_provenance_mut::<c_void>(1);
        let second = std::ptr::without_provenance_mut::<c_void>(2);
        for budget in 0..=2 {
            swr_test_clients_pool_allocation_budget(budget);
            assert_eq!(pool.push(key, first), OUT_OF_MEMORY);
            assert_eq!(pool.state(key).kind, u32::from(budget == 2));
        }
        swr_test_clients_pool_allocation_budget(-1);
        assert_eq!(pool.push(key, first), OK);
        assert_eq!(pool.push(key, second), OK);
        assert_eq!(pool.state(key).length, 2);
        assert_eq!(pool.remove_empty(key), 0);
        assert_eq!(pool.take(key), first);
        assert_eq!(pool.take(key), second);
        assert_eq!(pool.state(key).kind, 1);
        assert_eq!(pool.remove_empty(key), 1);
        assert_eq!(pool.state(key).kind, 0);
    }

    #[test]
    fn lazy_owner_failure_prefixes_match_the_retired_cpp_witness() {
        let token = std::ptr::without_provenance_mut::<c_void>(1);
        for budget in 0..4 {
            let mut pool = Witness(std::ptr::null_mut());
            swr_test_clients_pool_allocation_budget(budget);
            assert_eq!(pool.push(b"fault", token), OUT_OF_MEMORY);
            swr_test_clients_pool_allocation_budget(-1);
            assert_eq!(pool.state(b"fault").kind, u32::from(budget == 3));
            assert_eq!(pool.state(b"fault").length, 0);
            assert_eq!(pool.push(b"fault", token), OK);
            assert_eq!(pool.take(b"fault"), token);
            assert_eq!(pool.remove_empty(b"fault"), 1);
        }
    }

    #[test]
    fn queue_growth_failure_keeps_existing_fifo_and_tokens() {
        let mut pool = Witness(swr_v1_clients_pool_create());
        assert!(!pool.0.is_null());
        let key = b"growth";
        let token = std::ptr::without_provenance_mut::<c_void>(7);
        assert_eq!(pool.push(key, token), OK);
        let capacity = unsafe { &*pool.0.cast::<Pool>() }.queues[key.as_slice()].capacity();
        assert!(capacity <= 65536);
        for _ in 1..capacity {
            assert_eq!(pool.push(key, token), OK);
        }
        swr_test_clients_pool_allocation_budget(0);
        assert_eq!(pool.push(key, token), OUT_OF_MEMORY);
        assert_eq!(pool.state(key).length, capacity);
        for _ in 0..capacity {
            assert_eq!(pool.take(key), token);
        }
        assert_eq!(pool.state(key).kind, 1);
        assert!(pool.take(key).is_null());
    }

    thread_local! { static DESTROYED: Cell<u32> = const { Cell::new(0) }; }
    struct NativeToken {
        _id: u32,
    }
    impl Drop for NativeToken {
        fn drop(&mut self) {
            DESTROYED.with(|count| count.set(count.get() + 1));
        }
    }
    fn tokens() -> [NativeToken; 32] {
        DESTROYED.with(|count| count.set(0));
        std::array::from_fn(|id| NativeToken {
            _id: u32::try_from(id).unwrap(),
        })
    }
    fn destroyed() -> u32 {
        DESTROYED.with(Cell::get)
    }

    #[test]
    fn binary_keys_fifo_and_nonempty_teardown_match_the_cpp_witness() {
        let mut tokens = tokens();
        let keys = [
            vec![],
            b"x".to_vec(),
            b"x\0y".to_vec(),
            vec![0],
            b"other".to_vec(),
            vec![b'z'; 4096],
            (0..=255).collect(),
        ];
        {
            let mut pool = Witness(std::ptr::null_mut());
            let mut expected: BTreeMap<&[u8], VecDeque<*mut c_void>> = BTreeMap::new();
            assert_eq!(pool.state(b"absent").kind, 0);
            assert!(pool.take(b"absent").is_null());
            let mut random = 0x0009_1cd8_1a63_u64;
            let mut checks = 0;
            for _ in 0..100_000 {
                random = random
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let key = keys[usize::try_from((random >> 8) % 7).unwrap()].as_slice();
                match (random >> 32) % 4 {
                    0 | 1 => {
                        let token =
                            (&raw mut tokens[usize::try_from((random >> 48) % 32).unwrap()]).cast();
                        assert_eq!(pool.push(key, token), OK);
                        expected.entry(key).or_default().push_back(token);
                    }
                    2 => assert_eq!(
                        pool.take(key),
                        expected
                            .get_mut(key)
                            .and_then(VecDeque::pop_front)
                            .unwrap_or(std::ptr::null_mut())
                    ),
                    _ => {
                        pool.remove_empty(key);
                        if expected.get(key).is_some_and(VecDeque::is_empty) {
                            expected.remove(key);
                        }
                    }
                }
                for probe in &keys {
                    let queue = expected.get(probe.as_slice());
                    let kind = queue.map_or(0, |q| if q.is_empty() { 1 } else { 2 });
                    assert_eq!(
                        pool.state(probe),
                        PoolState {
                            kind,
                            length: queue.map_or(0, VecDeque::len)
                        }
                    );
                    checks += 1;
                }
            }
            assert_eq!(checks, 700_000);
        }
        assert_eq!(destroyed(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn fork_keeps_parent_fifo_and_never_destroys_native_tokens() {
        let mut tokens = tokens();
        let pointers = tokens
            .each_mut()
            .map(|token| std::ptr::from_mut(token).cast::<c_void>());
        {
            let mut pool = Witness(std::ptr::null_mut());
            let key = b"fork\0fifo";
            assert_eq!(pool.push(key, pointers[0]), OK);
            assert_eq!(pool.push(key, pointers[1]), OK);
            let child = unsafe { libc::fork() };
            assert!(child >= 0);
            if child == 0 {
                let mut code = 0;
                if pool.take(key) != pointers[0] {
                    code = 6;
                }
                if pool.push(key, pointers[2]) != OK
                    || pool.take(key) != pointers[1]
                    || pool.take(key) != pointers[2]
                {
                    code = 7;
                }
                pool.remove_empty(key);
                if pool.state(key).kind != 0 {
                    code = 8;
                }
                drop(pool);
                if destroyed() != 0 {
                    code = 9;
                }
                unsafe { libc::_exit(code) };
            }
            let mut status = 0;
            assert_eq!(unsafe { libc::waitpid(child, &raw mut status, 0) }, child);
            assert!(libc::WIFEXITED(status));
            assert_eq!(libc::WEXITSTATUS(status), 0);
            assert_eq!(pool.state(key).length, 2);
            assert_eq!(pool.take(key), pointers[0]);
            assert_eq!(pool.take(key), pointers[1]);
            pool.remove_empty(key);
        }
        assert_eq!(destroyed(), 0);
    }
}

//! The persistent-client container (`long_connections`): FIFO queues of tokens
//! by binary key. It never inspects or drops a token's referent.
use std::collections::{HashMap, VecDeque};
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
fn allocation_budget(budget: isize) {
    ALLOCATION_BUDGET.with(|value| value.set(usize::try_from(budget).ok()));
}

type Queues<T> = HashMap<Vec<u8>, VecDeque<T>, BuildHasherDefault<DefaultHasher>>;

pub struct Pool<T> {
    queues: Queues<T>,
}

/// A fallible allocation failed; the pool keeps what it held.
#[derive(Debug, PartialEq, Eq)]
pub struct OutOfMemory;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolState {
    /// 0 absent, 1 present and empty, 2 present and nonempty.
    pub kind: u32,
    pub length: usize,
}

impl<T> Pool<T> {
    /// An empty pool; key and queue allocations stay lazy and fallible.
    #[must_use]
    pub fn create() -> Option<Self> {
        allocation_allowed().then(|| Self {
            queues: HashMap::with_hasher(BuildHasherDefault::default()),
        })
    }

    /// Queues `token` under `key`. A new empty entry is published before queue
    /// growth, preserving its logical failure prefix.
    ///
    /// # Errors
    /// [`OutOfMemory`] when a fallible allocation fails.
    pub fn push(&mut self, key: &[u8], token: T) -> Result<(), OutOfMemory> {
        if !self.queues.contains_key(key) {
            let mut owned = Vec::new();
            if !key.is_empty()
                && (!allocation_allowed() || owned.try_reserve_exact(key.len()).is_err())
            {
                return Err(OutOfMemory);
            }
            owned.extend_from_slice(key);
            if self.queues.len() == self.queues.capacity()
                && (!allocation_allowed() || self.queues.try_reserve(1).is_err())
            {
                return Err(OutOfMemory);
            }
            self.queues.insert(owned, VecDeque::new());
        }
        let queue = self.queues.get_mut(key).ok_or(OutOfMemory)?;
        if queue.len() == queue.capacity()
            && (!allocation_allowed() || queue.try_reserve(1).is_err())
        {
            return Err(OutOfMemory);
        }
        queue.push_back(token);
        Ok(())
    }

    #[must_use]
    pub fn state(&self, key: &[u8]) -> PoolState {
        self.queues
            .get(key)
            .map_or_else(PoolState::default, |queue| PoolState {
                kind: if queue.is_empty() { 1 } else { 2 },
                length: queue.len(),
            })
    }

    /// The oldest token under `key`, retaining the entry when it empties.
    pub fn take(&mut self, key: &[u8]) -> Option<T> {
        self.queues.get_mut(key).and_then(VecDeque::pop_front)
    }

    /// Removes `key`'s entry only when it is present and empty.
    pub fn remove_empty(&mut self, key: &[u8]) -> bool {
        let empty = self.queues.get(key).is_some_and(VecDeque::is_empty);
        if empty {
            self.queues.remove(key);
        }
        empty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::BTreeMap;

    use std::ffi::c_void;

    const OK: u32 = 0;
    const OUT_OF_MEMORY: u32 = 1;

    struct Witness(Option<Pool<*mut c_void>>);
    impl Witness {
        fn push(&mut self, key: &[u8], token: *mut c_void) -> u32 {
            if self.0.is_none() {
                self.0 = Pool::create();
            }
            match self.0.as_mut().map(|pool| pool.push(key, token)) {
                Some(Ok(())) => OK,
                _ => OUT_OF_MEMORY,
            }
        }
        fn state(&self, key: &[u8]) -> PoolState {
            self.0
                .as_ref()
                .map(|pool| pool.state(key))
                .unwrap_or_default()
        }
        fn take(&mut self, key: &[u8]) -> *mut c_void {
            self.0
                .as_mut()
                .and_then(|pool| pool.take(key))
                .unwrap_or(std::ptr::null_mut())
        }
        fn remove_empty(&mut self, key: &[u8]) -> u8 {
            self.0
                .as_mut()
                .map_or(0, |pool| u8::from(pool.remove_empty(key)))
        }
    }
    impl Drop for Witness {
        fn drop(&mut self) {
            allocation_budget(-1);
        }
    }

    #[test]
    fn fifo_binary_keys_empty_prefix_and_allocation_failures() {
        allocation_budget(0);
        assert!(Pool::<*mut c_void>::create().is_none());
        allocation_budget(-1);
        let mut pool = Witness(Pool::create());
        assert!(pool.0.is_some());
        let key = b"pool\0binary";
        let first = std::ptr::without_provenance_mut::<c_void>(1);
        let second = std::ptr::without_provenance_mut::<c_void>(2);
        for budget in 0..=2 {
            allocation_budget(budget);
            assert_eq!(pool.push(key, first), OUT_OF_MEMORY);
            assert_eq!(pool.state(key).kind, u32::from(budget == 2));
        }
        allocation_budget(-1);
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
            let mut pool = Witness(None);
            allocation_budget(budget);
            assert_eq!(pool.push(b"fault", token), OUT_OF_MEMORY);
            allocation_budget(-1);
            assert_eq!(pool.state(b"fault").kind, u32::from(budget == 3));
            assert_eq!(pool.state(b"fault").length, 0);
            assert_eq!(pool.push(b"fault", token), OK);
            assert_eq!(pool.take(b"fault"), token);
            assert_eq!(pool.remove_empty(b"fault"), 1);
        }
    }

    #[test]
    fn queue_growth_failure_keeps_existing_fifo_and_tokens() {
        let mut pool = Witness(Pool::create());
        assert!(pool.0.is_some());
        let key = b"growth";
        let token = std::ptr::without_provenance_mut::<c_void>(7);
        assert_eq!(pool.push(key, token), OK);
        let capacity = pool.0.as_ref().unwrap().queues[key.as_slice()].capacity();
        assert!(capacity <= 65536);
        for _ in 1..capacity {
            assert_eq!(pool.push(key, token), OK);
        }
        allocation_budget(0);
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
            let mut pool = Witness(None);
            let mut expected: BTreeMap<&[u8], VecDeque<*mut c_void>> = BTreeMap::new();
            assert_eq!(pool.state(b"absent").kind, 0);
            assert!(pool.take(b"absent").is_null());
            let mut random = 0x0009_1cd8_1a63_u64;
            let mut checks = 0;
            // Miri interprets each round some thousand times slower: it checks a sample.
            let rounds = if cfg!(miri) { 1_000 } else { 100_000 };
            for _ in 0..rounds {
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
            assert_eq!(checks, rounds * keys.len());
        }
        assert_eq!(destroyed(), 0);
    }

    #[cfg(unix)]
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no fork")]
    fn fork_keeps_parent_fifo_and_never_destroys_native_tokens() {
        let mut tokens = tokens();
        let pointers = tokens
            .each_mut()
            .map(|token| std::ptr::from_mut(token).cast::<c_void>());
        {
            let mut pool = Witness(None);
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

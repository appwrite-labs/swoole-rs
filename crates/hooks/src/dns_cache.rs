//! `System::gethostbyname`'s DNS cache: a least-recently-used map from
//! `IPv4.`/`IPv6.` plus the host name to its address, with an optional expiry in
//! seconds, created on first lookup and counting hits and misses. Native code
//! resolves on a miss (c-ares or the async pool) and stores the answer here.
use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};
use swoole_rs_corebase::CCast;

struct Entry<T> {
    value: T,
    /// Unix time it expires at, or 0 for never.
    expires: i64,
    /// Its place in `Lru::order`.
    used: u64,
}

/// Upstream's `LRUCache`: expired entries keep their slot until replaced or evicted.
struct Lru<T> {
    capacity: usize,
    entries: HashMap<Vec<u8>, Entry<T>>,
    /// Least recently used first.
    order: BTreeMap<u64, Vec<u8>>,
    clock: u64,
}

impl<T> Lru<T> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
            order: BTreeMap::new(),
            clock: 0,
        }
    }

    fn touch(&mut self, key: &[u8]) {
        self.clock += 1;
        if let Some(entry) = self.entries.get_mut(key) {
            self.order.remove(&entry.used);
            entry.used = self.clock;
            self.order.insert(self.clock, key.to_vec());
        }
    }

    fn get(&mut self, key: &[u8], now: i64) -> Option<&T> {
        let entry = self.entries.get(key)?;
        if entry.expires < now && entry.expires > 0 {
            return None;
        }
        self.touch(key);
        self.entries.get(key).map(|entry| &entry.value)
    }

    fn set(&mut self, key: &[u8], value: T, expire: i64, now: i64) {
        let expires = if expire <= 0 { 0 } else { now + expire };
        if let Some(entry) = self.entries.get_mut(key) {
            entry.value = value;
            entry.expires = expires;
        } else {
            if self.entries.len() == self.capacity
                && let Some((_, oldest)) = self.order.pop_first()
            {
                self.entries.remove(&oldest);
            }
            let entry = Entry {
                value,
                expires,
                used: 0,
            };
            self.entries.insert(key.to_vec(), entry);
        }
        self.touch(key);
    }

    #[cfg(test)]
    fn del(&mut self, key: &[u8]) {
        if let Some(entry) = self.entries.remove(key) {
            self.order.remove(&entry.used);
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

struct Cache {
    capacity: usize,
    expire: i64,
    lru: Option<Lru<Vec<u8>>>,
    hits: usize,
    misses: usize,
}

static CACHE: Mutex<Cache> = Mutex::new(Cache {
    capacity: 1000,
    expire: 60,
    lru: None,
    hits: 0,
    misses: 0,
});

fn cache() -> MutexGuard<'static, Cache> {
    CACHE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs().cast_signed())
}

fn key(domain: i32, name: &[u8]) -> Vec<u8> {
    let mut key = if domain == libc::AF_INET {
        b"IPv4.".to_vec()
    } else {
        b"IPv6.".to_vec()
    };
    key.extend_from_slice(name);
    key
}

/// `System::set_dns_cache_expire`: entries stored from now on live `expire` seconds
/// (none: forever).
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6System20set_dns_cache_expireEl")
)]
pub extern "C" fn swr_v1_hook_dns_cache_expire(expire: i64) {
    cache().expire = expire;
}

/// `System::set_dns_cache_capacity`: clears and drops the cache; 0 disables it.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6System22set_dns_cache_capacityEm")
)]
pub extern "C" fn swr_v1_hook_dns_cache_capacity(capacity: usize) {
    let mut cache = cache();
    cache.capacity = capacity;
    cache.lru = None;
    cache.hits = 0;
    cache.misses = 0;
}

/// `System::clear_dns_cache`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6System15clear_dns_cacheEv")
)]
pub extern "C" fn swr_v1_hook_dns_cache_clear() {
    let mut cache = cache();
    if let Some(lru) = &mut cache.lru {
        lru.clear();
    }
    cache.hits = 0;
    cache.misses = 0;
}

/// `System::get_dns_cache_hit_ratio`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6System23get_dns_cache_hit_ratioEv")
)]
#[must_use]
pub extern "C" fn swr_v1_hook_dns_cache_hit_ratio() -> f32 {
    let cache = cache();
    let total = cache.hits + cache.misses;
    if total == 0 {
        0.0
    } else {
        cache.hits.c_cast::<f32>() / total.c_cast::<f32>()
    }
}

/// The address cached for `name` in `domain`, creating the cache first if enabled.
#[must_use]
pub fn get(domain: i32, name: &[u8]) -> Option<Vec<u8>> {
    let mut cache = cache();
    let capacity = cache.capacity;
    if cache.lru.is_none() && capacity != 0 {
        cache.lru = Some(Lru::new(capacity));
    }
    let found = cache.lru.as_mut()?.get(&key(domain, name), now()).cloned();
    if found.is_some() {
        cache.hits += 1;
    } else {
        cache.misses += 1;
    }
    found
}

/// Stores a resolved, non-empty `address` for `name`, if the cache exists.
pub fn set(domain: i32, name: &[u8], address: &[u8]) {
    let mut cache = cache();
    let expire = cache.expire;
    if let Some(lru) = &mut cache.lru
        && !address.is_empty()
    {
        lru.set(&key(domain, name), address.to_vec(), expire, now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used_and_honours_expiry() {
        let mut lru = Lru::new(2);
        lru.set(b"a", b"1", 0, 100);
        lru.set(b"b", b"2", 10, 100);
        assert_eq!(lru.get(b"a", 105), Some(&b"1"));
        lru.set(b"c", b"3", 0, 105);
        assert_eq!(lru.get(b"b", 105), None, "b was least recently used");
        assert_eq!(lru.get(b"c", 105), Some(&b"3"));
        lru.set(b"d", b"4", 1, 100);
        assert_eq!(lru.get(b"d", 101), Some(&b"4"), "expiry is exclusive");
        assert_eq!(lru.get(b"d", 102), None);
        lru.set(b"d", b"5", 0, 102);
        assert_eq!(lru.get(b"d", 999), Some(&b"5"));
        assert_eq!(lru.entries.len(), 2);
    }

    // Upstream's core-tests/src/memory/lru_cache.cpp, one test per TEST: `shared_ptr`
    // is `Rc`, pointer identity is `Rc::ptr_eq`, and `lru_cache_test_class`'s
    // destructor counter is `Counted`'s. Every `set` there has no expiry, so `now` is 0.
    use std::cell::Cell;
    use std::rc::Rc;

    struct Counted(Rc<Cell<usize>>);

    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    fn same<T>(found: Option<&Rc<T>>, value: &Rc<T>) -> bool {
        found.is_some_and(|found| Rc::ptr_eq(found, value))
    }

    #[test]
    fn lru_cache_basic() {
        let mut cache = Lru::new(2);
        let val = Rc::new(String::from("hello"));
        let val1 = Rc::new(String::from("hello1"));

        cache.set(b"test", Rc::clone(&val), 0, 0);
        assert!(same(cache.get(b"test", 0), &val));

        cache.set(b"test", Rc::clone(&val1), 0, 0);
        assert!(same(cache.get(b"test", 0), &val1));

        cache.del(b"test");
        assert!(cache.get(b"test", 0).is_none());

        cache.set(b"test", Rc::clone(&val), 0, 0);
        assert!(same(cache.get(b"test", 0), &val));

        cache.clear();
        assert!(cache.get(b"test", 0).is_none());
    }

    #[test]
    fn lru_cache_memory_free() {
        let dtor_num = Rc::new(Cell::new(0));
        let mut cache = Lru::new(2);
        let val = Rc::new(Counted(Rc::clone(&dtor_num)));
        cache.set(b"test", Rc::clone(&val), 0, 0);
        assert!(same(cache.get(b"test", 0), &val));
        drop(val);
        assert_eq!(dtor_num.get(), 0);
        cache.clear();
        assert_eq!(dtor_num.get(), 1);
    }

    #[test]
    fn lru_cache_lru_kick() {
        let dtor_num = Rc::new(Cell::new(0));
        let counted = || Rc::new(Counted(Rc::clone(&dtor_num)));
        let mut cache = Lru::new(2);
        let val = counted();
        let val1 = counted();
        let val2 = counted();
        let val3 = counted();

        cache.set(b"test", Rc::clone(&val), 0, 0);
        assert!(same(cache.get(b"test", 0), &val));
        drop(val);
        assert_eq!(dtor_num.get(), 0);

        cache.set(b"test1", Rc::clone(&val1), 0, 0);
        assert!(same(cache.get(b"test1", 0), &val1));
        drop(val1);
        assert_eq!(dtor_num.get(), 0);

        cache.set(b"test2", Rc::clone(&val2), 0, 0);
        assert!(same(cache.get(b"test2", 0), &val2));
        drop(val2);
        assert_eq!(dtor_num.get(), 1);
        assert!(cache.get(b"test", 0).is_none());

        let val4 = counted();
        cache.set(b"test1", Rc::clone(&val4), 0, 0); // update test1 and will del test2
        assert!(same(cache.get(b"test1", 0), &val4));
        assert_eq!(dtor_num.get(), 2);

        cache.set(b"test3", Rc::clone(&val3), 0, 0);
        assert!(same(cache.get(b"test3", 0), &val3));
        drop(val3);
        assert_eq!(dtor_num.get(), 3);
        assert!(cache.get(b"test2", 0).is_none());

        cache.clear();
        assert_eq!(dtor_num.get(), 4);
    }

    fn get(domain: i32, name: &str) -> Option<Vec<u8>> {
        super::get(domain, name.as_bytes())
    }

    fn set(domain: i32, name: &str, address: &str) {
        super::set(domain, name.as_bytes(), address.as_bytes());
    }

    /// One test owns the process-wide cache.
    #[test]
    fn caches_per_family_and_counts() {
        swr_v1_hook_dns_cache_capacity(1000);
        set(libc::AF_INET, "x", "1.2.3.4");
        assert_eq!(
            get(libc::AF_INET, "x"),
            None,
            "nothing is stored before the cache exists"
        );
        set(libc::AF_INET, "x", "1.2.3.4");
        set(libc::AF_INET, "y", "");
        assert_eq!(get(libc::AF_INET, "x"), Some(b"1.2.3.4".to_vec()));
        assert_eq!(get(libc::AF_INET6, "x"), None);
        assert_eq!(
            get(libc::AF_INET, "y"),
            None,
            "empty answers are not cached"
        );
        assert!((swr_v1_hook_dns_cache_hit_ratio() - 0.25).abs() < f32::EPSILON);
        swr_v1_hook_dns_cache_clear();
        assert_eq!(swr_v1_hook_dns_cache_hit_ratio(), 0.0);
        assert_eq!(get(libc::AF_INET, "x"), None);
        swr_v1_hook_dns_cache_capacity(0);
        set(libc::AF_INET, "x", "1.2.3.4");
        assert_eq!(get(libc::AF_INET, "x"), None);
        assert_eq!(
            swr_v1_hook_dns_cache_hit_ratio(),
            0.0,
            "a disabled cache counts nothing"
        );
        swr_v1_hook_dns_cache_capacity(1000);
    }
}

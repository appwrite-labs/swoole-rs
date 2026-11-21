//! Upstream's `core-tests/src/coroutine/gethostbyname.cpp`, one function per TEST with the
//! same inputs and expected values: `System::gethostbyname` is crates/hooks' `resolve::system_gethostbyname`
//! and the DNS cache calls are crates/hooks' `dns_cache.rs` under their upstream symbols.
//! Each looks up a public name, so `hooks-system-rust` excludes them (external DNS).

use super::hook;
use super::{AF_INET, AF_INET6, TEST_HTTP_DOMAIN, run};
use std::ffi::CStr;
use std::time::{Duration, Instant};

const TEST_DOMAIN_BAIDU: &CStr = c"www.baidu.com";

unsafe extern "C" {
    #[link_name = "_ZN6swoole9coroutine6System22set_dns_cache_capacityEm"]
    fn set_dns_cache_capacity(capacity: usize);
    #[link_name = "_ZN6swoole9coroutine6System20set_dns_cache_expireEl"]
    fn set_dns_cache_expire(expire: i64);
    #[link_name = "_ZN6swoole9coroutine6System15clear_dns_cacheEv"]
    fn clear_dns_cache();
    #[link_name = "_ZN6swoole9coroutine6System23get_dns_cache_hit_ratioEv"]
    fn get_dns_cache_hit_ratio() -> f32;
}

/// `System::gethostbyname(hostname, domain)`, whose timeout defaults to -1: "" when there
/// is no address.
fn gethostbyname(host: &CStr, domain: i32) -> String {
    hook::lookup(host, domain, -1.0)
}

fn hit_ratio() -> f32 {
    // SAFETY: reads the cache's counters.
    unsafe { get_dns_cache_hit_ratio() }
}

fn resolve_cache() {
    run(|| {
        // SAFETY: the DNS cache's settings.
        unsafe { set_dns_cache_capacity(10) };
        let addr1 = gethostbyname(TEST_DOMAIN_BAIDU, AF_INET);
        assert_ne!(addr1, "");
        for _ in 0..100 {
            let addr2 = gethostbyname(TEST_DOMAIN_BAIDU, AF_INET);
            assert_eq!(addr1, addr2);
        }
        assert!(hit_ratio() > 0.99);

        // SAFETY: as above.
        unsafe { set_dns_cache_capacity(0) };
        for _ in 0..5 {
            let addr2 = gethostbyname(TEST_DOMAIN_BAIDU, AF_INET);
            assert_ne!(addr2, "");
        }
        assert!(hit_ratio() < 0.01);
    });
}

fn resolve_cache_inet4_and_inet6() {
    run(|| {
        // SAFETY: the DNS cache's settings.
        unsafe { set_dns_cache_capacity(10) };

        let addr1 = gethostbyname(c"ipv6.sjtu.edu.cn", AF_INET);
        let addr2 = gethostbyname(c"ipv6.sjtu.edu.cn", AF_INET6);

        assert_ne!(addr1, "");
        assert_ne!(addr2, "");
        assert!(!addr1.contains(':'));
        assert!(addr2.contains(':'));

        // Timer::get_absolute_msec()
        let start = Instant::now();

        for _ in 0..100 {
            let addr3 = gethostbyname(c"ipv6.sjtu.edu.cn", AF_INET);
            let addr4 = gethostbyname(c"ipv6.sjtu.edu.cn", AF_INET6);

            assert_eq!(addr1, addr3);
            assert_eq!(addr2, addr4);
        }

        assert!(start.elapsed().as_millis() < 5);
    });
}

fn dns_expire() {
    run(|| {
        // SAFETY: the DNS cache's settings.
        unsafe {
            set_dns_cache_expire(1);
            clear_dns_cache();
        }

        gethostbyname(TEST_HTTP_DOMAIN, AF_INET);
        gethostbyname(TEST_HTTP_DOMAIN, AF_INET);
        assert!(hit_ratio() >= 0.5);

        // sleep(2), which blocks the thread as upstream's does.
        std::thread::sleep(Duration::from_secs(2));
        gethostbyname(TEST_HTTP_DOMAIN, AF_INET);
        assert!(hit_ratio() < 0.35);

        // SAFETY: as above.
        unsafe {
            clear_dns_cache();
            set_dns_cache_expire(60);
        }
    });
}

pub const CASES: super::Cases = &[
    ("resolve_cache", resolve_cache),
    (
        "resolve_cache_inet4_and_inet6",
        resolve_cache_inet4_and_inet6,
    ),
    ("dns_expire", dns_expire),
];

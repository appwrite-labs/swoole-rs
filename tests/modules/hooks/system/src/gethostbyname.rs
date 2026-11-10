//! Upstream's `core-tests/src/coroutine/gethostbyname.cpp`, one function per TEST with the
//! same inputs and expected values: `System::gethostbyname` is `swr_v1_system_gethostbyname`
//! and the DNS cache calls are crates/hooks' `dns_cache.rs` under their upstream symbols.
//! Each looks up a public name, so `hooks-system-rust` excludes them (external DNS).

use super::{AF_INET, AF_INET6, Put, TEST_HTTP_DOMAIN, run};
use std::ffi::{CStr, c_char, c_long, c_void};
use std::time::{Duration, Instant};

const TEST_DOMAIN_BAIDU: &CStr = c"www.baidu.com";

#[link(name = "swoole")]
unsafe extern "C" {
    fn swr_v1_system_gethostbyname(
        host: *const c_char,
        length: usize,
        family: c_long,
        timeout: f64,
        put: Put,
        out: *mut c_void,
    ) -> bool;
    #[link_name = "_ZN6swoole9coroutine6System22set_dns_cache_capacityEm"]
    fn set_dns_cache_capacity(capacity: usize);
    #[link_name = "_ZN6swoole9coroutine6System20set_dns_cache_expireEl"]
    fn set_dns_cache_expire(expire: i64);
    #[link_name = "_ZN6swoole9coroutine6System15clear_dns_cacheEv"]
    fn clear_dns_cache();
    #[link_name = "_ZN6swoole9coroutine6System23get_dns_cache_hit_ratioEv"]
    fn get_dns_cache_hit_ratio() -> f32;
}

unsafe extern "C" fn put_address(out: *mut c_void, data: *const c_char, length: usize) {
    // SAFETY: `out` is the caller's String; `data` holds `length` bytes.
    unsafe {
        let bytes = std::slice::from_raw_parts(data.cast(), length);
        *out.cast::<String>() = String::from_utf8(bytes.to_vec()).unwrap();
    }
}

/// `System::gethostbyname(hostname, domain)`, whose timeout defaults to -1: "" when there
/// is no address.
fn gethostbyname(host: &CStr, domain: i32) -> String {
    let mut address = String::new();
    // SAFETY: the host's bytes; `put_address` writes `address`, live for the call.
    unsafe {
        swr_v1_system_gethostbyname(
            host.as_ptr(),
            host.count_bytes(),
            c_long::from(domain),
            -1.0,
            put_address,
            (&raw mut address).cast(),
        )
    };
    address
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

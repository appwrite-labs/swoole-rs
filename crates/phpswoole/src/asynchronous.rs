//! `swoole_async_coro.cc`: `swoole_async_set()`, the AIO options it shares with
//! `Server::set()` and `Process\Pool::set()`, and `swoole_async_dns_lookup_coro()`
//! with its per-thread cache. Core settings go directly to corebase's record and
//! the lookup to crates/hooks; C++ lends the DNS cache refresh setting.
use crate::{
    constants::IOURING,
    module::PHP_FATAL_ERROR,
    options::{self, get},
};
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::CStr,
    sync::atomic::{AtomicU64, Ordering},
};
use swoole_rs_corebase::{
    log::set_last_error, swoole_rand_any, swr_v1_core_dns_lookup_random,
    swr_v1_core_set_aio_core_worker_num, swr_v1_core_set_aio_max_idle_time,
    swr_v1_core_set_aio_max_wait_time, swr_v1_core_set_aio_worker_num,
    swr_v1_core_set_dns_lookup_random, swr_v1_core_set_enable_coroutine,
    swr_v1_core_set_iouring_entries, swr_v1_core_set_iouring_flag, swr_v1_core_set_iouring_workers,
    swr_v1_core_set_wait_signal,
};
use swoole_rs_zend::{Array, Call, E_ERROR, E_WARNING, Value, bailout, handlers, php_error};

/// `SW_ERROR_OPERATION_NOT_SUPPORT` and `SW_ERROR_DNSLOOKUP_RESOLVE_FAILED`.
const OPERATION_NOT_SUPPORT: i32 = 507;
const DNSLOOKUP_RESOLVE_FAILED: i32 = 711;

handlers! {
    zif_swoole_async_set => async_set;
    zif_swoole_async_dns_lookup_coro => dns_lookup;
}

/// A cached lookup: the address and when it expires (`Timer::get_absolute_msec`).
struct Cached {
    address: Vec<u8>,
    expires: i64,
}

thread_local! {
    /// `request_cache_map`, keyed by the domain's bytes.
    static CACHE: RefCell<HashMap<Vec<u8>, Cached>> = RefCell::new(HashMap::new());
}

fn truthy(value: &Value) -> bool {
    value.truthy()
}

/// `SW_MAX(low, SW_MIN(v, UINT32_MAX))`.
fn bounded(value: i64, low: u32) -> u32 {
    u32::try_from(value.clamp(i64::from(low), i64::from(u32::MAX))).unwrap_or(u32::MAX)
}

/// `php_swoole_async_coro_rshutdown`: the cache goes with the request.
pub(crate) extern "C" fn rshutdown() {
    CACHE.with_borrow_mut(HashMap::clear);
}

/// `php_swoole_set_aio_option(options)`, after MINIT: the AIO pool's sizes and
/// timeouts, and `io_uring`'s when the build has it.
pub fn php_swoole_set_aio_option(options: &Array) {
    if let Some(value) = get(options, c"aio_core_worker_num").and_then(Value::applied_long) {
        swr_v1_core_set_aio_core_worker_num(bounded(value, 1));
    }
    if let Some(value) = get(options, c"aio_worker_num").and_then(Value::applied_long) {
        swr_v1_core_set_aio_worker_num(bounded(value, 1));
    }
    if let Some(value) = get(options, c"aio_max_wait_time").and_then(Value::applied_double) {
        swr_v1_core_set_aio_max_wait_time(value);
    }
    if let Some(value) = get(options, c"aio_max_idle_time").and_then(Value::applied_double) {
        swr_v1_core_set_aio_max_idle_time(value);
    }
    if crate::FLAGS.load(Ordering::Relaxed) & IOURING == 0 {
        return;
    }
    if let Some(value) = get(options, c"iouring_entries").and_then(Value::applied_long) {
        swr_v1_core_set_iouring_entries(bounded(value, 0));
    }
    if let Some(value) = get(options, c"iouring_workers").and_then(Value::applied_long) {
        swr_v1_core_set_iouring_workers(bounded(value, 0));
    }
    if let Some(value) = get(options, c"iouring_flag").and_then(Value::applied_long) {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the native setter assigned the PHP int64_t to uint32_t"
        )]
        let flag = value as u32;
        swr_v1_core_set_iouring_flag(flag);
    }
}

/// `php_swoole_fatal_error(level, message)`: the error code, then the error,
/// whose bailout waits for the handler's end.
fn fatal_error(level: i32, message: &CStr) {
    set_last_error(PHP_FATAL_ERROR);
    // SAFETY: an error from Rust, through the shim's zend_try.
    bailout::defer(unsafe { php_error(level, message) });
}

/// `SwooleG.dns_cache_refresh_time`'s bits: the coroutine DNS cache's lifetime
/// in seconds, 60 from MINIT.
static DNS_REFRESH_SECONDS: AtomicU64 = AtomicU64::new(0x404E_0000_0000_0000);

/// `swoole_async_set(array $settings)`.
fn async_set(call: &Call) {
    if !crate::optional::main_thread() {
        set_last_error(OPERATION_NOT_SUPPORT);
        call.set_bool(true);
        return;
    }
    if !swoole_rs_corebase::thread_reactor().is_null() {
        // E_ERROR bails out: upstream goes no further.
        fatal_error(
            E_ERROR,
            c"eventLoop has already been created. unable to change settings",
        );
        return;
    }
    let Ok(options) = call.arity(1, 1).and_then(|()| call.array_ref(1)) else {
        call.set_bool(false);
        return;
    };
    if !options::set_global(options) {
        return;
    }
    php_swoole_set_aio_option(options);
    if let Some(value) = get(options, c"wait_signal") {
        swr_v1_core_set_wait_signal(truthy(value));
    }
    if let Some(value) = get(options, c"dns_cache_refresh_time").and_then(Value::applied_double) {
        DNS_REFRESH_SECONDS.store(value.to_bits(), Ordering::Relaxed);
    }
    if let Some(value) = get(options, c"thread_num")
        .or_else(|| get(options, c"min_thread_num"))
        .and_then(Value::applied_long)
    {
        swr_v1_core_set_aio_core_worker_num(bounded(value, 1));
    }
    if let Some(value) = get(options, c"max_thread_num").and_then(Value::applied_long) {
        swr_v1_core_set_aio_worker_num(bounded(value, 1));
    }
    if let Some(value) = get(options, c"dns_lookup_random") {
        swr_v1_core_set_dns_lookup_random(truthy(value));
    }
    if let Some(value) = get(options, c"enable_coroutine") {
        swr_v1_core_set_enable_coroutine(truthy(value));
    }
    // A conversion's bailout (which `get` stops at) is the handler's to raise.
    if !bailout::pending() {
        call.set_bool(true);
    }
}

/// `swoole_async_dns_lookup_coro(string $domain, float $timeout = 5, int $type = AF_INET)`.
fn dns_lookup(call: &Call) {
    crate::coroutine_check();
    // SAFETY: a read of the process-wide default, which only the main thread's
    // option setters write.
    let default_timeout = unsafe { swoole_rs_reactor::swr_v1_socket_defaults.timeouts.dns };
    let parsed = call.arity(1, 3).and_then(|()| {
        Ok((
            call.value(1),
            call.double(2, default_timeout)?,
            call.long(3, i64::from(libc::AF_INET))?,
        ))
    });
    let Ok((domain, timeout, family)) = parsed else {
        call.set_bool(false);
        return;
    };
    let Some(name) = domain.and_then(Value::bytes) else {
        fatal_error(E_WARNING, c"invalid domain name");
        call.set_bool(false);
        return;
    };
    if name.is_empty() {
        fatal_error(E_WARNING, c"domain name empty");
        call.set_bool(false);
        return;
    }
    let cached = CACHE.with_borrow(|cache| {
        let now = swoole_rs_timer::absolute_msec();
        cache
            .get(name)
            .filter(|entry| entry.expires > now)
            .map(|entry| entry.address.clone())
    });
    if let Some(address) = cached {
        call.set_bytes(&address);
        return;
    }
    crate::check_reactor();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "C's (int) conversion of the family"
    )]
    let family = family as i32;
    // The coroutine yields.
    let mut answers = swoole_rs_hooks::resolve::dns_lookup(name, family, timeout);
    if answers.is_empty() {
        set_last_error(DNSLOOKUP_RESOLVE_FAILED);
        call.set_bool(false);
        return;
    }
    let pick = if swr_v1_core_dns_lookup_random() {
        usize::try_from(swoole_rand_any()).unwrap_or_default() % answers.len()
    } else {
        0
    };
    let address = answers.swap_remove(pick).into_bytes();
    call.set_bytes(&address);
    let refresh = f64::from_bits(DNS_REFRESH_SECONDS.load(Ordering::Relaxed));
    let now = swoole_rs_timer::absolute_msec();
    #[expect(clippy::cast_possible_truncation, reason = "C's (int64_t) conversion")]
    let expires = now + (refresh * 1000.0) as i64;
    CACHE.with_borrow_mut(|cache| cache.insert(name.to_vec(), Cached { address, expires }));
}

#[cfg(test)]
mod tests {
    use super::bounded;

    #[test]
    fn worker_counts_clamp_like_upstream() {
        assert_eq!(bounded(-5, 1), 1);
        assert_eq!(bounded(0, 1), 1);
        assert_eq!(bounded(0, 0), 0);
        assert_eq!(bounded(8, 1), 8);
        assert_eq!(bounded(i64::MAX, 1), u32::MAX);
    }

    #[test]
    fn dns_cache_lives_a_minute() {
        let bits = super::DNS_REFRESH_SECONDS.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(f64::from_bits(bits), 60.0);
    }
}

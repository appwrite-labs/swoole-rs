// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/dns.cc and src/coroutine/system.cc
// (Swoole contributors).
//! The resolver's coroutine side: `swoole_name_resolver_lookup`, `System::gethostbyname`
//! and `getaddrinfo` over the async pool, `coroutine::dns_lookup` and the pool's DNS
//! handlers. The query itself is crates/clients (dns.rs, cares.rs) and the request
//! record crates/corebase.
use crate::co_socket::Co;
use crate::offload::{AsyncEvent, AsyncOps, Handler, swr_v1_hook_async};
use crate::wait_event::native::{coroutine_yield_ex, current};
use core::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::ffi::CString;
use swoole_rs_clients::dns::DnsSocket;
use swoole_rs_clients::resolver::{self, Request};
use swoole_rs_corebase::{
    NameResolverContext, log, swoole_rand_any, swr_v1_core_dns_lookup_random,
    swr_v1_core_name_resolver_resolve,
};

/// Where an address goes: `put(out, text, length)`.
type Put = unsafe extern "C" fn(*mut c_void, *const c_char, usize);

/// `SW_TIMEOUT_ALL`.
const TIMEOUT_ALL: c_int = 15;

// SAFETY: the extension archive's.
unsafe extern "C" {
    /// `async::dispatch` (`async_threads.rs`, in the extension archive).
    #[link_name = "_ZN6swoole5async8dispatchEPKNS_10AsyncEventE"]
    fn async_dispatch(request: *const c_void) -> *mut c_void;
}

/// `co->yield()` of the current coroutine (fatal outside one).
pub(crate) fn park() {
    let co = current();
    let scheduler = swoole_rs_coroutine::scheduler();
    // SAFETY: this thread's scheduler and its running coroutine.
    unsafe { swoole_rs_coroutine::swr_v1_coroutine_yield(scheduler, co.cast()) };
}

/// `co->yield_ex(timeout)` of the current coroutine (fatal outside one).
pub(crate) fn park_for(timeout: f64) -> bool {
    let co = current();
    // SAFETY: the running coroutine.
    unsafe { coroutine_yield_ex(co.cast(), timeout) }
}

/// `co->resume()`.
pub(crate) unsafe extern "C" fn resume(co: *mut c_void) {
    // SAFETY: a coroutine parked in `coroutine::async`.
    unsafe { swoole_rs_coroutine::resume(co) };
}

/// `async::dispatch`.
unsafe extern "C" fn async_dispatch_event(event: *const AsyncEvent) -> *mut AsyncEvent {
    // SAFETY: a complete event, as the caller promises.
    unsafe { async_dispatch(event.cast()) }.cast()
}

extern "C" fn last_error() -> c_int {
    log::last_error()
}

/// `coroutine::async`'s coroutine and pool operations, which native's `async` uses too.
pub static COROUTINE_ASYNC: AsyncOps = AsyncOps {
    current,
    dispatch: async_dispatch_event,
    park,
    park_for,
    resume,
    last_error,
};

/// `async::handler_gethostbyname`: the request's address, on a pool thread.
unsafe extern "C" fn handler_gethostbyname(event: *mut AsyncEvent) {
    // SAFETY: the pool's copy of the event, exclusive while the handler runs.
    let event = unsafe { &mut *event };
    // SAFETY: its request record, held by the copy.
    (event.retval, event.error) = resolver::host(unsafe { &*event.data });
}

/// `async::handler_getaddrinfo`: the request's addresses, on a pool thread.
unsafe extern "C" fn handler_getaddrinfo(event: *mut AsyncEvent) {
    // SAFETY: the pool's copy of the event, exclusive while the handler runs.
    let event = unsafe { &mut *event };
    // SAFETY: its request record, held by the copy.
    (event.retval, event.error) = resolver::lookup(unsafe { &*event.data });
}

/// `coroutine::async(handler, event, timeout)` over `request`: the completion's
/// error, or none. `request_error` is checked only once the pool finished.
fn run(
    request: &Request,
    handler: Handler,
    retval: isize,
    timeout: f64,
    request_error: bool,
) -> Option<c_int> {
    let mut event = AsyncEvent::request(core::ptr::null_mut(), None, None, core::ptr::null_mut());
    event.data = request.as_ptr();
    event.retval = retval;
    // SAFETY: a complete event over a live request, in a coroutine.
    unsafe { swr_v1_hook_async(&mut event, handler, timeout, &COROUTINE_ASYNC) };
    request.failed(event.retval, event.error, request_error)
}

/// `gethostbyname_impl_with_async`: the address, or empty with the last error set.
fn gethostbyname_with_async(name: &[u8], domain: c_int, timeout: f64) -> Vec<u8> {
    let request = Request::new(domain, 0, 0, name, b"");
    if let Some(error) = run(&request, handler_gethostbyname, 1, timeout, false) {
        log::set_last_error(error);
        return Vec::new();
    }
    request.address()
}

/// `System::gethostbyname`: the DNS cache, then c-ares or the async pool; empty on
/// failure.
pub(crate) fn system_gethostbyname(name: &[u8], domain: c_int, timeout: f64) -> Vec<u8> {
    if let Some(cached) = crate::dns_cache::get(domain, name) {
        return cached;
    }
    let result = match swoole_rs_clients::cares_lookup(&resolver::c_name(name), domain, timeout) {
        Some(list) if list.is_empty() => Vec::new(),
        Some(mut list) => {
            let at = if swr_v1_core_dns_lookup_random() {
                swoole_rand_any().unsigned_abs() as usize % list.len()
            } else {
                0
            };
            list.swap_remove(at)
        }
        None => gethostbyname_with_async(name, domain, timeout),
    };
    crate::dns_cache::set(domain, name, &result);
    result
}
/// `swoole_name_resolver_lookup(name, ctx)`: the registered resolvers, then the
/// coroutine or blocking lookup; the answer goes to `assign(out, ..)`.
///
/// A PHP resolver may bail out, so this frame holds nothing to drop across them.
///
/// # Safety
/// `name` holds `length` bytes; `assign` writes `out`.
pub unsafe fn name_resolver_lookup(
    name: *const c_char,
    length: usize,
    ctx: &mut NameResolverContext,
    out: *mut c_void,
    assign: Put,
) {
    // SAFETY: as the caller promises.
    if unsafe { swr_v1_core_name_resolver_resolve(name, length, ctx, out, assign) } {
        return;
    }
    // SAFETY: as the caller promises.
    let name = unsafe { core::slice::from_raw_parts(name.cast::<u8>(), length) };
    let address = if swoole_rs_coroutine::current().is_null() {
        resolver::gethostbyname(ctx.kind, name)
    } else {
        system_gethostbyname(name, ctx.kind, ctx.timeout)
    };
    // SAFETY: `assign` copies the address.
    unsafe { assign(out, address.as_ptr().cast(), address.len()) };
}

/// `swoole_name_resolver_lookup_call`.
#[repr(C)]
pub struct LookupCall<'a> {
    pub name: *const c_char,
    pub length: usize,
    pub context: &'a mut NameResolverContext,
    pub out: *mut c_void,
    pub assign: Put,
}

/// `swoole_name_resolver_lookup` over a [`LookupCall`]: a `bailout::trap` body.
///
/// # Safety
/// `call` is a live [`LookupCall`] whose fields are live, as [`name_resolver_lookup`]
/// requires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_name_resolver_lookup_run(call: *mut c_void) {
    // SAFETY: as the caller promises.
    let call = unsafe { &mut *call.cast::<LookupCall<'_>>() };
    // SAFETY: as the caller promises.
    unsafe { name_resolver_lookup(call.name, call.length, call.context, call.out, call.assign) };
}

/// A coroutine socket's lookup: the context's type and timeout set first, the
/// answer through `put(out, ..)`; returns the context's `with_port`.
///
/// # Safety
/// `context` is a live `NameResolver::Context`; as [`name_resolver_lookup`].
pub(crate) unsafe fn socket_lookup(
    context: *mut c_void,
    kind: c_int,
    timeout: f64,
    name: &[u8],
    out: *mut c_void,
    put: Put,
) -> bool {
    // SAFETY: as the caller promises.
    let ctx = unsafe { &mut *context.cast::<NameResolverContext>() };
    ctx.kind = kind;
    ctx.timeout = timeout;
    // SAFETY: as the caller promises.
    unsafe { name_resolver_lookup(name.as_ptr().cast(), name.len(), ctx, out, put) };
    ctx.with_port
}

/// `System::gethostbyname(host, family, timeout)` for crates/phpcoroutines and the
/// coroutine socket: the address through `put`; false when there is none.
///
/// # Safety
/// `host` holds `length` bytes; `put` accepts `out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_system_gethostbyname(
    host: *const c_char,
    length: usize,
    family: c_long,
    timeout: f64,
    put: Put,
    out: *mut c_void,
) -> bool {
    // SAFETY: as the caller promises.
    let host = unsafe { core::slice::from_raw_parts(host.cast::<u8>(), length) };
    let address =
        system_gethostbyname(host, c_int::try_from(family).unwrap_or(c_int::MAX), timeout);
    if !address.is_empty() {
        // SAFETY: `put` copies the address.
        unsafe { put(out, address.as_ptr().cast(), address.len()) };
    }
    !address.is_empty()
}

/// `System::getaddrinfo`: each address through `put`; false when there is none.
///
/// # Safety
/// `host` and `service` are NUL-terminated; `put` accepts `out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_system_getaddrinfo(
    host: *const c_char,
    family: c_long,
    socktype: c_long,
    protocol: c_long,
    service: *const c_char,
    timeout: f64,
    put: Put,
    out: *mut c_void,
) -> bool {
    let int = |value: c_long| c_int::try_from(value).unwrap_or(c_int::MAX);
    // SAFETY: as the caller promises.
    let host = unsafe { CStr::from_ptr(host) };
    // SAFETY: as the caller promises.
    let service = unsafe { CStr::from_ptr(service) };
    let request = Request::new(
        int(family),
        int(socktype),
        int(protocol),
        host.to_bytes(),
        service.to_bytes(),
    );
    if let Some(error) = run(&request, handler_getaddrinfo, 0, timeout, true) {
        log::set_last_error(error);
        return false;
    }
    let addresses = request.addresses();
    for address in &addresses {
        // SAFETY: `put` copies the address.
        unsafe { put(out, address.as_ptr().cast(), address.len()) };
    }
    !addresses.is_empty()
}

/// `new Socket(SW_SOCK_UDP)`, deleted on drop.
struct DnsUdp(Co);

impl DnsUdp {
    /// The socket, with `timeout` set when positive.
    fn open(timeout: f64) -> Self {
        let socket = Self((crate::hooked::SOCKETS.open)(
            libc::AF_INET,
            libc::SOCK_DGRAM,
            0,
        ));
        if timeout > 0.0 {
            socket.0.set_timeout(timeout, TIMEOUT_ALL);
        }
        socket
    }
}

impl DnsSocket for DnsUdp {
    fn sendto(&mut self, host: &CStr, port: c_int, data: &[u8]) -> bool {
        self.0.sendto(host.to_bytes(), port, data) != 0
    }

    fn recv(&mut self, buffer: &mut [u8]) -> isize {
        let n = self.0.recv(buffer);
        if n <= 0 && self.0.err_code() == libc::ECANCELED {
            return -isize::try_from(libc::ECANCELED).unwrap_or(isize::MAX);
        }
        n
    }
}

impl Drop for DnsUdp {
    fn drop(&mut self) {
        // SAFETY: the socket `open` made.
        unsafe { (crate::hooked::SOCKETS.free)(self.0.this()) };
    }
}

/// `coroutine::dns_lookup(domain, family, timeout)`, in a coroutine: each address of
/// `domain` (up to its first NUL, as C reads it), up to its own first NUL; none on a
/// failure, which is the last error.
#[must_use]
pub fn dns_lookup(domain: &[u8], family: c_int, timeout: f64) -> Vec<CString> {
    // Only IPv4 and IPv6.
    let family = if family == libc::AF_INET6 {
        libc::AF_INET6
    } else {
        libc::AF_INET
    };
    let name = resolver::c_name(domain);
    let addresses = swoole_rs_clients::cares_lookup(&name, family, timeout)
        .unwrap_or_else(|| swoole_rs_clients::dns::lookup(&name, family, || DnsUdp::open(timeout)));
    addresses
        .iter()
        .map(|address| resolver::c_name(address))
        .collect()
}

/// [`dns_lookup`] for the hooks system cases: each address through `push(list, ..)`.
///
/// # Safety
/// `domain` is NUL-terminated; `push` accepts `list`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_dns_lookup(
    domain: *const c_char,
    family: c_int,
    timeout: f64,
    push: unsafe extern "C" fn(*mut c_void, *const c_char),
    list: *mut c_void,
) {
    // SAFETY: as the caller promises.
    let name = unsafe { CStr::from_ptr(domain) };
    for address in dns_lookup(name.to_bytes(), family, timeout) {
        // SAFETY: a NUL-terminated address `push` copies.
        unsafe { push(list, address.as_ptr()) };
    }
}

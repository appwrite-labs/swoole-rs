// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/dns.cc and src/coroutine/system.cc
// (Swoole contributors).
//! The resolver's coroutine side: `swoole_name_resolver_lookup`, `System::gethostbyname`
//! and `getaddrinfo` over the async pool, `coroutine::dns_lookup` and the pool's DNS
//! handlers. The query itself is crates/clients (dns.rs, cares.rs) and the request
//! record crates/corebase.
use crate::offload::{AsyncEvent, AsyncOps, Handler, swr_v1_hook_async};
use crate::wait_event::native::current;
use core::ffi::{CStr, c_int};
use std::ffi::CString;
use swoole_rs_clients::dns::DnsSocket;
use swoole_rs_clients::resolver::{self, Request};
use swoole_rs_corebase::{
    NameResolverContext, log, swoole_rand_any, swr_v1_core_dns_lookup_random,
    swr_v1_core_name_resolver_resolve,
};

/// `SW_TIMEOUT_ALL`.
const TIMEOUT_ALL: c_int = 15;

/// `co->yield()` of the current coroutine (fatal outside one).
pub(crate) fn park() {
    swoole_rs_coroutine::suspend(current().cast());
}

/// `co->yield_ex(timeout)` of the current coroutine (fatal outside one).
pub(crate) fn park_for(timeout: f64) -> bool {
    let code = swoole_rs_coroutine::yield_timed(current().cast(), timeout);
    swoole_rs_coroutine::yield_resumed(code)
}

extern "C" fn last_error() -> c_int {
    log::last_error()
}

/// `coroutine::async`'s coroutine and pool operations.
pub static COROUTINE_ASYNC: AsyncOps = AsyncOps {
    current,
    dispatch: crate::async_threads::dispatch,
    park,
    park_for,
    resume: swoole_rs_coroutine::resume,
    last_error,
};

/// `async::handler_gethostbyname`: the request's address, on a pool thread.
extern "C" fn handler_gethostbyname(event: &mut AsyncEvent) {
    // Only `run` dispatches it, with a request.
    if let Some(result) = event.record().map(resolver::host) {
        (event.retval, event.error) = result;
    }
}

/// `async::handler_getaddrinfo`: the request's addresses, on a pool thread.
extern "C" fn handler_getaddrinfo(event: &mut AsyncEvent) {
    // As `handler_gethostbyname`.
    if let Some(result) = event.record().map(resolver::lookup) {
        (event.retval, event.error) = result;
    }
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
    let mut event = AsyncEvent::empty();
    event.data = request.as_ptr();
    event.retval = retval;
    swr_v1_hook_async(&mut event, handler, timeout, &COROUTINE_ASYNC);
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
#[must_use]
pub fn system_gethostbyname(name: &[u8], domain: c_int, timeout: f64) -> Vec<u8> {
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
/// coroutine or blocking lookup; the answer goes to `out`.
///
/// A PHP resolver may bail out, so this frame holds nothing to drop across them.
pub fn name_resolver_lookup(name: &[u8], ctx: &mut NameResolverContext, out: &mut Vec<u8>) {
    if swr_v1_core_name_resolver_resolve(name, ctx, out) {
        return;
    }
    *out = if swoole_rs_coroutine::current().is_null() {
        resolver::gethostbyname(ctx.kind, name)
    } else {
        system_gethostbyname(name, ctx.kind, ctx.timeout)
    };
}

/// A coroutine socket's lookup: the context's type and timeout set first, the
/// answer into `out`; returns the context's `with_port`.
pub(crate) fn socket_lookup(
    ctx: &mut NameResolverContext,
    kind: c_int,
    timeout: f64,
    name: &[u8],
    out: &mut Vec<u8>,
) -> bool {
    ctx.kind = kind;
    ctx.timeout = timeout;
    name_resolver_lookup(name, ctx, out);
    ctx.with_port
}

/// `System::getaddrinfo` for crates/phpcoroutines: the addresses, or none with
/// the last error set.
#[must_use]
pub fn system_getaddrinfo(
    host: &[u8],
    family: c_int,
    socktype: c_int,
    protocol: c_int,
    service: &[u8],
    timeout: f64,
) -> Vec<Vec<u8>> {
    let request = Request::new(family, socktype, protocol, host, service);
    if let Some(error) = run(&request, handler_getaddrinfo, 0, timeout, true) {
        log::set_last_error(error);
        return Vec::new();
    }
    request.addresses()
}

/// `new Socket(SW_SOCK_UDP)`, deleted on drop.
struct DnsUdp(crate::hooked::Owned);

impl DnsUdp {
    /// The socket, with `timeout` set when positive.
    fn open(timeout: f64) -> Self {
        let socket = Self(crate::hooked::Owned::open(
            libc::AF_INET,
            libc::SOCK_DGRAM,
            0,
        ));
        if timeout > 0.0 {
            socket.0.socket().set_timeout(timeout, TIMEOUT_ALL);
        }
        socket
    }
}

impl DnsSocket for DnsUdp {
    fn sendto(&mut self, host: &CStr, port: c_int, data: &[u8]) -> bool {
        self.0.socket().sendto(host.to_bytes(), port, data) != 0
    }

    fn recv(&mut self, buffer: &mut [u8]) -> isize {
        let socket = self.0.socket();
        let n = socket.recv(buffer);
        if n <= 0 && socket.err_code() == libc::ECANCELED {
            return -isize::try_from(libc::ECANCELED).unwrap_or(isize::MAX);
        }
        n
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

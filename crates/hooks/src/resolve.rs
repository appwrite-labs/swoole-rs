// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/dns.cc and src/coroutine/system.cc
// (Swoole contributors).
//! The resolver's coroutine side: `swoole_name_resolver_lookup`, `System::gethostbyname`
//! and `getaddrinfo` over the async pool, `coroutine::dns_lookup` and the pool's DNS
//! handlers. The query itself is crates/clients (dns.rs, cares.rs) and the request
//! record crates/corebase.
use core::ffi::{CStr, c_int};
use swoole_rs_clients::dns::{DnsSocket, RecvError, received};
use swoole_rs_clients::resolver::{self, Request};
use swoole_rs_corebase::lent::lend_spare;
use swoole_rs_corebase::offload::{AsyncEvent, COROUTINE_ASYNC, Handler, swr_v1_hook_async};
use swoole_rs_corebase::{
    NameResolverContext, log, swoole_rand_any, swr_v1_core_dns_lookup_random,
    swr_v1_core_name_resolver_resolve,
};

/// `async::handler_getaddrinfo`: the request's addresses, on a pool thread.
extern "C" fn handler_getaddrinfo(event: &mut AsyncEvent) {
    if let Some((retval, error)) = event.record().map(resolver::lookup) {
        event.set_result(retval, error);
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
    event.set_record(request.reference());
    event.set_result(retval, 0);
    swr_v1_hook_async(&mut event, handler, timeout, &COROUTINE_ASYNC);
    let (retval, error) = event.result();
    request.failed(retval, error, request_error)
}

/// `gethostbyname_impl_with_async`: the address, or empty with the last error set.
fn gethostbyname_with_async(name: &[u8], domain: c_int, timeout: f64) -> Vec<u8> {
    let request = Request::new(domain, 0, 0, name, b"");
    if let Some(error) = run(&request, resolver::handler_gethostbyname, 1, timeout, false) {
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
    *out = if swoole_rs_corebase::coroutine::current().is_null() {
        resolver::gethostbyname(ctx.options.get().0, name)
    } else {
        system_gethostbyname(name, ctx.options.get().0, ctx.timeout)
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
    ctx.options.set_kind(kind);
    ctx.timeout = timeout;
    name_resolver_lookup(name, ctx, out);
    ctx.options.get().1
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
struct DnsUdp(crate::co_socket::OwnedCo);

impl DnsUdp {
    /// The socket, with `timeout` set when positive.
    fn open(timeout: f64) -> Self {
        let socket = Self(crate::hooked::open(libc::AF_INET, libc::SOCK_DGRAM, 0));
        if timeout > 0.0 {
            socket
                .0
                .co()
                .set_timeout(timeout, swoole_rs_corebase::reactor::TimeoutKind::ALL);
        }
        socket
    }
}

impl DnsSocket for DnsUdp {
    fn sendto(&mut self, host: &CStr, port: c_int, data: &[u8]) -> bool {
        self.0.co().sendto(host.to_bytes(), port, data) != Some(0)
    }

    fn recv(&mut self, buffer: &mut [u8]) -> Result<usize, RecvError> {
        let socket = self.0.co();
        let n = lend_spare(buffer, |out| socket.recv(out)).0;
        received(n, socket.err_code() == libc::ECANCELED)
    }
}

/// `coroutine::dns_lookup(domain, family, timeout)`, in a coroutine: each address of
/// `domain` (up to its first NUL, as C reads it), up to its own first NUL; none on a
/// failure, which is the last error.
#[must_use]
pub fn dns_lookup(domain: &[u8], family: c_int, timeout: f64) -> Vec<Vec<u8>> {
    // Only IPv4 and IPv6.
    let family = if family == libc::AF_INET6 {
        libc::AF_INET6
    } else {
        libc::AF_INET
    };
    let name = resolver::c_name(domain);
    let mut addresses = swoole_rs_clients::cares_lookup(&name, family, timeout)
        .unwrap_or_else(|| swoole_rs_clients::dns::lookup(&name, family, || DnsUdp::open(timeout)));
    for address in &mut addresses {
        let end = address.iter().position(|&byte| byte == 0);
        address.truncate(end.unwrap_or(address.len()));
    }
    addresses
}

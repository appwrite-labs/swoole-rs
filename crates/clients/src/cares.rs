// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/dns.cc (Swoole contributors).
//! Per-query c-ares channel, descriptor registry, results and deferred completion.
//! Only Swoole's reactor drives c-ares; no c-ares event thread is enabled.
use crate::ip::swr_v1_clients_ip_text;
use std::{
    cell::Cell,
    ffi::{CStr, c_int, c_long, c_void},
    ptr,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use swoole_rs_cares_sys::{self as sdk, swrs_cares_metadata as metadata};
use swoole_rs_corebase::constants::{FdType, SW_DNS_SERVER_PORT};
use swoole_rs_corebase::log;
use swoole_rs_corebase::sys::Text;
use swoole_rs_corebase::{CCast, Hostent};
use swoole_rs_error::ErrorCode;
use swoole_rs_reactor::{Event, Events, Running, SocketRecord, event_api};
use swoole_rs_tls::event_socket::EventSocket;

/// A channel's socket-state callback: `(query id, fd, readable, writable)`.
type SocketState = extern "C" fn(*mut c_void, c_int, c_int, c_int);
/// A lookup's callback: `(query id, status, timeouts, entry)`, the entry lent for
/// the call and only on success.
type HostCallback = extern "C" fn(*mut c_void, c_int, c_int, Option<Hostent<'_>>);
// The userdata `data` is a query's id, which c-ares hands back to the callbacks
// without reading it; they look the query up ([`registered`]).
unsafe extern "C" {
    /// c-ares' description of a status: a static NUL-terminated string for every code
    /// (`"unknown"` for one it has none for), which reads nothing else.
    safe fn ares_strerror(code: c_int) -> Text<'static>;
    /// `ares_init_options` through the shim: the SDK's status, and the channel in
    /// `channel` on success only (c-ares leaves it untouched otherwise).
    safe fn swrs_cares_init(
        channel: &mut Option<sdk::OwnedChannel>,
        lookups: Text<'_>,
        timeout: c_int,
        tries: c_int,
        callback: Option<SocketState>,
        data: *mut c_void,
        mask: c_int,
    ) -> c_int;
    /// Sets the channel's one IPv4 server, `host` parsed as `inet_pton` parses it,
    /// where the SDK's server API allows.
    safe fn swrs_cares_server(channel: &sdk::Channel, host: Text<'_>, port: u16);
    /// Starts a lookup of `name`, which c-ares copies; `callback(data, ...)` runs once.
    safe fn ares_gethostbyname(
        channel: &sdk::Channel,
        name: Text<'_>,
        family: c_int,
        callback: Option<HostCallback>,
        data: *mut c_void,
    );
}

/// The coroutine a query parks.
mod co {
    use std::ffi::c_long;
    use swoole_rs_coroutine::State;

    pub(super) use swoole_rs_coroutine::{
        Resume, Waker, current_cid, yield_resumed, yield_timed_armed,
    };

    /// `Coroutine::get_current_safe()`.
    pub(super) use swoole_rs_coroutine::current_safe as current;

    /// `co->is_suspending()` of coroutine `cid`: false once it ended.
    pub(super) fn suspending(cid: c_long) -> bool {
        swoole_rs_coroutine::coroutine_state(cid) == Some(State::Waiting)
    }
}
const ORIGIN: &std::ffi::CStr = c"coroutine::dns_lookup_impl_with_cares";
/// A query's state, shared with its c-ares callbacks through [`QUERIES`]: each
/// field is a cell set in place or taken out and put back around plain Rust work,
/// never around a c-ares call, which can run the callbacks.
struct Query {
    /// The channel the query runs on, set once initialised and taken by `lookup`
    /// to destroy it: a registered wrapper finds it here.
    channel: Cell<Option<Rc<sdk::OwnedChannel>>>,
    error: Cell<i32>,
    completed: Cell<bool>,
    /// The parked coroutine's waker, which its park stores; none before it parks
    /// and once a completion is queued.
    waker: Cell<Option<co::Waker>>,
    /// The query's coroutine's id, which the coroutine crate looks up.
    cid: c_long,
    retired: Arc<AtomicBool>,
    /// The wrapper of each descriptor c-ares watches, live until `socket_state` detaches it: one per
    /// server it asks, so a list beats a hash map.
    sockets: Cell<Vec<(i32, EventSocket)>>,
    results: Cell<Vec<Vec<u8>>>,
}
impl Query {
    /// The query's channel, while it has one.
    fn channel(&self) -> Option<Rc<sdk::OwnedChannel>> {
        let channel = self.channel.take();
        self.channel.set(channel.clone());
        channel
    }
}

thread_local! {
    /// The last query's id on this thread.
    static LAST_QUERY: Cell<usize> = const { Cell::new(0) };
    /// The queries running on this thread, each under the id c-ares hands back as
    /// its callbacks' userdata and a wrapper keeps as its object.
    static QUERIES: Cell<Vec<(usize, Rc<Query>)>> = const { Cell::new(Vec::new()) };
}

/// `f` over the running queries, taken out and put back.
fn with_queries<R>(f: impl FnOnce(&mut Vec<(usize, Rc<Query>)>) -> R) -> R {
    let mut queries = QUERIES.take();
    let result = f(&mut queries);
    QUERIES.set(queries);
    result
}

/// The running query whose id `data` is, if any.
fn registered(data: *mut c_void) -> Option<Rc<Query>> {
    let id = data.addr();
    with_queries(|queries| {
        let (_, query) = queries.iter().find(|&&(query, _)| query == id)?;
        Some(Rc::clone(query))
    })
}

struct Completion {
    retired: Arc<AtomicBool>,
    waker: co::Waker,
    resume: fn(co::Waker),
}
/// The deferred resume: the reactor runs it after the pass, unless the query
/// retired first, or drops it if the reactor is freed first.
impl event_api::Deferred for Completion {
    fn run(self) {
        if !self.retired.load(Ordering::Relaxed) {
            // A query is retired before its coroutine may end, so a live query's
            // coroutine is still parked waiting for this resume.
            (self.resume)(self.waker);
        }
    }
    fn dispose(self) {}
}
/// The `SW_FD_CARES` read and write handlers: the wrapper's object is its query.
fn readable(_: Running<'_>, event: &mut Event) -> i32 {
    if process(event, false) { 0 } else { -1 }
}
fn writable(_: Running<'_>, event: &mut Event) -> i32 {
    if process(event, true) { 0 } else { -1 }
}
/// Hands the ready descriptor to its query's channel: false for an event that has none.
fn process(event: &Event, write: bool) -> bool {
    let fd = event.fd();
    let Some(socket) = event.socket() else {
        return false;
    };
    // A wrapper's object is its query's id (`socket_state` sets it).
    let Some(query) = registered(socket.read(|socket| socket.object)) else {
        return false;
    };
    // A wrapper is registered only by the channel's callback, so the channel is set.
    let Some(channel) = query.channel() else {
        return false;
    };
    let (read, write) = if write { (-1, fd) } else { (fd, -1) };
    sdk::ares_process_fd(&channel, read, write);
    true
}
/// Reactor destruction: no query still uses the library after its channel was destroyed.
fn cleanup() {
    // SAFETY: the reactor runs this once, after its c-ares handlers retire. It takes
    // nothing, and c-ares counts its initialisations: a call without one does nothing.
    unsafe { sdk::ares_library_cleanup() };
}
/// The first query of a reactor's lifetime: its handlers and library cleanup.
fn install() {
    let readable: swoole_rs_reactor::Handler = readable;
    for (event, handler) in [(Events::READ, readable), (Events::WRITE, writable)] {
        event_api::set_handler(FD, event, Some(handler));
    }
    // `cleanup` runs once, when the reactor goes.
    event_api::on_destroy(cleanup);
}
const FD: i32 = FdType::SW_FD_CARES as i32;
extern "C" fn socket_state(data: *mut c_void, fd: i32, readable: i32, writable: i32) {
    let Some(query) = registered(data) else {
        return;
    };
    let events = if readable != 0 {
        Events::READ
    } else {
        Events::NONE
    } | if writable != 0 {
        Events::WRITE
    } else {
        Events::NONE
    };
    // Taken out and put back on every path below; while out, only the reactor's
    // registration calls run, which neither yield nor reach c-ares.
    let mut sockets = query.sockets.take();
    if let Some(index) = sockets.iter().position(|&(watched, _)| watched == fd) {
        if events.is_empty() {
            // Out of the registry, then out of the reactor and freed keeping the
            // descriptor, which c-ares keeps: `Socket::free` defers the record to the
            // next pass while a reactor runs, as the event being handled may hold it.
            let (_, wrapper) = sockets.remove(index);
            query.sockets.set(sockets);
            wrapper.free(true);
            return;
        }
        if let Some((_, wrapper)) = sockets.get(index) {
            // Update this registered wrapper, which lives until it is freed above.
            _ = if wrapper.events().is_empty() {
                wrapper.add(events)
            } else {
                wrapper.set(events)
            };
        }
        query.sockets.set(sockets);
        return;
    }
    query.sockets.set(sockets);
    if events.is_empty() {
        log::warning(ORIGIN, format_args!("error events, fd={fd}"));
        return;
    }
    // c-ares lends fd; the query's id becomes the wrapper's object.
    // `make_socket(fd, SW_FD_CARES)`: the fresh record, owned by the registry until
    // this function takes it out and frees it.
    let wrapper = EventSocket::adopt(Box::new(SocketRecord::with_fd(fd, FD)));
    wrapper.socket().edit(|wrapper| wrapper.object = data);
    // Register this fresh wrapper.
    _ = wrapper.add(events);
    let mut sockets = query.sockets.take();
    sockets.push((fd, wrapper));
    query.sockets.set(sockets);
}
extern "C" fn host(data: *mut c_void, status: i32, timeouts: i32, entry: Option<Hostent<'_>>) {
    let Some(query) = registered(data) else {
        return;
    };
    if timeouts > 0 || status != metadata().success {
        let error = if timeouts > 0 {
            ErrorCode::SW_ERROR_DNSLOOKUP_RESOLVE_TIMEOUT as i32
        } else {
            status
        };
        query.error.set(error);
    } else if let Some(entry) = entry {
        // The family of the storage the addresses are.
        let version = if entry.is_inet() { 4 } else { 6 };
        for address in entry.addresses() {
            let text = swr_v1_clients_ip_text(version, address);
            let length = usize::try_from(text.length).unwrap_or(0);
            let text = text.bytes.get(..length).unwrap_or_default().to_vec();
            let mut results = query.results.take();
            results.push(text);
            query.results.set(results);
        }
    }
    let waker = query.waker.get();
    // A waker is the query's original coroutine's park, which waits for this
    // completion while suspended.
    if let Some(waker) = waker.filter(|_| co::suspending(query.cid)) {
        let retired = query.retired.clone();
        let task = Completion {
            retired,
            waker,
            resume: co::Waker::wake,
        };
        // c-ares runs on a thread whose reactor is live.
        if event_api::swoole_event_defer_value(task).is_err() {
            swoole_rs_corebase::fatal("c-ares finished on a thread with no reactor");
        }
        // A completion was queued, so further SDK callbacks cannot enqueue it again.
        query.waker.set(None);
    } else {
        // Synchronous completion is recorded before the initiating SDK call returns.
        query.completed.set(true);
    }
}

/// `dns_lookup_impl_with_cares(domain, family, timeout)`: the addresses c-ares finds
/// through the configured DNS server; a failure is the last error. All calls and
/// callbacks for this query occur on the invoking reactor thread, in a coroutine.
pub(crate) fn lookup(domain: &CStr, family: i32, timeout: f64) -> Vec<Vec<u8>> {
    // The configured server's host, NUL-terminated: empty when none is set.
    let (server_host, port) = crate::dns::server();
    let server = CStr::from_bytes_until_nul(&server_host)
        .ok()
        .filter(|host| !host.is_empty());
    let port = port.c_cast::<u16>();
    if !event_api::isset_handler(FD, Events::READ) {
        // SAFETY: the handler absence identifies the first query in this reactor lifetime.
        unsafe { sdk::ares_library_init(metadata().library_all) };
        install();
    }
    let coroutine = co::current();
    // Shared from here on, by this frame and by the callbacks, which find it by id.
    let query = Rc::new(Query {
        channel: Cell::new(None),
        error: Cell::new(0),
        completed: Cell::new(false),
        waker: Cell::new(None),
        cid: co::current_cid(),
        retired: Arc::new(AtomicBool::new(false)),
        sockets: Cell::new(Vec::new()),
        results: Cell::new(Vec::new()),
    });
    let id = LAST_QUERY.get() + 1;
    LAST_QUERY.set(id);
    with_queries(|queries| queries.push((id, Rc::clone(&query))));
    let data = ptr::without_provenance_mut(id);
    let mut channel = None;
    let metadata = metadata();
    let mask = metadata.timeout_ms | metadata.tries | metadata.socket_state | metadata.lookups;
    let status = swrs_cares_init(
        &mut channel,
        c"fb".into(),
        millis(timeout),
        swoole_rs_corebase::swr_v1_core_dns_tries(),
        Some(socket_state),
        data,
        mask,
    );
    // Publish the initialized channel before issuing any SDK query.
    let channel = channel.filter(|_| status == metadata.success).map(Rc::new);
    query.channel.set(channel.clone());
    if let Some(channel) = &channel {
        if let Some(server) = server {
            if metadata.server_api != 0 {
                swrs_cares_server(channel, server.into(), port);
                if metadata.server_api == 1 && u32::from(port) != SW_DNS_SERVER_PORT {
                    log::warning(
                        ORIGIN,
                        format_args!("not support to set port of dns server"),
                    );
                }
            } else {
                log::warning(ORIGIN, format_args!("not support to set dns server"));
            }
        }
        ares_gethostbyname(channel, domain.into(), family, Some(host), data);
        let done = query.error.get() != 0 || query.completed.get();
        if !done {
            // `co->yield_ex(timeout)` of the query's coroutine, which is the running
            // one; no Rust reference spans the suspension. The code is the resume code
            // the coroutine holds as it runs again.
            let arm = |waker| query.waker.set(Some(waker));
            let code = co::yield_timed_armed(coroutine, timeout, arm);
            _ = co::yield_resumed(code);
            if code == co::Resume::Canceled {
                // Runs only callbacks on this live query, synchronously.
                sdk::ares_cancel(channel);
            } else if code == co::Resume::TimedOut {
                // Process SDK timeouts before recording the original timeout status.
                sdk::ares_process_fd(channel, -1, -1);
                // Record timeout after SDK callbacks, matching original order.
                query.error.set(metadata.timeout);
            }
        }
        // The stored outcome, before destroy can invoke more callbacks.
        let error = query.error.get();
        if error != 0 {
            let code = if error == metadata.cancelled {
                ErrorCode::SW_ERROR_CO_CANCELED
            } else if error == metadata.timeout {
                ErrorCode::SW_ERROR_DNSLOOKUP_RESOLVE_TIMEOUT
            } else {
                ErrorCode::SW_ERROR_DNSLOOKUP_RESOLVE_FAILED
            };
            // The exact native error mapping, before destroying the SDK channel.
            log::set_last_error(code as i32);
        }
        // All deferred completions share this token; none may resume a retired query.
        query.retired.store(true, Ordering::Relaxed);
    } else {
        let reason = ares_strerror(status).get().to_string_lossy();
        log::warning(
            ORIGIN,
            format_args!("ares_init_options() failed, Error: {reason}[{status}]"),
        );
    }
    // `ares_destroy`, while the query is registered for its synchronous callbacks.
    drop(query.channel.take());
    drop(channel);
    // The channel and its callbacks are gone: the query goes, once.
    with_queries(|queries| queries.retain(|&(query, _)| query != id));
    query.results.take()
}
fn millis(timeout: f64) -> i32 {
    (timeout * 1000.0).c_cast::<i32>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use swoole_rs_reactor::event_api::Deferred;
    thread_local! {
        static RESUMED: Cell<usize> = const { Cell::new(0) };
    }
    fn count_resume(_: co::Waker) {
        RESUMED.set(RESUMED.get() + 1);
    }
    fn completion(retired: &Arc<AtomicBool>) -> Completion {
        Completion {
            retired: retired.clone(),
            waker: co::Waker::NONE,
            resume: count_resume,
        }
    }
    #[test]
    fn retired_completion_releases_token_without_resuming() {
        let retired = Arc::new(AtomicBool::new(true));
        let task = completion(&retired);
        assert_eq!(
            Arc::strong_count(&retired),
            2,
            "queue retains retirement token"
        );
        task.run();
        assert_eq!(RESUMED.get(), 0, "a retired query is not resumed");
        assert_eq!(Arc::strong_count(&retired), 1, "run releases the token");
        completion(&retired).dispose();
        assert_eq!(
            Arc::strong_count(&retired),
            1,
            "discard releases payload without invocation"
        );
        assert_eq!(RESUMED.get(), 0);
    }
    #[test]
    fn live_completion_resumes_once() {
        let retired = Arc::new(AtomicBool::new(false));
        completion(&retired).run();
        assert_eq!(RESUMED.get(), 1, "resume was delivered exactly once");
        assert_eq!(Arc::strong_count(&retired), 1, "run released the token");
    }
}

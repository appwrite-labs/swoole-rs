// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/dns.cc (Swoole contributors).
//! Per-query c-ares channel, descriptor registry, results and deferred completion.
//! Only Swoole's reactor drives c-ares; no c-ares event thread is enabled.
use crate::ip::swr_v1_clients_ip_text;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ffi::{CStr, c_char, c_void},
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use swoole_rs_cares_sys as sdk;
use swoole_rs_corebase::constants::{EventType, FdType, SW_DNS_SERVER_PORT};
use swoole_rs_corebase::log;
use swoole_rs_error::ErrorCode;
use swoole_rs_reactor::{Event, Running, Socket, SocketRecord, event_api, socket_upstream};

/// The coroutine a query parks: a `swoole::Coroutine`, which starts with its
/// `swoole_rs_coroutine::Coroutine`.
mod co {
    use std::ffi::c_void;
    use swoole_rs_coroutine::{Coroutine, RC_CANCELED, RC_TIMEDOUT, STATE_WAITING};

    pub(super) use swoole_rs_coroutine::{resume, yield_ex};

    /// `Coroutine::get_current_safe()`.
    pub(super) fn current() -> *mut c_void {
        let co = swoole_rs_coroutine::current().cast::<c_void>();
        if co.is_null() {
            // SAFETY: Swoole's fatal error hook; this frame owns nothing.
            unsafe {
                swoole_rs_corebase::fatal_error(10001, c"API must be called in the coroutine");
            }
        }
        co
    }

    fn record(co: *mut c_void) -> (i32, i32) {
        let co = co.cast::<Coroutine>();
        // SAFETY: a live coroutine's scalar fields.
        let co = unsafe { &*co };
        (co.state, co.resume_code)
    }

    pub(super) fn suspending(co: *mut c_void) -> bool {
        record(co).0 == STATE_WAITING
    }

    pub(super) fn canceled(co: *mut c_void) -> bool {
        record(co).1 == RC_CANCELED
    }

    pub(super) fn timedout(co: *mut c_void) -> bool {
        record(co).1 == RC_TIMEDOUT
    }
}
const ORIGIN: &std::ffi::CStr = c"coroutine::dns_lookup_impl_with_cares";
/// A query's state, shared with its c-ares callbacks through the userdata
/// pointer: each field is changed in place, so no borrow of it spans an effect.
struct Query {
    channel: Cell<*mut sdk::Channel>,
    error: Cell<i32>,
    completed: Cell<bool>,
    coroutine: Cell<*mut c_void>,
    retired: Arc<AtomicBool>,
    sockets: RefCell<HashMap<i32, *mut SocketRecord>>,
    results: RefCell<Vec<Vec<u8>>>,
}
struct Completion {
    retired: Arc<AtomicBool>,
    coroutine: *mut c_void,
    resume: unsafe extern "C-unwind" fn(*mut c_void),
}
fn metadata() -> &'static sdk::Metadata {
    // SAFETY: the SDK shim returns its immutable process-lifetime constant table.
    let metadata = unsafe { sdk::swrs_cares_metadata() };
    // SAFETY: this immutable static table has the exact shared SDK adapter layout.
    unsafe { &*metadata }
}
/// The query behind a c-ares userdata or wrapper object.
/// # Safety
/// `state` is a live query's userdata: `lookup` keeps it until its channel is destroyed.
unsafe fn query<'a>(state: *mut c_void) -> &'a Query {
    // SAFETY: as the caller promises; the query is only shared, never borrowed mutably.
    unsafe { &*state.cast::<Query>() }
}
/// The deferred resume: the reactor runs it after the pass, unless the query
/// retired first, or drops it if the reactor is freed first.
impl event_api::Deferred for Completion {
    fn run(self) {
        if !self.retired.load(Ordering::Relaxed) {
            // SAFETY: a query is retired before its coroutine may end, so a live
            // query's coroutine is still parked waiting for this resume.
            unsafe { (self.resume)(self.coroutine) };
        }
    }
    fn dispose(self) {}
}
/// The `SW_FD_CARES` read and write handlers: the wrapper's object is its query.
extern "C" fn readable(_: Running<'_>, event: &mut Event) -> i32 {
    process(event, 0)
}
extern "C" fn writable(_: Running<'_>, event: &mut Event) -> i32 {
    process(event, 1)
}
fn process(event: &Event, write: u8) -> i32 {
    let fd = event.fd();
    let Some(socket) = event.socket() else {
        return -1;
    };
    let data = socket.read(|socket| socket.object);
    // SAFETY: the query's channel stays live while its wrappers are registered.
    let channel = unsafe { query(data) }.channel.get();
    let (read, write) = if write == 0 { (fd, -1) } else { (-1, fd) };
    // SAFETY: SDK callbacks run synchronously on this reactor thread.
    unsafe { sdk::ares_process_fd(channel, read, write) };
    0
}
/// Reactor destruction: no query still uses the library after its channel was destroyed.
unsafe extern "C-unwind" fn cleanup(_: *mut c_void) {
    // SAFETY: the reactor runs this once, after its c-ares handlers retire.
    unsafe { sdk::ares_library_cleanup() };
}
/// The first query of a reactor's lifetime: its handlers and library cleanup.
fn install() {
    let read = EventType::SW_EVENT_READ as i32;
    let write = EventType::SW_EVENT_WRITE as i32;
    // SAFETY: the handlers need only the live event of a registered c-ares wrapper.
    unsafe { event_api::swoole_event_set_handler(FD, read, Some(readable)) };
    // SAFETY: as above.
    unsafe { event_api::swoole_event_set_handler(FD, write, Some(writable)) };
    // SAFETY: `cleanup` runs once, when the reactor goes; it keeps no data.
    unsafe { event_api::on_destroy_with(cleanup, ptr::null_mut()) };
}
const FD: i32 = FdType::SW_FD_CARES as i32;
/// `Socket::free` of a wrapper whose descriptor c-ares keeps: the record goes on the
/// next deferred pass while a reactor runs, as the event being handled may hold it.
unsafe fn free(socket: *mut SocketRecord) {
    // SAFETY: a live wrapper; c-ares retains the OS descriptor.
    if let Some(wrapper) = unsafe { Socket::from_ptr(socket) } {
        wrapper.edit(|wrapper| wrapper.fd = -1);
    }
    let detached = Detached(socket);
    let Err(Detached(socket)) = (if event_api::swr_v1_event_reactor().is_null() {
        Err(detached)
    } else {
        event_api::swoole_event_defer_value(detached)
    }) else {
        return;
    };
    // SAFETY: the detached wrapper, unused since.
    unsafe { SocketRecord::free(socket) };
}
/// A detached wrapper the next deferred pass frees (a freed reactor leaks it, as
/// upstream's).
struct Detached(*mut SocketRecord);
impl event_api::Deferred for Detached {
    fn run(self) {
        // SAFETY: the detached wrapper, which nothing has used since `free`.
        unsafe { SocketRecord::free(self.0) };
    }
    fn dispose(self) {}
}
unsafe extern "C" fn socket_state(data: *mut c_void, fd: i32, readable: i32, writable: i32) {
    let events = if readable != 0 {
        EventType::SW_EVENT_READ as i32
    } else {
        0
    } | if writable != 0 {
        EventType::SW_EVENT_WRITE as i32
    } else {
        0
    };
    // SAFETY: c-ares passes the live query's userdata.
    let query = unsafe { query(data) };
    let socket = query.sockets.borrow().get(&fd).copied();
    let wrapper = if let Some(socket) = socket {
        if events == 0 {
            // SAFETY: remove the registered native socket before detaching its descriptor.
            unsafe { event_api::swoole_event_del(socket) };
            // SAFETY: free detaches fd first; c-ares retains ownership of the OS descriptor.
            unsafe { free(socket) };
            query.sockets.borrow_mut().remove(&fd);
            return;
        }
        // SAFETY: a registered wrapper, live while the registry holds it.
        unsafe { Socket::from_ptr(socket) }
    } else {
        if events == 0 {
            log::warning(ORIGIN, format_args!("error events, fd={fd}"));
            return;
        }
        // c-ares lends fd; this stable query becomes the wrapper's callback userdata.
        let socket = socket_upstream::make_socket_fd(fd, FD);
        // SAFETY: the fresh record, or null.
        let wrapper = unsafe { Socket::from_ptr(socket) };
        if let Some(wrapper) = wrapper {
            wrapper.edit(|wrapper| wrapper.object = data);
        }
        query.sockets.borrow_mut().insert(fd, socket);
        wrapper
    };
    let Some(wrapper) = wrapper else {
        return;
    };
    if wrapper.read(|wrapper| wrapper.events) != 0 {
        // SAFETY: update this existing registered wrapper with the requested event mask.
        unsafe { event_api::swoole_event_set(wrapper.as_ptr(), events) };
    } else {
        // SAFETY: register this fresh wrapper with the requested event mask.
        unsafe { event_api::swoole_event_add(wrapper.as_ptr(), events) };
    }
}
unsafe extern "C" fn host(
    data: *mut c_void,
    status: i32,
    timeouts: i32,
    entry: *mut libc::hostent,
) {
    // SAFETY: c-ares passes the live query's userdata.
    let query = unsafe { query(data) };
    if timeouts > 0 || status != metadata().success {
        let error = if timeouts > 0 {
            ErrorCode::SW_ERROR_DNSLOOKUP_RESOLVE_TIMEOUT as i32
        } else {
            status
        };
        query.error.set(error);
    } else {
        // SAFETY: a successful SDK callback lends hostent and its terminated address list.
        let mut addresses = unsafe { (*entry).h_addr_list };
        // SAFETY: the successful SDK hostent identifies the address storage family.
        let family = unsafe { (*entry).h_addrtype };
        while !addresses.is_null() {
            // SAFETY: each list slot is valid until the first null terminator.
            let address = unsafe { *addresses };
            if address.is_null() {
                break;
            }
            let (version, length) = if family == libc::AF_INET {
                (4, 4)
            } else {
                (6, 16)
            };
            // SAFETY: the successful hostent supplies 4 or 16 initialized address bytes.
            let address = unsafe { std::slice::from_raw_parts(address.cast::<u8>(), length) };
            let text = swr_v1_clients_ip_text(version, address);
            let length = usize::try_from(text.length).unwrap_or(0);
            let text = text.bytes.get(..length).unwrap_or_default().to_vec();
            query.results.borrow_mut().push(text);
            // SAFETY: the SDK address list has another slot or its terminating null.
            addresses = unsafe { addresses.add(1) };
        }
    }
    let coroutine = query.coroutine.get();
    // SAFETY: a nonnull identity is the query's original live coroutine.
    if !coroutine.is_null() && co::suspending(coroutine) {
        let retired = query.retired.clone();
        let task = Completion {
            retired,
            coroutine,
            resume: co::resume,
        };
        // c-ares runs on a thread whose reactor is live.
        if event_api::swoole_event_defer_value(task).is_err() {
            std::process::abort();
        }
        // A completion was queued, so further SDK callbacks cannot enqueue it again.
        query.coroutine.set(ptr::null_mut());
    } else {
        // Synchronous completion is recorded before the initiating SDK call returns.
        query.completed.set(true);
    }
}

/// `dns_lookup_impl_with_cares(domain, family, timeout)`: the addresses c-ares finds
/// through the configured DNS server; a failure is the last error. All calls and
/// callbacks for this query occur on the invoking reactor thread, in a coroutine.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "upstream passes the int port to c-ares' uint16_t"
)]
pub(crate) fn lookup(domain: &CStr, family: i32, timeout: f64) -> Vec<Vec<u8>> {
    let (server, port) = crate::dns::server();
    let (server, port) = (server.as_ptr().cast::<c_char>(), port as u16);
    let domain = domain.as_ptr();
    if !event_api::swoole_event_isset_handler(FD, EventType::SW_EVENT_READ as i32) {
        // SAFETY: the handler absence identifies the first query in this reactor lifetime.
        unsafe { sdk::ares_library_init(metadata().library_all) };
        install();
    }
    let coroutine = co::current();
    let owner = Box::into_raw(Box::new(Query {
        channel: Cell::new(ptr::null_mut()),
        error: Cell::new(0),
        completed: Cell::new(false),
        coroutine: Cell::new(coroutine),
        retired: Arc::new(AtomicBool::new(false)),
        sockets: RefCell::new(HashMap::new()),
        results: RefCell::new(Vec::new()),
    }));
    let state = owner.cast::<c_void>();
    // SAFETY: the allocation just made, freed only at the end; callbacks share it too.
    let query = unsafe { &*owner };
    let mut channel = ptr::null_mut();
    let metadata = metadata();
    let mask = metadata.timeout_ms | metadata.tries | metadata.socket_state | metadata.lookups;
    // SAFETY: the SDK shim adapts its options layout; all callbacks borrow the stable query.
    let status = unsafe {
        sdk::swrs_cares_init(
            &raw mut channel,
            c"fb".as_ptr(),
            millis(timeout),
            swoole_rs_corebase::swr_v1_core_dns_tries(),
            Some(socket_state),
            state,
            mask,
        )
    };
    if status == metadata.success {
        // Publish the initialized channel before issuing any SDK query.
        query.channel.set(channel);
        // SAFETY: inspect the borrowed static/input server string without retaining it.
        if unsafe { *server } != 0 {
            if metadata.server_api != 0 {
                // SAFETY: the wrapper uses the actual SDK node layout for this version.
                unsafe { sdk::swrs_cares_server(channel, server, port) };
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
        // SAFETY: c-ares copies the query name; the userdata remains live through destroy.
        unsafe { sdk::ares_gethostbyname(channel, domain, family, Some(host), state) };
        let done = query.error.get() != 0 || query.completed.get();
        if !done {
            // SAFETY: this is the query's existing current coroutine; no Rust reference spans suspension.
            unsafe { co::yield_ex(coroutine, timeout) };
            if co::canceled(coroutine) {
                // SAFETY: cancel synchronously invokes only callbacks on this live query.
                unsafe { sdk::ares_cancel(channel) };
            } else if co::timedout(coroutine) {
                // SAFETY: process SDK timeouts before recording the original timeout status.
                unsafe { sdk::ares_process_fd(channel, -1, -1) };
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
        // SAFETY: keep query storage alive until all synchronous destruction callbacks return.
        unsafe { sdk::ares_destroy(channel) };
    } else {
        // SAFETY: c-ares' static description of the status.
        let reason = unsafe { sdk::ares_strerror(status) };
        // SAFETY: NUL-terminated.
        let reason = unsafe { CStr::from_ptr(reason) }.to_string_lossy();
        log::warning(
            ORIGIN,
            format_args!("ares_init_options() failed, Error: {reason}[{status}]"),
        );
    }
    // SAFETY: the channel no longer retains userdata; consume this owner exactly once.
    unsafe { Box::from_raw(owner) }.results.into_inner()
}
#[expect(
    clippy::cast_possible_truncation,
    reason = "the native SDK timeout assignment narrows timeout * 1000 to int"
)]
fn millis(timeout: f64) -> i32 {
    (timeout * 1000.0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use swoole_rs_reactor::event_api::Deferred;
    thread_local! {
        static RESUMED: Cell<usize> = const { Cell::new(0) };
    }
    unsafe extern "C-unwind" fn count_resume(_: *mut c_void) {
        RESUMED.set(RESUMED.get() + 1);
    }
    fn completion(retired: &Arc<AtomicBool>) -> Completion {
        Completion {
            retired: retired.clone(),
            coroutine: ptr::null_mut(),
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

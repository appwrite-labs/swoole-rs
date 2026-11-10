// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/dns.cc (Swoole contributors).
//! Per-query c-ares channel, descriptor registry, results and deferred completion.
//! Only Swoole's reactor drives c-ares; no c-ares event thread is enabled.
use crate::ip::swr_v1_clients_ip_text;
use std::{
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
use swoole_rs_reactor::{Socket, SocketRecord, event_api, socket_upstream};

/// The coroutine a query parks: a `swoole::Coroutine`, which starts with its
/// `swoole_rs_coroutine::Coroutine`.
mod co {
    use std::ffi::c_void;
    use swoole_rs_coroutine::{Coroutine, RC_CANCELED, RC_TIMEDOUT, STATE_WAITING};

    pub(super) use swoole_rs_coroutine::resume;

    // SAFETY: crates/coroutine's.
    unsafe extern "C" {
        #[link_name = "_ZN6swoole9Coroutine8yield_exEd"]
        pub(super) fn yield_ex(co: *mut c_void, timeout: f64) -> bool;
    }

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
struct Query {
    channel: *mut sdk::Channel,
    error: i32,
    completed: bool,
    coroutine: *mut c_void,
    retired: Arc<AtomicBool>,
    sockets: HashMap<i32, *mut SocketRecord>,
    results: Vec<Vec<u8>>,
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
// These scoped borrows end before every SDK/native effect that may reenter.
unsafe fn query<'a>(state: *mut c_void) -> &'a Query {
    // SAFETY: callers keep the stable query allocation alive through all callbacks.
    unsafe { &*state.cast::<Query>() }
}
unsafe fn query_mut<'a>(state: *mut c_void) -> &'a mut Query {
    // SAFETY: the owner thread serializes access; no borrow spans an effect.
    unsafe { &mut *state.cast::<Query>() }
}
unsafe extern "C" fn complete(data: *mut c_void) {
    let (retired, coroutine, resume) = {
        // SAFETY: the native queue's captured owner keeps this payload live during invocation.
        let task = unsafe { &*data.cast::<Completion>() };
        (
            task.retired.load(Ordering::Relaxed),
            task.coroutine,
            task.resume,
        )
    };
    if !retired {
        // SAFETY: no Rust payload borrow spans resume; reentrant queue destruction may drop it.
        unsafe { resume(coroutine) };
    }
}
unsafe extern "C-unwind" fn release_completion(data: *mut c_void) {
    // SAFETY: the queue transfers this Rust allocation exactly once, whether it
    // invoked or discarded its callback.
    drop(unsafe { Box::from_raw(data.cast::<Completion>()) });
}
/// The deferred task: the queue popped it, so it no longer disposes of it.
unsafe extern "C-unwind" fn run_completion(data: *mut c_void) {
    // SAFETY: the queue's live payload; `complete` borrows none of it across resume.
    unsafe { complete(data) };
    // SAFETY: as above, released once.
    unsafe { release_completion(data) };
}
/// The thread's live reactor fields.
fn reactor_fields() -> *mut swoole_rs_reactor::ReactorFields {
    let reactor = swoole_rs_corebase::thread_reactor();
    // SAFETY: c-ares runs on a thread whose reactor is live.
    unsafe { swoole_rs_reactor::reactor_upstream::prepared(reactor) }.fields
}
/// The `SW_FD_CARES` read and write handlers: the wrapper's object is its query.
unsafe extern "C" fn readable(_reactor: *mut c_void, event: *mut c_void) -> i32 {
    // SAFETY: the reactor lends its live event.
    unsafe { process(event, 0) }
}
unsafe extern "C" fn writable(_reactor: *mut c_void, event: *mut c_void) -> i32 {
    // SAFETY: the reactor lends its live event.
    unsafe { process(event, 1) }
}
unsafe fn process(event: *mut c_void, write: u8) -> i32 {
    // SAFETY: the reactor's live event.
    let event = unsafe { &*event.cast::<swoole_rs_reactor::Event>() };
    let fd = event.fd;
    // SAFETY: its socket is a registered c-ares wrapper.
    let Some(socket) = (unsafe { Socket::from_ptr(event.socket.cast()) }) else {
        return -1;
    };
    let data = socket.read(|socket| socket.object);
    // SAFETY: the query's channel stays live while its wrappers are registered.
    let channel = unsafe { query(data).channel };
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
    // SAFETY: the live reactor's destroy queue; the call keeps no data.
    let queue = unsafe { &raw mut (*reactor_fields()).destroy_callbacks };
    // SAFETY: as above.
    if unsafe {
        swoole_rs_reactor::swr_v1_reactor_deferred_push(queue, cleanup, ptr::null_mut(), None)
    } != 0
    {
        std::alloc::handle_alloc_error(std::alloc::Layout::new::<usize>());
    }
}
const FD: i32 = FdType::SW_FD_CARES as i32;
/// `Socket::free` of a wrapper whose descriptor c-ares keeps: the record goes on the
/// next deferred pass while a reactor runs, as the event being handled may hold it.
unsafe fn free(socket: *mut SocketRecord) {
    unsafe extern "C-unwind" fn dealloc(socket: *mut c_void) {
        // SAFETY: the detached wrapper, unused since.
        unsafe { SocketRecord::free(socket.cast()) };
    }
    // SAFETY: a live wrapper; c-ares retains the OS descriptor.
    if let Some(wrapper) = unsafe { Socket::from_ptr(socket) } {
        wrapper.edit(|wrapper| wrapper.fd = -1);
    }
    let reactor = event_api::swr_v1_event_reactor();
    if reactor.is_null() {
        // SAFETY: as above.
        unsafe { dealloc(socket.cast()) };
    } else {
        // SAFETY: the available reactor is live.
        let fields = unsafe { swoole_rs_reactor::reactor_upstream::prepared(reactor) }.fields;
        // SAFETY: its live queue.
        unsafe { swoole_rs_reactor::defer(fields, dealloc, socket.cast(), None) };
    }
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
    // SAFETY: query registry access ends before any reactor effect.
    let socket = unsafe { query(data).sockets.get(&fd).copied() };
    let wrapper = if let Some(socket) = socket {
        if events == 0 {
            // SAFETY: remove the registered native socket before detaching its descriptor.
            unsafe { event_api::swoole_event_del(socket) };
            // SAFETY: free detaches fd first; c-ares retains ownership of the OS descriptor.
            unsafe { free(socket) };
            // SAFETY: no SDK call can reenter while this registry borrow is held.
            unsafe { query_mut(data).sockets.remove(&fd) };
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
        // SAFETY: registry insertion completes before the reactor can run any callback.
        unsafe { query_mut(data).sockets.insert(fd, socket) };
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
    if timeouts > 0 || status != metadata().success {
        let error = if timeouts > 0 {
            ErrorCode::SW_ERROR_DNSLOOKUP_RESOLVE_TIMEOUT as i32
        } else {
            status
        };
        // SAFETY: no state borrow spans a subsequent native or SDK callback.
        unsafe { query_mut(data).error = error };
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
            // SAFETY: the successful hostent supplies4 or16 initialized address bytes.
            let text = unsafe {
                swr_v1_clients_ip_text(if family == libc::AF_INET { 4 } else { 6 }, address.cast())
            };
            let length = usize::try_from(text.length).unwrap_or(0);
            let text = text.bytes.get(..length).unwrap_or_default().to_vec();
            // SAFETY: copy SDK data into the Rust owner before the callback returns.
            unsafe { query_mut(data).results.push(text) };
            // SAFETY: the SDK address list has another slot or its terminating null.
            addresses = unsafe { addresses.add(1) };
        }
    }
    // SAFETY: read the borrowed coroutine identity without retaining a query reference.
    let coroutine = unsafe { query(data).coroutine };
    // SAFETY: a nonnull identity is the query's original live coroutine.
    if !coroutine.is_null() && co::suspending(coroutine) {
        // SAFETY: clone only the shared retirement token; no query borrow escapes.
        let retired = unsafe { query(data).retired.clone() };
        let task = Box::into_raw(Box::new(Completion {
            retired,
            coroutine,
            resume: co::resume,
        }));
        // SAFETY: the live reactor's queue takes this owned completion payload once.
        let slot = unsafe { &raw mut (*reactor_fields()).defer_tasks };
        // SAFETY: as above.
        let pushed = unsafe {
            swoole_rs_reactor::swr_v1_reactor_deferred_push(
                slot,
                run_completion,
                task.cast(),
                Some(release_completion),
            )
        };
        if pushed != 0 {
            std::alloc::handle_alloc_error(std::alloc::Layout::new::<Completion>());
        }
        // SAFETY: a completion was queued, so further SDK callbacks cannot enqueue it again.
        unsafe { query_mut(data).coroutine = ptr::null_mut() };
    } else {
        // SAFETY: synchronous completion is recorded before the initiating SDK call returns.
        unsafe { query_mut(data).completed = true };
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
    let state = Box::into_raw(Box::new(Query {
        channel: ptr::null_mut(),
        error: 0,
        completed: false,
        coroutine,
        retired: Arc::new(AtomicBool::new(false)),
        sockets: HashMap::new(),
        results: Vec::new(),
    }))
    .cast::<c_void>();
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
        // SAFETY: publish the initialized channel before issuing any SDK query.
        unsafe { query_mut(state).channel = channel };
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
        // SAFETY: no state borrow spans the coroutine yield or a subsequent SDK effect.
        let done = unsafe {
            let state = query(state);
            state.error != 0 || state.completed
        };
        if !done {
            // SAFETY: this is the query's existing current coroutine; no Rust reference spans suspension.
            unsafe { co::yield_ex(coroutine, timeout) };
            if co::canceled(coroutine) {
                // SAFETY: cancel synchronously invokes only callbacks on this live query.
                unsafe { sdk::ares_cancel(channel) };
            } else if co::timedout(coroutine) {
                // SAFETY: process SDK timeouts before recording the original timeout status.
                unsafe { sdk::ares_process_fd(channel, -1, -1) };
                // SAFETY: record timeout after SDK callbacks, matching original order.
                unsafe { query_mut(state).error = metadata.timeout };
            }
        }
        // SAFETY: inspect the stored outcome before destroy can invoke more callbacks.
        let error = unsafe { query(state).error };
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
        // SAFETY: all deferred completions share this token; none may resume a retired query.
        unsafe { query(state).retired.store(true, Ordering::Relaxed) };
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
    unsafe { Box::from_raw(state.cast::<Query>()) }.results
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
    struct Resume {
        payload: *mut c_void,
        calls: usize,
    }
    unsafe extern "C-unwind" fn resume_and_discard(context: *mut c_void) {
        let context = unsafe { &mut *context.cast::<Resume>() };
        context.calls += 1;
        unsafe { release_completion(context.payload) };
    }
    #[test]
    fn completion_queue_discard_releases_token() {
        let retired = Arc::new(AtomicBool::new(true));
        let payload = Box::into_raw(Box::new(Completion {
            retired: retired.clone(),
            coroutine: ptr::null_mut(),
            resume: resume_and_discard,
        }));
        assert_eq!(
            Arc::strong_count(&retired),
            2,
            "queue retains retirement token"
        );
        unsafe { complete(payload.cast()) };
        assert_eq!(
            Arc::strong_count(&retired),
            2,
            "retired callback does not consume queue custody"
        );
        unsafe { release_completion(payload.cast()) };
        assert_eq!(
            Arc::strong_count(&retired),
            1,
            "discard releases payload without invocation"
        );
    }
    #[test]
    fn completion_can_be_discarded_during_resume() {
        let retired = Arc::new(AtomicBool::new(false));
        let mut context = Resume {
            payload: ptr::null_mut(),
            calls: 0,
        };
        let payload = Box::into_raw(Box::new(Completion {
            retired: retired.clone(),
            coroutine: (&raw mut context).cast(),
            resume: resume_and_discard,
        }));
        context.payload = payload.cast();
        unsafe { complete(payload.cast()) };
        assert_eq!(context.calls, 1, "resume was delivered exactly once");
        assert_eq!(
            Arc::strong_count(&retired),
            1,
            "reentrant queue destruction released the token"
        );
    }
}

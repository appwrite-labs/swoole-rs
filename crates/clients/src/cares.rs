// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/dns.cc (Swoole contributors).
//! Per-query c-ares channel, descriptor registry, results and deferred completion.
//! Only Swoole's reactor drives c-ares; no c-ares event thread is enabled.
use crate::{dns::Push, ip::swr_v1_clients_ip_text};
use std::{
    collections::HashMap,
    ffi::{c_char, c_void},
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use swoole_rs_cares_sys as sdk;
use swoole_rs_corebase::constants::{EventType, SW_DNS_SERVER_PORT};
use swoole_rs_error::ErrorCode;

/// Static native reactor/coroutine effects. No query state resides in this table.
#[repr(C)]
pub(crate) struct Ops {
    handlers: unsafe extern "C" fn() -> u8,
    install: unsafe extern "C" fn(),
    current: unsafe extern "C" fn() -> *mut c_void,
    suspending: unsafe extern "C" fn(*mut c_void) -> u8,
    yield_: unsafe extern "C" fn(*mut c_void, f64),
    canceled: unsafe extern "C" fn(*mut c_void) -> u8,
    timedout: unsafe extern "C" fn(*mut c_void) -> u8,
    resume: unsafe extern "C" fn(*mut c_void),
    defer: unsafe extern "C" fn(
        unsafe extern "C" fn(*mut c_void),
        unsafe extern "C" fn(*mut c_void),
        *mut c_void,
    ),
    make: unsafe extern "C" fn(i32, *mut c_void) -> *mut c_void,
    events: unsafe extern "C" fn(*mut c_void) -> i32,
    add: unsafe extern "C" fn(*mut c_void, i32),
    set: unsafe extern "C" fn(*mut c_void, i32),
    del: unsafe extern "C" fn(*mut c_void),
    free: unsafe extern "C" fn(*mut c_void),
    error: unsafe extern "C" fn(i32),
    warning: unsafe extern "C" fn(u8, i32),
    trace: unsafe extern "C" fn(u8, i32, i32, usize),
}
struct Query {
    ops: &'static Ops,
    channel: *mut sdk::Channel,
    error: i32,
    completed: bool,
    coroutine: *mut c_void,
    retired: Arc<AtomicBool>,
    sockets: HashMap<i32, *mut c_void>,
    results: Vec<Vec<u8>>,
}
struct Completion {
    retired: Arc<AtomicBool>,
    coroutine: *mut c_void,
    resume: unsafe extern "C" fn(*mut c_void),
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
unsafe extern "C" fn release_completion(data: *mut c_void) {
    // SAFETY: the last native captured owner transfers this Rust allocation exactly once,
    // whether the queue invoked or discarded its callback.
    drop(unsafe { Box::from_raw(data.cast::<Completion>()) });
}
unsafe extern "C" fn socket_state(data: *mut c_void, fd: i32, readable: i32, writable: i32) {
    // SAFETY: c-ares returns this query's stable callback userdata.
    let ops = unsafe { query(data).ops };
    let events = if readable != 0 {
        EventType::SW_EVENT_READ as i32
    } else {
        0
    } | if writable != 0 {
        EventType::SW_EVENT_WRITE as i32
    } else {
        0
    };
    // SAFETY: scalar trace observations; the native logger retains no pointers.
    unsafe { (ops.trace)(2, fd, readable, usize::try_from(writable).unwrap_or(0)) };
    // SAFETY: query registry access ends before any reactor effect.
    let socket = unsafe { query(data).sockets.get(&fd).copied() };
    let socket = if let Some(socket) = socket {
        if events == 0 {
            // SAFETY: trace of the live descriptor before its removal.
            unsafe { (ops.trace)(3, fd, 0, 0) };
            // SAFETY: remove the registered native socket before detaching its descriptor.
            unsafe { (ops.del)(socket) };
            // SAFETY: free detaches fd first; c-ares retains ownership of the OS descriptor.
            unsafe { (ops.free)(socket) };
            // SAFETY: no SDK call can reenter while this registry borrow is held.
            unsafe { query_mut(data).sockets.remove(&fd) };
            return;
        }
        socket
    } else {
        if events == 0 {
            // SAFETY: scalar warning, with no callback data retained.
            unsafe { (ops.warning)(0, fd) };
            return;
        }
        // SAFETY: c-ares lends fd; this stable query becomes the wrapper's callback userdata.
        let socket = unsafe { (ops.make)(fd, data) };
        // SAFETY: registry insertion completes before the reactor can run any callback.
        unsafe { query_mut(data).sockets.insert(fd, socket) };
        socket
    };
    // SAFETY: a live native wrapper's primitive registered-event flags.
    let registered = unsafe { (ops.events)(socket) } != 0;
    if registered {
        // SAFETY: update this existing registered wrapper with the requested event mask.
        unsafe { (ops.set)(socket, events) };
    } else {
        // SAFETY: register this fresh wrapper with the requested event mask.
        unsafe { (ops.add)(socket, events) };
    }
    // SAFETY: scalar trace after the reactor operation, matching native ordering.
    unsafe { (ops.trace)(if registered { 4 } else { 5 }, fd, events, 0) };
}
unsafe extern "C" fn host(
    data: *mut c_void,
    status: i32,
    timeouts: i32,
    entry: *mut libc::hostent,
) {
    // SAFETY: callback userdata remains live until ares_destroy returns.
    let ops = unsafe { query(data).ops };
    // SAFETY: the logger only observes these callback scalars.
    unsafe { (ops.trace)(6, status, timeouts, 0) };
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
            // SAFETY: copy SDK data into the Rust owner before the callback returns.
            unsafe { query_mut(data).results.push(text.bytes[..length].to_vec()) };
            // SAFETY: the SDK address list has another slot or its terminating null.
            addresses = unsafe { addresses.add(1) };
        }
    }
    // SAFETY: read the borrowed coroutine identity without retaining a query reference.
    let coroutine = unsafe { query(data).coroutine };
    // SAFETY: a nonnull identity is the query's original live coroutine.
    if !coroutine.is_null() && unsafe { (ops.suspending)(coroutine) } != 0 {
        // SAFETY: clone only the shared retirement token; no query borrow escapes.
        let retired = unsafe { query(data).retired.clone() };
        let task = Box::into_raw(Box::new(Completion {
            retired,
            coroutine,
            resume: ops.resume,
        }));
        // SAFETY: the native deferred queue takes this owned completion payload once.
        unsafe { (ops.defer)(complete, release_completion, task.cast()) };
        // SAFETY: a completion was queued, so further SDK callbacks cannot enqueue it again.
        unsafe { query_mut(data).coroutine = ptr::null_mut() };
    } else {
        // SAFETY: synchronous completion is recorded before the initiating SDK call returns.
        unsafe { query_mut(data).completed = true };
    }
}

/// # Safety
/// Called only from this query's reactor callback, on its owner thread.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_clients_cares_process(
    data: *mut c_void,
    fd: i32,
    write: u8,
) {
    // SAFETY: the native wrapper lends its live query userdata.
    let channel = unsafe { query(data).channel };
    // SAFETY: the channel remains live; SDK callbacks run synchronously on this reactor thread.
    unsafe {
        sdk::ares_process_fd(
            channel,
            if write == 0 { fd } else { -1 },
            if write == 0 { -1 } else { fd },
        );
    };
}
/// Reactor destruction: no query still uses the library after its channel was destroyed.
#[unsafe(no_mangle)]
pub(crate) extern "C" fn swr_v1_clients_cares_cleanup() {
    // SAFETY: the native reactor invokes this once after its c-ares handlers retire.
    unsafe { sdk::ares_library_cleanup() };
}

/// # Safety
/// Static native leaves; borrowed NUL-terminated domain/server; output accepts copied
/// text. All calls and callbacks for this query occur on the invoking reactor thread.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_clients_cares_lookup(
    ops: *const Ops,
    domain: *const c_char,
    family: i32,
    timeout: f64,
    server: *const c_char,
    port: u16,
    output: *mut c_void,
    push: Push,
) {
    // SAFETY: the caller provides a process-lifetime immutable table.
    let ops = unsafe { &*ops };
    // SAFETY: this presence probe invokes no user code.
    if unsafe { (ops.handlers)() } == 0 {
        // SAFETY: the handler absence identifies the first query in this reactor lifetime.
        unsafe { sdk::ares_library_init(metadata().library_all) };
        // SAFETY: install native handler/destroy trampolines, without retaining Rust references.
        unsafe { (ops.install)() };
    }
    // SAFETY: obtain the existing safe coroutine identity before allocating the query.
    let coroutine = unsafe { (ops.current)() };
    let state = Box::into_raw(Box::new(Query {
        ops,
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
                    // SAFETY: report the original unsupported-port warning.
                    unsafe { (ops.warning)(2, 0) };
                }
            } else {
                // SAFETY: the SDK version cannot configure a server, so retain the warning.
                unsafe { (ops.warning)(3, 0) };
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
            unsafe { (ops.yield_)(coroutine, timeout) };
            // SAFETY: coroutine outcome inspection has no callbacks.
            if unsafe { (ops.canceled)(coroutine) } != 0 {
                // SAFETY: cancel synchronously invokes only callbacks on this live query.
                unsafe { sdk::ares_cancel(channel) };
            // SAFETY: coroutine outcome inspection has no callbacks.
            } else if unsafe { (ops.timedout)(coroutine) } != 0 {
                // SAFETY: process SDK timeouts before recording the original timeout status.
                unsafe { sdk::ares_process_fd(channel, -1, -1) };
                // SAFETY: record timeout after SDK callbacks, matching original order.
                unsafe { query_mut(state).error = metadata.timeout };
            } else {
                // SAFETY: copy the scalar result count without retaining its collection borrow.
                let count = unsafe { query(state).results.len() };
                // SAFETY: the native logger observes only this completed query's result count.
                unsafe { (ops.trace)(7, 0, 0, count) };
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
            // SAFETY: publish the exact native error mapping before destroying the SDK channel.
            unsafe { (ops.error)(code as i32) };
        }
        // SAFETY: all deferred completions share this token; none may resume a retired query.
        unsafe { query(state).retired.store(true, Ordering::Relaxed) };
        // SAFETY: keep query storage alive until all synchronous destruction callbacks return.
        unsafe { sdk::ares_destroy(channel) };
    } else {
        // SAFETY: the reporter only reads the SDK status and its static description.
        unsafe { (ops.warning)(1, status) };
    }
    // SAFETY: the channel no longer retains userdata; consume this owner exactly once.
    let state = unsafe { Box::from_raw(state.cast::<Query>()) };
    for text in &state.results {
        // SAFETY: the native vector copies the borrowed text before returning.
        unsafe { push(output, text.as_ptr(), text.len()) };
    }
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
    unsafe extern "C" fn resume_and_discard(context: *mut c_void) {
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

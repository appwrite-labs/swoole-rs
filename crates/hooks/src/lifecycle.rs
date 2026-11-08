//! The coroutine socket's lifecycle over its record: `shutdown` (admission, the
//! TLS close, `shutdown(2)` and the half-close flags), `cancel` (wake the
//! coroutine bound to a direction), `close` (close-wait while coroutines are
//! bound, else release the descriptor), `connect(addr)` (the `connect(2)`
//! retries, the wait to write and the `SO_ERROR` check) and `check_liveness`.
//! Native code keeps the coroutine resume, the destructor callback, the TLS
//! session and the liveness probe ([`Host`](crate::co_io::Host)).
use crate::co_io::{CLOSE_WAIT_ERROR, Timer, available, errno, host, set_err};
use crate::co_socket::CoSocket;
use crate::shutdown::{
    READ, WRITE, swr_v1_hook_shutdown, swr_v1_hook_socket_bound_co, swr_v1_hook_socket_co,
    swr_v1_hook_socket_connect,
};
use crate::socket_wait::wait;
use core::ffi::{c_int, c_void};

/// The syscalls: `shutdown(2)`, `connect(2)` and `getsockopt(SO_ERROR)` into the
/// int it is given. Tests script them.
#[derive(Clone, Copy)]
struct Sys {
    shutdown: unsafe extern "C" fn(c_int, c_int) -> c_int,
    connect: unsafe extern "C" fn(c_int, *const libc::sockaddr, libc::socklen_t) -> c_int,
    option: unsafe fn(c_int, *mut c_int) -> c_int,
}

const SYS: Sys = Sys {
    shutdown: libc::shutdown,
    connect: libc::connect,
    option: so_error,
};

/// The record, read through briefly: native calls may change or free it.
fn record<'a>(socket: *mut CoSocket) -> &'a CoSocket {
    // SAFETY: every caller here holds a live record and keeps no other borrow.
    unsafe { &*socket }
}

unsafe extern "C" fn ssl_shutdown(socket: *mut c_void) {
    // SAFETY: the live record `shutdown` passes.
    unsafe { (host().ssl_shutdown)(socket.cast()) };
}

/// `Socket::shutdown(how)`.
///
/// # Safety
/// `socket` is a live record, not borrowed elsewhere for the call.
unsafe fn shutdown(socket: *mut CoSocket, how: c_int, sys: Sys) -> bool {
    // SAFETY: as the caller promises.
    unsafe { set_err(socket, 0) };
    let s = record(socket);
    let closed = s.is_closed();
    // SAFETY: an open record's network::Socket.
    let tls = !closed && !unsafe { s.record() }.ssl.is_null();
    // SAFETY: the record's state; `ssl_shutdown` acts on the record.
    let error = unsafe {
        swr_v1_hook_shutdown(
            sys.shutdown,
            s.sock_fd,
            how,
            closed,
            s.state,
            tls.then_some(ssl_shutdown as unsafe extern "C" fn(*mut c_void)),
            socket.cast(),
        )
    };
    if error == 0 {
        return true;
    }
    // SAFETY: as the caller promises.
    unsafe { set_err(socket, error) };
    false
}

/// `Socket::cancel(event)`: wake the coroutine bound to `event` with `ECANCELED`.
///
/// # Safety
/// As [`shutdown`].
unsafe fn cancel(socket: *mut CoSocket, event: u32) -> bool {
    let state = record(socket).state;
    // SAFETY: the record's live state.
    if unsafe { swr_v1_hook_socket_bound_co(state, event) }.is_null() {
        return false;
    }
    if event != READ && event != WRITE {
        // SAFETY: as the caller promises.
        unsafe { set_err(socket, libc::EINVAL) };
        return false;
    }
    // SAFETY: as the caller promises.
    unsafe { set_err(socket, libc::ECANCELED) };
    // SAFETY: the record's live state; `bound_co` found a coroutine for `event`.
    let co = unsafe { swr_v1_hook_socket_co(state, event) };
    // SAFETY: wakes that coroutine; the socket may be gone when it returns.
    unsafe { (host().resume)(co) };
    true
}

/// `Socket::close()`: true once the descriptor is released, false when already
/// closed or while coroutines are still bound (close-wait).
///
/// # Safety
/// As [`shutdown`]; releasing may destroy the socket, so nothing touches it after.
unsafe fn close(socket: *mut CoSocket, sys: Sys) -> bool {
    if record(socket).is_closed() {
        // SAFETY: as the caller promises.
        unsafe { set_err(socket, libc::EBADF) };
        return false;
    }
    // SAFETY: the record's live state.
    if unsafe { (*record(socket).state).flags.connected } {
        // SAFETY: as the caller promises.
        unsafe { shutdown(socket, libc::SHUT_RDWR, sys) };
    }
    // SAFETY: the record's live state.
    if !unsafe { swr_v1_hook_socket_bound_co(record(socket).state, READ | WRITE) }.is_null() {
        // SAFETY: an open record's network::Socket.
        unsafe { record(socket).record() }.output.close_wait = 1;
        // SAFETY: as the caller promises.
        unsafe { cancel(socket, WRITE) };
        // SAFETY: as the caller promises.
        unsafe { cancel(socket, READ) };
        // SAFETY: as the caller promises.
        unsafe { set_err(socket, CLOSE_WAIT_ERROR) };
        return false;
    }
    // SAFETY: as the caller promises.
    unsafe { (*socket).sock_fd = -1 };
    // SAFETY: the record's last effect here; it may destroy the socket.
    unsafe { crate::socket_extra::run_close_hook(socket) };
    true
}

/// `Socket::check_liveness()`.
///
/// # Safety
/// As [`shutdown`].
unsafe fn liveness(socket: *mut CoSocket) -> bool {
    if record(socket).is_closed() {
        // SAFETY: as the caller promises.
        unsafe { set_err(socket, libc::EBADF) };
        return false;
    }
    // SAFETY: the open record's probe.
    let alive = unsafe { (host().liveness)(socket) };
    let error = match (alive, errno()) {
        (true, _) => 0,
        (false, 0) => libc::ECONNRESET,
        (false, error) => error,
    };
    // SAFETY: as the caller promises.
    unsafe { set_err(socket, error) };
    alive
}

/// Waits to write under the connect timeout, then reads `SO_ERROR` into the
/// record's error code while the timer still runs: true once connected.
///
/// # Safety
/// As [`shutdown`]; the record is open.
unsafe fn settle(socket: *mut CoSocket, sys: Sys) -> bool {
    // SAFETY: an open record's network::Socket.
    let timeout = unsafe { record(socket).record() }.timeouts.connect;
    // SAFETY: the live record; the timer stops before this returns.
    let mut timer = unsafe { Timer::new(socket, WRITE, timeout) };
    // SAFETY: the live, open record.
    let woke = timer.start() && unsafe { wait(socket, WRITE, None, core::ptr::null_mut(), 0) };
    let connected = if woke {
        let fd = record(socket).sock_fd;
        // SAFETY: the live record's own error code.
        let code = unsafe { &raw mut (*socket).err_code };
        // SAFETY: the record's descriptor and its error code.
        let result = unsafe { (sys.option)(fd, code) };
        let error = record(socket).err_code;
        if result < 0 || error != 0 {
            // SAFETY: as the caller promises.
            unsafe { set_err(socket, error) };
            false
        } else {
            true
        }
    } else {
        if record(socket).is_closed() {
            // SAFETY: as the caller promises.
            unsafe { set_err(socket, libc::ECONNABORTED) };
        }
        false
    };
    timer.stop();
    connected
}

/// `Socket::connect(addr, addrlen)`, with `connect(2)` and the `SO_ERROR` read.
///
/// # Safety
/// As [`shutdown`]; `addr` holds `length` readable bytes.
unsafe fn connect(
    socket: *mut CoSocket,
    addr: *const libc::sockaddr,
    length: libc::socklen_t,
    sys: Sys,
) -> bool {
    #[cfg(feature = "iouring")]
    // SAFETY: as the caller promises.
    if let Some(ops) = unsafe { crate::uring_socket::of(socket) } {
        // SAFETY: the live view and the caller's address.
        return unsafe {
            crate::uring_socket::swr_v1_hook_uring_connect(&raw const ops, addr, length)
        };
    }
    // SAFETY: as the caller promises.
    if !unsafe { available(socket, READ | WRITE) } {
        return false;
    }
    let fd = record(socket).sock_fd;
    let result = loop {
        // SAFETY: the caller's descriptor and address.
        let result = unsafe { (sys.connect)(fd, addr, length) };
        if result >= 0 || errno() != libc::EINTR {
            break result;
        }
    };
    if result < 0 {
        let error = errno();
        if error != libc::EINPROGRESS {
            // SAFETY: as the caller promises.
            unsafe { set_err(socket, error) };
            return false;
        }
        // SAFETY: as the caller promises.
        if !unsafe { settle(socket, sys) } {
            return false;
        }
    }
    // SAFETY: the record's live state.
    unsafe { swr_v1_hook_socket_connect(record(socket).state) };
    // SAFETY: an open record's network::Socket; `get_name`'s failure is ignored.
    unsafe { swoole_rs_reactor::swr_v1_socket_get_name(record(socket).socket) };
    // SAFETY: as the caller promises.
    unsafe { set_err(socket, 0) };
    true
}

/// `getsockopt(fd, SOL_SOCKET, SO_ERROR)` into `error`.
unsafe fn so_error(fd: c_int, error: *mut c_int) -> c_int {
    let mut length = libc::socklen_t::try_from(size_of::<c_int>()).unwrap_or(4);
    // SAFETY: the caller's int and its size.
    unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ERROR,
            error.cast(),
            &raw mut length,
        )
    }
}

macro_rules! export {
    ($(#[$doc:meta])* fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty = $call:expr) => {
        $(#[$doc])*
        ///
        /// # Safety
        /// `socket` is a live record, not borrowed elsewhere for the call.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name($($arg: $ty),*) -> $ret {
            // SAFETY: as the caller promises.
            unsafe { $call }
        }
    };
}

export!(
    /// `shutdown(how)`.
    fn swr_v1_hook_co_socket_shutdown(socket: *mut CoSocket, how: c_int) -> bool = shutdown(socket, how, SYS)
);
export!(
    /// `cancel(event)`.
    fn swr_v1_hook_co_socket_cancel(socket: *mut CoSocket, event: u32) -> bool = cancel(socket, event)
);
export!(
    /// `close()`; the socket may be gone after it returns true.
    fn swr_v1_hook_co_socket_close(socket: *mut CoSocket) -> bool = close(socket, SYS)
);
export!(
    /// `check_liveness()`.
    fn swr_v1_hook_co_socket_check_liveness(socket: *mut CoSocket) -> bool = liveness(socket)
);
export!(
    /// `connect(addr, addrlen)`; `addr` holds `length` readable bytes.
    fn swr_v1_hook_co_socket_connect(socket: *mut CoSocket, addr: *const libc::sockaddr, length: libc::socklen_t) -> bool
        = connect(socket, addr, length, SYS)
);

/// The lifecycle gate's former scenarios, ported: each runs Rust's operation and
/// upstream's body (transcribed below over the same primitives) on two identical
/// sockets, and compares results, effects in order and the state left behind.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::co_io::set_errno;
    use crate::co_io::test_host::{
        ON_RELEASE, ON_RESUME, ON_SUSPEND, PROBE, SSL_ERRNO, TIMER_OK, install, log, release, take,
    };
    use crate::co_socket::TestSocket;
    use crate::shutdown::{SocketFlags, swr_v1_hook_socket_bind, swr_v1_hook_socket_set_flags};
    use core::ffi::c_long;
    use std::cell::{Cell, RefCell};

    // Upstream's bodies, transcribed: socket.cc's shutdown, cancel, close,
    // connect(addr) and check_liveness.
    unsafe fn upstream_shutdown(s: *mut CoSocket, mut how: c_int, sys: Sys) -> bool {
        unsafe { set_err(s, 0) };
        let state = record(s).state;
        let flags = unsafe { &mut (*state).flags };
        let connected = flags.connected && !record(s).is_closed();
        let mut error = 0;
        if !connected || how == libc::SHUT_RD && flags.read || how == libc::SHUT_WR && flags.write {
            error = libc::ENOTCONN;
        } else {
            if !unsafe { record(s).record() }.ssl.is_null() {
                unsafe { (host().ssl_shutdown)(s) };
            }
            if unsafe { (sys.shutdown)(record(s).sock_fd, how) } < 0 {
                error = errno();
            }
            if error == 0 || error == libc::ENOTCONN {
                if error == libc::ENOTCONN {
                    how = libc::SHUT_RDWR;
                }
                let flags = unsafe { &mut (*state).flags };
                match how {
                    libc::SHUT_RD => flags.read = true,
                    libc::SHUT_WR => flags.write = true,
                    _ => (flags.read, flags.write) = (true, true),
                }
                if flags.read && flags.write {
                    flags.connected = false;
                }
                return true;
            }
        }
        unsafe { set_err(s, error) };
        false
    }

    unsafe fn upstream_cancel(s: *mut CoSocket, event: u32) -> bool {
        let state = record(s).state;
        if unsafe { swr_v1_hook_socket_bound_co(state, event) }.is_null() {
            return false;
        }
        if event == READ || event == WRITE {
            unsafe { set_err(s, libc::ECANCELED) };
            unsafe { (host().resume)(swr_v1_hook_socket_co(state, event)) };
            true
        } else {
            unsafe { set_err(s, libc::EINVAL) };
            false
        }
    }

    unsafe fn upstream_close(s: *mut CoSocket, sys: Sys) -> bool {
        if record(s).is_closed() {
            unsafe { set_err(s, libc::EBADF) };
            return false;
        }
        if unsafe { (*record(s).state).flags.connected } {
            unsafe { upstream_shutdown(s, libc::SHUT_RDWR, sys) };
        }
        if unsafe { swr_v1_hook_socket_bound_co(record(s).state, READ | WRITE) }.is_null() {
            unsafe { (*s).sock_fd = -1 };
            unsafe { crate::socket_extra::run_close_hook(s) };
            true
        } else {
            unsafe { record(s).record() }.output.close_wait = 1;
            unsafe { upstream_cancel(s, WRITE) };
            unsafe { upstream_cancel(s, READ) };
            unsafe { set_err(s, CLOSE_WAIT_ERROR) };
            false
        }
    }

    unsafe fn upstream_connect(s: *mut CoSocket, sys: Sys) -> bool {
        if !unsafe { available(s, READ | WRITE) } {
            return false;
        }
        let address: libc::sockaddr = unsafe { core::mem::zeroed() };
        let length = libc::socklen_t::try_from(size_of::<libc::sockaddr>()).unwrap();
        let retval = loop {
            let retval = unsafe { (sys.connect)(record(s).sock_fd, &raw const address, length) };
            if !(retval < 0 && errno() == libc::EINTR) {
                break retval;
            }
        };
        if retval < 0 {
            if errno() != libc::EINPROGRESS {
                unsafe { set_err(s, errno()) };
                return false;
            }
            let timeout = unsafe { record(s).record() }.timeouts.connect;
            let mut timer = unsafe { Timer::new(s, WRITE, timeout) };
            if !timer.start() || !unsafe { wait(s, WRITE, None, core::ptr::null_mut(), 0) } {
                if record(s).is_closed() {
                    unsafe { set_err(s, libc::ECONNABORTED) };
                }
                timer.stop();
                return false;
            }
            if unsafe { (sys.option)(record(s).sock_fd, &raw mut (*s).err_code) } < 0
                || record(s).err_code != 0
            {
                unsafe { set_err(s, record(s).err_code) };
                timer.stop();
                return false;
            }
            timer.stop();
        }
        unsafe { swr_v1_hook_socket_connect(record(s).state) };
        unsafe { swoole_rs_reactor::swr_v1_socket_get_name(record(s).socket) };
        unsafe { set_err(s, 0) };
        true
    }

    unsafe fn upstream_liveness(s: *mut CoSocket) -> bool {
        if record(s).is_closed() {
            unsafe { set_err(s, libc::EBADF) };
            return false;
        }
        if !unsafe { (host().liveness)(s) } {
            let error = errno();
            unsafe { set_err(s, if error != 0 { error } else { libc::ECONNRESET }) };
            return false;
        }
        unsafe { set_err(s, 0) };
        true
    }

    thread_local! {
        /// shutdown(2)'s result and the errno it leaves (-1 for none).
        static SHUTDOWN: Cell<(c_int, c_int)> = const { Cell::new((0, -1)) };
        /// connect(2)'s scripted results (-errno for a failure), and SO_ERROR's mode.
        static CONNECTS: RefCell<Vec<c_int>> = const { RefCell::new(Vec::new()) };
        static OPTION: Cell<u32> = const { Cell::new(0) };
    }

    unsafe extern "C" fn fake_shutdown(fd: c_int, how: c_int) -> c_int {
        log(format!("shutdown:{fd}:{how}"));
        let (result, error) = SHUTDOWN.get();
        if error >= 0 {
            set_errno(error);
        }
        result
    }
    unsafe extern "C" fn fake_connect(
        fd: c_int,
        _: *const libc::sockaddr,
        _: libc::socklen_t,
    ) -> c_int {
        log(format!("connect:{fd}"));
        let value = CONNECTS.with_borrow_mut(|c| c.remove(0));
        if value < 0 {
            set_errno(-value);
            return -1;
        }
        value
    }
    /// Modes: 0 no error, 1 `ECONNREFUSED`, 2 the call fails, 3 the call fails with no error.
    unsafe fn fake_option(_: c_int, error: *mut c_int) -> c_int {
        let mode = OPTION.get();
        log(format!("get-option:{}", unsafe { *error }));
        match mode {
            0 | 3 => unsafe { *error = 0 },
            1 => unsafe { *error = libc::ECONNREFUSED },
            _ => {}
        }
        set_errno(libc::EIO);
        if mode >= 2 { -1 } else { 0 }
    }
    const FAKE: Sys = Sys {
        shutdown: fake_shutdown,
        connect: fake_connect,
        option: fake_option,
    };

    /// What a scenario leaves: its result, the effects and the socket's state.
    #[derive(Debug, PartialEq)]
    struct Outcome {
        result: bool,
        effects: Vec<String>,
        state: (c_int, SocketFlags, c_int, u8, c_long, c_long, c_int),
    }

    fn id(state: *mut crate::shutdown::SocketState, event: u32) -> c_long {
        let co = unsafe { swr_v1_hook_socket_co(state, event) };
        if co.is_null() {
            0
        } else {
            unsafe { *co.cast::<c_long>() }
        }
    }

    /// Runs `operation` on a fresh socket that `setup` prepares; the coroutines are
    /// ids 5 (reader), 7 (writer) and 11 (another reader).
    fn run(
        setup: &dyn Fn(&mut TestSocket, &[*mut c_long; 3]),
        operation: &dyn Fn(*mut CoSocket) -> bool,
    ) -> Outcome {
        let mut socket = TestSocket::new();
        let extra = crate::socket_extra::swr_v1_hook_socket_extra_new().owner;
        socket.0.extra = extra;
        let owner = socket.ptr().cast();
        unsafe {
            crate::socket_extra::set_close_hook(
                socket.ptr(),
                owner,
                release,
                core::ptr::null_mut(),
            );
        }
        unsafe { socket.0.record() }.timeouts.connect = 1.0;
        socket.0.err_code = 9;
        let mut ids: [Box<c_long>; 3] = [Box::new(5), Box::new(7), Box::new(11)];
        let cos = [&raw mut *ids[0], &raw mut *ids[1], &raw mut *ids[2]];
        setup(&mut socket, &cos);
        set_errno(0);
        take();
        let result = operation(socket.ptr());
        let state = socket.state();
        let flags = unsafe { (*state).flags };
        let outcome = Outcome {
            result,
            effects: take(),
            state: (
                socket.0.sock_fd,
                flags,
                socket.0.err_code,
                unsafe { socket.0.record() }.output.close_wait,
                id(state, READ),
                id(state, WRITE),
                errno(),
            ),
        };
        socket.0.sock_fd = 100;
        unsafe { swr_v1_hook_socket_bind(state, READ | WRITE, core::ptr::null_mut()) };
        unsafe { crate::socket_extra::swr_v1_hook_socket_extra_free(extra) };
        socket.0.extra = core::ptr::null_mut();
        outcome
    }

    /// Both bodies over identical sockets, each with its own fresh hooks.
    fn compare(
        scenario: usize,
        setup: &dyn Fn(&mut TestSocket, &[*mut c_long; 3]),
        rust: &dyn Fn(*mut CoSocket) -> bool,
        upstream: &dyn Fn(*mut CoSocket) -> bool,
        hooks: &dyn Fn(),
    ) {
        install();
        hooks();
        let got = run(setup, rust);
        install();
        hooks();
        let want = run(setup, upstream);
        assert_eq!(got, want, "scenario {scenario}");
    }

    fn set_flags(socket: &TestSocket, connected: bool, read: bool, write: bool) {
        unsafe {
            swr_v1_hook_socket_set_flags(
                socket.state(),
                SocketFlags {
                    connected,
                    read,
                    write,
                },
            );
        }
    }

    fn bind(socket: &TestSocket, bindings: usize, cos: &[*mut c_long; 3]) {
        let pick = |on: bool, co: *mut c_long| if on { co.cast() } else { core::ptr::null_mut() };
        unsafe { swr_v1_hook_socket_bind(socket.state(), READ, pick(bindings & 1 != 0, cos[0])) };
        unsafe { swr_v1_hook_socket_bind(socket.state(), WRITE, pick(bindings & 2 != 0, cos[1])) };
    }

    /// Scenarios 0..512: each `how`, connection and half-close state, open or
    /// closed, with or without TLS, and `shutdown(2)` succeeding, failing with
    /// `ENOTCONN` or `EINVAL`, or succeeding with `ENOTCONN` left in errno.
    #[test]
    fn ported_shutdown_matrix() {
        for k in 0..512 {
            let how = [libc::SHUT_RD, libc::SHUT_WR, libc::SHUT_RDWR, 77][k % 4];
            let mode = k / 128;
            let setup = move |s: &mut TestSocket, _: &[*mut c_long; 3]| {
                set_flags(s, (k / 4) % 2 == 1, (k / 16) % 2 == 1, (k / 32) % 2 == 1);
                s.0.sock_fd = if (k / 8) % 2 == 1 { -1 } else { 77 };
                if (k / 64) % 2 == 1 {
                    unsafe { s.0.record() }.ssl =
                        core::ptr::NonNull::<u8>::dangling().as_ptr().cast();
                }
            };
            let hooks = move || {
                SSL_ERRNO.set(libc::EIO);
                let result = if mode == 1 || mode == 2 { -1 } else { 0 };
                let error = match mode {
                    1 | 3 => libc::ENOTCONN,
                    2 => libc::EINVAL,
                    _ => -1,
                };
                SHUTDOWN.set((result, error));
            };
            compare(
                k,
                &setup,
                &|s| unsafe { shutdown(s, how, FAKE) },
                &|s| unsafe { upstream_shutdown(s, how, FAKE) },
                &hooks,
            );
        }
    }

    const EVENTS: [u32; 8] = [
        0,
        READ,
        WRITE,
        READ | WRITE,
        READ | 1 << 11,
        WRITE | 1 << 11,
        1 << 31,
        u32::MAX,
    ];

    /// Scenarios 512..576: cancel for each event and binding, with the resumed
    /// coroutine unbinding itself or not. (The gate's third mode, a coroutine
    /// that deletes the socket, is a lifetime check C++ owns: Rust returns
    /// without touching the record after the resume.)
    #[test]
    fn ported_cancel_matrix() {
        for k in 0..64 {
            let (event, bindings, unbinds) = (EVENTS[k % 8], (k / 8) % 4, k / 32 == 1);
            let setup = move |s: &mut TestSocket, cos: &[*mut c_long; 3]| {
                bind(s, bindings, cos);
                let state = s.state();
                ON_RESUME.set(Some(Box::new(move |cid| {
                    if unbinds {
                        let event = if cid == 5 { READ } else { WRITE };
                        unsafe { swr_v1_hook_socket_bind(state, event, core::ptr::null_mut()) };
                    }
                })));
            };
            compare(
                k + 512,
                &setup,
                &|s| unsafe { cancel(s, event) },
                &|s| unsafe { upstream_cancel(s, event) },
                &|| {},
            );
        }
    }

    /// Scenarios 608..704: close of an open or closed, connected or not socket for
    /// each binding, with resumed coroutines unbinding themselves or the writer
    /// handing the read side to another coroutine, and the destructor callback
    /// recorded or closing again. (The gate's destructor that deletes the socket
    /// is a lifetime check C++ owns.)
    #[test]
    fn ported_close_matrix() {
        for k in 0..96 {
            let (closed, connected, bindings, mode, reenter) = (
                k & 1 == 1,
                k & 2 == 2,
                (k / 4) % 4,
                (k / 16) % 3,
                k / 48 == 1,
            );
            let setup = move |s: &mut TestSocket, cos: &[*mut c_long; 3]| {
                s.0.sock_fd = if closed { -1 } else { 77 };
                set_flags(s, connected, false, false);
                bind(s, bindings, cos);
                let (state, other) = (s.state(), cos[2]);
                ON_RESUME.set(Some(Box::new(move |cid| unsafe {
                    match (cid, mode) {
                        (5 | 11, 1 | 2) => {
                            swr_v1_hook_socket_bind(state, READ, core::ptr::null_mut());
                        }
                        (7, 1) => swr_v1_hook_socket_bind(state, WRITE, core::ptr::null_mut()),
                        (7, 2) => {
                            swr_v1_hook_socket_bind(state, WRITE, core::ptr::null_mut());
                            swr_v1_hook_socket_bind(state, READ, other.cast());
                        }
                        _ => {}
                    }
                })));
            };
            let close_with = |close: unsafe fn(*mut CoSocket, Sys) -> bool| {
                move |s: *mut CoSocket| {
                    if reenter {
                        ON_RELEASE.set(Some(Box::new(move |owner| {
                            log(format!("reenter:{}", unsafe { close(owner, FAKE) }));
                        })));
                    }
                    let result = unsafe { close(s, FAKE) };
                    let bound =
                        !unsafe { swr_v1_hook_socket_bound_co((*s).state, READ | WRITE) }.is_null();
                    if mode != 0 && !bound && !record(s).is_closed() {
                        log(format!("close-again:{}", unsafe { close(s, FAKE) }));
                    }
                    result
                }
            };
            compare(
                k + 608,
                &setup,
                &close_with(close),
                &close_with(upstream_close),
                &|| {},
            );
        }
    }

    /// Scenarios 752..1232: connect(addr) over connect(2)'s outcomes (at once,
    /// after EINTR, in progress, failing), the timer and the wait succeeding or not,
    /// the socket closing during the wait, each `SO_ERROR` outcome, and an open,
    /// closed or close-waiting socket.
    #[test]
    fn ported_connect_matrix() {
        let results: [&[c_int]; 5] = [
            &[0],
            &[-libc::EINTR, 0],
            &[-libc::EINPROGRESS],
            &[-libc::EINVAL],
            &[-libc::EINTR, -libc::EINPROGRESS],
        ];
        for k in 0..480 {
            let (script, timer_ok, wake_ok, closes) = (
                results[k % 5],
                (k / 5) % 2 == 1,
                (k / 10) % 2 == 1,
                (k / 20) % 2 == 1,
            );
            let (option, admission) = (u32::try_from((k / 40) % 4).unwrap(), k / 160);
            let setup = move |s: &mut TestSocket, _: &[*mut c_long; 3]| {
                if admission == 1 {
                    s.0.sock_fd = -1;
                }
                if admission == 2 {
                    unsafe { s.0.record() }.output.close_wait = 1;
                }
            };
            let hooks = move || {
                CONNECTS.set(script.to_vec());
                OPTION.set(option);
                TIMER_OK.set(timer_ok);
                ON_SUSPEND.set(Some(Box::new(move |s: *mut CoSocket, _| unsafe {
                    if closes {
                        (*s).sock_fd = -1;
                    }
                    if !wake_ok {
                        set_err(s, libc::ETIMEDOUT);
                    }
                })));
            };
            compare(
                k + 752,
                &setup,
                &|s| unsafe { connect(s, core::ptr::null(), 16, FAKE) },
                &|s| unsafe { upstream_connect(s, FAKE) },
                &hooks,
            );
        }
    }

    /// Scenarios 1232..1244: `check_liveness`, open or closed, the probe passing or
    /// failing with no errno, `EIO` or `ECONNRESET`.
    #[test]
    fn ported_liveness_matrix() {
        for k in 0..12 {
            let closed = k & 1 == 1;
            let probe = (k & 2 == 2, [0, libc::EIO, libc::ECONNRESET][k / 4]);
            compare(
                k + 1232,
                &move |s: &mut TestSocket, _: &[*mut c_long; 3]| {
                    s.0.sock_fd = if closed { -1 } else { 77 }
                },
                &|s| unsafe { liveness(s) },
                &|s| unsafe { upstream_liveness(s) },
                &move || PROBE.set(probe),
            );
        }
    }
}

//! The coroutine socket's lifecycle, as methods of its handle: `shutdown`
//! (admission, the TLS close, `shutdown(2)` and the half-close flags), `cancel`
//! (wake the coroutine bound to a direction), `close` (close-wait while coroutines
//! are bound, else release the descriptor), `connect(addr)` (the `connect(2)`
//! retries, the wait to write and the `SO_ERROR` check) and `check_liveness`.
//! The coroutine resume, the TLS shutdown and the liveness probe are
//! [`NATIVE`](crate::co_io::NATIVE)'s.
use crate::co_io::{CLOSE_WAIT_ERROR, NATIVE, Timer, available, errno, set_err};
use crate::co_socket::Co;
use crate::shutdown::{READ, WRITE};
use crate::socket_wait::wait_for;
use core::ffi::c_int;
use swoole_rs_corebase::net_types::SocketAddress;

/// The syscalls: `shutdown(2)`, `connect(2)` to an address and
/// `getsockopt(SO_ERROR)` into the int it is given. Tests script them.
#[derive(Clone, Copy)]
struct Sys {
    shutdown: fn(c_int, c_int) -> c_int,
    connect: fn(c_int, &SocketAddress) -> bool,
    option: fn(c_int, &mut c_int) -> c_int,
}

const SYS: Sys = Sys {
    shutdown: crate::shutdown::sys_shutdown,
    connect: sys_connect,
    option: so_error,
};

/// `connect(2)` of `fd` to `peer`.
fn sys_connect(fd: c_int, peer: &SocketAddress) -> bool {
    peer.connect(fd)
}

impl Co {
    /// `Socket::shutdown(how)`.
    #[must_use]
    pub fn shutdown(self, how: c_int) -> bool {
        shutdown(self, how, SYS)
    }

    /// `Socket::cancel(event)`: wake the coroutine bound to `event` with `ECANCELED`.
    #[expect(
        clippy::must_use_candidate,
        reason = "callers cancelling both directions ignore it, as upstream's do"
    )]
    pub fn cancel(self, event: u32) -> bool {
        let bound = self.with_state(|state| !state.bound_co(event).is_null());
        if !bound {
            return false;
        }
        if event != READ && event != WRITE {
            set_err(self, libc::EINVAL);
            return false;
        }
        set_err(self, libc::ECANCELED);
        crate::co_io::resume_bound(self, event);
        true
    }

    /// `Socket::close()`: true once the descriptor is released, false when already
    /// closed or while coroutines are still bound (close-wait). Releasing may destroy
    /// the socket, so the caller uses it no more after true.
    #[expect(
        clippy::must_use_candidate,
        reason = "callers that only want the socket gone ignore it, as upstream's do"
    )]
    pub fn close(self) -> bool {
        close(self, SYS)
    }

    /// `Socket::check_liveness()`.
    #[must_use]
    pub fn check_liveness(self) -> bool {
        if self.is_closed() {
            set_err(self, libc::EBADF);
            return false;
        }
        let alive = (NATIVE.liveness)(self);
        let error = match (alive, errno()) {
            (true, _) => 0,
            (false, 0) => libc::ECONNRESET,
            (false, error) => error,
        };
        set_err(self, error);
        alive
    }

    /// `Socket::connect(addr, addrlen)`.
    #[must_use]
    pub fn connect_addr(self, peer: &SocketAddress) -> bool {
        connect(self, peer, SYS)
    }
}

fn shutdown(socket: Co, how: c_int, sys: Sys) -> bool {
    set_err(socket, 0);
    let closed = socket.is_closed();
    let tls = !closed && socket.net(|record| !record.ssl().is_null()) == Some(true);
    let ssl_shutdown = || (NATIVE.ssl_shutdown)(socket);
    let ssl_shutdown: Option<&dyn Fn()> = tls.then_some(&ssl_shutdown);
    let fd = socket.sock_fd();
    let error = socket.with_state(|state| {
        crate::shutdown::shutdown(
            sys.shutdown,
            fd,
            how,
            closed,
            &mut state.flags,
            ssl_shutdown,
        )
    });
    if error != 0 {
        set_err(socket, error);
    }
    error == 0
}

fn close(socket: Co, sys: Sys) -> bool {
    if socket.is_closed() {
        set_err(socket, libc::EBADF);
        return false;
    }
    if socket.with_state(|state| state.flags.connected) {
        shutdown(socket, libc::SHUT_RDWR, sys);
    }
    if !socket
        .with_state(|state| state.bound_co(READ | WRITE))
        .is_null()
    {
        socket.net(|record| record.output.close_wait = true);
        socket.cancel(WRITE);
        socket.cancel(READ);
        set_err(socket, CLOSE_WAIT_ERROR);
        return false;
    }
    socket.set_sock_fd(-1);
    // The socket's last use here: it may destroy the socket.
    crate::socket_extra::run_close_hook(socket);
    true
}

/// Waits to write under the connect timeout, then reads `SO_ERROR` into the
/// socket's error code while the timer still runs: true once connected. The
/// socket is open.
fn settle(socket: Co, sys: Sys) -> bool {
    let timeout = socket.net(|record| record.timeouts.connect).unwrap_or(0.0);
    let mut timer = Timer::new(socket, WRITE, timeout);
    let woke = timer.start() && wait_for(socket, WRITE);
    let connected = if woke {
        let mut error = socket.err_code();
        let result = (sys.option)(socket.sock_fd(), &mut error);
        socket.set_err_code(error);
        if result < 0 || error != 0 {
            set_err(socket, error);
            false
        } else {
            true
        }
    } else {
        if socket.is_closed() {
            set_err(socket, libc::ECONNABORTED);
        }
        false
    };
    timer.stop();
    connected
}

/// `Socket::connect(addr, addrlen)`, with `connect(2)` and the `SO_ERROR` read.
fn connect(socket: Co, peer: &SocketAddress, sys: Sys) -> bool {
    #[cfg(feature = "iouring")]
    if let Some(connected) = crate::uring_socket::with(socket, |ops| {
        crate::uring_socket::swr_v1_hook_uring_connect(ops, peer)
    }) {
        return connected;
    }
    if !available(socket, READ | WRITE) {
        return false;
    }
    let fd = socket.sock_fd();
    let connected = loop {
        let connected = (sys.connect)(fd, peer);
        if connected || errno() != libc::EINTR {
            break connected;
        }
    };
    if !connected {
        let error = errno();
        if error != libc::EINPROGRESS {
            set_err(socket, error);
            return false;
        }
        if !settle(socket, sys) {
            return false;
        }
    }
    socket.with_state(|state| state.flags.connected = true);
    // `get_name`'s failure is ignored.
    socket.net(swoole_rs_reactor::SocketFields::get_name);
    set_err(socket, 0);
    true
}

/// `getsockopt(fd, SOL_SOCKET, SO_ERROR)` into `error`.
fn so_error(fd: c_int, error: &mut c_int) -> c_int {
    swoole_rs_corebase::sys::getsockopt_int(fd, libc::SOL_SOCKET, libc::SO_ERROR, error)
}

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
    use crate::shutdown::{SocketFlags, SocketState};
    use core::ffi::{c_long, c_void};
    use std::cell::{Cell, RefCell};

    // Upstream's bodies, transcribed: socket.cc's shutdown, cancel, close,
    // connect(addr) and check_liveness.
    fn upstream_shutdown(s: Co, mut how: c_int, sys: Sys) -> bool {
        set_err(s, 0);
        let flags = s.with_state(|state| state.flags);
        let connected = flags.connected && !s.is_closed();
        let mut error = 0;
        if !connected || how == libc::SHUT_RD && flags.read || how == libc::SHUT_WR && flags.write {
            error = libc::ENOTCONN;
        } else {
            if !unsafe { (*s.socket()).fields.ssl().is_null() } {
                (NATIVE.ssl_shutdown)(s);
            }
            if (sys.shutdown)(s.sock_fd(), how) < 0 {
                error = errno();
            }
            if error == 0 || error == libc::ENOTCONN {
                if error == libc::ENOTCONN {
                    how = libc::SHUT_RDWR;
                }
                s.with_state(|state| {
                    let flags = &mut state.flags;
                    match how {
                        libc::SHUT_RD => flags.read = true,
                        libc::SHUT_WR => flags.write = true,
                        _ => (flags.read, flags.write) = (true, true),
                    }
                    if flags.read && flags.write {
                        flags.connected = false;
                    }
                });
                return true;
            }
        }
        set_err(s, error);
        false
    }

    fn upstream_cancel(s: Co, event: u32) -> bool {
        if s.with_state(|state| state.bound_co(event).is_null()) {
            return false;
        }
        if event == READ || event == WRITE {
            set_err(s, libc::ECANCELED);
            let co = s.with_state(|state| state.co(event));
            // SAFETY: the coroutine bound for `event`, parked in its wait on `s`.
            unsafe { (NATIVE.resume)(co) };
            true
        } else {
            set_err(s, libc::EINVAL);
            false
        }
    }

    fn upstream_close(s: Co, sys: Sys) -> bool {
        if s.is_closed() {
            set_err(s, libc::EBADF);
            return false;
        }
        if s.with_state(|state| state.flags.connected) {
            upstream_shutdown(s, libc::SHUT_RDWR, sys);
        }
        if s.with_state(|state| state.bound_co(READ | WRITE).is_null()) {
            s.set_sock_fd(-1);
            crate::socket_extra::run_close_hook(s);
            true
        } else {
            unsafe { (*s.socket()).fields.output.close_wait = true };
            upstream_cancel(s, WRITE);
            upstream_cancel(s, READ);
            set_err(s, CLOSE_WAIT_ERROR);
            false
        }
    }

    fn upstream_connect(s: Co, sys: Sys) -> bool {
        if !available(s, READ | WRITE) {
            return false;
        }
        let peer = SocketAddress {
            len: libc::socklen_t::try_from(size_of::<libc::sockaddr>()).unwrap(),
            ..SocketAddress::default()
        };
        let connected = loop {
            let connected = (sys.connect)(s.sock_fd(), &peer);
            if connected || errno() != libc::EINTR {
                break connected;
            }
        };
        if !connected {
            if errno() != libc::EINPROGRESS {
                set_err(s, errno());
                return false;
            }
            let timeout = unsafe { (*s.socket()).fields.timeouts.connect };
            let mut timer = Timer::new(s, WRITE, timeout);
            if !timer.start() || !wait_for(s, WRITE) {
                if s.is_closed() {
                    set_err(s, libc::ECONNABORTED);
                }
                timer.stop();
                return false;
            }
            if (sys.option)(s.sock_fd(), unsafe { &mut *(*s.raw()).err_code.as_ptr() }) < 0
                || s.err_code() != 0
            {
                set_err(s, s.err_code());
                timer.stop();
                return false;
            }
            timer.stop();
        }
        s.with_state(|state| state.flags.connected = true);
        unsafe { (*s.socket()).get_name() };
        set_err(s, 0);
        true
    }

    fn upstream_liveness(s: Co) -> bool {
        if s.is_closed() {
            set_err(s, libc::EBADF);
            return false;
        }
        if !(NATIVE.liveness)(s) {
            let error = errno();
            set_err(s, if error != 0 { error } else { libc::ECONNRESET });
            return false;
        }
        set_err(s, 0);
        true
    }

    thread_local! {
        /// shutdown(2)'s result and the errno it leaves (-1 for none).
        static SHUTDOWN: Cell<(c_int, c_int)> = const { Cell::new((0, -1)) };
        /// connect(2)'s scripted results (-errno for a failure), and SO_ERROR's mode.
        static CONNECTS: RefCell<Vec<c_int>> = const { RefCell::new(Vec::new()) };
        static OPTION: Cell<u32> = const { Cell::new(0) };
    }

    fn fake_shutdown(fd: c_int, how: c_int) -> c_int {
        log(format!("shutdown:{fd}:{how}"));
        let (result, error) = SHUTDOWN.get();
        if error >= 0 {
            set_errno(error);
        }
        result
    }
    fn fake_connect(fd: c_int, _: &SocketAddress) -> bool {
        log(format!("connect:{fd}"));
        let value = CONNECTS.with_borrow_mut(|c| c.remove(0));
        if value < 0 {
            set_errno(-value);
            return false;
        }
        true
    }
    /// Modes: 0 no error, 1 `ECONNREFUSED`, 2 the call fails, 3 the call fails with no error.
    fn fake_option(_: c_int, error: &mut c_int) -> c_int {
        let mode = OPTION.get();
        log(format!("get-option:{error}"));
        match mode {
            0 | 3 => *error = 0,
            1 => *error = libc::ECONNREFUSED,
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
        state: (c_int, SocketFlags, c_int, bool, c_long, c_long, c_int),
    }

    fn id(state: *mut SocketState, event: u32) -> c_long {
        let co = unsafe { (*state).co(event) };
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
        operation: &dyn Fn(Co) -> bool,
    ) -> Outcome {
        let mut socket = TestSocket::new();
        crate::socket_extra::set_close_hook(
            socket.co(),
            Box::new(|owner| unsafe { release(core::ptr::null_mut(), owner) }),
        );
        unsafe { socket.0.record() }.timeouts.connect = 1.0;
        socket.0.err_code.set(9);
        let mut ids: [Box<c_long>; 3] = [Box::new(5), Box::new(7), Box::new(11)];
        let cos = [&raw mut *ids[0], &raw mut *ids[1], &raw mut *ids[2]];
        setup(&mut socket, &cos);
        set_errno(0);
        take();
        let result = operation(socket.co());
        let state = socket.state();
        let flags = unsafe { (*state).flags };
        let outcome = Outcome {
            result,
            effects: take(),
            state: (
                socket.0.sock_fd.get(),
                flags,
                socket.0.err_code.get(),
                unsafe { socket.0.record() }.output.close_wait,
                id(state, READ),
                id(state, WRITE),
                errno(),
            ),
        };
        socket.0.sock_fd.set(100);
        unsafe { (*state).bind(READ | WRITE, core::ptr::null_mut()) };
        outcome
    }

    /// Both bodies over identical sockets, each with its own fresh hooks.
    fn compare(
        scenario: usize,
        setup: &dyn Fn(&mut TestSocket, &[*mut c_long; 3]),
        rust: &dyn Fn(Co) -> bool,
        upstream: &dyn Fn(Co) -> bool,
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
            (*socket.state()).flags = SocketFlags {
                connected,
                read,
                write,
            };
        }
    }

    fn bind(socket: &TestSocket, bindings: usize, cos: &[*mut c_long; 3]) {
        let pick = |on: bool, co: *mut c_long| if on { co.cast() } else { core::ptr::null_mut() };
        let state = unsafe { &mut *socket.state() };
        state.bind(READ, pick(bindings & 1 != 0, cos[0]));
        state.bind(WRITE, pick(bindings & 2 != 0, cos[1]));
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
                s.0.sock_fd.set(if (k / 8) % 2 == 1 { -1 } else { 77 });
                if (k / 64) % 2 == 1 {
                    unsafe {
                        s.0.record()
                            .set_ssl(core::ptr::NonNull::<u8>::dangling().as_ptr().cast());
                    }
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
                &|s| shutdown(s, how, FAKE),
                &|s| upstream_shutdown(s, how, FAKE),
                &hooks,
            );
        }
    }

    const EVENTS: [u32; 8] = [
        0,
        READ,
        WRITE,
        READ | WRITE,
        READ | (1 << 11),
        WRITE | (1 << 11),
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
                        unsafe { (*state).bind(event, core::ptr::null_mut()) };
                    }
                })));
            };
            compare(
                k + 512,
                &setup,
                &|s| s.cancel(event),
                &|s| upstream_cancel(s, event),
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
                s.0.sock_fd.set(if closed { -1 } else { 77 });
                set_flags(s, connected, false, false);
                bind(s, bindings, cos);
                let (state, other) = (s.state(), cos[2]);
                ON_RESUME.set(Some(Box::new(move |cid| unsafe {
                    match (cid, mode) {
                        (5 | 11, 1 | 2) => (*state).bind(READ, core::ptr::null_mut()),
                        (7, 1) => (*state).bind(WRITE, core::ptr::null_mut()),
                        (7, 2) => {
                            (*state).bind(WRITE, core::ptr::null_mut());
                            (*state).bind(READ, other.cast::<c_void>());
                        }
                        _ => {}
                    }
                })));
            };
            let close_with = |close: fn(Co, Sys) -> bool| {
                move |s: Co| {
                    if reenter {
                        ON_RELEASE.set(Some(Box::new(move |owner| {
                            log(format!("reenter:{}", close(owner, FAKE)));
                        })));
                    }
                    let result = close(s, FAKE);
                    let bound = !s.with_state(|state| state.bound_co(READ | WRITE).is_null());
                    if mode != 0 && !bound && !s.is_closed() {
                        log(format!("close-again:{}", close(s, FAKE)));
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
                    s.0.sock_fd.set(-1);
                }
                if admission == 2 {
                    unsafe { s.0.record() }.output.close_wait = true;
                }
            };
            let hooks = move || {
                CONNECTS.set(script.to_vec());
                OPTION.set(option);
                TIMER_OK.set(timer_ok);
                ON_SUSPEND.set(Some(Box::new(move |s: Co, _| {
                    if closes {
                        s.set_sock_fd(-1);
                    }
                    if !wake_ok {
                        set_err(s, libc::ETIMEDOUT);
                    }
                })));
            };
            let peer = SocketAddress {
                len: 16,
                ..SocketAddress::default()
            };
            compare(
                k + 752,
                &setup,
                &|s| connect(s, &peer, FAKE),
                &|s| upstream_connect(s, FAKE),
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
                    s.0.sock_fd.set(if closed { -1 } else { 77 });
                },
                &|s| s.check_liveness(),
                &|s| upstream_liveness(s),
                &move || PROBE.set(probe),
            );
        }
    }
}

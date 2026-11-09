//! The coroutine socket's effects: [`NATIVE`] (the last error and its messages, the
//! coroutine, the reactor and the write buffer) and [`Host`] (the timers, the read
//! buffer, `recv` and the packet callbacks). Over them,
//! the record's `set_err`, `check_return_value` and `is_available`, and the
//! per-direction timer (`TimerController`).
use crate::co_socket::CoSocket;
use crate::shutdown::{READ, swr_v1_hook_socket_bound_co, swr_v1_hook_socket_event_name};
use core::ffi::{CStr, c_char, c_int, c_long, c_void};
use core::ptr;
#[cfg(not(test))]
use core::sync::atomic::{AtomicPtr, Ordering};

/// `SW_ERROR_CO_SOCKET_CLOSE_WAIT`.
pub(crate) const CLOSE_WAIT_ERROR: c_int = 10019;
/// `SW_WAIT`.
const WAIT: c_int = 2;

/// The native socket's effects the packet framing uses: the timers, the read
/// `String`, `recv`, the `Protocol` callback and the packet diagnostics. The
/// runtime's table is installed when native registers the hooks host; tests set
/// theirs per thread. Each takes a live record.
#[repr(C)]
pub struct Host {
    /// A timer for the read or the `write` wait that calls `timer_fired`, or null.
    pub timer_add: extern "C" fn(*mut CoSocket, f64, bool) -> *mut c_void,
    pub timer_del: extern "C" fn(*mut c_void),
    /// The read String: 0 get, 1 store, 2 extend, 3 reduce; refreshes the view.
    pub read_buffer: extern "C" fn(*mut CoSocket, *mut crate::packet::ReadBuffer, u32, usize),
    /// `Socket::recv/recv_all` (all selects the latter).
    pub recv: extern "C" fn(*mut CoSocket, *mut u8, usize, bool) -> isize,
    /// The protocol's `get_package_length` over the read buffer's first bytes.
    pub package_length: extern "C" fn(*mut CoSocket, u32, *mut u32) -> isize,
    /// Packet error diagnostics, with the same kinds as the packet receive.
    pub packet_fail: extern "C" fn(*mut CoSocket, u32, isize),
}

#[cfg(not(test))]
static HOST: AtomicPtr<Host> = AtomicPtr::new(ptr::null_mut());

#[cfg(test)]
thread_local! {
    /// Each test thread's table: the recording host's unless a test sets its own.
    pub(crate) static HOST: core::cell::Cell<&'static Host> =
        const { core::cell::Cell::new(&test_host::HOST_TABLE) };
}

/// Installs the runtime's table when native registers the hooks host
/// (`stdio::swr_v1_hook_install`), so the runtime's timer and coroutine symbols
/// link only then, not into every binary that links hooks.
#[cfg(not(test))]
pub(crate) fn install_native() {
    HOST.store((&raw const native::HOST).cast_mut(), Ordering::Release);
}

/// The installed table.
#[cfg(not(test))]
pub(crate) fn host() -> &'static Host {
    // SAFETY: the runtime's static table is installed by swoole_init
    // (stdio::swr_v1_hook_install), before any socket operation runs.
    unsafe { &*HOST.load(Ordering::Acquire) }
}

/// This thread's table.
#[cfg(test)]
pub(crate) fn host() -> &'static Host {
    HOST.get()
}

/// The runtime effects the operations use, each over a live record or coroutine.
pub(crate) struct Ops {
    /// `swoole_set_last_error`.
    pub set_last_error: fn(c_int),
    /// `swoole_strerror`.
    pub strerror: fn(c_int) -> *const c_char,
    /// `Coroutine::get_current_safe()`.
    pub current: fn() -> *mut c_void,
    /// A coroutine's id.
    pub cid: fn(*mut c_void) -> c_long,
    /// `Coroutine::print_socket_bound_error(fd, event, cid)`.
    pub bound_error: fn(c_int, &CStr, c_long),
    /// `System::sleep(0.01)`.
    pub sleep: fn(),
    /// `swoole_event_add(events)` when `add`, else `swoole_event_set(events)`;
    /// false, with errno set, on failure.
    pub reactor: fn(*mut CoSocket, u32, bool) -> bool,
    /// `remove_read_event`/`remove_write_event`.
    pub remove: fn(*mut CoSocket, u32),
    /// Resumes a coroutine.
    pub resume: fn(*mut c_void),
    /// `socket->ssl_shutdown()`.
    pub ssl_shutdown: fn(*mut CoSocket),
    /// `socket->check_liveness()`, errno set when it fails.
    pub liveness: fn(*mut CoSocket) -> bool,
    /// Yields `co` until resumed, with `cancel(event)` as its cancel function.
    pub suspend: fn(*mut CoSocket, *mut c_void, u32),
    /// Moves a write's `length` bytes at `*buffer` to the write buffer unless they
    /// are there already or the socket is zero-copy.
    pub buffer: fn(*mut CoSocket, *mut *mut u8, usize),
}

/// The runtime's effects: the error slot, the coroutine, the timer and the reactor.
#[cfg(not(test))]
pub(crate) static NATIVE: Ops = Ops {
    set_last_error: swoole_rs_corebase::log::set_last_error,
    strerror: |error| swoole_rs_error::swoole_strerror(error),
    current: || crate::wait_event::native::current(),
    cid: native::cid,
    bound_error: native::bound_error,
    sleep: native::sleep,
    reactor: native::reactor,
    remove: native::remove,
    resume: crate::poll::native::resume,
    ssl_shutdown: native::ssl_shutdown,
    liveness: native::liveness,
    suspend: native::suspend,
    buffer: native::buffer,
};

/// The unit tests link no runtime: theirs is the recording fake.
#[cfg(test)]
pub(crate) static NATIVE: Ops = test_host::OPS;

#[cfg(not(test))]
mod native {
    use super::Host;
    use crate::co_socket::{CoSocket, Protocol};
    use crate::packet::ReadBuffer;
    use crate::shutdown::{READ, WRITE};
    use core::ffi::{CStr, c_char, c_int, c_long, c_void};
    use swoole_rs_buffer::string::CppString;
    use swoole_rs_reactor::SocketRecord;

    // SAFETY: crates/coroutine's.
    unsafe extern "C-unwind" {
        #[link_name = "_ZN6swoole9Coroutine24print_socket_bound_errorEiPKcl"]
        fn print_socket_bound_error(fd: c_int, event: *const c_char, cid: c_long);
        fn swr_v1_coroutine_yield_with_cancel(
            co: *mut c_void,
            cancel: unsafe extern "C-unwind" fn(*mut c_void, *mut c_void) -> bool,
            data: *mut c_void,
        );
    }

    pub(super) static HOST: Host = Host {
        timer_add,
        timer_del,
        read_buffer,
        recv,
        package_length,
        packet_fail,
    };

    /// A record and a direction in one pointer: the record is aligned, so its low
    /// bit is free for `write`.
    fn tag(socket: *mut CoSocket, write: bool) -> *mut c_void {
        socket.map_addr(|addr| addr | usize::from(write)).cast()
    }

    fn untag(data: *mut c_void) -> (*mut CoSocket, u32) {
        let event = if data.addr() & 1 == 0 { READ } else { WRITE };
        (data.map_addr(|addr| addr & !1).cast(), event)
    }

    extern "C" fn timer_add(socket: *mut CoSocket, timeout: f64, write: bool) -> *mut c_void {
        extern "C-unwind" fn fired(data: *mut c_void) {
            let (socket, event) = untag(data);
            // SAFETY: the record whose timer fired: its wait deletes the timer before
            // the record goes.
            unsafe { super::timer_fired(socket, event) };
        }
        crate::poll::native::timer_add_with(timeout, fired, tag(socket, write))
    }

    extern "C" fn timer_del(node: *mut c_void) {
        crate::poll::native::timer_del(node);
    }

    /// `co->yield(&cancel_fn)`, `cancel_fn` being `socket->cancel(event)`.
    pub(super) fn suspend(socket: *mut CoSocket, co: *mut c_void, event: u32) {
        extern "C-unwind" fn cancel(_: *mut c_void, data: *mut c_void) -> bool {
            let (socket, event) = untag(data);
            // SAFETY: the record whose wait the coroutine is suspended in.
            unsafe { crate::lifecycle::swr_v1_hook_co_socket_cancel(socket, event) }
        }
        let data = tag(socket, event != READ);
        // SAFETY: the current coroutine, waiting on the live record, whose frames own
        // nothing with a destructor.
        unsafe { swr_v1_coroutine_yield_with_cancel(co, cancel, data) };
    }

    /// `get_write_buffer()`, cleared and holding the write's bytes.
    pub(super) fn buffer(socket: *mut CoSocket, at: *mut *mut u8, length: usize) {
        // SAFETY: the waiting write's live record, not otherwise borrowed.
        let record = unsafe { &mut *socket };
        if record.zero_copy || length == 0 {
            return;
        }
        // SAFETY: the write's live buffer slot.
        let at = unsafe { &mut *at };
        let string = crate::co_socket::buffer(record, true);
        if *at != string.str {
            string.length = 0;
            string.offset = 0;
            // SAFETY: the write's `length` bytes, outside the String.
            unsafe { string.append(*at, length) };
            *at = string.str;
        }
    }

    extern "C" fn read_buffer(
        socket: *mut CoSocket,
        view: *mut ReadBuffer,
        action: u32,
        size: usize,
    ) {
        // SAFETY: the operation's live record, not otherwise borrowed.
        let string = crate::co_socket::buffer(unsafe { &mut *socket }, false);
        // SAFETY: the operation's live view.
        let view = unsafe { &mut *view };
        if action != 0 {
            string.length = view.length;
            string.offset = view.offset;
            if action == 2 {
                string.extend(size);
            } else if action == 3 {
                string.reduce(size.cast_signed() as i64);
            }
        }
        *view = ReadBuffer {
            length: string.length,
            size: string.size,
            offset: string.offset,
            str: string.str,
        };
    }

    extern "C" fn recv(socket: *mut CoSocket, at: *mut u8, length: usize, all: bool) -> isize {
        let call = if all {
            crate::socket_members::recv_all
        } else {
            crate::socket_members::recv
        };
        // SAFETY: the live record, and `length` free bytes of its read buffer at `at`.
        unsafe { call(socket.cast(), at.cast(), length) }
    }

    /// `swoole::PacketLength`.
    #[repr(C)]
    struct PacketLength {
        buf: *const u8,
        buf_size: u32,
        header_len: u32,
    }

    /// `Protocol::get_package_length`.
    type LengthFunc =
        unsafe extern "C" fn(*const Protocol, *mut SocketRecord, *mut PacketLength) -> isize;

    extern "C" fn package_length(socket: *mut CoSocket, size: u32, header: *mut u32) -> isize {
        // SAFETY: the live record, read before the callback runs.
        let record = unsafe { &*socket };
        let mut packet = PacketLength {
            // SAFETY: the read String the packet receive fills.
            buf: unsafe { (*record.read_buffer.cast::<CppString>()).str },
            buf_size: size,
            header_len: 0,
        };
        // SAFETY: a length-checking protocol's callback is set.
        let call = unsafe {
            core::mem::transmute::<*mut c_void, LengthFunc>(record.protocol.get_package_length)
        };
        // SAFETY: the record's protocol and network socket, and `size` bytes at `buf`.
        let length = unsafe { call(&raw const record.protocol, record.socket, &raw mut packet) };
        // SAFETY: the caller's header length.
        unsafe { *header = packet.header_len };
        length
    }

    /// `SW_ERROR_PACKAGE_LENGTH_TOO_LARGE`, `SW_ERROR_PACKAGE_LENGTH_NOT_FOUND` and
    /// `SW_LOG_WARNING`.
    const TOO_LARGE: c_int = 1201;
    const NOT_FOUND: c_int = 1202;
    const LOG_WARNING: c_int = 4;

    extern "C" fn packet_fail(socket: *mut CoSocket, kind: u32, length: isize) {
        let (error, message) = match kind {
            0 => (NOT_FOUND, c"get package length failed".as_ptr()),
            1 => {
                use swoole_rs_clients::address::{
                    swr_v1_clients_address_host, swr_v1_clients_address_port,
                };
                // SAFETY: the live record.
                let network = unsafe { (*socket).socket };
                // SAFETY: its open network socket.
                let peer = unsafe { &(*network).info };
                // SAFETY: the peer's live address.
                let addr = unsafe { swr_v1_clients_address_host(peer) };
                // SAFETY: NUL-terminated text, unchanged until the next address.
                let addr = unsafe { CStr::from_ptr(addr) };
                // SAFETY: the peer's live address.
                let port = unsafe { swr_v1_clients_address_port(peer) };
                swoole_rs_corebase::log::error_log(
                    LOG_WARNING,
                    TOO_LARGE,
                    c"Socket::recv_packet_with_length_protocol",
                    format_args!(
                        "packet length is too big, remote_addr={}:{port}, length={}",
                        addr.to_string_lossy(),
                        length.cast_unsigned()
                    ),
                );
                (TOO_LARGE, swoole_rs_error::swoole_get_last_error_msg())
            }
            _ => (
                TOO_LARGE,
                c"no package eof, package_max_length exceeded".as_ptr(),
            ),
        };
        // SAFETY: the live record; each message is static or the thread's sw_error.
        unsafe { super::swr_v1_hook_co_socket_borrowed_err(socket, error, message) };
    }

    pub(super) fn cid(co: *mut c_void) -> c_long {
        // SAFETY: a live coroutine, which starts with its Rust state.
        unsafe { (*co.cast::<swoole_rs_coroutine::Coroutine>()).cid }
    }

    pub(super) fn bound_error(fd: c_int, event: &CStr, cid: c_long) {
        // SAFETY: a NUL-terminated name; the report is fatal in a running server.
        unsafe { print_socket_bound_error(fd, event.as_ptr(), cid) };
    }

    pub(super) fn sleep() {
        // SAFETY: socket waits run in a coroutine.
        _ = unsafe { crate::sleep::system_sleep(0.01) };
    }

    pub(super) fn reactor(socket: *mut CoSocket, events: u32, add: bool) -> bool {
        use swoole_rs_reactor::event_api::{swoole_event_add, swoole_event_set};
        // SAFETY: the record of a live socket.
        let record = unsafe { (*socket).socket };
        let events = events.cast_signed();
        let result = if add {
            // SAFETY: an open socket's record, on this thread's running reactor.
            unsafe { swoole_event_add(record, events) }
        } else {
            // SAFETY: as above.
            unsafe { swoole_event_set(record, events) }
        };
        result == 0
    }

    pub(super) fn remove(socket: *mut CoSocket, event: u32) {
        /// `Reactor::interest`'s `remove_read_event`; `remove_write_event` follows it.
        const REMOVE_READ: u32 = 4;
        let kind = REMOVE_READ + u32::from(event != crate::shutdown::READ);
        // SAFETY: the record of a live socket.
        let record = unsafe { (*socket).socket };
        let reactor = swoole_rs_corebase::thread_reactor();
        // SAFETY: this thread's live reactor and a socket registered in it.
        unsafe { swoole_rs_reactor::reactor_upstream::reactor_interest(reactor, record, kind, 0) };
    }

    pub(super) fn ssl_shutdown(socket: *mut CoSocket) {
        // SAFETY: the record of a live socket.
        let record = unsafe { (*socket).socket };
        // SAFETY: an open record holding an SSL.
        unsafe { swoole_rs_tls::upstream::socket_ssl_shutdown(record) };
    }

    pub(super) fn liveness(socket: *mut CoSocket) -> bool {
        // SAFETY: the record of a live socket.
        let record = unsafe { (*socket).socket };
        // SAFETY: an open record.
        unsafe { swoole_rs_tls::upstream::socket_check_liveness(record) }
    }
}

pub(crate) fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

pub(crate) use swoole_rs_corebase::set_errno;

/// `set_err(e)`: the record's code and message, errno and the last error.
///
/// # Safety
/// `socket` is a live record, not borrowed elsewhere for the call.
pub(crate) unsafe fn set_err(socket: *mut CoSocket, error: c_int) {
    // SAFETY: as the caller promises.
    unsafe { (*socket).err_code = error };
    set_errno(error);
    (NATIVE.set_last_error)(error);
    let message = if error == 0 {
        c"".as_ptr()
    } else {
        (NATIVE.strerror)(error)
    };
    // SAFETY: as the caller promises.
    unsafe { (*socket).err_msg = message };
}

/// `check_return_value`: clear the error after a success; publish errno after a
/// failure that set none.
///
/// # Safety
/// As [`set_err`].
pub(crate) unsafe fn check(socket: *mut CoSocket, result: isize) {
    if result >= 0 {
        // SAFETY: forwarded.
        unsafe { set_err(socket, 0) };
    // SAFETY: as the caller promises.
    } else if unsafe { (*socket).err_code } == 0 {
        // SAFETY: forwarded.
        unsafe { set_err(socket, errno()) };
    }
}

/// `is_available(event)`: no other coroutine holds `event` (`SW_EVENT_NULL`
/// skips that check), the socket is open and not waiting to close.
///
/// # Safety
/// As [`set_err`]; the record's state is live.
pub(crate) unsafe fn available(socket: *mut CoSocket, event: u32) -> bool {
    if event != 0 {
        // SAFETY: forwarded; the report may close the socket, so it is read again after.
        unsafe { check_bound(socket, event) };
    }
    // SAFETY: as the caller promises; read again after the report.
    let record = unsafe { &*socket };
    let error = if record.is_closed() {
        libc::EBADF
    // SAFETY: an open record's network::Socket, not otherwise borrowed.
    } else if unsafe { record.record() }.output.close_wait != 0 {
        CLOSE_WAIT_ERROR
    } else {
        return true;
    };
    // SAFETY: forwarded.
    unsafe { set_err(socket, error) };
    false
}

/// `get_bound_cid(event)`: the id of the coroutine `get_bound_co` finds, or 0.
///
/// # Safety
/// `socket` is a live record.
unsafe fn bound_cid(socket: *const CoSocket, event: u32) -> c_long {
    // SAFETY: as the caller promises.
    let state = unsafe { (*socket).state };
    // SAFETY: the record's live state.
    let co = unsafe { swr_v1_hook_socket_bound_co(state, event) };
    if co.is_null() { 0 } else { (NATIVE.cid)(co) }
}

/// `get_event_str(event)`.
///
/// # Safety
/// `socket` is a live record.
unsafe fn event_str(socket: *const CoSocket, event: u32) -> &'static CStr {
    // SAFETY: as the caller promises.
    let state = unsafe { (*socket).state };
    // SAFETY: the record's live state.
    match unsafe { swr_v1_hook_socket_event_name(state, event) } {
        0 => c"reading",
        1 => c"writing",
        _ => c"reading or writing",
    }
}

/// `check_bound_co(event)`: report a coroutine with a nonzero id holding `event`.
///
/// # Safety
/// `socket` is a live record.
pub(crate) unsafe fn check_bound(socket: *const CoSocket, event: u32) {
    // SAFETY: as the caller promises.
    let cid = unsafe { bound_cid(socket, event) };
    if cid != 0 {
        // SAFETY: as the caller promises.
        let fd = unsafe { (*socket).sock_fd };
        // SAFETY: as the caller promises.
        let name = unsafe { event_str(socket, event) };
        (NATIVE.bound_error)(fd, name, cid);
    }
}

/// `TimeoutController::has_timedout(kind)` at `now`: the first call starts the
/// clock; later ones set the time left as `kind`'s timeout, or time out
/// (`ETIMEDOUT`) with less than `SW_TIMER_MIN_SEC` left. No limit unless positive.
///
/// # Safety
/// `socket` is a live, open record, not borrowed elsewhere for the call.
unsafe fn timed_out(
    socket: *mut CoSocket,
    timeout: f64,
    startup: &mut f64,
    now: f64,
    kind: c_int,
) -> bool {
    /// `SW_TIMER_MIN_SEC`.
    const MIN_SEC: f64 = 0.001;
    if timeout <= 0.0 || timeout.is_nan() {
        return false;
    }
    if *startup == 0.0 {
        *startup = now;
        return false;
    }
    let left = timeout - (now - *startup);
    if left < MIN_SEC {
        // SAFETY: forwarded.
        unsafe { set_err(socket, libc::ETIMEDOUT) };
        return true;
    }
    // SAFETY: as the caller promises.
    let socket = unsafe { &*socket };
    // SAFETY: an open record's network::Socket, not otherwise borrowed.
    unsafe { socket.record() }.timeouts.set(left, kind);
    false
}

macro_rules! export {
    ($(#[$doc:meta])* fn $name:ident($($arg:ident: $ty:ty),*) $(-> $ret:ty)? = $call:expr) => {
        $(#[$doc])*
        ///
        /// # Safety
        /// `socket` is a live record, not borrowed elsewhere for the call.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name($($arg: $ty),*) $(-> $ret)? {
            // SAFETY: as the caller promises.
            unsafe { $call }
        }
    };
}

export!(
    /// `set_err(e)`.
    fn swr_v1_hook_co_socket_set_err(socket: *mut CoSocket, error: c_int) = set_err(socket, error)
);
export!(
    /// `check_return_value(retval)`.
    fn swr_v1_hook_co_socket_check(socket: *mut CoSocket, result: isize) = check(socket, result)
);
export!(
    /// `is_available(event)`.
    fn swr_v1_hook_co_socket_available(socket: *mut CoSocket, event: u32) -> bool = available(socket, event)
);
export!(
    /// `get_bound_cid(event)`.
    fn swr_v1_hook_co_socket_bound_cid(socket: *const CoSocket, event: u32) -> c_long = bound_cid(socket, event)
);
export!(
    /// `check_bound_co(event)`.
    fn swr_v1_hook_co_socket_check_bound(socket: *const CoSocket, event: u32) = check_bound(socket, event)
);
export!(
    /// `get_event_str(event)`, a static string.
    fn swr_v1_hook_co_socket_event_str(socket: *const CoSocket, event: u32) -> *const c_char = event_str(socket, event).as_ptr()
);

/// `has_timedout(kind)` with `startup` its controller's clock and `now` `microtime()`.
///
/// # Safety
/// `socket` is a live, open record, not borrowed elsewhere for the call; `startup`
/// is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_timed_out(
    socket: *mut CoSocket,
    timeout: f64,
    startup: *mut f64,
    now: f64,
    kind: c_int,
) -> bool {
    // SAFETY: as the caller promises.
    let startup = unsafe { &mut *startup };
    // SAFETY: as the caller promises.
    unsafe { timed_out(socket, timeout, startup, now, kind) }
}

/// `catch_read_error`/`catch_write_error(errno) == SW_WAIT`.
///
/// # Safety
/// `socket` is an open record.
pub(crate) unsafe fn would_block(socket: *mut CoSocket) -> bool {
    let error = errno();
    // SAFETY: as the caller promises.
    let socket = unsafe { &*socket };
    // SAFETY: an open record's network::Socket, not otherwise borrowed.
    unsafe { socket.record() }.catch_error(error) == WAIT
}

/// A timer slot's value while a negative timeout's wait runs without a timer.
const SENTINEL: *mut c_void = usize::MAX as *mut c_void;

/// `TimerController`: one wait's timer in the socket's slot for its direction.
#[repr(C)]
pub struct Timer {
    socket: *mut CoSocket,
    slot: *mut *mut c_void,
    timeout: f64,
    enabled: bool,
    write: bool,
}

impl Timer {
    /// A timer for `event`'s direction (`SW_EVENT_READ`, else writing).
    ///
    /// # Safety
    /// `socket` is a live record; the timer does not outlive it.
    pub(crate) unsafe fn new(socket: *mut CoSocket, event: u32, timeout: f64) -> Self {
        // SAFETY: as the caller promises.
        let state = unsafe { (*socket).state };
        // SAFETY: the record's live state.
        let timers = unsafe { &raw mut (*state).timers };
        let write = event != READ;
        // The index is 0 or 1, so the slot stays inside the two-entry array.
        let slot = timers
            .cast::<*mut c_void>()
            .wrapping_add(usize::from(write));
        Self {
            socket,
            slot,
            timeout,
            enabled: false,
            write,
        }
    }

    /// `start()`: unless the slot holds a timer already, one firing after
    /// `timeout` (false if none could be added), or the sentinel for a negative
    /// timeout. No timer for a zero timeout.
    pub(crate) fn start(&mut self) -> bool {
        // SAFETY: the live slot (see `new`).
        if self.timeout == 0.0 || !unsafe { *self.slot }.is_null() {
            return true;
        }
        self.enabled = true;
        if self.timeout > 0.0 {
            let node = (host().timer_add)(self.socket, self.timeout, self.write);
            // SAFETY: as above.
            unsafe { *self.slot = node };
            return !node.is_null();
        }
        // SAFETY: as above.
        unsafe { *self.slot = SENTINEL };
        true
    }

    /// The destructor: delete the timer this one started and clear the slot.
    pub(crate) fn stop(&mut self) {
        // SAFETY: the live slot (see `new`).
        let node = unsafe { *self.slot };
        if !self.enabled || node.is_null() {
            return;
        }
        if node != SENTINEL {
            (host().timer_del)(node);
        }
        // SAFETY: as above.
        unsafe { *self.slot = ptr::null_mut() };
    }
}

/// `Socket::timer_callback`: the timer for `event`'s direction fired. Its slot
/// clears, the socket's error becomes `ETIMEDOUT` and the coroutine waiting in that
/// direction resumes; an empty slot aborts, as upstream's unknown node does.
///
/// # Safety
/// `socket` is the live record whose timer fired.
pub(crate) unsafe fn timer_fired(socket: *mut CoSocket, event: u32) {
    // SAFETY: as the caller promises.
    unsafe { set_err(socket, libc::ETIMEDOUT) };
    // SAFETY: as the caller promises.
    let state = unsafe { (*socket).state };
    // SAFETY: the record's live state.
    let state = unsafe { &mut *state };
    let [read, write] = &mut state.timers;
    let slot = if event == READ { read } else { write };
    if slot.is_null() {
        std::process::abort();
    }
    *slot = ptr::null_mut();
    // SAFETY: the record's live state.
    let co = unsafe { crate::shutdown::swr_v1_hook_socket_co(state, event) };
    (NATIVE.resume)(co);
}

/// A recording host for the operations' tests. Each thread records its own events.
#[cfg(test)]
pub(crate) mod test_host {
    use super::*;
    use std::cell::{Cell, RefCell};

    type Suspend = Box<dyn FnMut(*mut CoSocket, u32)>;
    type OnResume = Box<dyn FnMut(c_long)>;
    type OnRelease = Box<dyn FnMut(*mut CoSocket)>;

    thread_local! {
        static EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        pub(crate) static TIMER_OK: Cell<bool> = const { Cell::new(true) };
        pub(crate) static REACTOR_OK: Cell<bool> = const { Cell::new(true) };
        /// What a suspended coroutine does before it is resumed.
        pub(crate) static ON_SUSPEND: RefCell<Option<Suspend>> = RefCell::new(None);
        /// What the bound-coroutine report does after it is recorded.
        pub(crate) static ON_BOUND: RefCell<Option<Box<dyn FnMut()>>> = RefCell::new(None);
        /// What a resumed coroutine (its id) and the destructor callback do.
        pub(crate) static ON_RESUME: RefCell<Option<OnResume>> = RefCell::new(None);
        pub(crate) static ON_RELEASE: RefCell<Option<OnRelease>> = RefCell::new(None);
        /// The liveness probe's answer and errno; the errno a TLS shutdown leaves.
        pub(crate) static PROBE: Cell<(bool, c_int)> = const { Cell::new((true, 0)) };
        pub(crate) static SSL_ERRNO: Cell<c_int> = const { Cell::new(0) };
        static NODES: Cell<usize> = const { Cell::new(0) };
    }

    fn hook<T: ?Sized>(
        slot: &'static std::thread::LocalKey<RefCell<Option<Box<T>>>>,
        run: impl FnOnce(&mut T),
    ) {
        let taken = slot.take();
        if let Some(mut taken) = taken {
            run(&mut taken);
            if slot.with_borrow(Option::is_none) {
                slot.set(Some(taken));
            }
        }
    }

    fn resume(co: *mut c_void) {
        let cid = unsafe { *co.cast::<c_long>() };
        log(format!("resume:{cid}"));
        hook(&ON_RESUME, |f| f(cid));
    }
    /// A test socket's destructor callback; its owner is the record.
    pub(crate) unsafe extern "C" fn release(_: *mut c_void, socket: *mut c_void) {
        log("release");
        hook(&ON_RELEASE, |f| f(socket.cast()));
    }
    fn ssl_shutdown(_: *mut CoSocket) {
        log("ssl-shutdown");
        set_errno(SSL_ERRNO.get());
    }
    fn liveness(_: *mut CoSocket) -> bool {
        log("probe");
        let (alive, error) = PROBE.get();
        set_errno(error);
        alive
    }

    pub(crate) fn log(event: impl Into<String>) {
        EVENTS.with_borrow_mut(|events| events.push(event.into()));
    }

    pub(crate) fn take() -> Vec<String> {
        EVENTS.with_borrow_mut(core::mem::take)
    }

    /// Installs the host and clears this thread's events and settings.
    pub(crate) fn install() {
        super::HOST.set(&HOST_TABLE);
        take();
        TIMER_OK.set(true);
        REACTOR_OK.set(true);
        ON_SUSPEND.set(None);
        ON_BOUND.set(None);
        ON_RESUME.set(None);
        ON_RELEASE.set(None);
        PROBE.set((true, 0));
        SSL_ERRNO.set(0);
    }

    fn set_last_error(error: c_int) {
        log(format!("last-error:{error}"));
    }
    fn strerror(_: c_int) -> *const c_char {
        c"native-error".as_ptr()
    }
    fn current() -> *mut c_void {
        ptr::dangling_mut::<u64>().cast()
    }
    /// A test coroutine is a `c_long` holding its id.
    fn cid(co: *mut c_void) -> c_long {
        let cid = unsafe { *co.cast::<c_long>() };
        log(format!("cid:{cid}"));
        cid
    }
    fn bound_error(fd: c_int, event: &CStr, cid: c_long) {
        let event = event.to_str().unwrap();
        log(format!("bound-error:{fd}:{event}:{cid}"));
        let hook = ON_BOUND.take();
        if let Some(mut hook) = hook {
            hook();
            ON_BOUND.set(Some(hook));
        }
    }
    extern "C" fn timer_add(_: *mut CoSocket, timeout: f64, _: bool) -> *mut c_void {
        log(format!("timer:add:{timeout}"));
        if !TIMER_OK.get() {
            return ptr::null_mut();
        }
        NODES.set(NODES.get() + 1);
        ptr::dangling_mut::<u32>().cast()
    }
    extern "C" fn timer_del(_: *mut c_void) {
        log("timer:delete");
    }
    fn sleep() {
        log("sleep");
    }
    fn reactor(_: *mut CoSocket, events: u32, add: bool) -> bool {
        log(format!("{}:{events}", if add { "add" } else { "set" }));
        REACTOR_OK.get()
    }
    fn suspend(socket: *mut CoSocket, _: *mut c_void, event: u32) {
        log(format!("yield:{event}"));
        let hook = ON_SUSPEND.take();
        if let Some(mut hook) = hook {
            hook(socket, event);
            ON_SUSPEND.set(Some(hook));
        }
    }
    fn buffer(_: *mut CoSocket, _: *mut *mut u8, length: usize) {
        log(format!("buffer:{length}"));
    }
    fn remove(_: *mut CoSocket, event: u32) {
        log(format!("remove:{event}"));
    }

    pub(crate) const OPS: Ops = Ops {
        set_last_error,
        strerror,
        current,
        cid,
        bound_error,
        sleep,
        reactor,
        remove,
        resume,
        ssl_shutdown,
        liveness,
        suspend,
        buffer,
    };
    pub(super) static HOST_TABLE: Host = Host {
        timer_add,
        timer_del,
        read_buffer: crate::packet::tests::read_buffer,
        recv: crate::packet::tests::receive,
        package_length: crate::packet::tests::get_length,
        packet_fail: crate::packet::tests::failure,
    };
}

#[cfg(test)]
mod tests {
    use super::test_host::{ON_BOUND, TIMER_OK, install, take};
    use super::*;
    use crate::co_socket::TestSocket;
    use crate::shutdown::{WRITE, swr_v1_hook_socket_bind};

    fn slot_state(socket: &TestSocket) -> usize {
        let node = unsafe { (*socket.state()).timers[0] };
        if node.is_null() {
            0
        } else if node == SENTINEL {
            2
        } else {
            1
        }
    }

    /// The ordered gate's former scenarios 931..1003: upstream's `TimerController`
    /// start (twice in half of them) and destructor over each timeout, starting
    /// slot and `enabled`, with `swoole_timer_add` succeeding or failing.
    #[test]
    fn ported_timer_controller_matrix() {
        install();
        let times = [-1.0, 0.0, 0.002];
        for k in 0..72 {
            let (timeout, start_state) = (times[k % 3], (k / 3) % 3);
            let (timer_ok, enabled, again) =
                ((k / 9) % 2 == 1, (k / 18) % 2 == 1, (k / 36) % 2 == 1);
            let mut socket = TestSocket::new();
            let preset =
                [ptr::null_mut(), ptr::dangling_mut::<u16>().cast(), SENTINEL][start_state];
            unsafe { (*socket.state()).timers[0] = preset };
            TIMER_OK.set(timer_ok);
            take();
            // Upstream: start() adds only into an empty slot for a nonzero timeout.
            let mut expected = Vec::new();
            let mut node = start_state;
            let mut on = enabled;
            let start = |expected: &mut Vec<String>, node: &mut usize, on: &mut bool| {
                if timeout != 0.0 && *node == 0 {
                    *on = true;
                    if timeout > 0.0 {
                        expected.push(format!("timer:add:{timeout}"));
                        *node = usize::from(timer_ok);
                        return timer_ok;
                    }
                    *node = 2;
                }
                true
            };
            let first = start(&mut expected, &mut node, &mut on);
            let second = again.then(|| start(&mut expected, &mut node, &mut on));
            if on && node != 0 {
                if node != 2 {
                    expected.push("timer:delete".into());
                }
                node = 0;
            }
            let mut timer = unsafe { Timer::new(socket.ptr(), READ, timeout) };
            timer.enabled = enabled;
            assert_eq!(timer.start(), first, "scenario {}", k + 931);
            if let Some(second) = second {
                assert_eq!(timer.start(), second, "scenario {}", k + 931);
            }
            timer.stop();
            assert_eq!(take(), expected, "scenario {}", k + 931);
            assert_eq!(slot_state(&socket), node, "scenario {}", k + 931);
            unsafe { (*socket.state()).timers[0] = ptr::null_mut() };
        }
    }

    /// Upstream's `timer_callback`: the firing timer's slot clears, the error is
    /// `ETIMEDOUT` and the coroutine bound in that direction resumes.
    #[test]
    fn timer_fired_resumes_its_direction() {
        install();
        let mut socket = TestSocket::new();
        let mut ids: [c_long; 2] = [3, 4];
        bind(&socket, 3, &mut ids);
        let node: *mut c_void = ptr::dangling_mut::<u32>().cast();
        for (slot, event, cid) in [(1, WRITE, 4), (0, READ, 3)] {
            unsafe { (*socket.state()).timers[slot] = node };
            unsafe { timer_fired(socket.ptr(), event) };
            assert!(unsafe { (*socket.state()).timers[slot] }.is_null());
            assert_eq!(unsafe { (*socket.ptr()).err_code }, libc::ETIMEDOUT);
            assert_eq!(
                take(),
                [
                    format!("last-error:{}", libc::ETIMEDOUT),
                    format!("resume:{cid}")
                ]
            );
        }
    }

    const MASKS: [u32; 6] = [0, READ, WRITE, READ | WRITE, 1 << 11, u32::MAX];

    /// Binds a reader with id `reader` and a writer with id `writer` per `bound`'s bits.
    fn bind(socket: &TestSocket, bound: usize, ids: &mut [c_long; 2]) {
        let [reader, writer] = ids;
        let co = |on: bool, id: &mut c_long| {
            if on {
                ptr::from_mut(id).cast()
            } else {
                ptr::null_mut()
            }
        };
        unsafe { swr_v1_hook_socket_bind(socket.state(), READ, co(bound & 1 != 0, reader)) };
        unsafe { swr_v1_hook_socket_bind(socket.state(), WRITE, co(bound & 2 != 0, writer)) };
    }

    /// Upstream's `get_event_str`, `get_bound_cid` and `check_bound_co`.
    fn upstream_bound(
        fd: c_int,
        bound: usize,
        [reader, writer]: [c_long; 2],
        event: u32,
    ) -> (c_long, &'static str, Vec<String>) {
        let (read, write) = (bound & 1 != 0, bound & 2 != 0);
        let name = if event == READ {
            "reading"
        } else if event == WRITE {
            "writing"
        } else if read && write {
            "reading or writing"
        } else if read {
            "reading"
        } else {
            "writing"
        };
        let co = if event & READ != 0 && read {
            Some(reader)
        } else if event & WRITE != 0 && write {
            Some(writer)
        } else {
            None
        };
        let mut effects = Vec::new();
        let cid = co.map_or(0, |cid| {
            effects.push(format!("cid:{cid}"));
            cid
        });
        if cid != 0 {
            effects.push(format!("bound-error:{fd}:{name}:{cid}"));
        }
        (cid, name, effects)
    }

    /// The socket policy gate's former scenarios 0..72: the bound coroutine, its id,
    /// the event's name and the bound report, for each binding, id sign and mask.
    #[test]
    fn ported_bound_matrix() {
        install();
        for k in 0..72 {
            let (bound, event) = ((k / 6) % 4, MASKS[k % 6]);
            let ids = [[0, 0], [5, 7], [-5, -7]][k / 24];
            let socket = TestSocket::new();
            let mut cells = ids;
            bind(&socket, bound, &mut cells);
            let (cid, name, effects) = upstream_bound(100, bound, ids, event);
            assert_eq!(
                unsafe { swr_v1_hook_co_socket_bound_cid(&raw const *socket.0, event) },
                cid
            );
            let got = unsafe {
                CStr::from_ptr(swr_v1_hook_co_socket_event_str(&raw const *socket.0, event))
            };
            assert_eq!(got.to_str().unwrap(), name, "scenario {k}");
            take();
            unsafe { swr_v1_hook_co_socket_check_bound(&raw const *socket.0, event) };
            assert_eq!(take(), effects, "scenario {k}");
        }
    }

    /// The socket policy gate's former scenarios 72..216: `is_available` over each
    /// binding, mask and socket state (open, closed, close-wait), with the bound
    /// report recorded or closing the socket. (Its third mode, a report that
    /// throws, has no counterpart: the real report is a fatal error.)
    #[test]
    fn ported_availability_matrix() {
        install();
        for k in 0..144 {
            let (event, bound, state, closes) =
                (MASKS[k % 6], (k / 6) % 4, (k / 24) % 3, k / 72 == 1);
            let mut socket = TestSocket::new();
            let mut ids = [5, 7];
            bind(&socket, bound, &mut ids);
            socket.0.sock_fd = if state == 1 { -1 } else { 100 };
            unsafe { socket.0.record() }.output.close_wait = u8::from(state == 2);
            let target = socket.ptr();
            if closes {
                ON_BOUND.set(Some(Box::new(move || unsafe { (*target).sock_fd = -1 })));
            }
            set_errno(0);
            take();
            let mut expected = Vec::new();
            let mut closed = state == 1;
            if event != 0 {
                let (_, _, effects) = upstream_bound(socket.0.sock_fd, bound, [5, 7], event);
                closed |= closes && effects.len() == 2;
                expected = effects;
            }
            let error = if closed {
                libc::EBADF
            } else if state == 2 {
                CLOSE_WAIT_ERROR
            } else {
                0
            };
            if error != 0 {
                expected.push(format!("last-error:{error}"));
            }
            let available = unsafe { swr_v1_hook_co_socket_available(target, event) };
            assert_eq!(
                (available, socket.0.err_code, errno()),
                (error == 0, error, error),
                "scenario {}",
                k + 72
            );
            assert_eq!(take(), expected, "scenario {}", k + 72);
            ON_BOUND.set(None);
            socket.0.sock_fd = 100;
        }
    }

    /// The socket policy gate's former scenarios 288..324: `check_return_value`
    /// over the result's extremes, a preset error and errno.
    #[test]
    fn ported_check_matrix() {
        install();
        for k in 0..36 {
            let result = [isize::MIN, -1, 0, isize::MAX][k % 4];
            let preset = [0, 9, -9][(k / 4) % 3];
            let system = [0, libc::EINVAL, libc::EAGAIN][k / 12];
            let mut socket = TestSocket::new();
            socket.0.err_code = preset;
            socket.0.err_msg = c"preset".as_ptr();
            set_errno(system);
            take();
            // Upstream: set_err(0) after a success, set_err(errno) if no error is set.
            let set = if result >= 0 {
                Some(0)
            } else if preset == 0 {
                Some(system)
            } else {
                None
            };
            unsafe { swr_v1_hook_co_socket_check(socket.ptr(), result) };
            let message = unsafe { CStr::from_ptr(socket.0.err_msg) }
                .to_str()
                .unwrap();
            let expected = set.map_or((preset, system, "preset", vec![]), |e| {
                (
                    e,
                    e,
                    if e == 0 { "" } else { "native-error" },
                    vec![format!("last-error:{e}")],
                )
            });
            assert_eq!(
                (socket.0.err_code, errno(), message, take()),
                expected,
                "scenario {}",
                k + 288
            );
        }
    }

    /// The socket policy gate's former scenarios 324..348: `set_timeout(timeval)`'s
    /// conversion, upstream's two divisions.
    #[test]
    #[expect(
        clippy::cast_precision_loss,
        reason = "upstream converts both fields to double"
    )]
    fn ported_timeval_conversion() {
        for seconds in [-1i64, 0, 1, 9_007_199_254_740_993] {
            for micros in [-1i64, 0, 1, 999_999, 1_000_000, i64::from(i32::MAX)] {
                let expected = seconds as f64 + micros as f64 / 1000.0 / 1000.0;
                let got = crate::swr_v1_hook_timeval(seconds as f64, micros as f64);
                assert_eq!(got.to_bits(), expected.to_bits(), "{seconds} {micros}");
            }
        }
    }

    /// The socket policy gate's former scenarios 348..420: `has_timedout` over each
    /// timeout, starting clock and start time, three calls half a second apart.
    #[test]
    fn ported_has_timedout_matrix() {
        install();
        let timeouts = [-1.0, 0.0, 0.001, 1.0, f64::NAN, f64::INFINITY];
        for k in 0..72 {
            let (timeout, start) = (timeouts[k % 6], [0.0, -0.0, 1.0, 2.0][(k / 6) % 4]);
            let mut now = 1.0 + f64::from(u32::try_from(k / 24).unwrap()) * 2.0;
            let mut socket = TestSocket::new();
            unsafe { socket.0.record() }.timeouts.read = -7.0;
            let (mut startup, mut model) = (start, start);
            for _ in 0..3 {
                take();
                // Upstream, transcribed.
                let (mut expired, mut set) = (false, None);
                if timeout > 0.0 {
                    if model == 0.0 {
                        model = now;
                    } else {
                        let used = now - model;
                        if timeout - used < 0.001 {
                            expired = true;
                        } else {
                            set = Some(timeout - used);
                        }
                    }
                }
                let before = unsafe { socket.0.record() }.timeouts.read;
                let got = unsafe {
                    swr_v1_hook_co_socket_timed_out(socket.ptr(), timeout, &raw mut startup, now, 4)
                };
                let after = unsafe { socket.0.record() }.timeouts.read;
                assert_eq!(
                    (got, startup.to_bits()),
                    (expired, model.to_bits()),
                    "scenario {}",
                    k + 348
                );
                assert_eq!(
                    after.to_bits(),
                    set.unwrap_or(before).to_bits(),
                    "scenario {}",
                    k + 348
                );
                assert_eq!(
                    take(),
                    if expired {
                        vec![format!("last-error:{}", libc::ETIMEDOUT)]
                    } else {
                        vec![]
                    }
                );
                now += 0.5;
            }
        }
    }

    #[test]
    fn check_clears_or_publishes_errno_unless_an_error_is_set() {
        install();
        let mut socket = TestSocket::new();
        set_errno(libc::EINVAL);
        unsafe { check(socket.ptr(), -1) };
        assert_eq!(socket.0.err_code, libc::EINVAL);
        assert_eq!(
            unsafe { core::ffi::CStr::from_ptr(socket.0.err_msg) },
            c"native-error"
        );
        set_errno(libc::EAGAIN);
        unsafe { check(socket.ptr(), -1) };
        assert_eq!(socket.0.err_code, libc::EINVAL);
        unsafe { check(socket.ptr(), 0) };
        assert_eq!((socket.0.err_code, errno()), (0, 0));
        assert_eq!(unsafe { *socket.0.err_msg }, 0);
        assert_eq!(take(), ["last-error:22", "last-error:0"]);
    }
}

/// Publishes the calling thread's current error without changing its errno or
/// last-error slot. Unlike `set_err(0)`, the zero message comes from strerror.
/// # Safety
/// socket is live and exclusively writable; `last_error` is the caller's TLS value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_default_err(
    socket: *mut CoSocket,
    last_error: c_int,
) {
    let error = if last_error == 0 { errno() } else { last_error };
    // SAFETY: the caller lends a live writable socket.
    unsafe { (*socket).err_code = error };
    let message = (NATIVE.strerror)(error);
    // SAFETY: the socket remains live through the leaf operation.
    unsafe { (*socket).err_msg = message };
}

/// # Safety
/// socket is exclusively writable; message remains readable while published.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_borrowed_err(
    socket: *mut CoSocket,
    error: c_int,
    message: *const c_char,
) {
    // SAFETY: the caller lends the live socket.
    unsafe { (*socket).err_code = error };
    set_errno(error);
    (NATIVE.set_last_error)(error);
    // SAFETY: the borrowed message outlives publication.
    unsafe { (*socket).err_msg = message };
}

/// Sets owned error text, publishing error/errno first and the message pointer
/// only after successful assignment. Native cache exceptions are contained and
/// returned as status, to be rethrown after this Rust frame returns.
/// # Safety
/// socket/extra are live and exclusive; bytes are readable for length; the
/// optional native cache only updates `std::string` and catches every exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_owned_err(
    socket: *mut CoSocket,
    error: c_int,
    bytes: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: the caller lends a live writable socket.
    unsafe { (*socket).err_code = error };
    set_errno(error);
    (NATIVE.set_last_error)(error);
    // SAFETY: the metadata owner outlives this synchronized call.
    let extra = unsafe { (*socket).extra };
    // SAFETY: its first stable slot owns the error string; take no Rust borrow.
    let slot = unsafe { &raw const (*extra).strings[0] };
    // SAFETY: readable bytes; the native cache contains all exceptions below Rust.
    let status = unsafe { swoole_rs_corebase::swr_v1_core_string_set(slot, bytes, length) };
    if status == 0 {
        // SAFETY: successful assignment leaves stable terminated bytes in the slot.
        let view = unsafe { swoole_rs_corebase::swr_v1_core_string_view(slot) };
        // SAFETY: publish only after the complete string assignment succeeds.
        unsafe { (*socket).err_msg = view.data };
    }
    status
}

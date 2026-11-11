//! The coroutine socket's effects: [`NATIVE`] (the last error and its messages, the
//! coroutine, the reactor and the write buffer) and [`Host`] (the timers, the read
//! buffer, `recv` and the packet callbacks). Over them,
//! the record's `set_err`, `check_return_value` and `is_available`, and the
//! per-direction timer (`TimerController`).
use crate::co_socket::Co;
use crate::shutdown::READ;
use core::ffi::{CStr, c_char, c_int, c_long, c_void};
use core::ptr;

/// `SW_ERROR_CO_SOCKET_CLOSE_WAIT`.
pub(crate) const CLOSE_WAIT_ERROR: c_int = 10019;
/// `SW_WAIT`.
const WAIT: c_int = 2;

/// The native socket's effects the packet framing uses: the timers, the read
/// `String`, `recv`, the `Protocol` callback and the packet diagnostics. The
/// runtime's table is installed when native registers the hooks host; tests set
/// theirs per thread. Each takes a live record.
pub struct Host {
    /// A timer for the read or the `write` wait that calls `timer_fired`, or null.
    pub timer_add: fn(Co, f64, bool) -> *mut c_void,
    pub timer_del: fn(*mut c_void),
    /// The read String: 0 get, 1 store, 2 extend, 3 reduce; refreshes the view.
    pub read_buffer: fn(Co, &mut crate::packet::ReadBuffer, u32, usize),
    /// `Socket::recv/recv_all` (all selects the latter) into the read String's
    /// free space.
    pub recv: fn(Co, &mut [u8], bool) -> isize,
    /// The protocol's `get_package_length` over the read buffer's first bytes;
    /// writes the header length.
    pub package_length: fn(Co, u32, &mut u32) -> isize,
    /// Packet error diagnostics, with the same kinds as the packet receive.
    pub packet_fail: fn(Co, u32, isize),
}

#[cfg(not(test))]
static HOST: std::sync::OnceLock<&'static Host> = std::sync::OnceLock::new();

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
    HOST.get_or_init(|| &native::HOST);
}

/// The installed table.
#[cfg(not(test))]
pub(crate) fn host() -> &'static Host {
    // swoole_init installs it (stdio::swr_v1_hook_install) before any socket
    // operation runs.
    HOST.get().copied().unwrap_or_else(|| std::process::abort())
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
    pub reactor: fn(Co, u32, bool) -> bool,
    /// `remove_read_event`/`remove_write_event`.
    pub remove: fn(Co, u32),
    /// Resumes a coroutine.
    pub resume: fn(*mut c_void),
    /// `socket->ssl_shutdown()`.
    pub ssl_shutdown: fn(Co),
    /// `socket->check_liveness()`, errno set when it fails.
    pub liveness: fn(Co) -> bool,
    /// Yields `co` until resumed, with `cancel(event)` as its cancel function.
    pub suspend: fn(Co, *mut c_void, u32),
    /// Moves a write's `length` bytes at `*buffer` to the write buffer unless they
    /// are there already or the socket is zero-copy.
    pub buffer: fn(Co, *mut *mut u8, usize),
}

/// The runtime's effects: the error slot, the coroutine, the timer and the reactor.
#[cfg(not(test))]
pub(crate) static NATIVE: Ops = Ops {
    set_last_error: swoole_rs_corebase::log::set_last_error,
    strerror: |error| swoole_rs_error_ffi::swoole_strerror(error),
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
    use crate::co_socket::Co;
    use crate::packet::ReadBuffer;
    use crate::shutdown::{READ, WRITE};
    use core::ffi::{CStr, c_int, c_long, c_void};
    use swoole_rs_buffer::string::CppString;
    use swoole_rs_corebase::net_types::PacketLength;
    use swoole_rs_coroutine::print_socket_bound_error;

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
    fn tag(socket: Co, write: bool) -> *mut c_void {
        socket.this().map_addr(|addr| addr | usize::from(write))
    }

    /// The record and direction [`tag`] packed.
    ///
    /// # Safety
    /// `data` came from [`tag`] of a record still live.
    unsafe fn untag(data: *mut c_void) -> (Co, u32) {
        let event = if data.addr() & 1 == 0 { READ } else { WRITE };
        // SAFETY: as the caller promises.
        (unsafe { Co::of(data.map_addr(|addr| addr & !1)) }, event)
    }

    fn timer_add(socket: Co, timeout: f64, write: bool) -> *mut c_void {
        /// # Safety
        /// `data` is the [`tag`] of the record whose timer fired: its wait deletes
        /// the timer before the record goes.
        unsafe extern "C-unwind" fn fired(data: *mut c_void) {
            // SAFETY: per the caller.
            let (socket, event) = unsafe { untag(data) };
            super::timer_fired(socket, event);
        }
        // SAFETY: `fired` gets the live record's tag, as it asks.
        unsafe {
            swoole_rs_timer::swr_v1_timer_add_plain_sec(timeout, false, fired, tag(socket, write))
        }
    }

    fn timer_del(node: *mut c_void) {
        crate::poll::native::timer_del(node);
    }

    /// `co->yield(&cancel_fn)`, `cancel_fn` being `socket->cancel(event)`.
    pub(super) fn suspend(socket: Co, co: *mut c_void, event: u32) {
        let event = if event == READ { READ } else { WRITE };
        // SAFETY: the current coroutine, waiting on the live record, whose frames own
        // nothing with a destructor.
        unsafe { swoole_rs_coroutine::yield_with_cancel(co, move |_| socket.cancel(event)) };
    }

    /// `get_write_buffer()`, cleared and holding the write's bytes.
    pub(super) fn buffer(socket: Co, at: *mut *mut u8, length: usize) {
        if socket.zero_copy() || length == 0 {
            return;
        }
        socket.with_buffer(true, |string| {
            // SAFETY: the write's live buffer slot.
            let at = unsafe { &mut *at };
            if *at != string.as_mut_ptr() {
                string.clear();
                // SAFETY: the write's `length` bytes, outside the String.
                unsafe { swoole_rs_buffer::string::swr_string_append(string, *at, length) };
                *at = string.as_mut_ptr();
            }
        });
    }

    fn read_buffer(socket: Co, view: &mut ReadBuffer, action: u32, size: usize) {
        socket.with_buffer(false, |string| {
            if action != 0 {
                // SAFETY: the packet receive's view of this String counts only
                // the bytes its receives wrote (crate::packet).
                unsafe { string.set_len(view.length) };
                string.set_offset(view.offset);
                if action == 2 {
                    string.extend(size);
                } else if action == 3 {
                    string.reduce(size.cast_signed() as i64);
                }
            }
            *view = ReadBuffer {
                length: string.len(),
                size: string.size(),
                offset: string.offset(),
                str: string.as_mut_ptr(),
            };
        });
    }

    fn recv(socket: Co, buf: &mut [u8], all: bool) -> isize {
        if all {
            socket.recv_all(buf)
        } else {
            socket.recv(buf)
        }
    }

    fn package_length(socket: Co, size: u32, header: &mut u32) -> isize {
        // The record's read String, which the packet receive fills.
        let buf = socket.with_buffer(false, CppString::as_mut_ptr);
        let mut packet = PacketLength {
            buf,
            buf_size: size,
            header_len: 0,
        };
        let (protocol, callback) = (socket.protocol_ptr(), socket.protocol().get_package_length);
        let Some(call) = callback else {
            return -1;
        };
        // SAFETY: the record's protocol and network socket, and `size` bytes at `buf`.
        let length = unsafe { call(protocol, socket.socket().cast(), &mut packet) };
        *header = packet.header_len;
        length
    }

    /// `SW_ERROR_PACKAGE_LENGTH_TOO_LARGE`, `SW_ERROR_PACKAGE_LENGTH_NOT_FOUND` and
    /// `SW_LOG_WARNING`.
    const TOO_LARGE: c_int = 1201;
    const NOT_FOUND: c_int = 1202;
    const LOG_WARNING: c_int = 4;

    fn packet_fail(socket: Co, kind: u32, length: isize) {
        use super::Message;
        let (error, message) = match kind {
            0 => (NOT_FOUND, Message::Static(c"get package length failed")),
            1 => {
                use swoole_rs_clients::address::{
                    swr_v1_clients_address_host, swr_v1_clients_address_port,
                };
                let (addr, port) = socket
                    .net(|record| {
                        let peer = &record.info;
                        let addr = swr_v1_clients_address_host(peer);
                        // SAFETY: NUL-terminated text, unchanged until the next address.
                        let addr = unsafe { CStr::from_ptr(addr) }
                            .to_string_lossy()
                            .into_owned();
                        (addr, swr_v1_clients_address_port(peer))
                    })
                    .unwrap_or_default();
                swoole_rs_corebase::log::error_log(
                    LOG_WARNING,
                    TOO_LARGE,
                    c"Socket::recv_packet_with_length_protocol",
                    format_args!(
                        "packet length is too big, remote_addr={addr}:{port}, length={}",
                        length.cast_unsigned()
                    ),
                );
                (TOO_LARGE, Message::LastError)
            }
            _ => (
                TOO_LARGE,
                Message::Static(c"no package eof, package_max_length exceeded"),
            ),
        };
        super::borrowed_err(socket, error, message);
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

    pub(super) fn reactor(socket: Co, events: u32, add: bool) -> bool {
        use swoole_rs_reactor::event_api::{swoole_event_add, swoole_event_set};
        let record = socket.socket();
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

    pub(super) fn remove(socket: Co, event: u32) {
        /// `Reactor::interest`'s `remove_read_event`; `remove_write_event` follows it.
        const REMOVE_READ: u32 = 4;
        let kind = REMOVE_READ + u32::from(event != crate::shutdown::READ);
        let reactor = swoole_rs_corebase::thread_reactor();
        // SAFETY: this thread's live reactor and a socket registered in it.
        unsafe {
            swoole_rs_reactor::reactor_upstream::reactor_interest(
                reactor,
                socket.socket(),
                kind,
                0,
            );
        };
    }

    pub(super) fn ssl_shutdown(socket: Co) {
        socket.with_network(swoole_rs_tls::upstream::socket_ssl_shutdown);
    }

    pub(super) fn liveness(socket: Co) -> bool {
        socket
            .with_network(swoole_rs_tls::upstream::check_liveness)
            .unwrap_or(false)
    }
}

pub(crate) fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

pub(crate) use swoole_rs_corebase::set_errno;

/// `set_err(e)`: the record's code and message, errno and the last error.
pub(crate) fn set_err(socket: Co, error: c_int) {
    let message = if error == 0 {
        Message::Static(c"")
    } else {
        Message::Strerror
    };
    borrowed_err(socket, error, message);
}

/// `check_return_value`: clear the error after a success; publish errno after a
/// failure that set none.
pub(crate) fn check(socket: Co, result: isize) {
    if result >= 0 {
        set_err(socket, 0);
    } else if socket.err_code() == 0 {
        set_err(socket, errno());
    }
}

/// `is_available(event)`: no other coroutine holds `event` (`SW_EVENT_NULL`
/// skips that check), the socket is open and not waiting to close.
pub(crate) fn available(socket: Co, event: u32) -> bool {
    if event != 0 {
        // The report may close the socket, so it is read again after.
        check_bound(socket, event);
    }
    let error = if socket.is_closed() {
        libc::EBADF
    } else if socket.net(|record| record.output.close_wait != 0) == Some(true) {
        CLOSE_WAIT_ERROR
    } else {
        return true;
    };
    set_err(socket, error);
    false
}

/// `check_bound_co(event)`: report a coroutine with a nonzero id holding `event`.
pub(crate) fn check_bound(socket: Co, event: u32) {
    let cid = socket.bound_cid(event);
    if cid != 0 {
        (NATIVE.bound_error)(socket.sock_fd(), event_str(socket, event), cid);
    }
}

/// `get_event_str(event)`.
#[must_use]
pub fn event_str(socket: Co, event: u32) -> &'static CStr {
    match socket.with_state(|state| state.event_name(event)) {
        0 => c"reading",
        1 => c"writing",
        _ => c"reading or writing",
    }
}

impl Co {
    /// `set_err(code)`: the code and its message.
    pub fn set_err(self, error: c_int) {
        set_err(self, error);
    }

    /// `set_err(code, message)`, copying the message; non-zero when the copy failed.
    #[must_use]
    pub fn set_error(self, error: c_int, message: &[u8]) -> c_int {
        owned_err(self, error, message)
    }

    /// `get_bound_cid(event)`: the id of the coroutine holding `event`, or 0.
    #[must_use]
    pub fn bound_cid(self, event: u32) -> c_long {
        let co = self.with_state(|state| state.bound_co(event));
        if co.is_null() { 0 } else { (NATIVE.cid)(co) }
    }

    /// `check_bound_co(event)`: a fatal error if another coroutine holds `event`.
    pub fn check_bound(self, event: u32) {
        check_bound(self, event);
    }

    /// `has_bound(event)`.
    #[must_use]
    pub fn has_bound(self, event: u32) -> bool {
        !self.with_state(|state| state.bound_co(event)).is_null()
    }

    /// `is_connected()`.
    #[must_use]
    pub fn is_connected(self) -> bool {
        !self.is_closed() && self.with_state(|state| state.flags.connected)
    }
}

/// `TimeoutController::has_timedout(kind)` at `now`: the first call starts the
/// clock; later ones set the time left as `kind`'s timeout, or time out
/// (`ETIMEDOUT`) with less than `SW_TIMER_MIN_SEC` left. No limit unless positive.
pub fn timed_out(socket: Co, timeout: f64, startup: &mut f64, now: f64, kind: c_int) -> bool {
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
        set_err(socket, libc::ETIMEDOUT);
        return true;
    }
    socket.set_timeout(left, kind);
    false
}

/// `catch_read_error`/`catch_write_error(errno) == SW_WAIT`.
pub(crate) fn would_block(socket: Co) -> bool {
    socket.catch_error(errno()) == WAIT
}

/// A timer slot's value while a negative timeout's wait runs without a timer.
const SENTINEL: *mut c_void = usize::MAX as *mut c_void;

/// `TimerController`: one wait's timer in the socket's slot for its direction.
pub struct Timer {
    socket: Co,
    timeout: f64,
    enabled: bool,
    write: bool,
}

impl Timer {
    /// A timer for `event`'s direction (`SW_EVENT_READ`, else writing).
    pub(crate) fn new(socket: Co, event: u32, timeout: f64) -> Self {
        Self {
            socket,
            timeout,
            enabled: false,
            write: event != READ,
        }
    }

    fn slot(&self) -> *mut c_void {
        let [read, write] = self.socket.with_state(|state| state.timers);
        if self.write { write } else { read }
    }

    fn set_slot(&self, node: *mut c_void) {
        self.socket.with_state(|state| {
            let [read, write] = &mut state.timers;
            *if self.write { write } else { read } = node;
        });
    }

    /// `start()`: unless the slot holds a timer already, one firing after
    /// `timeout` (false if none could be added), or the sentinel for a negative
    /// timeout. No timer for a zero timeout.
    pub(crate) fn start(&mut self) -> bool {
        if self.timeout == 0.0 || !self.slot().is_null() {
            return true;
        }
        self.enabled = true;
        if self.timeout > 0.0 {
            let node = (host().timer_add)(self.socket, self.timeout, self.write);
            self.set_slot(node);
            return !node.is_null();
        }
        self.set_slot(SENTINEL);
        true
    }

    /// The destructor: delete the timer this one started and clear the slot.
    pub(crate) fn stop(&mut self) {
        let node = self.slot();
        if !self.enabled || node.is_null() {
            return;
        }
        if node != SENTINEL {
            (host().timer_del)(node);
        }
        self.set_slot(ptr::null_mut());
    }
}

/// `Socket::timer_callback`: the timer for `event`'s direction fired. Its slot
/// clears, the socket's error becomes `ETIMEDOUT` and the coroutine waiting in that
/// direction resumes; an empty slot aborts, as upstream's unknown node does.
pub(crate) fn timer_fired(socket: Co, event: u32) {
    set_err(socket, libc::ETIMEDOUT);
    let co = socket.with_state(|state| {
        let [read, write] = &mut state.timers;
        let slot = if event == READ { read } else { write };
        if slot.is_null() {
            std::process::abort();
        }
        *slot = ptr::null_mut();
        state.co(event)
    });
    (NATIVE.resume)(co);
}

/// A recording host for the operations' tests. Each thread records its own events.
#[cfg(test)]
pub(crate) mod test_host {
    use super::*;
    use std::cell::{Cell, RefCell};

    type Suspend = Box<dyn FnMut(Co, u32)>;
    type OnResume = Box<dyn FnMut(c_long)>;
    type OnRelease = Box<dyn FnMut(Co)>;

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
        hook(&ON_RELEASE, |f| f(unsafe { Co::of(socket) }));
    }
    fn ssl_shutdown(_: Co) {
        log("ssl-shutdown");
        set_errno(SSL_ERRNO.get());
    }
    fn liveness(_: Co) -> bool {
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
    fn timer_add(_: Co, timeout: f64, _: bool) -> *mut c_void {
        log(format!("timer:add:{timeout}"));
        if !TIMER_OK.get() {
            return ptr::null_mut();
        }
        NODES.set(NODES.get() + 1);
        ptr::dangling_mut::<u32>().cast()
    }
    fn timer_del(_: *mut c_void) {
        log("timer:delete");
    }
    fn sleep() {
        log("sleep");
    }
    fn reactor(_: Co, events: u32, add: bool) -> bool {
        log(format!("{}:{events}", if add { "add" } else { "set" }));
        REACTOR_OK.get()
    }
    fn suspend(socket: Co, _: *mut c_void, event: u32) {
        log(format!("yield:{event}"));
        let hook = ON_SUSPEND.take();
        if let Some(mut hook) = hook {
            hook(socket, event);
            ON_SUSPEND.set(Some(hook));
        }
    }
    fn buffer(_: Co, _: *mut *mut u8, length: usize) {
        log(format!("buffer:{length}"));
    }
    fn remove(_: Co, event: u32) {
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
    use crate::shutdown::WRITE;

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
            let mut timer = Timer::new(socket.co(), READ, timeout);
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
            timer_fired(socket.co(), event);
            assert!(unsafe { (*socket.state()).timers[slot] }.is_null());
            assert_eq!(socket.co().err_code(), libc::ETIMEDOUT);
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
        let state = unsafe { &mut *socket.state() };
        state.bind(READ, co(bound & 1 != 0, reader));
        state.bind(WRITE, co(bound & 2 != 0, writer));
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
            let mut socket = TestSocket::new();
            let mut cells = ids;
            bind(&socket, bound, &mut cells);
            let (cid, name, effects) = upstream_bound(100, bound, ids, event);
            let state = unsafe { &*socket.state() };
            let co = state.bound_co(event);
            assert_eq!(if co.is_null() { 0 } else { (NATIVE.cid)(co) }, cid);
            let got =
                ["reading", "writing", "reading or writing"][state.event_name(event) as usize];
            assert_eq!(got, name, "scenario {k}");
            take();
            check_bound(socket.co(), event);
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
            let target = socket.co();
            if closes {
                ON_BOUND.set(Some(Box::new(move || target.set_sock_fd(-1))));
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
            let available = available(target, event);
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
            check(socket.co(), result);
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
                let got = crate::timeval(seconds as f64, micros as f64);
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
                let got = timed_out(socket.co(), timeout, &mut startup, now, 4);
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
        check(socket.co(), -1);
        assert_eq!(socket.0.err_code, libc::EINVAL);
        assert_eq!(
            unsafe { core::ffi::CStr::from_ptr(socket.0.err_msg) },
            c"native-error"
        );
        set_errno(libc::EAGAIN);
        check(socket.co(), -1);
        assert_eq!(socket.0.err_code, libc::EINVAL);
        check(socket.co(), 0);
        assert_eq!((socket.0.err_code, errno()), (0, 0));
        assert_eq!(unsafe { *socket.0.err_msg }, 0);
        assert_eq!(take(), ["last-error:22", "last-error:0"]);
    }

    /// `rust_socket_scopes.error_text_publication`, formerly the hooks system fixture's
    /// C++ witness over `Socket::set_err`'s three overloads.
    #[test]
    fn error_text_publication() {
        install();
        let mut socket = TestSocket::new();
        let co = socket.co();
        let message = |co: Co| unsafe { core::ffi::CStr::from_ptr(co.err_msg()) }.to_bytes();
        set_errno(libc::EINVAL);
        default_err(co, 0);
        assert_eq!((co.err_code(), errno()), (libc::EINVAL, libc::EINVAL));
        default_err(co, libc::EACCES);
        assert_eq!((co.err_code(), errno()), (libc::EACCES, libc::EINVAL));
        assert_eq!(take(), Vec::<String>::new());
        let borrowed = c"borrowed";
        borrowed_err(co, libc::EIO, Message::Static(borrowed));
        assert_eq!(co.err_msg(), borrowed.as_ptr());
        assert_eq!(errno(), libc::EIO);
        assert_eq!(take(), ["last-error:5"]);
        let (first, second) = ([b'a'; 100], [b'b'; 600]);
        assert_eq!(owned_err(co, libc::ERANGE, &first), 0);
        assert_eq!(message(co), first);
        assert_eq!(owned_err(co, libc::ENOSPC, &second), 0);
        assert_eq!((co.err_code(), errno()), (libc::ENOSPC, libc::ENOSPC));
        assert_eq!(message(co), second);
        assert_eq!(owned_err(co, 0, b"a\0b"), 0);
        assert_eq!(
            unsafe { core::slice::from_raw_parts(co.err_msg().cast::<u8>(), 3) },
            b"a\0b"
        );
    }
}

/// `set_err()`: the thread's last error (errno when there is none) as the record's
/// code and message, without changing errno or the last-error slot.
pub(crate) fn default_err(socket: Co, last_error: c_int) {
    let error = if last_error == 0 { errno() } else { last_error };
    socket.set_err_code(error);
    set_message(socket, error, Message::Strerror);
}

/// An error message the record borrows rather than owns.
#[derive(Clone, Copy)]
pub(crate) enum Message {
    Static(&'static CStr),
    /// `swoole_strerror(error)`.
    Strerror,
    /// The thread's last error message.
    #[cfg_attr(
        test,
        expect(dead_code, reason = "only the native host's packet failures use it")
    )]
    LastError,
}

fn set_message(socket: Co, error: c_int, message: Message) {
    let text = match message {
        Message::Static(text) => text.as_ptr(),
        Message::Strerror => (NATIVE.strerror)(error),
        Message::LastError => swoole_rs_error::swoole_get_last_error_msg(),
    };
    // SAFETY: static, swoole_strerror's or the thread's last error message.
    unsafe { socket.set_err_msg(text) };
}

/// `set_err(error, message)` with a message the record does not own.
pub(crate) fn borrowed_err(socket: Co, error: c_int, message: Message) {
    socket.set_err_code(error);
    set_errno(error);
    (NATIVE.set_last_error)(error);
    set_message(socket, error, message);
}

/// `set_err(error, message)` with owned text: the code, errno and last error, then
/// the message once the record's string holds a copy. A failed copy (allocation)
/// leaves the previous message and returns nonzero.
#[expect(
    clippy::must_use_candidate,
    reason = "only callers that report allocation failure read it"
)]
pub fn owned_err(socket: Co, error: c_int, message: &[u8]) -> c_int {
    socket.set_err_code(error);
    set_errno(error);
    (NATIVE.set_last_error)(error);
    // The metadata's first slot owns the error string.
    let Some(slot) = socket.with_extra(|extra| &raw const extra.strings[0]) else {
        return -1;
    };
    // SAFETY: the slot and the message's bytes; the native cache contains every
    // exception below Rust.
    let status = unsafe {
        swoole_rs_corebase::swr_v1_core_string_set(slot, message.as_ptr().cast(), message.len())
    };
    if status == 0 {
        // SAFETY: the assignment left stable, terminated bytes in the slot.
        let view = unsafe { swoole_rs_corebase::swr_v1_core_string_view(slot) };
        // SAFETY: the record's own error string, as above.
        unsafe { socket.set_err_msg(view.data) };
    }
    status
}

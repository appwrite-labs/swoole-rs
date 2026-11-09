//! The coroutine socket under symbols, for one caller: the hooks system tests
//! (tests/modules/hooks/system/src/socket.rs, upstream's core-tests socket.cpp),
//! which link the hybrid's libswoole.so and reach the socket by symbol. Each is
//! a thin call of the `Co` method it names; nothing in the build calls them.
//! They go once that package links the crates instead of the library.
#![expect(
    clippy::must_use_candidate,
    reason = "exports for C callers, which may ignore any result"
)]
use crate::co_socket::{Co, swr_v1_hook_co_socket_free};
use crate::socket_scope::TimeoutScope;
use core::ffi::{CStr, c_char, c_int, c_void};
use swoole_rs_corebase::CoreStringView;

/// The caller's `n` bytes at `buf`.
///
/// # Safety
/// `buf` holds `n` readable bytes, or `n` is 0.
unsafe fn bytes<'a>(buf: *const c_void, n: usize) -> &'a [u8] {
    if n == 0 {
        return &[];
    }
    // SAFETY: per the caller.
    unsafe { core::slice::from_raw_parts(buf.cast(), n) }
}

/// The caller's `n` writable bytes at `buf`.
///
/// # Safety
/// `buf` holds `n` writable bytes, or `n` is 0.
unsafe fn bytes_mut<'a>(buf: *mut c_void, n: usize) -> &'a mut [u8] {
    if n == 0 {
        return &mut [];
    }
    // SAFETY: per the caller.
    unsafe { core::slice::from_raw_parts_mut(buf.cast(), n) }
}

/// A view's bytes.
///
/// # Safety
/// The view lends `length` readable bytes.
unsafe fn view<'a>(text: CoreStringView) -> &'a [u8] {
    // SAFETY: per the caller.
    unsafe { bytes(text.data.cast(), text.length) }
}

/// Exports `$name` as `$symbol`: a member over `this` and slices of the
/// caller's buffers.
macro_rules! buffered {
    ($($symbol:literal fn $name:ident($buf:ident: *$q:ident c_void) = $method:ident($slice:ident);)*) => {$(
        /// # Safety
        /// `this` is a live socket and `buf` holds `n` bytes.
        #[unsafe(export_name = $symbol)]
        pub unsafe extern "C" fn $name(this: Co, $buf: *$q c_void, n: usize) -> isize {
            // SAFETY: per the caller.
            this.$method(unsafe { $slice($buf, n) })
        }
    )*};
}

buffered! {
    "_ZN6swoole9coroutine6Socket4recvEPvm" fn recv(buf: *mut c_void) = recv(bytes_mut);
    "_ZN6swoole9coroutine6Socket4sendEPKvm" fn send(buf: *const c_void) = send(bytes);
    "_ZN6swoole9coroutine6Socket8recv_allEPvm" fn recv_all(buf: *mut c_void) = recv_all(bytes_mut);
    "_ZN6swoole9coroutine6Socket8send_allEPKvm" fn send_all(buf: *const c_void) = send_all(bytes);
    "_ZN6swoole9coroutine6Socket4readEPvm" fn read(buf: *mut c_void) = read(bytes_mut);
    "_ZN6swoole9coroutine6Socket5writeEPKvm" fn write(buf: *const c_void) = write(bytes);
    "_ZN6swoole9coroutine6Socket4peekEPvm" fn peek(buf: *mut c_void) = peek(bytes_mut);
    "_ZN6swoole9coroutine6Socket8recvfromEPvm" fn recvfrom(buf: *mut c_void) = recvfrom_peer(bytes_mut);
}

/// Exports `$name` as `$symbol`: an unsafe member over one vector.
macro_rules! vectored {
    ($($symbol:literal fn $name:ident;)*) => {$(
        /// # Safety
        /// `this` is a live socket and `vector` a live `IOVector`.
        #[unsafe(export_name = $symbol)]
        pub unsafe extern "C" fn $name(this: Co, vector: *mut c_void) -> isize {
            // SAFETY: per the caller.
            unsafe { this.$name(vector) }
        }
    )*};
}

vectored! {
    "_ZN6swoole9coroutine6Socket5readvEPNS_7network8IOVectorE" fn readv;
    "_ZN6swoole9coroutine6Socket9readv_allEPNS_7network8IOVectorE" fn readv_all;
    "_ZN6swoole9coroutine6Socket6writevEPNS_7network8IOVectorE" fn writev;
    "_ZN6swoole9coroutine6Socket10writev_allEPNS_7network8IOVectorE" fn writev_all;
}

/// # Safety
/// `this` is a live socket; `msg` describes writable buffers.
#[unsafe(export_name = "_ZN6swoole9coroutine6Socket7recvmsgEP6msghdri")]
pub unsafe extern "C" fn recvmsg(this: Co, msg: *mut libc::msghdr, flags: c_int) -> isize {
    // SAFETY: per the caller.
    unsafe { this.recvmsg(msg, flags) }
}

/// # Safety
/// `this` is a live socket; `msg` describes readable buffers.
#[unsafe(export_name = "_ZN6swoole9coroutine6Socket7sendmsgEPK6msghdri")]
pub unsafe extern "C" fn sendmsg(this: Co, msg: *const libc::msghdr, flags: c_int) -> isize {
    // SAFETY: per the caller.
    unsafe { this.sendmsg(msg, flags) }
}

/// # Safety
/// `this` is a live socket; `buf` holds `n` writable bytes and one more.
#[unsafe(export_name = "_ZN6swoole9coroutine6Socket9recv_lineEPvm")]
pub unsafe extern "C" fn recv_line(this: Co, buf: *mut c_void, n: usize) -> isize {
    // SAFETY: per the caller.
    unsafe { this.recv_line(buf.cast(), n) }
}

/// # Safety
/// `this` is a live socket; `addr` holds `*length` writable bytes.
#[unsafe(export_name = "_ZN6swoole9coroutine6Socket8recvfromEPvmP8sockaddrPj")]
pub unsafe extern "C" fn recvfrom_addr(
    this: Co,
    buf: *mut c_void,
    n: usize,
    addr: *mut libc::sockaddr,
    length: *mut libc::socklen_t,
) -> isize {
    // SAFETY: per the caller.
    unsafe { this.recvfrom(buf, n, addr, length) }
}

/// # Safety
/// `this` is a live socket and `host` NUL-terminated.
#[unsafe(export_name = "_ZN6swoole9coroutine6Socket4bindEPKci")]
pub unsafe extern "C" fn bind(this: Co, host: *const c_char, port: c_int) -> bool {
    // SAFETY: per the caller.
    this.bind(unsafe { CStr::from_ptr(host) }.to_bytes(), port)
}

/// # Safety
/// `this` is a live socket, `host` NUL-terminated and `buf` holds `n` bytes.
#[unsafe(export_name = "_ZN6swoole9coroutine6Socket6sendtoEPKciPKvm")]
pub unsafe extern "C" fn sendto(
    this: Co,
    host: *const c_char,
    port: c_int,
    buf: *const c_void,
    n: usize,
) -> isize {
    // SAFETY: per the caller.
    let host = unsafe { CStr::from_ptr(host) };
    // SAFETY: per the caller.
    this.sendto(host.to_bytes(), port, unsafe { bytes(buf, n) })
}

/// # Safety
/// `this` is a live socket and `file` NUL-terminated.
#[unsafe(export_name = "_ZN6swoole9coroutine6Socket8sendfileEPKclm")]
pub unsafe extern "C" fn sendfile(
    this: Co,
    file: *const c_char,
    offset: i64,
    length: usize,
) -> bool {
    // SAFETY: per the caller.
    this.sendfile(unsafe { CStr::from_ptr(file) }, offset, length)
}

/// # Safety
/// `this` is a live socket and `value` a writable int.
#[unsafe(export_name = "_ZNK6swoole9coroutine6Socket10get_optionEiiPi")]
pub unsafe extern "C" fn get_option(
    this: Co,
    level: c_int,
    name: c_int,
    value: *mut c_int,
) -> bool {
    // SAFETY: per the caller.
    this.get_int_option(level, name, unsafe { &mut *value })
}

#[unsafe(export_name = "_ZN6swoole9coroutine6Socket6listenEi")]
pub extern "C" fn listen(this: Co, backlog: c_int) -> bool {
    this.listen(backlog)
}

#[unsafe(export_name = "_ZN6swoole9coroutine6Socket6acceptEd")]
pub extern "C" fn accept(this: Co, timeout: f64) -> *mut c_void {
    this.accept(timeout).map_or(core::ptr::null_mut(), Co::this)
}

#[unsafe(export_name = "_ZN6swoole9coroutine6Socket11recv_packetEd")]
pub extern "C" fn recv_packet(this: Co, timeout: f64) -> isize {
    crate::packet::recv_packet(this, timeout)
}

#[unsafe(export_name = "_ZNK6swoole9coroutine6Socket11getsocknameEv")]
pub extern "C" fn getsockname(this: Co) -> bool {
    this.getsockname()
}

#[unsafe(export_name = "_ZNK6swoole9coroutine6Socket11set_timeoutEdi")]
pub extern "C" fn set_timeout(this: Co, timeout: f64, kind: c_int) {
    this.set_timeout(timeout, kind);
}

#[unsafe(export_name = "_ZNK6swoole9coroutine6Socket11get_timeoutE13swTimeoutType")]
pub extern "C" fn get_timeout(this: Co, kind: c_int) -> f64 {
    this.timeout(kind)
}

#[unsafe(export_name = "_ZNK6swoole9coroutine6Socket10set_optionEiii")]
pub extern "C" fn set_option(this: Co, level: c_int, name: c_int, value: c_int) -> bool {
    this.set_int_option(level, name, value)
}

#[unsafe(export_name = "_ZN6swoole9coroutine6Socket13ssl_handshakeEv")]
pub extern "C" fn ssl_handshake(this: Co) -> bool {
    this.ssl_handshake()
}

#[unsafe(export_name = "_ZN6swoole9coroutine6Socket10ssl_verifyEb")]
pub extern "C" fn ssl_verify(this: Co, allow_self_signed: bool) -> bool {
    this.ssl_verify(allow_self_signed)
}

#[unsafe(export_name = "swr_v1_hook_co_socket_make")]
pub extern "C" fn make(fd: c_int, domain: c_int, kind: c_int, protocol: c_int, family: bool) -> Co {
    crate::co_socket::make(fd, domain, kind, protocol, family)
}

/// # Safety
/// `socket` is live and owned; it is gone after the call.
#[unsafe(export_name = "swr_v1_hook_co_socket_free")]
pub unsafe extern "C" fn free(socket: *mut c_void) {
    // SAFETY: per the caller.
    unsafe { swr_v1_hook_co_socket_free(socket) };
}

#[unsafe(export_name = "swr_v1_hook_co_socket_close")]
pub extern "C" fn close(this: Co) -> bool {
    this.close()
}

#[unsafe(export_name = "swr_v1_hook_co_socket_shutdown")]
pub extern "C" fn shutdown(this: Co, how: c_int) -> bool {
    this.shutdown(how)
}

#[unsafe(export_name = "swr_v1_hook_co_socket_cancel")]
pub extern "C" fn cancel(this: Co, event: u32) -> bool {
    this.cancel(event)
}

#[unsafe(export_name = "swr_v1_hook_co_socket_check_liveness")]
pub extern "C" fn check_liveness(this: Co) -> bool {
    this.check_liveness()
}

#[unsafe(export_name = "swr_v1_hook_co_socket_event_str")]
pub extern "C" fn event_str(this: Co, event: u32) -> *const c_char {
    crate::co_io::event_str(this, event).as_ptr()
}

/// # Safety
/// `this` is a live socket and `bytes` holds `length` bytes.
#[unsafe(export_name = "swr_v1_hook_co_socket_owned_err")]
pub unsafe extern "C" fn owned_err(
    this: Co,
    error: c_int,
    text: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: per the caller.
    this.set_error(error, unsafe { bytes(text.cast(), length) })
}

/// # Safety
/// `this` is a live socket and each view lends its bytes.
#[unsafe(export_name = "swr_v1_hook_co_socket_proxy")]
pub unsafe extern "C" fn proxy(
    this: Co,
    http: bool,
    host: CoreStringView,
    port: c_int,
    user: CoreStringView,
    password: CoreStringView,
) {
    // SAFETY: per the caller.
    let (host, user) = (unsafe { view(host) }, unsafe { view(user) });
    // SAFETY: per the caller.
    this.set_proxy(http, host, port, user, unsafe { view(password) });
}

/// `TimeoutSetter(socket, timeout, kind)`; [`timeout_restore`] ends it.
#[unsafe(export_name = "swr_v1_hook_timeout_scope")]
pub extern "C" fn timeout_scope(this: Co, timeout: f64, kind: c_int) -> TimeoutScope {
    TimeoutScope::new(this, timeout, kind)
}

/// `~TimeoutSetter`.
///
/// # Safety
/// `scope` is one [`timeout_scope`] made, ended once.
#[unsafe(export_name = "swr_v1_hook_timeout_restore")]
pub unsafe extern "C" fn timeout_restore(scope: *const TimeoutScope) {
    // SAFETY: per the caller; dropping the scope restores the socket.
    drop(unsafe { core::ptr::read(scope) });
}

/// `TimeoutController::has_timedout(kind)` with the controller's clock at `startup`.
///
/// # Safety
/// `this` is a live socket and `startup` a writable clock.
#[unsafe(export_name = "swr_v1_hook_co_socket_timed_out")]
pub unsafe extern "C" fn timed_out(
    this: Co,
    timeout: f64,
    startup: *mut f64,
    now: f64,
    kind: c_int,
) -> bool {
    // SAFETY: per the caller.
    crate::co_io::timed_out(this, timeout, unsafe { &mut *startup }, now, kind)
}

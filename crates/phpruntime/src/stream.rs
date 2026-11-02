//! Swoole's coroutine socket streams (`HOOK_TCP` and friends): the
//! "socket/coroutine" operations, the transport API (connect, bind, listen,
//! accept, names, send/recv, shutdown), crypto and the SSL context options,
//! the factory's coroutine branch, `php_swoole_create_stream_from_socket` and
//! `stream_socket_pair`. The operations tables stay C++
//! (`swoole_runtime_socket_ops`); the socket is reached through `socket`.
//!
//! An operation that can run PHP code (a warning's error handler, a progress
//! notifier, `openssl_x509_read`) traps a bailout and reports it through its
//! `bailout` flag; the C++ entry re-raises it once these frames are gone.
use crate::socket::{Io, Report, SW_WAIT, Socket, check_reactor, kind, report, timeout, x509_read};
use libc::{c_char, c_int, c_long, c_void, sockaddr_storage, socklen_t};
use std::{
    ffi::{CStr, CString},
    mem::size_of,
    ptr,
    rc::Rc,
};
use swoole_rs_hooks::{address::swr_v1_hooks_parse_address, swr_v1_hook_timeval};
use swoole_rs_zend::{
    Call, E_WARNING, Message,
    bailout::Bailout,
    message, string,
    sys::{
        IS_ARRAY, IS_NULL, add_assoc_bool_ex, add_assoc_long_ex, add_assoc_string_ex,
        add_assoc_zval_ex, add_next_index_resource, php_stream, php_stream_context, php_stream_ops,
        php_stream_statbuf, php_stream_xport_crypto_param, php_stream_xport_param, sockaddr,
        swr_zend_array_init, swr_zend_efree, swr_zend_is_true, swr_zend_stream_alloc,
        swr_zend_stream_avoid_blocking, swr_zend_stream_buffered, swr_zend_stream_close,
        swr_zend_stream_context, swr_zend_stream_context_options, swr_zend_stream_crypto_op,
        swr_zend_stream_default_timeout, swr_zend_stream_empty_string, swr_zend_stream_eof,
        swr_zend_stream_expose, swr_zend_stream_option, swr_zend_stream_progress,
        swr_zend_stream_set_eof, swr_zend_stream_set_option, swr_zend_stream_share_context,
        swr_zend_stream_vm_active, swr_zend_stream_xport_flags, swr_zend_stream_xport_op,
        swr_zval_try_addref, timeval, zend_array, zend_hash_next_index_insert, zend_hash_str_find,
        zend_string, zval, zval_ptr_dtor,
    },
    type_of,
    value::table,
};

unsafe extern "C" {
    /// The "socket/coroutine" operations table (ext-src/swoole_runtime.cc).
    static swoole_runtime_socket_ops: php_stream_ops;
    fn php_network_populate_name_from_sockaddr(
        sa: *mut sockaddr,
        sl: socklen_t,
        textaddr: *mut *mut zend_string,
        addr: *mut *mut sockaddr,
        addrlen: *mut socklen_t,
    );
    fn php_network_get_sock_name(
        sock: c_int,
        textaddr: *mut *mut zend_string,
        addr: *mut *mut sockaddr,
        addrlen: *mut socklen_t,
    ) -> c_int;
    fn php_network_get_peer_name(
        sock: c_int,
        textaddr: *mut *mut zend_string,
        addr: *mut *mut sockaddr,
        addrlen: *mut socklen_t,
    ) -> c_int;
    fn php_socket_error_str(err: c_long) -> *mut zend_string;
    fn php_socket_strerror(err: c_long, buf: *mut c_char, bufsize: usize) -> *mut c_char;
}

const SUCCESS: c_int = 0;
const FAILURE: c_int = -1;

/// `PHP_STREAM_OPTION_*`.
mod option {
    pub(super) const BLOCKING: i32 = 1;
    pub(super) const READ_TIMEOUT: i32 = 4;
    pub(super) const XPORT_API: i32 = 7;
    pub(super) const CRYPTO_API: i32 = 8;
    pub(super) const META_DATA_API: i32 = 11;
    pub(super) const CHECK_LIVENESS: i32 = 12;
    pub(super) const RETURN_OK: i32 = 0;
    pub(super) const RETURN_ERR: i32 = -1;
}

/// `STREAM_XPORT_OP_*`.
mod xport {
    pub(super) const BIND: i32 = 0;
    pub(super) const CONNECT: i32 = 1;
    pub(super) const LISTEN: i32 = 2;
    pub(super) const ACCEPT: i32 = 3;
    pub(super) const CONNECT_ASYNC: i32 = 4;
    pub(super) const GET_NAME: i32 = 5;
    pub(super) const GET_PEER_NAME: i32 = 6;
    pub(super) const RECV: i32 = 7;
    pub(super) const SEND: i32 = 8;
    pub(super) const SHUTDOWN: i32 = 9;
}

/// `STREAM_OOB`, `STREAM_PEEK`.
const STREAM_OOB: i32 = 1;
const STREAM_PEEK: i32 = 2;

/// `PHP_STREAM_AS_*`.
const AS_STDIO: c_int = 0;
const AS_FD: c_int = 1;
const AS_SOCKETD: c_int = 2;
const AS_FD_FOR_SELECT: c_int = 3;

/// `E_NOTICE`.
#[expect(clippy::cast_possible_wrap, reason = "error levels are small")]
const E_NOTICE: i32 = swoole_rs_zend::sys::E_NOTICE as i32;

/// A stream's `abstract`: the socket, shared with an operation in flight so a
/// close while it waits only drops this reference.
struct NetStream {
    socket: Rc<Socket>,
    blocking: bool,
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Report a trapped bailout through `flag`; `failed` stands in for the result.
pub(crate) fn reraise<T>(flag: *mut u8, result: Result<T, Bailout>, failed: T) -> T {
    result.unwrap_or_else(|Bailout| {
        // SAFETY: the C++ entry lends its flag.
        unsafe { *flag = 1 };
        failed
    })
}

/// The stream's `NetStream`, null once closed.
fn net(stream: *mut php_stream) -> *mut NetStream {
    // SAFETY: a live stream of these operations.
    unsafe { (*stream).abstract_ }.cast()
}

/// `php_stream_alloc_rel(ops, abstract, persistent_id, "r+")` of `socket`;
/// null (the socket freed) if it fails.
fn alloc(
    ops: *const php_stream_ops,
    socket: Socket,
    persistent_id: *const c_char,
) -> *mut php_stream {
    let net = Box::into_raw(Box::new(NetStream {
        socket: Rc::new(socket),
        blocking: true,
    }));
    // SAFETY: Zend's allocator; the stream owns `net` from here.
    let stream = unsafe { swr_zend_stream_alloc(ops, net.cast(), persistent_id) };
    if stream.is_null() {
        // SAFETY: not handed over.
        drop(unsafe { Box::from_raw(net) });
    }
    stream
}

fn socket_ops() -> *const php_stream_ops {
    &raw const swoole_runtime_socket_ops
}

/// `php_stream_notify_progress_increment` of `bytes`.
fn progress(stream: *mut php_stream, bytes: isize) -> Result<(), Bailout> {
    // SAFETY: a live stream; the notifier runs inside the shim's zend_try.
    if unsafe { swr_zend_stream_progress(stream, bytes.unsigned_abs()) } != 0 {
        return Err(Bailout);
    }
    Ok(())
}

fn set_eof(stream: *mut php_stream) {
    // SAFETY: a live stream.
    unsafe { swr_zend_stream_set_eof(stream) };
}

/// The seconds of a `timeval`, as Swoole's `set_timeout(timeval *)`.
pub(crate) fn seconds(time: *const timeval) -> f64 {
    // SAFETY: Zend lends a live timeval.
    let time = unsafe { *time };
    #[expect(clippy::cast_precision_loss, reason = "upstream's (double) casts")]
    #[expect(clippy::cast_lossless, reason = "tv_usec's width differs per platform")]
    swr_v1_hook_timeval(time.tv_sec as f64, time.tv_usec as f64)
}

/// A context option of the stream, if set.
fn context_option(stream: *mut php_stream, wrapper: &CStr, name: &CStr) -> Option<*mut zval> {
    // SAFETY: a live stream; Zend copies nothing.
    let value = unsafe { swr_zend_stream_option(stream, wrapper.as_ptr(), name.as_ptr()) };
    (!value.is_null()).then_some(value)
}

/// `zval_is_true` of a context option, false when unset.
fn option_true(stream: *mut php_stream, wrapper: &CStr, name: &CStr) -> bool {
    // SAFETY: a live option value.
    context_option(stream, wrapper, name).is_some_and(|value| unsafe { swr_zend_is_true(value) })
}

/// `php_swoole_array_get_value_ex`: the entry, unless missing or null.
fn find(array: *mut zend_array, key: &str) -> Option<*mut zval> {
    // SAFETY: a live array; the key is read for the call.
    let value = unsafe { zend_hash_str_find(array, key.as_ptr().cast(), key.len()) };
    // SAFETY: a live entry.
    (!value.is_null() && unsafe { type_of(value) } != IS_NULL).then_some(value)
}

/// Bytes up to the first NUL, as C reads a string.
fn c_str(bytes: &[u8]) -> &[u8] {
    bytes.split(|&b| b == 0).next().unwrap_or_default()
}

/// `parse_ip_address_ex`: host and port of `name`, else the error text when
/// `error` wants it.
fn parse_address(name: &[u8], error: Option<*mut *mut zend_string>) -> Option<(CString, i32)> {
    // SAFETY: `name` lends its bytes for the call.
    let plan = unsafe { swr_v1_hooks_parse_address(name.as_ptr(), name.len()) };
    if plan.status != 0 {
        if let Some(error) = error {
            let what = if plan.status == 1 {
                "IPv6 address"
            } else {
                "address"
            };
            let text = format!(
                "Failed to parse {what} \"{}\"",
                String::from_utf8_lossy(c_str(name))
            );
            // SAFETY: the caller's output slot.
            unsafe { *error = string::copy(text.as_bytes()) };
        }
        return None;
    }
    let host = c_str(&name[plan.host_offset..plan.host_offset + plan.host_length]);
    Some((CString::new(host).unwrap_or_default(), plan.port))
}

/// `socket_write`.
fn write(stream: *mut php_stream, buf: *const c_char, count: usize) -> Result<isize, Bailout> {
    let net = net(stream);
    if net.is_null() {
        return Ok(-1);
    }
    // SAFETY: an open stream's NetStream.
    let net = unsafe { &*net };
    let (socket, blocking) = (Rc::clone(&net.socket), net.blocking);
    let buf = buf.cast_mut();
    // SAFETY: Zend lends `count` bytes at `buf` for every branch below.
    if !unsafe { swr_zend_stream_vm_active() } {
        // SAFETY: as above.
        return Ok(unsafe { socket.io(Io::SendSync, buf, count) });
    }
    let mut written = if blocking {
        // SAFETY: as above.
        unsafe { socket.io(Io::SendAll, buf, count) }
    } else {
        // SAFETY: as above.
        let written = unsafe { socket.io(Io::SendOnce, buf, count) };
        socket.set_error(errno());
        written
    };
    if written < 0 || written.unsigned_abs() != count {
        // the caller does not loop on send, and a partial count is meaningless
        written = -1;
        let code = socket.error();
        if code != libc::ETIMEDOUT {
            if socket.catch_error(true, code) == SW_WAIT {
                written = 0;
            } else {
                let reason = socket
                    .message()
                    .map(CStr::to_string_lossy)
                    .unwrap_or_default();
                let message = message!("Send of {count} bytes failed with errno={code} {reason}");
                report(Report::Docref, E_NOTICE, message.as_cstr())?;
                set_eof(stream);
            }
        }
    } else {
        progress(stream, written)?;
        if written == 0 {
            set_eof(stream);
        }
    }
    Ok(written)
}

/// `socket_read`.
fn read(stream: *mut php_stream, buf: *mut c_char, count: usize) -> Result<isize, Bailout> {
    let net = net(stream);
    if net.is_null() {
        return Ok(-1);
    }
    // SAFETY: an open stream's NetStream.
    let net = unsafe { &*net };
    let (socket, blocking) = (Rc::clone(&net.socket), net.blocking);
    // SAFETY: Zend lends `count` writable bytes at `buf` for every branch.
    if !unsafe { swr_zend_stream_vm_active() } {
        // SAFETY: as above.
        return Ok(unsafe { socket.io(Io::RecvSync, buf, count) });
    }
    // SAFETY: a live stream.
    let mut read = if blocking && !unsafe { swr_zend_stream_buffered(stream) } {
        // SAFETY: as above.
        unsafe { socket.io(Io::Recv, buf, count) }
    } else {
        // SAFETY: as above.
        let read = unsafe { socket.io(Io::RecvOnce, buf, count) };
        socket.set_error(errno());
        read
    };
    if read > 0 {
        progress(stream, read)?;
    }
    if read < 0 && socket.error() != libc::ETIMEDOUT {
        if socket.catch_error(false, socket.error()) == SW_WAIT {
            read = 0;
        } else {
            set_eof(stream);
        }
    } else if read == 0 {
        set_eof(stream);
    }
    Ok(read)
}

/// The stream's `write`: bytes written, or -1.
/// # Safety
/// Called by Zend with a stream of these operations and `count` bytes at `buf`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpruntime_socket_write(
    stream: *mut php_stream,
    buf: *const c_char,
    count: usize,
    bailout: *mut u8,
) -> isize {
    reraise(bailout, write(stream, buf, count), -1)
}

/// The stream's `read`: bytes read, or -1.
/// # Safety
/// Called by Zend with a stream of these operations and `count` writable
/// bytes at `buf`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpruntime_socket_read(
    stream: *mut php_stream,
    buf: *mut c_char,
    count: usize,
    bailout: *mut u8,
) -> isize {
    reraise(bailout, read(stream, buf, count), -1)
}

/// The stream's `close`: the socket closes now and is freed once no
/// operation holds it.
/// # Safety
/// Called by Zend with a stream of these operations.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpruntime_socket_close(
    stream: *mut php_stream,
    _close_handle: c_int,
) -> c_int {
    let net = net(stream);
    if net.is_null() {
        return FAILURE;
    }
    // SAFETY: per the caller; cleared at once so nothing reaches it again.
    unsafe { (*stream).abstract_ = ptr::null_mut() };
    // SAFETY: the stream owned it.
    let net = unsafe { Box::from_raw(net) };
    net.socket.close();
    SUCCESS
}

/// The stream's `flush`: nothing is buffered.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpruntime_socket_flush(_stream: *mut php_stream) -> c_int {
    0
}

/// The stream's `cast`: the fd, or a `FILE *` over it.
/// # Safety
/// Called by Zend with a stream of these operations; `ret` is null or holds
/// the requested type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpruntime_socket_cast(
    stream: *mut php_stream,
    castas: c_int,
    ret: *mut *mut c_void,
) -> c_int {
    let net = net(stream);
    if net.is_null() {
        return FAILURE;
    }
    // SAFETY: an open stream's NetStream.
    let fd = unsafe { (*net).socket.fd() };
    match castas {
        AS_STDIO => {
            if ret.is_null() {
                return SUCCESS;
            }
            // SAFETY: per the caller.
            let mode = unsafe { (*stream).mode.as_ptr() };
            // SAFETY: `mode` is NUL-terminated.
            let file = unsafe { libc::fdopen(fd, mode) };
            // SAFETY: `ret` holds a `FILE *`.
            unsafe { *ret = file.cast() };
            if file.is_null() { FAILURE } else { SUCCESS }
        }
        AS_FD_FOR_SELECT | AS_FD | AS_SOCKETD => {
            if !ret.is_null() {
                // SAFETY: `ret` holds a `php_socket_t`.
                unsafe { *ret.cast::<c_int>() = fd };
            }
            SUCCESS
        }
        _ => FAILURE,
    }
}

/// The stream's `stat`: `fstat` of the fd.
/// # Safety
/// Called by Zend with a stream of these operations and a writable `ssb`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpruntime_socket_stat(
    stream: *mut php_stream,
    ssb: *mut php_stream_statbuf,
) -> c_int {
    let net = net(stream);
    if net.is_null() {
        return FAILURE;
    }
    // SAFETY: an open stream's NetStream.
    let fd = unsafe { (*net).socket.fd() };
    // SAFETY: per the caller.
    let sb = unsafe { &raw mut (*ssb).sb };
    // SAFETY: `sb` is a writable `struct stat`.
    unsafe { libc::fstat(fd, sb.cast()) }
}

/// The stream's `set_option`.
/// # Safety
/// Called by Zend with a stream of these operations and the option's
/// parameter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpruntime_socket_set_option(
    stream: *mut php_stream,
    option: c_int,
    value: c_int,
    ptrparam: *mut c_void,
    bailout: *mut u8,
) -> c_int {
    reraise(
        bailout,
        set_option(stream, option, value, ptrparam),
        option::RETURN_ERR,
    )
}

fn set_option(
    stream: *mut php_stream,
    option: c_int,
    value: c_int,
    ptrparam: *mut c_void,
) -> Result<c_int, Bailout> {
    let net = net(stream);
    if net.is_null() {
        return Ok(option::RETURN_ERR);
    }
    // SAFETY: an open stream's NetStream.
    let socket = unsafe { Rc::clone(&(*net).socket) };
    match option {
        // SAFETY: as above.
        option::BLOCKING => unsafe { (*net).blocking = value != 0 },
        option::XPORT_API => xport_api(stream, &socket, ptrparam.cast())?,
        option::META_DATA_API => meta_data(stream, &socket, ptrparam.cast()),
        // PHP socket streams share this timeout between reads and writes
        option::READ_TIMEOUT => socket.set_timeout(seconds(ptrparam.cast()), timeout::RDWR),
        option::CRYPTO_API => {
            let param = ptrparam.cast::<php_stream_xport_crypto_param>();
            // SAFETY: Zend lends the crypto parameter.
            match unsafe { swr_zend_stream_crypto_op(param) } {
                // SAFETY: as above.
                0 => unsafe { (*param).outputs.returncode = 0 },
                1 => {
                    // SAFETY: as above.
                    let activate = unsafe { (*param).inputs.activate } != 0;
                    let code = enable_crypto(stream, &socket, activate)?;
                    // SAFETY: as above.
                    unsafe { (*param).outputs.returncode = code };
                }
                _ => {}
            }
        }
        option::CHECK_LIVENESS => {
            return Ok(if socket.check_liveness() {
                option::RETURN_OK
            } else {
                option::RETURN_ERR
            });
        }
        _ => {}
    }
    Ok(option::RETURN_OK)
}

/// `PHP_STREAM_OPTION_META_DATA_API`: the TLS session, `timed_out`, `eof`
/// and `blocked`.
fn meta_data(stream: *mut php_stream, socket: &Socket, data: *mut zval) {
    let ssl = socket.ssl();
    if !ssl.is_null() {
        // SAFETY: an undefined zval, initialised as an array.
        let mut crypto: zval = unsafe { std::mem::zeroed() };
        let array = &raw mut crypto;
        // SAFETY: as above.
        unsafe { swr_zend_array_init(array) };
        // SAFETY: a live connection.
        let protocol: &CStr = match unsafe { openssl_sys::SSL_version(ssl) } {
            0x304 => c"TLSv1.3",
            0x303 => c"TLSv1.2",
            0x302 => c"TLSv1.1",
            0x301 => c"TLSv1",
            0x300 => c"SSLv3",
            _ => c"UNKNOWN",
        };
        // SAFETY: a live connection; OpenSSL takes a null cipher below.
        let cipher = unsafe { openssl_sys::SSL_get_current_cipher(ssl) };
        add_string(array, c"protocol", protocol.as_ptr());
        // SAFETY: as above.
        add_string(array, c"cipher_name", unsafe {
            openssl_sys::SSL_CIPHER_get_name(cipher)
        });
        // SAFETY: as above.
        let bits = unsafe { openssl_sys::SSL_CIPHER_get_bits(cipher, ptr::null_mut()) };
        add_long(array, c"cipher_bits", bits.into());
        // SAFETY: as above.
        add_string(array, c"cipher_version", unsafe {
            openssl_sys::SSL_CIPHER_get_version(cipher)
        });
        // SAFETY: Zend lends the metadata array, which takes `crypto`.
        unsafe { add_assoc_zval_ex(data, c"crypto".as_ptr(), 6, array) };
    }
    add_bool(data, c"timed_out", socket.error() == libc::ETIMEDOUT);
    // SAFETY: a live stream.
    add_bool(data, c"eof", unsafe { swr_zend_stream_eof(stream) });
    add_bool(data, c"blocked", true);
}

/// `add_assoc_string(array, key, value)` of a live array and a C string.
fn add_string(array: *mut zval, key: &CStr, value: *const c_char) {
    // SAFETY: the callers pass a live array and a NUL-terminated value.
    unsafe { add_assoc_string_ex(array, key.as_ptr(), key.count_bytes(), value) };
}

/// `add_assoc_long(array, key, value)` of a live array.
fn add_long(array: *mut zval, key: &CStr, value: i64) {
    // SAFETY: the callers pass a live array.
    unsafe { add_assoc_long_ex(array, key.as_ptr(), key.count_bytes(), value) };
}

/// `add_assoc_bool(array, key, value)` of a live array.
fn add_bool(array: *mut zval, key: &CStr, value: bool) {
    // SAFETY: the callers pass a live array.
    unsafe { add_assoc_bool_ex(array, key.as_ptr(), key.count_bytes(), value) };
}

/// `PHP_STREAM_OPTION_XPORT_API`.
fn xport_api(
    stream: *mut php_stream,
    socket: &Socket,
    param: *mut php_stream_xport_param,
) -> Result<(), Bailout> {
    // SAFETY: Zend lends the transport parameter for the call.
    let op = unsafe { swr_zend_stream_xport_op(param) };
    // SAFETY: as above.
    let flags = unsafe { swr_zend_stream_xport_flags(param) };
    // SAFETY: as above.
    let p = unsafe { &mut *param };
    let want_addr = flags & 1 != 0;
    let want_textaddr = flags & 2 != 0;
    let want_errortext = flags & 4 != 0;
    let textaddr = if want_textaddr {
        &raw mut p.outputs.textaddr
    } else {
        ptr::null_mut()
    };
    let (addr, addrlen) = if want_addr {
        (&raw mut p.outputs.addr, &raw mut p.outputs.addrlen)
    } else {
        (ptr::null_mut(), ptr::null_mut())
    };
    let error_text = want_errortext.then_some(&raw mut p.outputs.error_text);
    match op {
        xport::LISTEN => {
            p.outputs.returncode = if socket.listen(p.inputs.backlog) {
                0
            } else {
                -1
            }
        }
        xport::CONNECT | xport::CONNECT_ASYNC => {
            let mut code = connect(stream, socket, p, error_text);
            if code == 0 && socket.ssl_enabled() && enable_crypto(stream, socket, true)? < 0 {
                code = -1;
            }
            p.outputs.returncode = code;
        }
        xport::BIND => {
            if socket.domain() != libc::AF_UNIX {
                // SAFETY: a live stream.
                if unsafe { swr_zend_stream_context(stream) }.is_null() {
                    return Ok(());
                }
                socket.set_option(libc::SOL_SOCKET, libc::SO_REUSEADDR, 1, false);
                if option_true(stream, c"socket", c"ipv6_v6only") {
                    socket.set_option(libc::IPPROTO_IPV6, libc::IPV6_V6ONLY, 1, false);
                }
                if option_true(stream, c"socket", c"so_reuseport") {
                    socket.set_option(libc::SOL_SOCKET, libc::SO_REUSEPORT, 1, false);
                }
                if option_true(stream, c"socket", c"so_broadcast") {
                    socket.set_option(libc::SOL_SOCKET, libc::SO_BROADCAST, 1, true);
                }
            }
            p.outputs.returncode = bind(socket, p, error_text);
        }
        xport::ACCEPT => {
            p.outputs.returncode = accept(stream, socket, p, textaddr, addr, addrlen, error_text);
        }
        xport::GET_NAME => {
            // SAFETY: the output slots Zend asked for.
            p.outputs.returncode =
                unsafe { php_network_get_sock_name(socket.fd(), textaddr, addr, addrlen) };
        }
        xport::GET_PEER_NAME => {
            // SAFETY: as above.
            p.outputs.returncode =
                unsafe { php_network_get_peer_name(socket.fd(), textaddr, addr, addrlen) };
        }
        xport::SEND => {
            if p.inputs.flags & STREAM_OOB == STREAM_OOB {
                return oob(p);
            }
            let sent = if p.inputs.addr.is_null() {
                // SAFETY: Zend lends `buflen` bytes at `buf`.
                unsafe { socket.io(Io::Send, p.inputs.buf, p.inputs.buflen) }
            } else {
                // SAFETY: as above, and `addrlen` bytes at `addr`.
                unsafe {
                    libc::sendto(
                        socket.fd(),
                        p.inputs.buf.cast(),
                        p.inputs.buflen,
                        0,
                        p.inputs.addr.cast(),
                        p.inputs.addrlen,
                    )
                }
            };
            p.outputs.returncode = returncode(sent);
            if p.outputs.returncode == -1 {
                // SAFETY: Zend allocates the message.
                let error = unsafe { php_socket_strerror(errno().into(), ptr::null_mut(), 0) };
                let mut message = Message::new();
                // SAFETY: a NUL-terminated message.
                message.push(unsafe { CStr::from_ptr(error) }.to_bytes());
                message.push(b"\n");
                // SAFETY: copied; freed once.
                unsafe { swr_zend_efree(error.cast()) };
                report(Report::Docref, E_WARNING, message.as_cstr())?;
            }
        }
        xport::RECV => {
            if p.inputs.flags & STREAM_OOB == STREAM_OOB {
                return oob(p);
            }
            p.outputs.returncode = if p.inputs.flags & STREAM_PEEK == STREAM_PEEK {
                // SAFETY: Zend lends `buflen` writable bytes at `buf`.
                returncode(unsafe { socket.io(Io::Peek, p.inputs.buf, p.inputs.buflen) })
            } else {
                recvfrom(socket, p, textaddr, addr, addrlen)
            };
        }
        xport::SHUTDOWN => {
            let how = [libc::SHUT_RD, libc::SHUT_WR, libc::SHUT_RDWR];
            let how = how
                .get((flags >> 3) as usize)
                .copied()
                .unwrap_or(libc::SHUT_RDWR);
            p.outputs.returncode = socket.shutdown(how).into();
        }
        _ => {}
    }
    Ok(())
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "upstream stores transfers in an int"
)]
const fn returncode(bytes: isize) -> c_int {
    bytes as c_int
}

/// `STREAM_OOB`: unsupported.
fn oob(param: &mut php_stream_xport_param) -> Result<(), Bailout> {
    report(
        Report::Swoole,
        E_WARNING,
        c"STREAM_OOB flags is not supports",
    )?;
    param.outputs.returncode = -1;
    Ok(())
}

/// `socket_connect`: 0, or -1 with the error.
fn connect(
    stream: *mut php_stream,
    socket: &Socket,
    param: &mut php_stream_xport_param,
    error: Option<*mut *mut zend_string>,
) -> c_int {
    if socket.fd() < 0 {
        return FAILURE;
    }
    let (host, port) = if socket.is_inet() {
        // SAFETY: Zend lends `namelen` bytes at `name`.
        let name = unsafe {
            std::slice::from_raw_parts(param.inputs.name.cast::<u8>(), param.inputs.namelen)
        };
        let parsed = parse_address(name, error);
        if socket.kind() == libc::SOCK_STREAM {
            socket.set_nodelay(1);
        }
        match parsed {
            Some(parsed) => parsed,
            None => return FAILURE,
        }
    } else if param.inputs.name.is_null() {
        return FAILURE;
    } else {
        // SAFETY: Zend's NUL-terminated name.
        (unsafe { CStr::from_ptr(param.inputs.name) }.to_owned(), 0)
    };
    if let Some(bindto) = context_option(stream, c"socket", c"bindto") {
        // SAFETY: a live option value.
        let Some(name) = (unsafe { swoole_rs_zend::string_of(bindto) }) else {
            if let Some(error) = error {
                // SAFETY: the caller's output slot.
                unsafe { *error = string::copy(b"local_addr context option is not a string.") };
            }
            return FAILURE;
        };
        let Some((local, local_port)) = parse_address(name, error) else {
            return FAILURE;
        };
        let local = match local.as_bytes() {
            b"0" | b"" => c"0.0.0.0",
            _ => local.as_c_str(),
        };
        if !socket.bind(local, local_port) {
            return FAILURE;
        }
    }
    if !param.inputs.timeout.is_null() {
        socket.set_timeout(seconds(param.inputs.timeout), timeout::CONNECT);
    }
    if !socket.connect(&host, port) {
        param.outputs.error_code = socket.error();
        if let Some(message) = socket.message() {
            param.outputs.error_text = string::copy(message.to_bytes());
        }
        return -1;
    }
    0
}

/// `socket_bind`: 0, or -1.
fn bind(
    socket: &Socket,
    param: &php_stream_xport_param,
    error: Option<*mut *mut zend_string>,
) -> c_int {
    let parsed = if socket.is_inet() {
        // SAFETY: Zend lends `namelen` bytes at `name`.
        let name = unsafe {
            std::slice::from_raw_parts(param.inputs.name.cast::<u8>(), param.inputs.namelen)
        };
        parse_address(name, error)
    } else if param.inputs.name.is_null() {
        None
    } else {
        // SAFETY: Zend's NUL-terminated name.
        Some((unsafe { CStr::from_ptr(param.inputs.name) }.to_owned(), 0))
    };
    let Some((host, port)) = parsed else {
        socket.set_error(libc::EINVAL);
        return -1;
    };
    if socket.bind(&host, port) { 0 } else { -1 }
}

/// `socket_accept`: `SUCCESS` with the client stream, or `FAILURE`.
fn accept(
    stream: *mut php_stream,
    socket: &Socket,
    param: &mut php_stream_xport_param,
    textaddr: *mut *mut zend_string,
    addr: *mut *mut sockaddr,
    addrlen: *mut socklen_t,
    error: Option<*mut *mut zend_string>,
) -> c_int {
    param.outputs.client = ptr::null_mut();
    let nodelay = option_true(stream, c"socket", c"tcp_nodelay");
    if !param.inputs.timeout.is_null() {
        socket.set_timeout(seconds(param.inputs.timeout), timeout::READ);
    }
    let mut client = socket.accept();
    if let Some(connection) = &client
        && connection.ssl_enabled()
        && !connection.ssl_handshake()
    {
        socket.set_error_code(connection.error());
        client = None;
    }
    let Some(client) = client else {
        let code = socket.error();
        param.outputs.error_code = code;
        if let Some(error) = error {
            // SAFETY: Zend formats the error.
            let text = unsafe { php_socket_error_str(code.into()) };
            // SAFETY: the caller's output slot.
            unsafe { *error = text };
        }
        return FAILURE;
    };
    // Upstream fills the peer's name from this never-written address; zeroed,
    // its family is unset, so the text address is left alone.
    // SAFETY: an all-zero sockaddr_storage is valid.
    let mut peer: sockaddr_storage = unsafe { std::mem::zeroed() };
    #[expect(clippy::cast_possible_truncation, reason = "sizeof(sockaddr_storage)")]
    // SAFETY: the address and the output slots Zend asked for.
    unsafe {
        php_network_populate_name_from_sockaddr(
            (&raw mut peer).cast(),
            size_of::<sockaddr_storage>() as socklen_t,
            textaddr,
            addr,
            addrlen,
        );
    }
    if nodelay {
        client.set_nodelay(1);
    }
    // SAFETY: a live stream.
    let connection = alloc(unsafe { (*stream).ops }, client, ptr::null());
    if !connection.is_null() {
        // SAFETY: both live; the client counts its reference.
        unsafe { swr_zend_stream_share_context(connection, stream) };
    }
    param.outputs.client = connection;
    SUCCESS
}

/// `socket_recvfrom`.
fn recvfrom(
    socket: &Socket,
    param: &php_stream_xport_param,
    textaddr: *mut *mut zend_string,
    addr: *mut *mut sockaddr,
    addrlen: *mut socklen_t,
) -> c_int {
    let (buf, length) = (param.inputs.buf, param.inputs.buflen);
    if textaddr.is_null() && addr.is_null() {
        // SAFETY: Zend lends `length` writable bytes at `buf`.
        return returncode(unsafe { socket.io(Io::Recv, buf, length) });
    }
    // SAFETY: an all-zero sockaddr_storage is valid.
    let mut peer: sockaddr_storage = unsafe { std::mem::zeroed() };
    #[expect(clippy::cast_possible_truncation, reason = "sizeof(sockaddr_storage)")]
    let mut size = size_of::<sockaddr_storage>() as socklen_t;
    // SAFETY: as above, and the address holds `size` bytes.
    let received = unsafe { socket.recvfrom(buf, length, (&raw mut peer).cast(), &raw mut size) };
    if size != 0 {
        // SAFETY: the address and the output slots Zend asked for.
        unsafe {
            php_network_populate_name_from_sockaddr(
                (&raw mut peer).cast(),
                size,
                textaddr,
                addr,
                addrlen,
            );
        };
    } else {
        if !textaddr.is_null() {
            // SAFETY: the interned empty string.
            let empty = unsafe { swr_zend_stream_empty_string() };
            // SAFETY: Zend's output slot.
            unsafe { *textaddr = empty };
        }
        if !addr.is_null() {
            // SAFETY: Zend's output slot.
            unsafe { *addr = ptr::null_mut() };
            // SAFETY: as above.
            unsafe { *addrlen = 0 };
        }
    }
    returncode(received)
}

/// `socket_enable_crypto`: 1, or -1.
fn enable_crypto(
    stream: *mut php_stream,
    socket: &Socket,
    activate: bool,
) -> Result<c_int, Bailout> {
    // SAFETY: a live stream.
    let context = unsafe { swr_zend_stream_context(stream) };
    if activate && !socket.ssl_available() {
        socket.ssl_encrypt();
        if !ssl_set_options(socket, context)? || !socket.ssl_handshake() {
            return Ok(-1);
        }
    } else if !activate && socket.ssl_available() && !socket.ssl().is_null() {
        socket.ssl_close();
        return Ok(-1);
    }
    if !context.is_null()
        && socket.ssl_available()
        && option_true(stream, c"ssl", c"capture_peer_cert")
        && !capture_peer_certs(stream, socket)?
    {
        return Ok(-1);
    }
    Ok(1)
}

/// `php_openssl_capture_peer_certs`: the `peer_certificate` (and with
/// `capture_peer_cert_chain` the `peer_certificate_chain`) context options;
/// false without a peer certificate.
fn capture_peer_certs(stream: *mut php_stream, socket: &Socket) -> Result<bool, Bailout> {
    let cert = socket.peer_certs(false).pop().unwrap_or_default();
    if cert.is_empty() {
        return Ok(false);
    }
    // SAFETY: an undefined zval to fill.
    let mut certificate: zval = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    unsafe { x509_read(&cert, &raw mut certificate) }?;
    set_context_option(stream, c"peer_certificate", &raw mut certificate);
    if option_true(stream, c"ssl", c"capture_peer_cert_chain") {
        let chain = socket.peer_certs(true);
        // SAFETY: a null zval until it becomes the array.
        let mut array: zval = unsafe { std::mem::zeroed() };
        array.u1.type_info = IS_NULL;
        if !chain.is_empty() {
            // SAFETY: as above.
            unsafe { swr_zend_array_init(&raw mut array) };
            for cert in chain {
                // SAFETY: an undefined zval to fill.
                let mut certificate: zval = unsafe { std::mem::zeroed() };
                // SAFETY: as above.
                if let Err(bailout) = unsafe { x509_read(&cert, &raw mut certificate) } {
                    // SAFETY: the array built so far.
                    unsafe { zval_ptr_dtor(&raw mut array) };
                    return Err(bailout);
                }
                // SAFETY: an array zval.
                let list = unsafe { table(&raw const array) };
                // SAFETY: the array takes the certificate's reference.
                unsafe { zend_hash_next_index_insert(list, &raw mut certificate) };
            }
        }
        set_context_option(stream, c"peer_certificate_chain", &raw mut array);
    }
    Ok(true)
}

/// Set the stream's `ssl` context option `name` to `value`, which is ours
/// and released here.
fn set_context_option(stream: *mut php_stream, name: &CStr, value: *mut zval) {
    // SAFETY: a live stream; the context takes its own reference.
    unsafe { swr_zend_stream_set_option(stream, c"ssl".as_ptr(), name.as_ptr(), value) };
    // SAFETY: ours.
    unsafe { zval_ptr_dtor(value) };
}

/// The `ssl` context options and the names Swoole's socket takes them under.
const SSL_ALIASES: [(&str, &str); 10] = [
    ("peer_name", "ssl_host_name"),
    ("verify_peer", "ssl_verify_peer"),
    ("allow_self_signed", "ssl_allow_self_signed"),
    ("cafile", "ssl_cafile"),
    ("capath", "ssl_capath"),
    ("local_cert", "ssl_cert_file"),
    ("local_pk", "ssl_key_file"),
    ("passphrase", "ssl_passphrase"),
    ("verify_depth", "ssl_verify_depth"),
    ("disable_compression", "ssl_disable_compression"),
];

/// `socket_ssl_set_options`: an encrypted socket takes the context's `ssl`
/// options; false if Swoole refuses them.
fn ssl_set_options(socket: &Socket, context: *mut php_stream_context) -> Result<bool, Bailout> {
    // SAFETY: null or a live context.
    let options = unsafe { swr_zend_stream_context_options(context) };
    if options.is_null() || !socket.ssl_enabled() {
        return Ok(true);
    }
    let Some(ssl) = find(options, "ssl") else {
        return Ok(true);
    };
    // SAFETY: a live entry.
    if unsafe { type_of(ssl) } != IS_ARRAY {
        return Ok(true);
    }
    // SAFETY: an array zval.
    let ssl = unsafe { table(ssl) };
    // SAFETY: an undefined zval, initialised as an array.
    let mut aliases: zval = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    unsafe { swr_zend_array_init(&raw mut aliases) };
    for (name, alias) in SSL_ALIASES {
        if let Some(value) = find(ssl, name) {
            // SAFETY: a live value; the alias array takes this reference.
            unsafe { swr_zval_try_addref(value) };
            // SAFETY: as above.
            unsafe {
                add_assoc_zval_ex(&raw mut aliases, alias.as_ptr().cast(), alias.len(), value);
            };
        }
    }
    // SAFETY: a live array.
    let result = unsafe { socket.set_ssl(&raw mut aliases) };
    // SAFETY: ours.
    unsafe { zval_ptr_dtor(&raw mut aliases) };
    result
}

/// The transport factory's coroutine branch.
/// # Safety
/// Called by the C++ factory in a coroutine with Zend's arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpruntime_socket_create(
    proto: *const c_char,
    protolen: usize,
    resourcename: *const c_char,
    persistent_id: *const c_char,
    context: *mut php_stream_context,
    bailout: *mut u8,
) -> *mut php_stream {
    // SAFETY: per the caller.
    let result = unsafe { create(proto, protolen, resourcename, persistent_id, context) };
    reraise(bailout, result, ptr::null_mut())
}

/// # Safety
/// As `swr_v1_phpruntime_socket_create`.
unsafe fn create(
    proto: *const c_char,
    protolen: usize,
    resourcename: *const c_char,
    persistent_id: *const c_char,
    context: *mut php_stream_context,
) -> Result<*mut php_stream, Bailout> {
    // SAFETY: Zend lends `protolen` bytes at `proto`.
    let proto = unsafe { std::slice::from_raw_parts(proto.cast::<u8>(), protolen) };
    let proto = proto.strip_prefix(b"async.").unwrap_or(proto);
    // SAFETY: a NUL-terminated resource name.
    let tcp = if unsafe { *resourcename.cast::<u8>() } == b'[' {
        kind::TCP6
    } else {
        kind::TCP
    };
    let (kind, ssl) = match proto {
        b"tcp" => (tcp, false),
        b"ssl" | b"tls" => (tcp, true),
        b"unix" => (kind::UNIX_STREAM, false),
        b"udp" => (kind::UDP, false),
        b"udg" => (kind::UNIX_DGRAM, false),
        _ => {
            let message = message!(
                "unknown protocol '{}'",
                String::from_utf8_lossy(c_str(proto))
            );
            report(Report::Fatal, E_WARNING, message.as_cstr())?;
            return Ok(ptr::null_mut());
        }
    };
    let Some(socket) = Socket::new(kind, ssl) else {
        return Ok(ptr::null_mut());
    };
    socket.set_zero_copy();
    let stream = alloc(socket_ops(), socket, persistent_id);
    if stream.is_null() {
        return Ok(stream);
    }
    // SAFETY: the stream just made.
    let socket = unsafe { Rc::clone(&(*net(stream)).socket) };
    if !ssl_set_options(&socket, context)? {
        drop(socket);
        // SAFETY: ours, not yet handed out.
        unsafe { swr_zend_stream_close(stream) };
        return Ok(ptr::null_mut());
    }
    Ok(stream)
}

/// `php_swoole_create_stream_from_socket`: a coroutine stream over `fd`,
/// with the default socket timeout.
/// # Safety
/// Called while PHP runs; the stream takes `fd`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpruntime_stream_from_socket(
    fd: c_int,
    domain: c_int,
    kind: c_int,
    protocol: c_int,
) -> *mut php_stream {
    let Some(socket) = Socket::from_fd(fd, domain, kind, protocol) else {
        return ptr::null_mut();
    };
    // SAFETY: reads a PHP global.
    let seconds = unsafe { swr_zend_stream_default_timeout() };
    if seconds > 0 {
        #[expect(clippy::cast_precision_loss, reason = "upstream's (double) cast")]
        socket.set_timeout(seconds as f64, timeout::ALL);
    }
    let stream = alloc(socket_ops(), socket, ptr::null());
    if !stream.is_null() {
        // SAFETY: the stream just made.
        unsafe { swr_zend_stream_avoid_blocking(stream) };
    }
    stream
}

/// `stream_socket_pair(int $domain, int $type, int $protocol): array|false`:
/// 1 if a bailout was trapped.
/// # Safety
/// Called from the C++ handler with its live frame and return slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpruntime_socket_pair(
    frame: *mut swoole_rs_zend::sys::zend_execute_data,
    value: *mut zval,
) -> u8 {
    // SAFETY: per the caller.
    let call = unsafe { Call::new(frame, value) };
    u8::from(socket_pair(&call).is_err())
}

fn socket_pair(call: &Call) -> Result<(), Bailout> {
    let parsed = call
        .arity(3, 3)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, 0)?, call.long(3, 0)?)));
    let Ok((domain, kind, protocol)) = parsed else {
        call.set_bool(false);
        return Ok(());
    };
    #[expect(clippy::cast_possible_truncation, reason = "upstream's (int) casts")]
    let (domain, kind, protocol) = (domain as c_int, kind as c_int, protocol as c_int);
    let mut pair = [0; 2];
    // SAFETY: two writable fds.
    if unsafe { libc::socketpair(domain, kind, protocol, pair.as_mut_ptr()) } != 0 {
        let code = errno();
        // SAFETY: strerror's text, valid until the next call.
        let reason = unsafe { libc::strerror(code) };
        // SAFETY: NUL-terminated.
        let reason = unsafe { CStr::from_ptr(reason) }.to_string_lossy();
        let message = message!("failed to create sockets: [{code}]: {reason}");
        report(Report::Swoole, E_WARNING, message.as_cstr())?;
        call.set_bool(false);
        return Ok(());
    }
    call.set_array();
    check_reactor();
    for fd in pair {
        // SAFETY: the stream takes the fd.
        let stream = unsafe { swr_v1_phpruntime_stream_from_socket(fd, domain, kind, protocol) };
        // SAFETY: a fresh stream, exposed as the array's resource.
        let resource = unsafe { swr_zend_stream_expose(stream) };
        // SAFETY: the return value is an array.
        unsafe { add_next_index_resource(call.return_value(), resource) };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{SSL_ALIASES, c_str};

    #[test]
    fn c_str_stops_at_the_first_nul() {
        assert_eq!(c_str(b"tcp"), b"tcp");
        assert_eq!(c_str(b"a\0b"), b"a");
        assert_eq!(c_str(b""), b"");
    }

    #[test]
    fn ssl_aliases_are_upstreams() {
        let names: Vec<_> = SSL_ALIASES.iter().map(|(name, _)| *name).collect();
        assert_eq!(names.len(), 10);
        assert!(SSL_ALIASES.contains(&("local_cert", "ssl_cert_file")));
        assert!(SSL_ALIASES.contains(&("peer_name", "ssl_host_name")));
    }
}

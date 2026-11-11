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
use swoole_rs_zend::{
    Array, ArrayMut, Call, E_WARNING, Message, Owned, OwnedArray, Value,
    bailout::Bailout,
    message, string,
    sys::{
        PHP_STREAM_FLAG_AVOID_BLOCKING, add_next_index_resource, php_stream, php_stream_context,
        php_stream_ops, php_stream_statbuf, php_stream_xport_crypto_param, php_stream_xport_param,
        sockaddr, swr_zend_efree, swr_zend_stream_buffered, swr_zend_stream_context,
        swr_zend_stream_context_options, swr_zend_stream_default_timeout, swr_zend_stream_expose,
        swr_zend_stream_option, swr_zend_stream_progress, swr_zend_stream_set_option,
        swr_zend_stream_share_context, swr_zend_stream_timeval, swr_zend_stream_vm_active, timeval,
        zend_string, zval,
    },
    value::empty_string,
};

unsafe extern "C" {
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
    pub(super) use swoole_rs_zend::sys::{
        _php_stream_xport_param_STREAM_XPORT_OP_ACCEPT as ACCEPT,
        _php_stream_xport_param_STREAM_XPORT_OP_BIND as BIND,
        _php_stream_xport_param_STREAM_XPORT_OP_CONNECT as CONNECT,
        _php_stream_xport_param_STREAM_XPORT_OP_CONNECT_ASYNC as CONNECT_ASYNC,
        _php_stream_xport_param_STREAM_XPORT_OP_GET_NAME as GET_NAME,
        _php_stream_xport_param_STREAM_XPORT_OP_GET_PEER_NAME as GET_PEER_NAME,
        _php_stream_xport_param_STREAM_XPORT_OP_LISTEN as LISTEN,
        _php_stream_xport_param_STREAM_XPORT_OP_RECV as RECV,
        _php_stream_xport_param_STREAM_XPORT_OP_SEND as SEND,
        _php_stream_xport_param_STREAM_XPORT_OP_SHUTDOWN as SHUTDOWN,
    };
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
    blocking: std::cell::Cell<bool>,
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Report a trapped bailout through `flag`; `failed` stands in for the result.
pub(crate) fn reraise<T: Copy>(flag: *mut bool, result: Result<T, Bailout>, failed: T) -> T {
    let (value, status) = match result {
        Ok(value) => (value, Ok(())),
        Err(error) => (failed, Err(error)),
    };
    if swoole_rs_zend::bailout::relay_to_c(status) {
        // SAFETY: the native entry lends its flag; owned Rust cleanup has already finished.
        unsafe { *flag = true };
        failed
    } else {
        value
    }
}

/// The value, or `failed` after a bailout, which is raised last: the caller is
/// the native callback boundary and owns nothing by then.
pub(crate) fn raise<T: Copy>(result: Result<T, Bailout>, failed: T) -> T {
    let (value, status) = match result {
        Ok(value) => (value, Ok(())),
        Err(error) => (failed, Err(error)),
    };
    swoole_rs_zend::bailout::raise(status);
    value
}

// PHP's required operations table is Rust-owned alongside its stream data and bodies.
// The three callbacks that may execute PHP raise a trapped bailout last.
#[unsafe(no_mangle)]
pub static mut swoole_runtime_socket_ops: php_stream_ops = php_stream_ops {
    write: Some(socket_write),
    read: Some(socket_read),
    close: Some(swr_v1_phpruntime_socket_close),
    flush: Some(swr_v1_phpruntime_socket_flush),
    label: c"socket/coroutine".as_ptr(),
    seek: None,
    cast: Some(swr_v1_phpruntime_socket_cast),
    stat: Some(swr_v1_phpruntime_socket_stat),
    set_option: Some(socket_set_option),
};

/// The stream's `NetStream`, null once closed.
fn net_ptr(stream: *mut php_stream) -> *mut NetStream {
    // SAFETY: a live stream of these operations.
    unsafe { (*stream).abstract_ }.cast()
}

/// The stream's `NetStream`, none once closed. The stream owns it until its
/// close, which no borrow outlives: each is taken for one operation.
fn net<'a>(stream: *mut php_stream) -> Option<&'a NetStream> {
    // SAFETY: null or the open stream's live NetStream.
    unsafe { net_ptr(stream).as_ref() }
}

/// `php_stream_alloc(ops, abstract, persistent_id, "r+")` of `socket`;
/// null (the socket freed) if it fails.
fn alloc(
    ops: *const php_stream_ops,
    socket: Rc<Socket>,
    persistent_id: *const c_char,
) -> Result<*mut php_stream, Bailout> {
    let net = Box::into_raw(Box::new(NetStream {
        socket,
        blocking: std::cell::Cell::new(true),
    }));
    let mut stream = ptr::null_mut();
    // SAFETY: PHP receives this stable owned abstract pointer before resource registration.
    // On bailout it remains available to PHP's partial-resource/request cleanup, as before.
    if unsafe {
        swoole_rs_zend::sys::swr_zend_try_stream_alloc(
            ops,
            net.cast(),
            persistent_id,
            &raw mut stream,
        )
    } != 0
    {
        return Err(Bailout);
    }
    if stream.is_null() {
        // SAFETY: not handed over.
        drop(unsafe { Box::from_raw(net) });
    }
    Ok(stream)
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
    unsafe { (*stream).set_eof(1) };
}

/// The seconds of a `timeval`, as Swoole's `set_timeout(timeval *)`.
pub(crate) fn seconds(time: *const timeval) -> f64 {
    let (mut seconds, mut micros) = (0.0, 0.0);
    // SAFETY: Zend lends a live timeval.
    unsafe { swr_zend_stream_timeval(time, &raw mut seconds, &raw mut micros) };
    swoole_rs_hooks::timeval(seconds, micros)
}

/// A context option of the stream, if set, borrowed for one operation, which
/// sets no option.
fn context_option<'a>(stream: *mut php_stream, wrapper: &CStr, name: &CStr) -> Option<&'a Value> {
    // SAFETY: a live stream; Zend copies nothing.
    let value = unsafe { swr_zend_stream_option(stream, wrapper.as_ptr(), name.as_ptr()) };
    // SAFETY: null or the context's live option.
    unsafe { Value::from_ptr(value) }
}

/// `zval_is_true` of a context option, false when unset.
fn option_true(stream: *mut php_stream, wrapper: &CStr, name: &CStr) -> bool {
    context_option(stream, wrapper, name).is_some_and(Value::truthy)
}

/// Bytes up to the first NUL, as C reads a string.
fn c_str(bytes: &[u8]) -> &[u8] {
    bytes.split(|&b| b == 0).next().unwrap_or_default()
}

/// `parse_ip_address_ex`: host and port of `name`, else the error text when
/// `error` wants it.
fn parse_address(name: &[u8], error: Option<*mut *mut zend_string>) -> Option<(CString, i32)> {
    let plan = swoole_rs_hooks::address::parse(name);
    if plan.status != 0 {
        let what = if plan.status == 1 {
            "IPv6 address"
        } else {
            "address"
        };
        let name = String::from_utf8_lossy(c_str(name));
        set_error(
            error,
            format!("Failed to parse {what} \"{name}\"").as_bytes(),
        );
        return None;
    }
    let host = name.get(plan.host_offset..plan.host_offset + plan.host_length);
    let host = c_str(host.unwrap_or_default());
    Some((CString::new(host).unwrap_or_default(), plan.port))
}

/// A copy of `text` in Zend's error text slot, when the caller wants one.
fn set_error(error: Option<*mut *mut zend_string>, text: &[u8]) {
    if let Some(error) = error {
        // SAFETY: the transport parameter's output slot, lent for the operation.
        unsafe { *error = string::copy(text) };
    }
}

/// `socket_write`.
fn write(stream: *mut php_stream, buf: *const c_char, count: usize) -> Result<isize, Bailout> {
    let Some(net) = net(stream) else {
        return Ok(-1);
    };
    let (socket, blocking) = (Rc::clone(&net.socket), net.blocking.get());
    // SAFETY: Zend lends `count` bytes at `buf` for the call, which a send only reads.
    let send = |op| unsafe { socket.io(op, buf.cast_mut(), count) };
    // SAFETY: no preconditions.
    if !unsafe { swr_zend_stream_vm_active() } {
        return Ok(send(Io::SendSync));
    }
    let mut written = if blocking {
        send(Io::SendAll)
    } else {
        let written = send(Io::SendOnce);
        socket.set_error(errno());
        written
    };
    if written < 0 || written.unsigned_abs() != count {
        // the caller does not loop on send, and a partial count is meaningless
        written = -1;
        let code = socket.error();
        if code != libc::ETIMEDOUT {
            if socket.catch_error(code) == SW_WAIT {
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
    let Some(net) = net(stream) else {
        return Ok(-1);
    };
    let (socket, blocking) = (Rc::clone(&net.socket), net.blocking.get());
    // SAFETY: Zend lends `count` writable bytes at `buf` for the call.
    let recv = |op| unsafe { socket.io(op, buf, count) };
    // SAFETY: no preconditions.
    if !unsafe { swr_zend_stream_vm_active() } {
        return Ok(recv(Io::RecvSync));
    }
    // SAFETY: a live stream.
    let mut read = if blocking && !unsafe { swr_zend_stream_buffered(stream) } {
        recv(Io::Recv)
    } else {
        let read = recv(Io::RecvOnce);
        socket.set_error(errno());
        read
    };
    if read > 0 {
        progress(stream, read)?;
    }
    if read < 0 && socket.error() != libc::ETIMEDOUT {
        if socket.catch_error(socket.error()) == SW_WAIT {
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
unsafe extern "C" fn socket_write(
    stream: *mut php_stream,
    buf: *const c_char,
    count: usize,
) -> isize {
    raise(write(stream, buf, count), -1)
}

/// The stream's `read`: bytes read, or -1.
/// # Safety
/// Called by Zend with a stream of these operations and `count` writable
/// bytes at `buf`.
unsafe extern "C" fn socket_read(stream: *mut php_stream, buf: *mut c_char, count: usize) -> isize {
    raise(read(stream, buf, count), -1)
}

/// The stream's `close`: the socket closes now and is freed once no
/// operation holds it.
/// # Safety
/// Called by Zend with a stream of these operations.
unsafe extern "C" fn swr_v1_phpruntime_socket_close(
    stream: *mut php_stream,
    close_handle: c_int,
) -> c_int {
    let _ = close_handle;
    // SAFETY: Zend holds this stream through its close callback.
    let status = match unsafe { close_stream(stream) } {
        Ok(status) => status,
        Err(error) => {
            error.defer();
            FAILURE
        }
    };
    // This is a native callback boundary: close_stream has already dropped its Box/Rc owners.
    swoole_rs_zend::bailout::finish();
    status
}

unsafe fn close_stream(stream: *mut php_stream) -> Result<c_int, Bailout> {
    let net = net_ptr(stream);
    if net.is_null() {
        return Ok(FAILURE);
    }
    // SAFETY: detach first, so reentrant PHP cannot reach this stream owner again.
    unsafe { (*stream).abstract_ = ptr::null_mut() };
    // SAFETY: transfer the stream's unique owner back from its stable abstract pointer.
    let net = unsafe { Box::from_raw(net) };
    let result = net.socket.close();
    drop(net);
    result.map(|_| SUCCESS)
}

/// The stream's `flush`: nothing is buffered.
extern "C" fn swr_v1_phpruntime_socket_flush(stream: *mut php_stream) -> c_int {
    let _ = stream;
    0
}

/// The stream's `cast`: the fd, or a `FILE *` over it.
/// # Safety
/// Called by Zend with a stream of these operations; `ret` is null or holds
/// the requested type.
unsafe extern "C" fn swr_v1_phpruntime_socket_cast(
    stream: *mut php_stream,
    castas: c_int,
    ret: *mut *mut c_void,
) -> c_int {
    let Some(net) = net(stream) else {
        return FAILURE;
    };
    let fd = net.socket.fd();
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
unsafe extern "C" fn swr_v1_phpruntime_socket_stat(
    stream: *mut php_stream,
    ssb: *mut php_stream_statbuf,
) -> c_int {
    let Some(net) = net(stream) else {
        return FAILURE;
    };
    let fd = net.socket.fd();
    // SAFETY: per the caller.
    let sb = unsafe { &raw mut (*ssb).sb };
    // SAFETY: `sb` is a writable `struct stat`.
    unsafe { libc::fstat(fd, sb.cast()) }
}

/// The stream's `set_option`.
/// # Safety
/// Called by Zend with a stream of these operations and the option's
/// parameter.
unsafe extern "C" fn socket_set_option(
    stream: *mut php_stream,
    option: c_int,
    value: c_int,
    ptrparam: *mut c_void,
) -> c_int {
    raise(
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
    let Some(net) = net(stream) else {
        return Ok(option::RETURN_ERR);
    };
    let socket = Rc::clone(&net.socket);
    match option {
        option::BLOCKING => net.blocking.set(value != 0),
        option::XPORT_API => xport_api(stream, &socket, ptrparam.cast())?,
        option::META_DATA_API => meta_data(stream, &socket, ptrparam.cast()),
        // PHP socket streams share this timeout between reads and writes
        option::READ_TIMEOUT => socket.set_timeout(seconds(ptrparam.cast()), timeout::RDWR),
        option::CRYPTO_API => {
            // SAFETY: Zend lends the crypto parameter for the call.
            let param = unsafe { &mut *ptrparam.cast::<php_stream_xport_crypto_param>() };
            match param.op {
                0 => param.outputs.returncode = 0,
                1 => {
                    let activate = param.inputs.activate != 0;
                    param.outputs.returncode = enable_crypto(stream, &socket, activate)?;
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
    // SAFETY: Zend lends `stream_get_meta_data`'s result array for the call.
    let data = unsafe { ArrayMut::from_ptr(data) };
    let ssl = socket.ssl();
    if !ssl.is_null() {
        data.insert(b"crypto", crypto(ssl).into_owned());
    }
    data.insert_bool(b"timed_out", socket.error() == libc::ETIMEDOUT);
    // SAFETY: a live stream.
    data.insert_bool(b"eof", unsafe { (*stream).eof() } != 0);
    data.insert_bool(b"blocked", true);
}

/// The `crypto` metadata of a live connection.
fn crypto(ssl: *mut openssl_sys::SSL) -> OwnedArray {
    // SAFETY: a live connection.
    let protocol: &[u8] = match unsafe { openssl_sys::SSL_version(ssl) } {
        0x304 => b"TLSv1.3",
        0x303 => b"TLSv1.2",
        0x302 => b"TLSv1.1",
        0x301 => b"TLSv1",
        0x300 => b"SSLv3",
        _ => b"UNKNOWN",
    };
    // SAFETY: a live connection; OpenSSL takes a null cipher below.
    let cipher = unsafe { openssl_sys::SSL_get_current_cipher(ssl) };
    // SAFETY: as above.
    let name = text(unsafe { openssl_sys::SSL_CIPHER_get_name(cipher) });
    // SAFETY: as above.
    let bits = unsafe { openssl_sys::SSL_CIPHER_get_bits(cipher, ptr::null_mut()) };
    // SAFETY: as above.
    let version = text(unsafe { openssl_sys::SSL_CIPHER_get_version(cipher) });
    let mut crypto = OwnedArray::default();
    crypto.insert_bytes(b"protocol", Some(protocol));
    crypto.insert_bytes(b"cipher_name", Some(name));
    crypto.insert_long(b"cipher_bits", bits.into());
    crypto.insert_bytes(b"cipher_version", Some(version));
    crypto
}

/// The bytes of OpenSSL's static cipher text.
fn text(text: *const c_char) -> &'static [u8] {
    // SAFETY: OpenSSL's cipher names and versions are static and NUL-terminated.
    unsafe { CStr::from_ptr(text) }.to_bytes()
}

/// `PHP_STREAM_OPTION_XPORT_API`.
fn xport_api(
    stream: *mut php_stream,
    socket: &Socket,
    param: *mut php_stream_xport_param,
) -> Result<(), Bailout> {
    // SAFETY: Zend lends the transport parameter for the call.
    let p = unsafe { &mut *param };
    let want_addr = p.want_addr() != 0;
    let want_textaddr = p.want_textaddr() != 0;
    let want_errortext = p.want_errortext() != 0;
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
    let (buf, buflen) = (p.inputs.buf, p.inputs.buflen);
    // SAFETY: Zend lends `buflen` bytes at `buf`, writable unless it sends.
    let io = |op| unsafe { socket.io(op, buf, buflen) };
    match p.op {
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
            p.outputs.returncode = accept(stream, socket, p, textaddr, addr, addrlen, error_text)?;
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
                io(Io::Send)
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
                returncode(io(Io::Peek))
            } else {
                recvfrom(socket, p, textaddr, addr, addrlen)
            };
        }
        xport::SHUTDOWN => {
            let how = [libc::SHUT_RD, libc::SHUT_WR, libc::SHUT_RDWR];
            let how = how
                .get(p.how() as usize)
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
    let inet = socket.is_inet();
    let parsed = target(inet, param, error);
    if inet && socket.socket_type() == libc::SOCK_STREAM {
        socket.set_nodelay(1);
    }
    let Some((host, port)) = parsed else {
        return FAILURE;
    };
    if let Some(bindto) = context_option(stream, c"socket", c"bindto") {
        let Some(name) = bindto.bytes() else {
            set_error(error, b"local_addr context option is not a string.");
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

/// The transport parameter's name: on an `inet` socket the host and port it
/// parses to (else none, with `error`), on another the NUL-terminated path.
fn target(
    inet: bool,
    param: &php_stream_xport_param,
    error: Option<*mut *mut zend_string>,
) -> Option<(CString, i32)> {
    if inet {
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
    }
}

/// `socket_bind`: 0, or -1.
fn bind(
    socket: &Socket,
    param: &php_stream_xport_param,
    error: Option<*mut *mut zend_string>,
) -> c_int {
    let Some((host, port)) = target(socket.is_inet(), param, error) else {
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
) -> Result<c_int, Bailout> {
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
        return Ok(FAILURE);
    };
    // The peer's name, as upstream reports it: from an empty address.
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
    let connection = alloc(unsafe { (*stream).ops }, Rc::new(client), ptr::null())?;
    if !connection.is_null() {
        // SAFETY: both live; the client counts its reference.
        unsafe { swr_zend_stream_share_context(connection, stream) };
    }
    param.outputs.client = connection;
    Ok(SUCCESS)
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
            // SAFETY: Zend's output slot; the empty string is interned.
            unsafe { *textaddr = empty_string() };
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
        socket.enable_ssl();
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
    let cert = socket.peer_cert();
    if cert.is_empty() {
        return Ok(false);
    }
    let certificate = x509_read(&cert)?;
    set_context_option(stream, c"peer_certificate", &certificate);
    if option_true(stream, c"ssl", c"capture_peer_cert_chain") {
        let chain = socket.peer_chain();
        let list = if chain.is_empty() {
            Owned::null()
        } else {
            let mut list = OwnedArray::default();
            for cert in chain {
                list.push(x509_read(&cert)?);
            }
            list.into_owned()
        };
        set_context_option(stream, c"peer_certificate_chain", &list);
    }
    Ok(true)
}

/// Set the stream's `ssl` context option `name` to `value`.
fn set_context_option(stream: *mut php_stream, name: &CStr, value: &Value) {
    // SAFETY: a live stream; the context takes its own reference.
    unsafe { swr_zend_stream_set_option(stream, c"ssl".as_ptr(), name.as_ptr(), value.as_ptr()) };
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
    // SAFETY: null or the context's live options, which no PHP code changes
    // while this runs.
    let options = unsafe { Array::from_ptr(options) };
    // `php_swoole_array_get_value_ex`: present and not null.
    let ssl = options.and_then(|options| options.get(b"ssl")?.array());
    let Some(ssl) = ssl.filter(|_| socket.ssl_enabled()) else {
        return Ok(true);
    };
    let mut aliases = OwnedArray::default();
    for (name, alias) in SSL_ALIASES {
        if let Some(value) = ssl.get(name.as_bytes()) {
            aliases.insert(alias.as_bytes(), value.copy());
        }
    }
    socket.set_ssl(&aliases)
}

/// The complete factory: PHP's original transport outside a coroutine, otherwise
/// Rust's coroutine socket construction and stream ownership. zend-sys's
/// `swr_zend_stream_factory` is the registered factory and raises the bailout.
/// # Safety
/// All borrowed PHP factory arguments stay live through this call; bailout is writable.
pub(crate) unsafe extern "C" fn transport_factory_body(
    proto: *const c_char,
    protolen: usize,
    name: *const c_char,
    namelen: usize,
    persistent: *const c_char,
    options: c_int,
    flags: c_int,
    time: *mut timeval,
    context: *mut php_stream_context,
    bailed: *mut bool,
) -> *mut php_stream {
    // SAFETY: the native entry lends every original PHP factory argument through invocation.
    let result = unsafe {
        transport_factory(
            proto, protolen, name, namelen, persistent, options, flags, time, context,
        )
    };
    reraise(bailed, result, ptr::null_mut())
}

#[expect(
    clippy::too_many_arguments,
    reason = "preserves the complete native PHP transport factory signature"
)]
unsafe fn transport_factory(
    proto: *const c_char,
    protolen: usize,
    name: *const c_char,
    namelen: usize,
    persistent: *const c_char,
    options: c_int,
    flags: c_int,
    time: *mut timeval,
    context: *mut php_stream_context,
) -> Result<*mut php_stream, Bailout> {
    if crate::in_coroutine() {
        // SAFETY: Rust's coroutine factory receives the same borrowed arguments as before.
        return unsafe { create(proto, protolen, name, persistent, context) };
    }
    // SAFETY: PHP lends exactly protolen bytes for the registry lookup.
    let protocol = unsafe { std::slice::from_raw_parts(proto.cast::<u8>(), protolen) };
    let factory = crate::transport::original(protocol);
    if factory.is_null() {
        // SAFETY: the native diagnostic used %s, so retain its complete NUL-terminated spelling.
        let spelling = unsafe { CStr::from_ptr(proto) }.to_bytes();
        let message = [b"unknown protocol '".as_slice(), spelling, b"'"].concat();
        let message = CString::new(message).unwrap_or_default();
        report(Report::Fatal, E_WARNING, &message)?;
        return Ok(ptr::null_mut());
    }
    let mut bailed = false;
    // SAFETY: the original process-lived PHP callback runs below Rust in a C Zend trap,
    // with the compiler supplying its exact STREAMS_CC debug arguments.
    let stream = unsafe {
        swoole_rs_zend::sys::swr_zend_try_stream_factory(
            factory,
            proto,
            protolen,
            name,
            namelen,
            persistent,
            options,
            flags,
            time,
            context,
            &raw mut bailed,
        )
    };
    if bailed { Err(Bailout) } else { Ok(stream) }
}

/// The transport factory's coroutine branch.
/// # Safety
/// Called in a coroutine with Zend's factory arguments.
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
    let socket = Rc::new(socket);
    let stream = alloc(socket_ops(), Rc::clone(&socket), persistent_id)?;
    if stream.is_null() {
        return Ok(stream);
    }
    if !ssl_set_options(&socket, context)? {
        drop(socket);
        // SAFETY: the stream is ours; its close callback runs below the C trap.
        if unsafe { swoole_rs_zend::sys::swr_zend_try_stream_close(stream) } != 0 {
            return Err(Bailout);
        }
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
    let result = stream_from_socket(fd, domain, kind, protocol);
    let stream = match result {
        Ok(stream) => stream,
        Err(error) => {
            error.defer();
            ptr::null_mut()
        }
    };
    swoole_rs_zend::bailout::finish();
    stream
}

fn stream_from_socket(
    fd: c_int,
    domain: c_int,
    kind: c_int,
    protocol: c_int,
) -> Result<*mut php_stream, Bailout> {
    let Some(socket) = Socket::from_fd(fd, domain, kind, protocol) else {
        return Ok(ptr::null_mut());
    };
    // SAFETY: reads a PHP global.
    let seconds = unsafe { swr_zend_stream_default_timeout() };
    if seconds > 0 {
        #[expect(clippy::cast_precision_loss, reason = "upstream's (double) cast")]
        socket.set_timeout(seconds as f64, timeout::ALL);
    }
    let stream = alloc(socket_ops(), Rc::new(socket), ptr::null())?;
    if !stream.is_null() {
        // SAFETY: the stream just made.
        unsafe { (*stream).flags |= PHP_STREAM_FLAG_AVOID_BLOCKING };
    }
    Ok(stream)
}

/// `stream_socket_pair(int $domain, int $type, int $protocol): array|false`.
/// # Safety
/// Called by Zend with a live frame and return slot.
pub(crate) unsafe extern "C" fn stream_socket_pair(
    frame: *mut swoole_rs_zend::sys::zend_execute_data,
    value: *mut zval,
) {
    // SAFETY: per the caller.
    let result = socket_pair(&unsafe { Call::new(frame, value) });
    swoole_rs_zend::bailout::raise(result);
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
        let stream = stream_from_socket(fd, domain, kind, protocol)?;
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

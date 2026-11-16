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
use crate::socket::{RecvOp, Report, SendOp, Socket, check_reactor, kind, report, x509_read};
use libc::{c_int, sockaddr_storage, socklen_t};
use std::{
    ffi::{CStr, CString},
    io::IoSlice,
    marker::PhantomData,
    mem::{self, MaybeUninit, size_of},
    ptr::{self, NonNull},
    rc::Rc,
};
use swoole_rs_corebase::{
    CCast,
    sys::{Text, errno},
};
use swoole_rs_reactor::{Caught, TimeoutKind};
use swoole_rs_zend::{
    ArrayMut, Call, E_WARNING, Message, Owned, OwnedArray, StreamContext, Value,
    bailout::Bailout,
    message, string,
    sys::{
        PHP_STREAM_FLAG_AVOID_BLOCKING, php_stream, php_stream_xport_crypto_param,
        php_stream_xport_param, sockaddr, timeval, zend_long, zend_string,
    },
    value::empty_string,
};

unsafe extern "C" {
    /// `EG(active)`: a read of this PHP thread's executor, which runs no PHP code.
    safe fn swr_zend_stream_vm_active() -> bool;
    /// `FG(default_socket_timeout)`: a read of this PHP thread's file globals,
    /// which runs no PHP code.
    safe fn swr_zend_stream_default_timeout() -> zend_long;
    /// `stream->has_buffered_data` (false before PHP 8.3): a read of the
    /// stream's own flag.
    safe fn swr_zend_stream_buffered(stream: &php_stream) -> bool;
    /// `PHP_STREAM_CONTEXT(stream)`: the context the stream's resource names,
    /// which it keeps while the stream lives; null (`None`) for none. A read
    /// of the stream's own fields.
    safe fn swr_zend_stream_context<'a>(stream: Stream<'a>) -> Option<StreamContext<'a>>;
    /// `php_stream_to_zval`: `value` holds the fresh stream's resource, taking
    /// over the one reference nothing else holds, in place of what it held,
    /// which is not released; the stream is marked exposed.
    safe fn swr_zend_stream_to_zval(stream: Fresh, value: &Owned);
    /// `fdopen(3)`: a `FILE` over `fd` opened with `mode`, or null.
    safe fn fdopen(fd: c_int, mode: Text<'_>) -> *mut libc::FILE;
    /// `php_stream_context_get_option`: the option `name` that the stream's
    /// context holds for `wrapper`; null (`None`) without a context, without
    /// options for `wrapper`, or where either level is no array. The option
    /// lives until PHP code changes the context, which none does while the
    /// stream is borrowed.
    safe fn swr_zend_stream_option<'a>(
        stream: &'a php_stream,
        wrapper: Text<'_>,
        name: Text<'_>,
    ) -> Option<&'a Value>;
    /// `client->ctx = server->ctx`, with a reference of its own when there is
    /// one. What `client` named before is not released.
    safe fn swr_zend_stream_share_context(client: &mut php_stream, server: &php_stream);
    /// `php_stream_notify_progress_increment` of the stream's context, which has
    /// none or a notifier, inside `zend_try`: the notifier runs PHP code. 1 if it
    /// bailed out.
    safe fn swr_zend_stream_progress(stream: Stream<'_>, bytes: usize) -> c_int;
    /// The context's option `name` for `wrapper` becomes a copy of `value`, with a
    /// reference of its own; the option it replaces is released.
    safe fn php_stream_context_set_option(
        context: StreamContext<'_>,
        wrapper: Text<'_>,
        name: Text<'_>,
        value: &Value,
    );
    /// `php_stream_close` of a stream no zval holds yet, inside `zend_try`: its
    /// close operation may run PHP code. 1 if it bailed out.
    safe fn swr_zend_try_stream_close(stream: Fresh) -> c_int;
    /// `php_stream_alloc` of `swoole_runtime_socket_ops`, mode "r+", with no
    /// abstract yet, inside `zend_try`: the new stream in `stream` (null if it
    /// failed), else 1 if it bailed out. No operation runs before the abstract
    /// is set, and each reads a null one as closed.
    safe fn swr_zend_try_socket_stream_alloc(
        persistent: Option<Text<'_>>,
        stream: &mut Option<Fresh>,
    ) -> c_int;
    /// `php_network_populate_name_from_sockaddr` of the first `size` bytes of
    /// `peer`, at most all of it, into the slots given, as
    /// [`php_network_get_sock_name`] writes them.
    safe fn swr_zend_stream_populate_name(
        peer: &sockaddr_storage,
        size: socklen_t,
        textaddr: Option<&mut *mut zend_string>,
        addr: Option<&mut *mut sockaddr>,
        addrlen: &mut socklen_t,
    );
    /// `getsockname` of `sock`, then the name in each slot given: a new string
    /// in `textaddr` (left alone for a family it can't print), and in `addr` a
    /// new copy of the address with its length in `addrlen`, which is left alone
    /// without `addr`. A slot is written without reading what it held. It
    /// reports no error and runs no PHP code.
    safe fn php_network_get_sock_name(
        sock: c_int,
        textaddr: Option<&mut *mut zend_string>,
        addr: Option<&mut *mut sockaddr>,
        addrlen: &mut socklen_t,
    ) -> c_int;
    /// [`php_network_get_sock_name`] with `getpeername`.
    safe fn php_network_get_peer_name(
        sock: c_int,
        textaddr: Option<&mut *mut zend_string>,
        addr: Option<&mut *mut sockaddr>,
        addrlen: &mut socklen_t,
    ) -> c_int;
}

const SUCCESS: c_int = 0;
const FAILURE: c_int = -1;

/// The `PHP_STREAM_OPTION_*` that take no parameter.
mod option {
    pub(super) const BLOCKING: i32 = 1;
    pub(super) const CHECK_LIVENESS: i32 = 12;
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

/// Report a trapped bailout through `flag`; `failed` stands in for the result.
fn reraise<T: Copy>(flag: &mut bool, result: Result<T, Bailout>, failed: T) -> T {
    let (value, status) = match result {
        Ok(value) => (value, Ok(())),
        Err(error) => (failed, Err(error)),
    };
    if swoole_rs_zend::bailout::relay_to_c(status) {
        *flag = true;
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

/// A stream of zend-sys's `swoole_runtime_socket_ops`: one its C entries lend an
/// operation below for the call, or one [`alloc`] just made. Only those make one,
/// so its `abstract_` is null or its [`NetStream`], which it owns.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Stream<'a>(NonNull<php_stream>, PhantomData<&'a mut php_stream>);

impl<'a> Stream<'a> {
    fn as_ptr(self) -> *mut php_stream {
        self.0.as_ptr()
    }

    /// `f` of the stream's own fields, for an immediate access that runs no PHP.
    fn fields<T>(self, f: impl FnOnce(&mut php_stream) -> T) -> T {
        // SAFETY: a live stream (see `Stream`), and nothing else reaches it while
        // `f`, which runs no PHP, holds the reference.
        f(unsafe { &mut *self.as_ptr() })
    }

    /// The stream's `NetStream`, none once closed. The stream owns it until its
    /// close, which no borrow outlives: each is taken for one operation.
    fn net<'n>(self) -> Option<&'n NetStream> {
        let net = self.fields(|stream| stream.abstract_.cast::<NetStream>());
        // SAFETY: null or the stream's live NetStream (see `Stream`).
        unsafe { net.as_ref() }
    }

    /// `PHP_STREAM_CONTEXT(stream)`: the stream's context, if it has one.
    fn context(self) -> Option<StreamContext<'a>> {
        swr_zend_stream_context(self)
    }
}

/// A stream [`alloc`] just made, whose resource's one reference no zval holds
/// yet: given to PHP once, by value.
#[repr(transparent)]
struct Fresh(Stream<'static>);

impl Fresh {
    fn stream(&self) -> Stream<'_> {
        self.0
    }

    /// The stream, for C code that takes its resource's reference.
    fn into_ptr(self) -> *mut php_stream {
        self.0.as_ptr()
    }
}

/// `php_stream_alloc(ops, abstract, persistent_id, "r+")` of `socket` with these
/// operations; none (the socket freed) if it fails.
fn alloc(socket: Rc<Socket>, persistent_id: Option<Text<'_>>) -> Result<Option<Fresh>, Bailout> {
    let mut stream: Option<Fresh> = None;
    if swr_zend_try_socket_stream_alloc(persistent_id, &mut stream) != 0 {
        return Err(Bailout);
    }
    if let Some(fresh) = &stream {
        // The stream owns its NetStream from here until its close takes it back.
        let net = Box::into_raw(Box::new(NetStream {
            socket,
            blocking: std::cell::Cell::new(true),
        }));
        fresh
            .stream()
            .fields(|stream| stream.abstract_ = net.cast());
    }
    Ok(stream)
}

/// `php_stream_notify_progress_increment` of `bytes`.
fn progress(stream: Stream<'_>, bytes: isize) -> Result<(), Bailout> {
    if swr_zend_stream_progress(stream, bytes.unsigned_abs()) != 0 {
        return Err(Bailout);
    }
    Ok(())
}

fn set_eof(stream: Stream<'_>) {
    stream.fields(|stream| stream.set_eof(1));
}

/// The seconds of a `timeval`, as Swoole's `set_timeout(timeval *)`.
fn seconds(time: &timeval) -> f64 {
    swoole_rs_hooks::timeval(time.tv_sec.c_cast::<f64>(), time.tv_usec.c_cast::<f64>())
}

/// A copy of the stream's context option, if set.
fn context_option(stream: Stream<'_>, wrapper: &CStr, name: &CStr) -> Option<Owned> {
    // The context's live option is copied before any PHP runs.
    stream.fields(|stream| {
        swr_zend_stream_option(stream, wrapper.into(), name.into()).map(Value::copy)
    })
}

/// `zval_is_true` of a context option, false when unset.
fn option_true(stream: Stream<'_>, wrapper: &CStr, name: &CStr) -> bool {
    context_option(stream, wrapper, name).is_some_and(|value| value.truthy())
}

/// Bytes up to the first NUL, as C reads a string.
fn c_str(bytes: &[u8]) -> &[u8] {
    bytes.split(|&b| b == 0).next().unwrap_or_default()
}

/// `parse_ip_address_ex`: host and port of `name`, else the error text when
/// `param` wants it.
fn parse_address(name: &[u8], param: &mut php_stream_xport_param) -> Option<(CString, i32)> {
    use swoole_rs_hooks::address::{AddressError, parse};
    match parse(name) {
        Ok((host, port)) => Some((CString::new(c_str(host)).unwrap_or_default(), port)),
        Err(error) => {
            let what = match error {
                AddressError::Ipv6 => "IPv6 address",
                AddressError::Address => "address",
            };
            let name = String::from_utf8_lossy(c_str(name));
            set_error(
                param,
                format!("Failed to parse {what} \"{name}\"").as_bytes(),
            );
            None
        }
    }
}

/// A copy of `text` in the transport parameter's error text, when it wants one.
fn set_error(param: &mut php_stream_xport_param, text: &[u8]) {
    if param.want_errortext() != 0 {
        param.outputs.error_text = string::copy(text);
    }
}

/// `EG(active)`: whether the executor is running, so a coroutine can wait.
fn vm_active() -> bool {
    swr_zend_stream_vm_active()
}

/// The bytes a read fills, as the `iovec` zend-sys's operations lend: writable
/// for the call, reached through nothing else meanwhile, maybe not written yet,
/// and never null. Only C makes one.
#[repr(transparent)]
struct Lent<'a>(libc::iovec, PhantomData<&'a mut [MaybeUninit<u8>]>);

impl Lent<'_> {
    fn slots(&mut self) -> &mut [MaybeUninit<u8>] {
        let Self(lent, _) = self;
        // SAFETY: `iov_len` writable bytes at a non-null `iov_base`, lent to this
        // one owner (see `Lent`), which the borrow keeps.
        unsafe { std::slice::from_raw_parts_mut(lent.iov_base.cast(), lent.iov_len) }
    }
}

/// The stream's `write` (`socket_write`): bytes written, or -1.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_phpruntime_socket_write(stream: Stream<'_>, data: IoSlice<'_>) -> isize {
    raise(write(stream, &data), -1)
}

fn write(stream: Stream<'_>, data: &[u8]) -> Result<isize, Bailout> {
    let Some(net) = stream.net() else {
        return Ok(-1);
    };
    let (socket, blocking) = (Rc::clone(&net.socket), net.blocking.get());
    let count = data.len();
    if !vm_active() {
        return Ok(socket
            .send(SendOp::Sync, data)
            .map_or(-1, usize::cast_signed));
    }
    let mut written = if blocking {
        socket
            .send(SendOp::All, data)
            .map_or(-1, usize::cast_signed)
    } else {
        let written = socket
            .send(SendOp::Once, data)
            .map_or(-1, usize::cast_signed);
        socket.set_error(errno());
        written
    };
    if written < 0 || written.unsigned_abs() != count {
        // the caller does not loop on send, and a partial count is meaningless
        written = -1;
        let code = socket.error();
        if code != libc::ETIMEDOUT {
            if socket.catch_error(code) == Some(Caught::Wait) {
                written = 0;
            } else {
                let reason = socket.message().unwrap_or_default();
                let reason = reason.to_string_lossy();
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

/// The stream's `read` (`socket_read`): bytes read, or -1.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_phpruntime_socket_read(stream: Stream<'_>, mut out: Lent<'_>) -> isize {
    raise(read(stream, out.slots()), -1)
}

fn read(stream: Stream<'_>, out: &mut [MaybeUninit<u8>]) -> Result<isize, Bailout> {
    let Some(net) = stream.net() else {
        return Ok(-1);
    };
    let (socket, blocking) = (Rc::clone(&net.socket), net.blocking.get());
    if !vm_active() {
        return Ok(socket
            .recv(RecvOp::Sync, out)
            .map_or(-1, usize::cast_signed));
    }
    let mut read = if blocking && !stream.fields(|stream| swr_zend_stream_buffered(stream)) {
        socket
            .recv(RecvOp::Recv, out)
            .map_or(-1, usize::cast_signed)
    } else {
        let read = socket
            .recv(RecvOp::Once, out)
            .map_or(-1, usize::cast_signed);
        socket.set_error(errno());
        read
    };
    if read > 0 {
        progress(stream, read)?;
    }
    if read < 0 && socket.error() != libc::ETIMEDOUT {
        if socket.catch_error(socket.error()) == Some(Caught::Wait) {
            read = 0;
        } else {
            set_eof(stream);
        }
    } else if read == 0 {
        set_eof(stream);
    }
    Ok(read)
}

/// The stream's `close`: the socket closes now and is freed once no
/// operation holds it. Whether it closed.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_phpruntime_socket_close(stream: Stream<'_>) -> bool {
    // Detach first, so reentrant PHP cannot reach this stream owner again.
    let net: *mut NetStream = stream
        .fields(|stream| mem::replace(&mut stream.abstract_, ptr::null_mut()))
        .cast();
    let closed = !net.is_null() && {
        // SAFETY: the stream's own NetStream (see `Stream`), detached above, so
        // this takes its unique owner back.
        let net = unsafe { Box::from_raw(net) };
        let result = net.socket.close();
        drop(net);
        result.map_err(Bailout::defer).is_ok()
    };
    // This is a native callback boundary: the Box/Rc owners have already dropped.
    swoole_rs_zend::bailout::finish();
    closed
}

/// The stream's `cast`, whose slot C gives by `castas`: for `PHP_STREAM_AS_STDIO`
/// in `file` a `FILE *` over the fd, else in `fd` the fd; none when the caller
/// only asks whether it can. Whether it could.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_phpruntime_socket_cast(
    stream: Stream<'_>,
    castas: c_int,
    file: Option<&mut *mut libc::FILE>,
    fd: Option<&mut c_int>,
) -> bool {
    let Some(net) = stream.net() else {
        return false;
    };
    let socket = net.socket.fd();
    match castas {
        AS_STDIO => {
            let Some(file) = file else {
                return true;
            };
            let mode = stream.fields(|stream| stream.mode.map(|c| c.to_ne_bytes()[0]));
            let Ok(mode) = CStr::from_bytes_until_nul(&mode) else {
                return false;
            };
            *file = fdopen(socket, mode.into());
            !file.is_null()
        }
        AS_FD_FOR_SELECT | AS_FD | AS_SOCKETD => {
            if let Some(fd) = fd {
                *fd = socket;
            }
            true
        }
        _ => false,
    }
}

/// The stream's `stat`: `fstat` of the fd into `sb`; whether it could.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_phpruntime_socket_stat(stream: Stream<'_>, sb: &mut libc::stat) -> bool {
    let Some(net) = stream.net() else {
        return false;
    };
    swoole_rs_corebase::sys::fstat(net.socket.fd())
        .map(|stat| *sb = stat)
        .is_some()
}

/// `PHP_STREAM_OPTION_XPORT_API`'s parameter with the inputs it points to, as
/// zend-sys's `set_option` lends them. Each transport operation zeroes the
/// parameter before it sets the inputs it uses.
struct Xport<'a> {
    param: &'a mut php_stream_xport_param,
    /// `inputs.timeout`, if set.
    timeout: Option<&'a timeval>,
    /// `inputs.name`'s `namelen` bytes, if set.
    name: Option<&'a [u8]>,
    /// `inputs.buf`'s `buflen` bytes, for `STREAM_XPORT_OP_SEND`.
    send: Option<&'a [u8]>,
    /// `inputs.buf`'s `buflen` bytes, for `STREAM_XPORT_OP_RECV`.
    recv: Option<&'a mut Lent<'a>>,
}

/// The stream's `set_option` of `PHP_STREAM_OPTION_XPORT_API`, its parameter's
/// pointers lent typed. Whether it is `PHP_STREAM_OPTION_RETURN_OK`.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_phpruntime_socket_xport<'a>(
    stream: Stream<'_>,
    param: &'a mut php_stream_xport_param,
    timeout: Option<&'a timeval>,
    name: Option<&'a IoSlice<'a>>,
    send: Option<&'a IoSlice<'a>>,
    recv: Option<&'a mut Lent<'a>>,
) -> bool {
    let xport = Xport {
        param,
        timeout,
        name: name.map(|name| &**name),
        send: send.map(|send| &**send),
        recv,
    };
    let result = stream.net().map_or(Ok(false), |net| {
        let socket = Rc::clone(&net.socket);
        xport_api(stream, &socket, xport).map(|()| true)
    });
    raise(result, false)
}

/// The stream's `set_option` of any other option, with its parameter for
/// `PHP_STREAM_OPTION_META_DATA_API` (`data`), `_READ_TIMEOUT` (`timeout`) and
/// `_CRYPTO_API` (`crypto`). Whether it is `PHP_STREAM_OPTION_RETURN_OK`.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_phpruntime_socket_option(
    stream: Stream<'_>,
    option: c_int,
    value: c_int,
    data: Option<&mut ArrayMut>,
    timeout: Option<&timeval>,
    crypto: Option<&mut php_stream_xport_crypto_param>,
) -> bool {
    let Some(net) = stream.net() else {
        return false;
    };
    if option == option::BLOCKING {
        net.blocking.set(value != 0);
        return true;
    }
    let socket = Rc::clone(&net.socket);
    if option == option::CHECK_LIVENESS {
        return socket.check_liveness();
    }
    if let Some(data) = data {
        meta_data(stream, &socket, data);
    }
    if let Some(time) = timeout {
        // PHP socket streams share this timeout between reads and writes
        socket.set_timeout(seconds(time), TimeoutKind::RDWR);
    }
    let Some(param) = crypto else {
        return true;
    };
    let returncode = match param.op {
        0 => Ok(0),
        1 => {
            let activate = param.inputs.activate != 0;
            enable_crypto(stream, &socket, activate).map(|enabled| if enabled { 1 } else { -1 })
        }
        _ => return true,
    };
    let Some(returncode) = raise(returncode.map(Some), None) else {
        return false;
    };
    param.outputs.returncode = returncode;
    true
}

/// `PHP_STREAM_OPTION_META_DATA_API`: the TLS session, `timed_out`, `eof`
/// and `blocked`.
fn meta_data(stream: Stream<'_>, socket: &Socket, data: &mut ArrayMut) {
    if let Some(session) = socket.ssl_session() {
        let protocol: &[u8] = match session.version {
            0x304 => b"TLSv1.3",
            0x303 => b"TLSv1.2",
            0x302 => b"TLSv1.1",
            0x301 => b"TLSv1",
            0x300 => b"SSLv3",
            _ => b"UNKNOWN",
        };
        let mut crypto = OwnedArray::default();
        crypto.insert_bytes(b"protocol", Some(protocol));
        crypto.insert_bytes(b"cipher_name", Some(session.cipher_name.to_bytes()));
        crypto.insert_long(b"cipher_bits", session.cipher_bits.into());
        crypto.insert_bytes(b"cipher_version", Some(session.cipher_version.to_bytes()));
        data.insert(b"crypto", crypto.into_owned());
    }
    data.insert_bool(b"timed_out", socket.error() == libc::ETIMEDOUT);
    data.insert_bool(b"eof", stream.fields(|stream| stream.eof()) != 0);
    data.insert_bool(b"blocked", true);
}

/// `PHP_STREAM_OPTION_XPORT_API`.
fn xport_api(stream: Stream<'_>, socket: &Socket, xport: Xport<'_>) -> Result<(), Bailout> {
    let Xport {
        param: p,
        timeout,
        name,
        send,
        recv,
    } = xport;
    match p.op {
        xport::LISTEN => {
            p.outputs.returncode = status(socket.listen(p.inputs.backlog));
        }
        xport::CONNECT | xport::CONNECT_ASYNC => {
            let connected = connect(stream, socket, p, timeout, name)
                && (!socket.ssl_enabled() || enable_crypto(stream, socket, true)?);
            p.outputs.returncode = status(connected);
        }
        xport::BIND => {
            if socket.domain() != libc::AF_UNIX {
                if stream.context().is_none() {
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
            p.outputs.returncode = status(bind(socket, p, name));
        }
        xport::ACCEPT => {
            p.outputs.returncode = status(accept(stream, socket, p, timeout)?);
        }
        xport::GET_NAME => {
            let mut unwanted = 0;
            let (textaddr, addr, addrlen) = name_slots(p, &mut unwanted);
            let code = php_network_get_sock_name(socket.fd(), textaddr, addr, addrlen);
            p.outputs.returncode = code;
        }
        xport::GET_PEER_NAME => {
            let mut unwanted = 0;
            let (textaddr, addr, addrlen) = name_slots(p, &mut unwanted);
            let code = php_network_get_peer_name(socket.fd(), textaddr, addr, addrlen);
            p.outputs.returncode = code;
        }
        xport::SEND => {
            if p.inputs.flags & STREAM_OOB == STREAM_OOB {
                return oob(p);
            }
            let data = send.unwrap_or_default();
            let sent = if p.inputs.addr.is_null() {
                socket
                    .send(SendOp::Send, data)
                    .map_or(-1, usize::cast_signed)
            } else {
                // SAFETY: `data` is readable for its length, and Zend lends
                // `addrlen` bytes at `addr` for the call.
                unsafe {
                    libc::sendto(
                        socket.fd(),
                        data.as_ptr().cast(),
                        data.len(),
                        0,
                        p.inputs.addr.cast(),
                        p.inputs.addrlen,
                    )
                }
            };
            p.outputs.returncode = returncode(sent);
            if p.outputs.returncode == -1 {
                // `php_socket_strerror`: `strerror` of the code.
                let mut message = Message::new();
                message.push(swoole_rs_corebase::sys::strerror(errno()).as_bytes());
                message.push(b"\n");
                report(Report::Docref, E_WARNING, message.as_cstr())?;
            }
        }
        xport::RECV => {
            if p.inputs.flags & STREAM_OOB == STREAM_OOB {
                return oob(p);
            }
            let mut none = [];
            let buffer = recv.map_or(&mut none[..], Lent::slots);
            p.outputs.returncode = if p.inputs.flags & STREAM_PEEK == STREAM_PEEK {
                returncode(
                    socket
                        .recv(RecvOp::Peek, buffer)
                        .map_or(-1, usize::cast_signed),
                )
            } else {
                recvfrom(socket, p, buffer)
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

fn returncode(bytes: isize) -> c_int {
    bytes.c_cast::<c_int>()
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

/// `socket_connect`: whether it connected; false with the error.
fn connect(
    stream: Stream<'_>,
    socket: &Socket,
    param: &mut php_stream_xport_param,
    time: Option<&timeval>,
    name: Option<&[u8]>,
) -> bool {
    if socket.fd() < 0 {
        return false;
    }
    let inet = socket.is_inet();
    let parsed = target(inet, name, param);
    if inet && socket.socket_type() == libc::SOCK_STREAM {
        socket.set_nodelay(1);
    }
    let Some((host, port)) = parsed else {
        return false;
    };
    if let Some(bindto) = context_option(stream, c"socket", c"bindto") {
        let Some(name) = bindto.bytes() else {
            set_error(param, b"local_addr context option is not a string.");
            return false;
        };
        let Some((local, local_port)) = parse_address(name, param) else {
            return false;
        };
        let local = match local.as_bytes() {
            b"0" | b"" => c"0.0.0.0",
            _ => local.as_c_str(),
        };
        if !socket.bind(local, local_port) {
            return false;
        }
    }
    if let Some(time) = time {
        socket.set_timeout(seconds(time), TimeoutKind::CONNECT);
    }
    if !socket.connect(&host, port) {
        param.outputs.error_code = socket.error();
        if let Some(message) = socket.message() {
            param.outputs.error_text = string::copy(message.to_bytes());
        }
        return false;
    }
    true
}

/// The transport parameter's name: on an `inet` socket the host and port it
/// parses to (else none, with the error text), on another the path up to its NUL.
fn target(
    inet: bool,
    name: Option<&[u8]>,
    param: &mut php_stream_xport_param,
) -> Option<(CString, i32)> {
    if inet {
        parse_address(name.unwrap_or_default(), param)
    } else {
        name.map(|name| (CString::new(c_str(name)).unwrap_or_default(), 0))
    }
}

/// `socket_bind`: whether it bound.
fn bind(socket: &Socket, param: &mut php_stream_xport_param, name: Option<&[u8]>) -> bool {
    let Some((host, port)) = target(socket.is_inet(), name, param) else {
        socket.set_error(libc::EINVAL);
        return false;
    };
    socket.bind(&host, port)
}

/// `SUCCESS` or `FAILURE`.
fn status(done: bool) -> c_int {
    if done { SUCCESS } else { FAILURE }
}

/// `socket_accept`: `SUCCESS` with the client stream, or `FAILURE`.
fn accept(
    stream: Stream<'_>,
    socket: &Socket,
    param: &mut php_stream_xport_param,
    time: Option<&timeval>,
) -> Result<bool, Bailout> {
    param.outputs.client = ptr::null_mut();
    let nodelay = option_true(stream, c"socket", c"tcp_nodelay");
    if let Some(time) = time {
        socket.set_timeout(seconds(time), TimeoutKind::READ);
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
        if param.want_errortext() != 0 {
            // `php_socket_error_str`: `strerror` of the code.
            param.outputs.error_text =
                string::copy(swoole_rs_corebase::sys::strerror(code).as_bytes());
        }
        return Ok(false);
    };
    // The peer's name, as upstream reports it: from an empty address.
    let peer: sockaddr_storage = swoole_rs_corebase::sys::zeroed();
    populate_name(param, &peer, STORAGE_SIZE);
    if nodelay {
        client.set_nodelay(1);
    }
    let connection = alloc(Rc::new(client), None)?;
    if let Some(connection) = &connection {
        // The new stream, which names no context yet, counts its reference.
        connection.stream().fields(|client| {
            stream.fields(|server| {
                swr_zend_stream_share_context(client, server);
            });
        });
    }
    param.outputs.client = connection.map_or(ptr::null_mut(), Fresh::into_ptr);
    Ok(true)
}

/// The output slots of the names the transport parameter wants: `textaddr` and
/// `addr`, each none when it wants none, and `addrlen`, which is `unwanted` when
/// it wants no `addr`.
fn name_slots<'a>(
    param: &'a mut php_stream_xport_param,
    unwanted: &'a mut socklen_t,
) -> (
    Option<&'a mut *mut zend_string>,
    Option<&'a mut *mut sockaddr>,
    &'a mut socklen_t,
) {
    let (want_textaddr, want_addr) = (param.want_textaddr() != 0, param.want_addr() != 0);
    let outputs = &mut param.outputs;
    let textaddr = want_textaddr.then_some(&mut outputs.textaddr);
    if want_addr {
        (textaddr, Some(&mut outputs.addr), &mut outputs.addrlen)
    } else {
        (textaddr, None, unwanted)
    }
}

/// `sizeof(sockaddr_storage)`.
#[expect(clippy::cast_possible_truncation, reason = "sizeof(sockaddr_storage)")]
const STORAGE_SIZE: socklen_t = size_of::<sockaddr_storage>() as socklen_t;

/// `php_network_populate_name_from_sockaddr` of the first `size` bytes of `peer`
/// (at most all of it) into the names the transport parameter wants.
fn populate_name(param: &mut php_stream_xport_param, peer: &sockaddr_storage, size: socklen_t) {
    let mut unwanted = 0;
    let (textaddr, addr, addrlen) = name_slots(param, &mut unwanted);
    swr_zend_stream_populate_name(peer, size, textaddr, addr, addrlen);
}

/// `socket_recvfrom` into `buffer`, the receive buffer Zend lends with `param`.
fn recvfrom(
    socket: &Socket,
    param: &mut php_stream_xport_param,
    buffer: &mut [MaybeUninit<u8>],
) -> c_int {
    let (want_textaddr, want_addr) = (param.want_textaddr() != 0, param.want_addr() != 0);
    if !want_textaddr && !want_addr {
        return returncode(
            socket
                .recv(RecvOp::Recv, buffer)
                .map_or(-1, usize::cast_signed),
        );
    }
    let mut peer: sockaddr_storage = swoole_rs_corebase::sys::zeroed();
    let (received, size) = socket.recvfrom(buffer, &mut peer);
    if size != 0 {
        populate_name(param, &peer, size);
    } else {
        if want_textaddr {
            // The empty string is interned.
            param.outputs.textaddr = empty_string();
        }
        if want_addr {
            param.outputs.addr = ptr::null_mut();
            param.outputs.addrlen = 0;
        }
    }
    returncode(received)
}

/// `socket_enable_crypto`: whether it succeeded (upstream's 1, else -1).
fn enable_crypto(stream: Stream<'_>, socket: &Socket, activate: bool) -> Result<bool, Bailout> {
    let context = stream.context();
    if activate && !socket.ssl_available() {
        socket.enable_ssl();
        if !ssl_set_options(socket, context)? || !socket.ssl_handshake() {
            return Ok(false);
        }
    } else if !activate && socket.ssl_available() && socket.has_ssl() {
        socket.ssl_close();
        return Ok(false);
    }
    if context.is_some()
        && socket.ssl_available()
        && option_true(stream, c"ssl", c"capture_peer_cert")
        && !capture_peer_certs(stream, socket)?
    {
        return Ok(false);
    }
    Ok(true)
}

/// `php_openssl_capture_peer_certs`: the `peer_certificate` (and with
/// `capture_peer_cert_chain` the `peer_certificate_chain`) context options;
/// false without a peer certificate.
fn capture_peer_certs(stream: Stream<'_>, socket: &Socket) -> Result<bool, Bailout> {
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

/// Set the `ssl` option `name` of the stream's context, if it has one, to `value`.
fn set_context_option(stream: Stream<'_>, name: &CStr, value: &Value) {
    if let Some(context) = stream.context() {
        php_stream_context_set_option(context, c"ssl".into(), name.into(), value);
    }
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
fn ssl_set_options(socket: &Socket, context: Option<StreamContext<'_>>) -> Result<bool, Bailout> {
    // `php_swoole_array_get_value_ex`: present and not null.
    let ssl = context
        .and_then(StreamContext::options)
        .and_then(|options| options.get(b"ssl")?.array());
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

/// [`transport_factory_body`]'s type, as zend-sys's `swr_zend_stream_factory`
/// calls it with `php_stream_xport_create`'s arguments: the protocol's
/// `protolen` bytes start a NUL-terminated string (the whole resource name),
/// the `namelen`-byte name is NUL-terminated, the persistent id is a string or
/// null, the timeout and context are null or live, and the flag is its own.
pub(crate) type FactoryBody = extern "C" fn(
    proto: Text<'_>,
    protolen: usize,
    name: Text<'_>,
    namelen: usize,
    persistent: Option<Text<'_>>,
    options: c_int,
    flags: c_int,
    time: Option<&mut timeval>,
    context: Option<StreamContext<'_>>,
    bailed: &mut bool,
) -> *mut php_stream;

/// The complete factory: PHP's original transport outside a coroutine, otherwise
/// Rust's coroutine socket construction and stream ownership. zend-sys's
/// `swr_zend_stream_factory` is the registered factory and raises the bailout.
pub(crate) extern "C" fn transport_factory_body(
    proto: Text<'_>,
    protolen: usize,
    name: Text<'_>,
    namelen: usize,
    persistent: Option<Text<'_>>,
    options: c_int,
    flags: c_int,
    time: Option<&mut timeval>,
    context: Option<StreamContext<'_>>,
    flag: &mut bool,
) -> *mut php_stream {
    let time = time.map_or(ptr::null_mut(), ptr::from_mut);
    let spelling = proto.get().to_bytes();
    let protocol = spelling.get(..protolen).unwrap_or(spelling);
    let factory = || {
        if crate::in_coroutine() {
            // The coroutine branch: Rust's coroutine socket and its stream.
            let proto = protocol.strip_prefix(b"async.").unwrap_or(protocol);
            let tcp = if name.get().to_bytes().first() == Some(&b'[') {
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
            let Some(stream) = alloc(Rc::clone(&socket), persistent)? else {
                return Ok(ptr::null_mut());
            };
            if !ssl_set_options(&socket, context)? {
                drop(socket);
                if swr_zend_try_stream_close(stream) != 0 {
                    return Err(Bailout);
                }
                return Ok(ptr::null_mut());
            }
            return Ok(stream.into_ptr());
        }
        let Some(factory) = crate::transport::original(protocol) else {
            // The native diagnostic used %s, so retain its complete NUL-terminated spelling.
            let message = [b"unknown protocol '".as_slice(), spelling, b"'"].concat();
            let message = CString::new(message).unwrap_or_default();
            report(Report::Fatal, E_WARNING, &message)?;
            return Ok(ptr::null_mut());
        };
        let mut bailed = false;
        // SAFETY: the original process-lived PHP callback runs below Rust in a C Zend trap,
        // with the compiler supplying its exact STREAMS_CC debug arguments.
        let stream = unsafe {
            swoole_rs_zend::sys::swr_zend_try_stream_factory(
                factory.as_ptr(),
                proto.get().as_ptr(),
                protolen,
                name.get().as_ptr(),
                namelen,
                persistent.map_or(ptr::null(), |id| id.get().as_ptr()),
                options,
                flags,
                time,
                context.map_or(ptr::null_mut(), StreamContext::as_ptr),
                &raw mut bailed,
            )
        };
        if bailed { Err(Bailout) } else { Ok(stream) }
    };
    reraise(flag, factory(), ptr::null_mut())
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
        Ok(stream) => stream.map_or(ptr::null_mut(), Fresh::into_ptr),
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
) -> Result<Option<Fresh>, Bailout> {
    let Some(socket) = Socket::from_fd(fd, domain, kind, protocol) else {
        return Ok(None);
    };
    let seconds = swr_zend_stream_default_timeout();
    if seconds > 0 {
        socket.set_timeout(seconds.c_cast::<f64>(), TimeoutKind::ALL);
    }
    let stream = alloc(Rc::new(socket), None)?;
    if let Some(stream) = &stream {
        stream.stream().fields(|stream| {
            stream.flags |= PHP_STREAM_FLAG_AVOID_BLOCKING;
        });
    }
    Ok(stream)
}

swoole_rs_zend::handlers! {
    zif_swoole_stream_socket_pair => stream_socket_pair;
}

/// `stream_socket_pair(int $domain, int $type, int $protocol): array|false`.
fn stream_socket_pair(call: &Call) {
    swoole_rs_zend::bailout::defer(socket_pair(call));
}

fn socket_pair(call: &Call) -> Result<(), Bailout> {
    let parsed = call
        .arity(3, 3)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, 0)?, call.long(3, 0)?)));
    let Ok((domain, kind, protocol)) = parsed else {
        call.set_bool(false);
        return Ok(());
    };
    let (domain, kind, protocol) = (
        domain.c_cast::<c_int>(),
        kind.c_cast::<c_int>(),
        protocol.c_cast::<c_int>(),
    );
    let Some(pair) = swoole_rs_corebase::sys::socketpair(domain, kind, protocol) else {
        let code = errno();
        let reason = swoole_rs_corebase::sys::strerror(code);
        let message = message!("failed to create sockets: [{code}]: {reason}");
        report(Report::Swoole, E_WARNING, message.as_cstr())?;
        call.set_bool(false);
        return Ok(());
    };
    check_reactor();
    let mut streams = OwnedArray::default();
    for fd in pair {
        if let Some(stream) = stream_from_socket(fd, domain, kind, protocol)? {
            let resource = Owned::default();
            swr_zend_stream_to_zval(stream, &resource);
            streams.push(resource);
        }
    }
    call.set_owned(streams.into_owned());
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

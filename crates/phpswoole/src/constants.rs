//! The global constants `PHP_MINIT_FUNCTION(swoole)` registered, in its order
//! (`get_defined_constants()` and reflection list them so). Values are the
//! pinned headers' enums and macros; `flags` stands for the `#ifdef`s.
use std::ffi::CStr;
use swoole_rs_corebase::constants::SocketType;
use swoole_rs_corebase::tls::{SslMethod, SslVersion};
use swoole_rs_error::ErrorCode;
use swoole_rs_zend::Minit;

/// `swr_v1_phpswoole_minit` flags: which `#ifdef`s the build has.
pub(crate) const DEBUG: u32 = 1 << 0;
pub(crate) const SHORTNAME: u32 = 1 << 1;
pub(crate) const COMPRESSION: u32 = 1 << 2;
pub(crate) const ZLIB: u32 = 1 << 3;
const BROTLI: u32 = 1 << 4;
const TLS1_1: u32 = 1 << 5;
const TLS1_2: u32 = 1 << 6;
const TLS1_3: u32 = 1 << 7;
const DTLS: u32 = 1 << 8;
const SSL3: u32 = 1 << 9;
pub(crate) const IOURING: u32 = 1 << 10;
pub(crate) const THREAD: u32 = 1 << 12;

/// The flags of the build's switches; MINIT adds [`SHORTNAME`] from the ini.
pub(crate) fn flags() -> u32 {
    use swoole_rs_corebase::{Switch as S, built_with};
    [
        (built_with(S::Debug), DEBUG),
        (crate::codecs() != 0, COMPRESSION),
        (built_with(S::Zlib), ZLIB),
        (built_with(S::Brotli), BROTLI),
        (built_with(S::Tls1_1), TLS1_1),
        (built_with(S::Tls1_2), TLS1_2),
        (built_with(S::Tls1_3), TLS1_3),
        (built_with(S::Dtls), DTLS),
        (built_with(S::Ssl3), SSL3),
        (built_with(S::IoUring), IOURING),
        // stdext is a cargo feature here, not a switch: `typed_array`'s alias needs it.
        (cfg!(feature = "stdext"), crate::table::STDEXT),
        (built_with(S::Thread), THREAD),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .fold(0, |flags, (_, bit)| flags | bit)
}

/// `SWOOLE_VERSION`.
pub(crate) const VERSION: &CStr = c"6.2.6";

#[derive(Clone, Copy)]
enum Value {
    Long(i64),
    Bool(bool),
    Str(&'static CStr),
    /// A bool that is whether `flags` has this bit.
    Flag(u32),
}
use Value::{Bool, Flag, Long, Str};

#[derive(Clone, Copy)]
struct Constant {
    /// The flag the constant needs (0: always registered).
    when: u32,
    name: &'static CStr,
    value: Value,
}

const fn always(name: &'static CStr, value: Value) -> Constant {
    Constant {
        when: 0,
        name,
        value,
    }
}

const fn error(name: &'static CStr, code: ErrorCode) -> Constant {
    always(name, Long(code as i64))
}

const fn when(flag: u32, name: &'static CStr, value: Value) -> Constant {
    Constant {
        when: flag,
        name,
        value,
    }
}

#[rustfmt::skip]
static CONSTANTS: [Constant; 251] = [
    always(c"SWOOLE_VERSION", Str(VERSION)),
    always(c"SWOOLE_VERSION_ID", Long(60206)),
    always(c"SWOOLE_MAJOR_VERSION", Long(6)),
    always(c"SWOOLE_MINOR_VERSION", Long(2)),
    always(c"SWOOLE_RELEASE_VERSION", Long(6)),
    always(c"SWOOLE_EXTRA_VERSION", Str(c"")),
    always(c"SWOOLE_DEBUG", Flag(DEBUG)),
    when(COMPRESSION, c"SWOOLE_HAVE_COMPRESSION", Bool(true)),
    when(ZLIB, c"SWOOLE_HAVE_ZLIB", Bool(true)),
    when(BROTLI, c"SWOOLE_HAVE_BROTLI", Bool(true)),
    always(c"SWOOLE_USE_HTTP2", Bool(true)),
    always(c"SWOOLE_USE_SHORTNAME", Flag(SHORTNAME)),
    always(c"SWOOLE_SOCK_TCP", Long(SocketType::SW_SOCK_TCP.raw() as i64)),
    always(c"SWOOLE_SOCK_TCP6", Long(SocketType::SW_SOCK_TCP6.raw() as i64)),
    always(c"SWOOLE_SOCK_UDP", Long(SocketType::SW_SOCK_UDP.raw() as i64)),
    always(c"SWOOLE_SOCK_UDP6", Long(SocketType::SW_SOCK_UDP6.raw() as i64)),
    always(c"SWOOLE_SOCK_UNIX_DGRAM", Long(SocketType::SW_SOCK_UNIX_DGRAM.raw() as i64)),
    always(c"SWOOLE_SOCK_UNIX_STREAM", Long(SocketType::SW_SOCK_UNIX_STREAM.raw() as i64)),
    always(c"SWOOLE_SOCK_RAW", Long(SocketType::SW_SOCK_RAW.raw() as i64)),
    always(c"SWOOLE_SOCK_RAW6", Long(SocketType::SW_SOCK_RAW6.raw() as i64)),
    always(c"SWOOLE_TCP", Long(SocketType::SW_SOCK_TCP.raw() as i64)),
    always(c"SWOOLE_TCP6", Long(SocketType::SW_SOCK_TCP6.raw() as i64)),
    always(c"SWOOLE_UDP", Long(SocketType::SW_SOCK_UDP.raw() as i64)),
    always(c"SWOOLE_UDP6", Long(SocketType::SW_SOCK_UDP6.raw() as i64)),
    always(c"SWOOLE_UNIX_DGRAM", Long(SocketType::SW_SOCK_UNIX_DGRAM.raw() as i64)),
    always(c"SWOOLE_UNIX_STREAM", Long(SocketType::SW_SOCK_UNIX_STREAM.raw() as i64)),
    always(c"SWOOLE_RAW", Long(SocketType::SW_SOCK_RAW.raw() as i64)),
    always(c"SWOOLE_RAW6", Long(SocketType::SW_SOCK_RAW6.raw() as i64)),
    always(c"SWOOLE_SOCK_SYNC", Bool(false)),
    always(c"SWOOLE_SOCK_ASYNC", Bool(true)),
    always(c"SWOOLE_SSL", Long(swoole_rs_corebase::constants::SocketFlag::SW_SOCK_SSL.bits() as i64)),
    always(c"SWOOLE_SSLv3_METHOD", Long(SslMethod::SW_SSLv3_METHOD as i64)),
    always(c"SWOOLE_SSLv3_SERVER_METHOD", Long(SslMethod::SW_SSLv3_SERVER_METHOD as i64)),
    always(c"SWOOLE_SSLv3_CLIENT_METHOD", Long(SslMethod::SW_SSLv3_CLIENT_METHOD as i64)),
    always(c"SWOOLE_TLSv1_METHOD", Long(SslMethod::SW_TLSv1_METHOD as i64)),
    always(c"SWOOLE_TLSv1_SERVER_METHOD", Long(SslMethod::SW_TLSv1_SERVER_METHOD as i64)),
    always(c"SWOOLE_TLSv1_CLIENT_METHOD", Long(SslMethod::SW_TLSv1_CLIENT_METHOD as i64)),
    when(TLS1_1, c"SWOOLE_TLSv1_1_METHOD", Long(SslMethod::SW_TLSv1_1_METHOD as i64)),
    when(TLS1_1, c"SWOOLE_TLSv1_1_SERVER_METHOD", Long(SslMethod::SW_TLSv1_1_SERVER_METHOD as i64)),
    when(TLS1_1, c"SWOOLE_TLSv1_1_CLIENT_METHOD", Long(SslMethod::SW_TLSv1_1_CLIENT_METHOD as i64)),
    when(TLS1_2, c"SWOOLE_TLSv1_2_METHOD", Long(SslMethod::SW_TLSv1_2_METHOD as i64)),
    when(TLS1_2, c"SWOOLE_TLSv1_2_SERVER_METHOD", Long(SslMethod::SW_TLSv1_2_SERVER_METHOD as i64)),
    when(TLS1_2, c"SWOOLE_TLSv1_2_CLIENT_METHOD", Long(SslMethod::SW_TLSv1_2_CLIENT_METHOD as i64)),
    when(DTLS, c"SWOOLE_DTLS_SERVER_METHOD", Long(SslMethod::SW_DTLS_SERVER_METHOD as i64)),
    when(DTLS, c"SWOOLE_DTLS_CLIENT_METHOD", Long(SslMethod::SW_DTLS_CLIENT_METHOD as i64)),
    always(c"SWOOLE_SSLv23_METHOD", Long(SslMethod::SW_SSLv23_METHOD as i64)),
    always(c"SWOOLE_SSLv23_SERVER_METHOD", Long(SslMethod::SW_SSLv23_SERVER_METHOD as i64)),
    always(c"SWOOLE_SSLv23_CLIENT_METHOD", Long(SslMethod::SW_SSLv23_CLIENT_METHOD as i64)),
    always(c"SWOOLE_TLS_METHOD", Long(SslMethod::SW_SSLv23_METHOD as i64)),
    always(c"SWOOLE_TLS_SERVER_METHOD", Long(SslMethod::SW_SSLv23_SERVER_METHOD as i64)),
    always(c"SWOOLE_TLS_CLIENT_METHOD", Long(SslMethod::SW_SSLv23_CLIENT_METHOD as i64)),
    when(SSL3, c"SWOOLE_SSL_SSLv3", Long(SslVersion::SW_SSL_SSLv3.bits() as i64)),
    always(c"SWOOLE_SSL_TLSv1", Long(SslVersion::SW_SSL_TLSv1.bits() as i64)),
    when(TLS1_1, c"SWOOLE_SSL_TLSv1_1", Long(SslVersion::SW_SSL_TLSv1_1.bits() as i64)),
    when(TLS1_2, c"SWOOLE_SSL_TLSv1_2", Long(SslVersion::SW_SSL_TLSv1_2.bits() as i64)),
    when(TLS1_3, c"SWOOLE_SSL_TLSv1_3", Long(SslVersion::SW_SSL_TLSv1_3.bits() as i64)),
    when(DTLS, c"SWOOLE_SSL_DTLS", Long(SslVersion::SW_SSL_DTLS.bits() as i64)),
    always(c"SWOOLE_SSL_SSLv2", Long(SslVersion::SW_SSL_SSLv2.bits() as i64)),
    always(c"SWOOLE_EVENT_READ", Long(512)),
    always(c"SWOOLE_EVENT_WRITE", Long(1024)),
    always(c"SWOOLE_STRERROR_SYSTEM", Long(0)),
    always(c"SWOOLE_STRERROR_GAI", Long(1)),
    always(c"SWOOLE_STRERROR_DNS", Long(2)),
    always(c"SWOOLE_STRERROR_SWOOLE", Long(9)),
    error(c"SWOOLE_ERROR_MALLOC_FAIL", ErrorCode::SW_ERROR_MALLOC_FAIL),
    error(c"SWOOLE_ERROR_SYSTEM_CALL_FAIL", ErrorCode::SW_ERROR_SYSTEM_CALL_FAIL),
    error(c"SWOOLE_ERROR_PHP_FATAL_ERROR", ErrorCode::SW_ERROR_PHP_FATAL_ERROR),
    error(c"SWOOLE_ERROR_NAME_TOO_LONG", ErrorCode::SW_ERROR_NAME_TOO_LONG),
    error(c"SWOOLE_ERROR_INVALID_PARAMS", ErrorCode::SW_ERROR_INVALID_PARAMS),
    error(c"SWOOLE_ERROR_QUEUE_FULL", ErrorCode::SW_ERROR_QUEUE_FULL),
    error(c"SWOOLE_ERROR_OPERATION_NOT_SUPPORT", ErrorCode::SW_ERROR_OPERATION_NOT_SUPPORT),
    error(c"SWOOLE_ERROR_PROTOCOL_ERROR", ErrorCode::SW_ERROR_PROTOCOL_ERROR),
    error(c"SWOOLE_ERROR_WRONG_OPERATION", ErrorCode::SW_ERROR_WRONG_OPERATION),
    error(c"SWOOLE_ERROR_PHP_RUNTIME_NOTICE", ErrorCode::SW_ERROR_PHP_RUNTIME_NOTICE),
    error(c"SWOOLE_ERROR_FOR_TEST", ErrorCode::SW_ERROR_FOR_TEST),
    error(c"SWOOLE_ERROR_NO_PAYLOAD", ErrorCode::SW_ERROR_NO_PAYLOAD),
    error(c"SWOOLE_ERROR_UNDEFINED_BEHAVIOR", ErrorCode::SW_ERROR_UNDEFINED_BEHAVIOR),
    error(c"SWOOLE_ERROR_NOT_THREAD_SAFETY", ErrorCode::SW_ERROR_NOT_THREAD_SAFETY),
    error(c"SWOOLE_ERROR_FILE_NOT_EXIST", ErrorCode::SW_ERROR_FILE_NOT_EXIST),
    error(c"SWOOLE_ERROR_FILE_TOO_LARGE", ErrorCode::SW_ERROR_FILE_TOO_LARGE),
    error(c"SWOOLE_ERROR_FILE_EMPTY", ErrorCode::SW_ERROR_FILE_EMPTY),
    error(c"SWOOLE_ERROR_DIR_NOT_EXIST", ErrorCode::SW_ERROR_DIR_NOT_EXIST),
    error(c"SWOOLE_ERROR_DNSLOOKUP_DUPLICATE_REQUEST", ErrorCode::SW_ERROR_DNSLOOKUP_DUPLICATE_REQUEST),
    error(c"SWOOLE_ERROR_DNSLOOKUP_RESOLVE_FAILED", ErrorCode::SW_ERROR_DNSLOOKUP_RESOLVE_FAILED),
    error(c"SWOOLE_ERROR_DNSLOOKUP_RESOLVE_TIMEOUT", ErrorCode::SW_ERROR_DNSLOOKUP_RESOLVE_TIMEOUT),
    error(c"SWOOLE_ERROR_DNSLOOKUP_UNSUPPORTED", ErrorCode::SW_ERROR_DNSLOOKUP_UNSUPPORTED),
    error(c"SWOOLE_ERROR_DNSLOOKUP_NO_SERVER", ErrorCode::SW_ERROR_DNSLOOKUP_NO_SERVER),
    error(c"SWOOLE_ERROR_BAD_IPV6_ADDRESS", ErrorCode::SW_ERROR_BAD_IPV6_ADDRESS),
    error(c"SWOOLE_ERROR_UNREGISTERED_SIGNAL", ErrorCode::SW_ERROR_UNREGISTERED_SIGNAL),
    error(c"SWOOLE_ERROR_BAD_HOST_ADDR", ErrorCode::SW_ERROR_BAD_HOST_ADDR),
    error(c"SWOOLE_ERROR_BAD_PORT", ErrorCode::SW_ERROR_BAD_PORT),
    error(c"SWOOLE_ERROR_BAD_SOCKET_TYPE", ErrorCode::SW_ERROR_BAD_SOCKET_TYPE),
    error(c"SWOOLE_ERROR_EVENT_REMOVE_FAILED", ErrorCode::SW_ERROR_EVENT_REMOVE_FAILED),
    error(c"SWOOLE_ERROR_EVENT_ADD_FAILED", ErrorCode::SW_ERROR_EVENT_ADD_FAILED),
    error(c"SWOOLE_ERROR_EVENT_UPDATE_FAILED", ErrorCode::SW_ERROR_EVENT_UPDATE_FAILED),
    error(c"SWOOLE_ERROR_EVENT_UNKNOWN_DATA", ErrorCode::SW_ERROR_EVENT_UNKNOWN_DATA),
    error(c"SWOOLE_ERROR_SESSION_CLOSED_BY_SERVER", ErrorCode::SW_ERROR_SESSION_CLOSED_BY_SERVER),
    error(c"SWOOLE_ERROR_SESSION_CLOSED_BY_CLIENT", ErrorCode::SW_ERROR_SESSION_CLOSED_BY_CLIENT),
    error(c"SWOOLE_ERROR_SESSION_CLOSING", ErrorCode::SW_ERROR_SESSION_CLOSING),
    error(c"SWOOLE_ERROR_SESSION_CLOSED", ErrorCode::SW_ERROR_SESSION_CLOSED),
    error(c"SWOOLE_ERROR_SESSION_NOT_EXIST", ErrorCode::SW_ERROR_SESSION_NOT_EXIST),
    error(c"SWOOLE_ERROR_SESSION_INVALID_ID", ErrorCode::SW_ERROR_SESSION_INVALID_ID),
    error(c"SWOOLE_ERROR_SESSION_DISCARD_TIMEOUT_DATA", ErrorCode::SW_ERROR_SESSION_DISCARD_TIMEOUT_DATA),
    error(c"SWOOLE_ERROR_SESSION_DISCARD_DATA", ErrorCode::SW_ERROR_SESSION_DISCARD_DATA),
    error(c"SWOOLE_ERROR_OUTPUT_BUFFER_OVERFLOW", ErrorCode::SW_ERROR_OUTPUT_BUFFER_OVERFLOW),
    error(c"SWOOLE_ERROR_OUTPUT_SEND_YIELD", ErrorCode::SW_ERROR_OUTPUT_SEND_YIELD),
    error(c"SWOOLE_ERROR_SSL_NOT_READY", ErrorCode::SW_ERROR_SSL_NOT_READY),
    error(c"SWOOLE_ERROR_SSL_CANNOT_USE_SENFILE", ErrorCode::SW_ERROR_SSL_CANNOT_USE_SENFILE),
    error(c"SWOOLE_ERROR_SSL_EMPTY_PEER_CERTIFICATE", ErrorCode::SW_ERROR_SSL_EMPTY_PEER_CERTIFICATE),
    error(c"SWOOLE_ERROR_SSL_VERIFY_FAILED", ErrorCode::SW_ERROR_SSL_VERIFY_FAILED),
    error(c"SWOOLE_ERROR_SSL_BAD_CLIENT", ErrorCode::SW_ERROR_SSL_BAD_CLIENT),
    error(c"SWOOLE_ERROR_SSL_BAD_PROTOCOL", ErrorCode::SW_ERROR_SSL_BAD_PROTOCOL),
    error(c"SWOOLE_ERROR_SSL_RESET", ErrorCode::SW_ERROR_SSL_RESET),
    error(c"SWOOLE_ERROR_SSL_HANDSHAKE_FAILED", ErrorCode::SW_ERROR_SSL_HANDSHAKE_FAILED),
    error(c"SWOOLE_ERROR_SSL_CREATE_CONTEXT_FAILED", ErrorCode::SW_ERROR_SSL_CREATE_CONTEXT_FAILED),
    error(c"SWOOLE_ERROR_SSL_CREATE_SESSION_FAILED", ErrorCode::SW_ERROR_SSL_CREATE_SESSION_FAILED),
    error(c"SWOOLE_ERROR_PACKAGE_LENGTH_TOO_LARGE", ErrorCode::SW_ERROR_PACKAGE_LENGTH_TOO_LARGE),
    error(c"SWOOLE_ERROR_PACKAGE_LENGTH_NOT_FOUND", ErrorCode::SW_ERROR_PACKAGE_LENGTH_NOT_FOUND),
    error(c"SWOOLE_ERROR_DATA_LENGTH_TOO_LARGE", ErrorCode::SW_ERROR_DATA_LENGTH_TOO_LARGE),
    error(c"SWOOLE_ERROR_PACKAGE_MALFORMED_DATA", ErrorCode::SW_ERROR_PACKAGE_MALFORMED_DATA),
    error(c"SWOOLE_ERROR_TASK_PACKAGE_TOO_BIG", ErrorCode::SW_ERROR_TASK_PACKAGE_TOO_BIG),
    error(c"SWOOLE_ERROR_TASK_DISPATCH_FAIL", ErrorCode::SW_ERROR_TASK_DISPATCH_FAIL),
    error(c"SWOOLE_ERROR_TASK_TIMEOUT", ErrorCode::SW_ERROR_TASK_TIMEOUT),
    error(c"SWOOLE_ERROR_HTTP2_STREAM_ID_TOO_BIG", ErrorCode::SW_ERROR_HTTP2_STREAM_ID_TOO_BIG),
    error(c"SWOOLE_ERROR_HTTP2_STREAM_NO_HEADER", ErrorCode::SW_ERROR_HTTP2_STREAM_NO_HEADER),
    error(c"SWOOLE_ERROR_HTTP2_STREAM_NOT_FOUND", ErrorCode::SW_ERROR_HTTP2_STREAM_NOT_FOUND),
    error(c"SWOOLE_ERROR_HTTP2_STREAM_IGNORE", ErrorCode::SW_ERROR_HTTP2_STREAM_IGNORE),
    error(c"SWOOLE_ERROR_HTTP2_SEND_CONTROL_FRAME_FAILED", ErrorCode::SW_ERROR_HTTP2_SEND_CONTROL_FRAME_FAILED),
    error(c"SWOOLE_ERROR_HTTP2_INTERNAL_ERROR", ErrorCode::SW_ERROR_HTTP2_INTERNAL_ERROR),
    error(c"SWOOLE_ERROR_AIO_BAD_REQUEST", ErrorCode::SW_ERROR_AIO_BAD_REQUEST),
    error(c"SWOOLE_ERROR_AIO_CANCELED", ErrorCode::SW_ERROR_AIO_CANCELED),
    error(c"SWOOLE_ERROR_AIO_TIMEOUT", ErrorCode::SW_ERROR_AIO_TIMEOUT),
    error(c"SWOOLE_ERROR_CLIENT_NO_CONNECTION", ErrorCode::SW_ERROR_CLIENT_NO_CONNECTION),
    error(c"SWOOLE_ERROR_SOCKET_CLOSED", ErrorCode::SW_ERROR_SOCKET_CLOSED),
    error(c"SWOOLE_ERROR_SOCKET_POLL_TIMEOUT", ErrorCode::SW_ERROR_SOCKET_POLL_TIMEOUT),
    error(c"SWOOLE_ERROR_SOCKET_NOT_EXISTS", ErrorCode::SW_ERROR_SOCKET_NOT_EXISTS),
    error(c"SWOOLE_ERROR_SOCKS5_UNSUPPORT_VERSION", ErrorCode::SW_ERROR_SOCKS5_UNSUPPORT_VERSION),
    error(c"SWOOLE_ERROR_SOCKS5_UNSUPPORT_METHOD", ErrorCode::SW_ERROR_SOCKS5_UNSUPPORT_METHOD),
    error(c"SWOOLE_ERROR_SOCKS5_AUTH_FAILED", ErrorCode::SW_ERROR_SOCKS5_AUTH_FAILED),
    error(c"SWOOLE_ERROR_SOCKS5_SERVER_ERROR", ErrorCode::SW_ERROR_SOCKS5_SERVER_ERROR),
    error(c"SWOOLE_ERROR_SOCKS5_HANDSHAKE_FAILED", ErrorCode::SW_ERROR_SOCKS5_HANDSHAKE_FAILED),
    error(c"SWOOLE_ERROR_SOCKS5_CONNECT_FAILED", ErrorCode::SW_ERROR_SOCKS5_CONNECT_FAILED),
    error(c"SWOOLE_ERROR_HTTP_PROXY_HANDSHAKE_ERROR", ErrorCode::SW_ERROR_HTTP_PROXY_HANDSHAKE_ERROR),
    error(c"SWOOLE_ERROR_HTTP_INVALID_PROTOCOL", ErrorCode::SW_ERROR_HTTP_INVALID_PROTOCOL),
    error(c"SWOOLE_ERROR_HTTP_PROXY_HANDSHAKE_FAILED", ErrorCode::SW_ERROR_HTTP_PROXY_HANDSHAKE_FAILED),
    error(c"SWOOLE_ERROR_HTTP_PROXY_BAD_RESPONSE", ErrorCode::SW_ERROR_HTTP_PROXY_BAD_RESPONSE),
    error(c"SWOOLE_ERROR_HTTP_CONFLICT_HEADER", ErrorCode::SW_ERROR_HTTP_CONFLICT_HEADER),
    error(c"SWOOLE_ERROR_HTTP_CONTEXT_UNAVAILABLE", ErrorCode::SW_ERROR_HTTP_CONTEXT_UNAVAILABLE),
    error(c"SWOOLE_ERROR_HTTP_COOKIE_UNAVAILABLE", ErrorCode::SW_ERROR_HTTP_COOKIE_UNAVAILABLE),
    error(c"SWOOLE_ERROR_WEBSOCKET_BAD_CLIENT", ErrorCode::SW_ERROR_WEBSOCKET_BAD_CLIENT),
    error(c"SWOOLE_ERROR_WEBSOCKET_BAD_OPCODE", ErrorCode::SW_ERROR_WEBSOCKET_BAD_OPCODE),
    error(c"SWOOLE_ERROR_WEBSOCKET_UNCONNECTED", ErrorCode::SW_ERROR_WEBSOCKET_UNCONNECTED),
    error(c"SWOOLE_ERROR_WEBSOCKET_HANDSHAKE_FAILED", ErrorCode::SW_ERROR_WEBSOCKET_HANDSHAKE_FAILED),
    error(c"SWOOLE_ERROR_WEBSOCKET_PACK_FAILED", ErrorCode::SW_ERROR_WEBSOCKET_PACK_FAILED),
    error(c"SWOOLE_ERROR_WEBSOCKET_UNPACK_FAILED", ErrorCode::SW_ERROR_WEBSOCKET_UNPACK_FAILED),
    error(c"SWOOLE_ERROR_WEBSOCKET_INCOMPLETE_PACKET", ErrorCode::SW_ERROR_WEBSOCKET_INCOMPLETE_PACKET),
    error(c"SWOOLE_ERROR_SERVER_MUST_CREATED_BEFORE_CLIENT", ErrorCode::SW_ERROR_SERVER_MUST_CREATED_BEFORE_CLIENT),
    error(c"SWOOLE_ERROR_SERVER_TOO_MANY_SOCKET", ErrorCode::SW_ERROR_SERVER_TOO_MANY_SOCKET),
    error(c"SWOOLE_ERROR_SERVER_WORKER_TERMINATED", ErrorCode::SW_ERROR_SERVER_WORKER_TERMINATED),
    error(c"SWOOLE_ERROR_SERVER_INVALID_LISTEN_PORT", ErrorCode::SW_ERROR_SERVER_INVALID_LISTEN_PORT),
    error(c"SWOOLE_ERROR_SERVER_TOO_MANY_LISTEN_PORT", ErrorCode::SW_ERROR_SERVER_TOO_MANY_LISTEN_PORT),
    error(c"SWOOLE_ERROR_SERVER_PIPE_BUFFER_FULL", ErrorCode::SW_ERROR_SERVER_PIPE_BUFFER_FULL),
    error(c"SWOOLE_ERROR_SERVER_NO_IDLE_WORKER", ErrorCode::SW_ERROR_SERVER_NO_IDLE_WORKER),
    error(c"SWOOLE_ERROR_SERVER_ONLY_START_ONE", ErrorCode::SW_ERROR_SERVER_ONLY_START_ONE),
    error(c"SWOOLE_ERROR_SERVER_SEND_IN_MASTER", ErrorCode::SW_ERROR_SERVER_SEND_IN_MASTER),
    error(c"SWOOLE_ERROR_SERVER_INVALID_REQUEST", ErrorCode::SW_ERROR_SERVER_INVALID_REQUEST),
    error(c"SWOOLE_ERROR_SERVER_CONNECT_FAIL", ErrorCode::SW_ERROR_SERVER_CONNECT_FAIL),
    error(c"SWOOLE_ERROR_SERVER_INVALID_COMMAND", ErrorCode::SW_ERROR_SERVER_INVALID_COMMAND),
    error(c"SWOOLE_ERROR_SERVER_IS_NOT_REGULAR_FILE", ErrorCode::SW_ERROR_SERVER_IS_NOT_REGULAR_FILE),
    error(c"SWOOLE_ERROR_SERVER_SEND_TO_WOKER_TIMEOUT", ErrorCode::SW_ERROR_SERVER_SEND_TO_WOKER_TIMEOUT),
    error(c"SWOOLE_ERROR_SERVER_INVALID_CALLBACK", ErrorCode::SW_ERROR_SERVER_INVALID_CALLBACK),
    error(c"SWOOLE_ERROR_SERVER_UNRELATED_THREAD", ErrorCode::SW_ERROR_SERVER_UNRELATED_THREAD),
    error(c"SWOOLE_ERROR_SERVER_WORKER_EXIT_TIMEOUT", ErrorCode::SW_ERROR_SERVER_WORKER_EXIT_TIMEOUT),
    error(c"SWOOLE_ERROR_SERVER_WORKER_ABNORMAL_PIPE_DATA", ErrorCode::SW_ERROR_SERVER_WORKER_ABNORMAL_PIPE_DATA),
    error(c"SWOOLE_ERROR_SERVER_WORKER_UNPROCESSED_DATA", ErrorCode::SW_ERROR_SERVER_WORKER_UNPROCESSED_DATA),
    error(c"SWOOLE_ERROR_CO_OUT_OF_COROUTINE", ErrorCode::SW_ERROR_CO_OUT_OF_COROUTINE),
    error(c"SWOOLE_ERROR_CO_HAS_BEEN_BOUND", ErrorCode::SW_ERROR_CO_HAS_BEEN_BOUND),
    error(c"SWOOLE_ERROR_CO_HAS_BEEN_DISCARDED", ErrorCode::SW_ERROR_CO_HAS_BEEN_DISCARDED),
    error(c"SWOOLE_ERROR_CO_MUTEX_DOUBLE_UNLOCK", ErrorCode::SW_ERROR_CO_MUTEX_DOUBLE_UNLOCK),
    error(c"SWOOLE_ERROR_CO_BLOCK_OBJECT_LOCKED", ErrorCode::SW_ERROR_CO_BLOCK_OBJECT_LOCKED),
    error(c"SWOOLE_ERROR_CO_BLOCK_OBJECT_WAITING", ErrorCode::SW_ERROR_CO_BLOCK_OBJECT_WAITING),
    error(c"SWOOLE_ERROR_CO_YIELD_FAILED", ErrorCode::SW_ERROR_CO_YIELD_FAILED),
    error(c"SWOOLE_ERROR_CO_GETCONTEXT_FAILED", ErrorCode::SW_ERROR_CO_GETCONTEXT_FAILED),
    error(c"SWOOLE_ERROR_CO_SWAPCONTEXT_FAILED", ErrorCode::SW_ERROR_CO_SWAPCONTEXT_FAILED),
    error(c"SWOOLE_ERROR_CO_MAKECONTEXT_FAILED", ErrorCode::SW_ERROR_CO_MAKECONTEXT_FAILED),
    error(c"SWOOLE_ERROR_CO_IOCPINIT_FAILED", ErrorCode::SW_ERROR_CO_IOCPINIT_FAILED),
    error(c"SWOOLE_ERROR_CO_PROTECT_STACK_FAILED", ErrorCode::SW_ERROR_CO_PROTECT_STACK_FAILED),
    error(c"SWOOLE_ERROR_CO_STD_THREAD_LINK_ERROR", ErrorCode::SW_ERROR_CO_STD_THREAD_LINK_ERROR),
    error(c"SWOOLE_ERROR_CO_DISABLED_MULTI_THREAD", ErrorCode::SW_ERROR_CO_DISABLED_MULTI_THREAD),
    error(c"SWOOLE_ERROR_CO_CANNOT_CANCEL", ErrorCode::SW_ERROR_CO_CANNOT_CANCEL),
    error(c"SWOOLE_ERROR_CO_NOT_EXISTS", ErrorCode::SW_ERROR_CO_NOT_EXISTS),
    error(c"SWOOLE_ERROR_CO_CANCELED", ErrorCode::SW_ERROR_CO_CANCELED),
    error(c"SWOOLE_ERROR_CO_TIMEDOUT", ErrorCode::SW_ERROR_CO_TIMEDOUT),
    error(c"SWOOLE_ERROR_CO_SOCKET_CLOSE_WAIT", ErrorCode::SW_ERROR_CO_SOCKET_CLOSE_WAIT),
    always(c"SWOOLE_TRACE_SERVER", Long(2)),
    always(c"SWOOLE_TRACE_CLIENT", Long(4)),
    always(c"SWOOLE_TRACE_BUFFER", Long(8)),
    always(c"SWOOLE_TRACE_CONN", Long(16)),
    always(c"SWOOLE_TRACE_EVENT", Long(32)),
    always(c"SWOOLE_TRACE_WORKER", Long(64)),
    always(c"SWOOLE_TRACE_MEMORY", Long(128)),
    always(c"SWOOLE_TRACE_REACTOR", Long(256)),
    always(c"SWOOLE_TRACE_PHP", Long(512)),
    always(c"SWOOLE_TRACE_HTTP", Long(1024)),
    always(c"SWOOLE_TRACE_HTTP2", Long(2048)),
    always(c"SWOOLE_TRACE_EOF_PROTOCOL", Long(4096)),
    always(c"SWOOLE_TRACE_LENGTH_PROTOCOL", Long(8192)),
    always(c"SWOOLE_TRACE_CLOSE", Long(16384)),
    always(c"SWOOLE_TRACE_WEBSOCKET", Long(32768)),
    always(c"SWOOLE_TRACE_REDIS_CLIENT", Long(65536)),
    always(c"SWOOLE_TRACE_MYSQL_CLIENT", Long(131_072)),
    always(c"SWOOLE_TRACE_HTTP_CLIENT", Long(262_144)),
    always(c"SWOOLE_TRACE_AIO", Long(524_288)),
    always(c"SWOOLE_TRACE_SSL", Long(1_048_576)),
    always(c"SWOOLE_TRACE_NORMAL", Long(2_097_152)),
    always(c"SWOOLE_TRACE_CHANNEL", Long(4_194_304)),
    always(c"SWOOLE_TRACE_TIMER", Long(8_388_608)),
    always(c"SWOOLE_TRACE_SOCKET", Long(16_777_216)),
    always(c"SWOOLE_TRACE_COROUTINE", Long(33_554_432)),
    always(c"SWOOLE_TRACE_CONTEXT", Long(67_108_864)),
    always(c"SWOOLE_TRACE_CO_HTTP_SERVER", Long(134_217_728)),
    always(c"SWOOLE_TRACE_TABLE", Long(268_435_456)),
    always(c"SWOOLE_TRACE_CO_CURL", Long(536_870_912)),
    always(c"SWOOLE_TRACE_CARES", Long(1_073_741_824)),
    always(c"SWOOLE_TRACE_ZLIB", Long(2_147_483_648)),
    always(c"SWOOLE_TRACE_CO_PGSQL", Long(4_294_967_296)),
    always(c"SWOOLE_TRACE_CO_ODBC", Long(8_589_934_592)),
    always(c"SWOOLE_TRACE_CO_ORACLE", Long(17_179_869_184)),
    always(c"SWOOLE_TRACE_CO_SQLITE", Long(34_359_738_368)),
    always(c"SWOOLE_TRACE_CO_FIREBIRD", Long(68_719_476_736)),
    always(c"SWOOLE_TRACE_CO_SSH2", Long(137_438_953_472)),
    always(c"SWOOLE_TRACE_THREAD", Long(1_099_511_627_776)),
    always(c"SWOOLE_TRACE_ALL", Long(9_223_372_036_854_775_807)),
    always(c"SWOOLE_LOG_DEBUG", Long(0)),
    always(c"SWOOLE_LOG_TRACE", Long(1)),
    always(c"SWOOLE_LOG_INFO", Long(2)),
    always(c"SWOOLE_LOG_NOTICE", Long(3)),
    always(c"SWOOLE_LOG_WARNING", Long(4)),
    always(c"SWOOLE_LOG_ERROR", Long(5)),
    always(c"SWOOLE_LOG_NONE", Long(6)),
    always(c"SWOOLE_LOG_ROTATION_SINGLE", Long(0)),
    always(c"SWOOLE_LOG_ROTATION_MONTHLY", Long(1)),
    always(c"SWOOLE_LOG_ROTATION_DAILY", Long(2)),
    always(c"SWOOLE_LOG_ROTATION_HOURLY", Long(3)),
    always(c"SWOOLE_LOG_ROTATION_EVERY_MINUTE", Long(4)),
    always(c"SWOOLE_IPC_NONE", Long(0)),
    always(c"SWOOLE_IPC_UNIXSOCK", Long(1)),
    always(c"SWOOLE_IPC_SOCKET", Long(3)),
    always(c"SWOOLE_IOV_MAX", Long(1024)),
    when(IOURING, c"SWOOLE_IOURING_DEFAULT", Long(0)),
    when(IOURING, c"SWOOLE_IOURING_SQPOLL", Long(2)),
];

/// The constants a build with `flags` registers, in order, with their values.
fn selected(flags: u32) -> impl Iterator<Item = (&'static CStr, Value)> {
    CONSTANTS
        .iter()
        .filter(move |constant| constant.when & flags == constant.when)
        .map(move |constant| match constant.value {
            Flag(bit) => (constant.name, Bool(flags & bit != 0)),
            value => (constant.name, value),
        })
}

/// `SW_REGISTER_*_CONSTANT` of every constant the build's `flags` select.
pub(crate) fn register(minit: &Minit, flags: u32) {
    for (name, value) in selected(flags) {
        match value {
            Long(long) => minit.long_constant(name, long),
            Bool(on) => minit.bool_constant(name, on),
            Str(string) => minit.string_constant(name, string),
            Flag(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(flags: u32) -> Vec<&'static CStr> {
        selected(flags).map(|(name, _)| name).collect()
    }

    #[test]
    fn names_are_unique_and_prefixed() {
        let all = names(u32::MAX);
        let mut sorted = all.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len(), "no constant is registered twice");
        assert!(
            all.iter()
                .all(|name| name.to_bytes().starts_with(b"SWOOLE_"))
        );
    }

    #[test]
    fn flags_select_conditional_constants() {
        let plain = names(0);
        assert!(
            !plain.contains(&c"SWOOLE_HAVE_ZLIB"),
            "zlib needs SW_HAVE_ZLIB"
        );
        assert!(!plain.contains(&c"SWOOLE_IOURING_SQPOLL"));
        assert!(
            plain.contains(&c"SWOOLE_DEBUG"),
            "SWOOLE_DEBUG is always there"
        );
        let tls = names(TLS1_3);
        assert!(tls.contains(&c"SWOOLE_SSL_TLSv1_3"));
        assert_eq!(tls.len(), plain.len() + 1);
    }

    #[test]
    fn flag_constants_follow_the_build() {
        let debug = |flags| {
            selected(flags)
                .find(|(name, _)| *name == c"SWOOLE_DEBUG")
                .map(|(_, value)| matches!(value, Bool(true)))
        };
        assert_eq!(debug(0), Some(false));
        assert_eq!(debug(DEBUG), Some(true));
    }

    #[test]
    fn upstream_order_and_values() {
        let all: Vec<_> = selected(u32::MAX).collect();
        assert_eq!(all[0].0, c"SWOOLE_VERSION");
        assert!(matches!(all[1].1, Long(60206)), "SWOOLE_VERSION_ID");
        let trace_all = all.iter().find(|(name, _)| *name == c"SWOOLE_TRACE_ALL");
        assert!(matches!(trace_all, Some((_, Long(i64::MAX)))));
        let last = all.last().map(|(name, _)| *name);
        assert_eq!(last, Some(c"SWOOLE_IOURING_SQPOLL"));
    }
}

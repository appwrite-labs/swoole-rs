//! The global constants `PHP_MINIT_FUNCTION(swoole)` registered, in its order
//! (`get_defined_constants()` and reflection list them so). Values are the
//! pinned headers' enums and macros; `flags` stands for the `#ifdef`s.
use std::ffi::CStr;
use swoole_rs_zend::{
    register_long_constant,
    sys::{CONST_PERSISTENT, zend_register_bool_constant, zend_register_string_constant},
};

/// `swr_v1_phpswoole_minit` flags: which `#ifdef`s the build has.
pub(crate) const DEBUG: u32 = 1 << 0;
pub(crate) const SHORTNAME: u32 = 1 << 1;
const COMPRESSION: u32 = 1 << 2;
const ZLIB: u32 = 1 << 3;
const BROTLI: u32 = 1 << 4;
const TLS1_1: u32 = 1 << 5;
const TLS1_2: u32 = 1 << 6;
const TLS1_3: u32 = 1 << 7;
const DTLS: u32 = 1 << 8;
const SSL3: u32 = 1 << 9;
const IOURING: u32 = 1 << 10;

/// `SWOOLE_VERSION`.
pub(crate) const VERSION: &CStr = c"6.2.4";

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
    always(c"SWOOLE_VERSION_ID", Long(60204)),
    always(c"SWOOLE_MAJOR_VERSION", Long(6)),
    always(c"SWOOLE_MINOR_VERSION", Long(2)),
    always(c"SWOOLE_RELEASE_VERSION", Long(4)),
    always(c"SWOOLE_EXTRA_VERSION", Str(c"")),
    always(c"SWOOLE_DEBUG", Flag(DEBUG)),
    when(COMPRESSION, c"SWOOLE_HAVE_COMPRESSION", Bool(true)),
    when(ZLIB, c"SWOOLE_HAVE_ZLIB", Bool(true)),
    when(BROTLI, c"SWOOLE_HAVE_BROTLI", Bool(true)),
    always(c"SWOOLE_USE_HTTP2", Bool(true)),
    always(c"SWOOLE_USE_SHORTNAME", Flag(SHORTNAME)),
    always(c"SWOOLE_SOCK_TCP", Long(1)),
    always(c"SWOOLE_SOCK_TCP6", Long(3)),
    always(c"SWOOLE_SOCK_UDP", Long(2)),
    always(c"SWOOLE_SOCK_UDP6", Long(4)),
    always(c"SWOOLE_SOCK_UNIX_DGRAM", Long(6)),
    always(c"SWOOLE_SOCK_UNIX_STREAM", Long(5)),
    always(c"SWOOLE_SOCK_RAW", Long(7)),
    always(c"SWOOLE_SOCK_RAW6", Long(8)),
    always(c"SWOOLE_TCP", Long(1)),
    always(c"SWOOLE_TCP6", Long(3)),
    always(c"SWOOLE_UDP", Long(2)),
    always(c"SWOOLE_UDP6", Long(4)),
    always(c"SWOOLE_UNIX_DGRAM", Long(6)),
    always(c"SWOOLE_UNIX_STREAM", Long(5)),
    always(c"SWOOLE_RAW", Long(7)),
    always(c"SWOOLE_RAW6", Long(8)),
    always(c"SWOOLE_SOCK_SYNC", Bool(false)),
    always(c"SWOOLE_SOCK_ASYNC", Bool(true)),
    always(c"SWOOLE_SSL", Long(512)),
    always(c"SWOOLE_SSLv3_METHOD", Long(1)),
    always(c"SWOOLE_SSLv3_SERVER_METHOD", Long(2)),
    always(c"SWOOLE_SSLv3_CLIENT_METHOD", Long(3)),
    always(c"SWOOLE_TLSv1_METHOD", Long(6)),
    always(c"SWOOLE_TLSv1_SERVER_METHOD", Long(7)),
    always(c"SWOOLE_TLSv1_CLIENT_METHOD", Long(8)),
    when(TLS1_1, c"SWOOLE_TLSv1_1_METHOD", Long(9)),
    when(TLS1_1, c"SWOOLE_TLSv1_1_SERVER_METHOD", Long(10)),
    when(TLS1_1, c"SWOOLE_TLSv1_1_CLIENT_METHOD", Long(11)),
    when(TLS1_2, c"SWOOLE_TLSv1_2_METHOD", Long(12)),
    when(TLS1_2, c"SWOOLE_TLSv1_2_SERVER_METHOD", Long(13)),
    when(TLS1_2, c"SWOOLE_TLSv1_2_CLIENT_METHOD", Long(14)),
    when(DTLS, c"SWOOLE_DTLS_SERVER_METHOD", Long(16)),
    when(DTLS, c"SWOOLE_DTLS_CLIENT_METHOD", Long(15)),
    always(c"SWOOLE_SSLv23_METHOD", Long(0)),
    always(c"SWOOLE_SSLv23_SERVER_METHOD", Long(4)),
    always(c"SWOOLE_SSLv23_CLIENT_METHOD", Long(5)),
    always(c"SWOOLE_TLS_METHOD", Long(0)),
    always(c"SWOOLE_TLS_SERVER_METHOD", Long(4)),
    always(c"SWOOLE_TLS_CLIENT_METHOD", Long(5)),
    when(SSL3, c"SWOOLE_SSL_SSLv3", Long(4)),
    always(c"SWOOLE_SSL_TLSv1", Long(8)),
    when(TLS1_1, c"SWOOLE_SSL_TLSv1_1", Long(16)),
    when(TLS1_2, c"SWOOLE_SSL_TLSv1_2", Long(32)),
    when(TLS1_3, c"SWOOLE_SSL_TLSv1_3", Long(64)),
    when(DTLS, c"SWOOLE_SSL_DTLS", Long(128)),
    always(c"SWOOLE_SSL_SSLv2", Long(2)),
    always(c"SWOOLE_EVENT_READ", Long(512)),
    always(c"SWOOLE_EVENT_WRITE", Long(1024)),
    always(c"SWOOLE_STRERROR_SYSTEM", Long(0)),
    always(c"SWOOLE_STRERROR_GAI", Long(1)),
    always(c"SWOOLE_STRERROR_DNS", Long(2)),
    always(c"SWOOLE_STRERROR_SWOOLE", Long(9)),
    always(c"SWOOLE_ERROR_MALLOC_FAIL", Long(501)),
    always(c"SWOOLE_ERROR_SYSTEM_CALL_FAIL", Long(502)),
    always(c"SWOOLE_ERROR_PHP_FATAL_ERROR", Long(503)),
    always(c"SWOOLE_ERROR_NAME_TOO_LONG", Long(504)),
    always(c"SWOOLE_ERROR_INVALID_PARAMS", Long(505)),
    always(c"SWOOLE_ERROR_QUEUE_FULL", Long(506)),
    always(c"SWOOLE_ERROR_OPERATION_NOT_SUPPORT", Long(507)),
    always(c"SWOOLE_ERROR_PROTOCOL_ERROR", Long(508)),
    always(c"SWOOLE_ERROR_WRONG_OPERATION", Long(509)),
    always(c"SWOOLE_ERROR_PHP_RUNTIME_NOTICE", Long(510)),
    always(c"SWOOLE_ERROR_FOR_TEST", Long(511)),
    always(c"SWOOLE_ERROR_NO_PAYLOAD", Long(550)),
    always(c"SWOOLE_ERROR_UNDEFINED_BEHAVIOR", Long(600)),
    always(c"SWOOLE_ERROR_NOT_THREAD_SAFETY", Long(601)),
    always(c"SWOOLE_ERROR_FILE_NOT_EXIST", Long(700)),
    always(c"SWOOLE_ERROR_FILE_TOO_LARGE", Long(701)),
    always(c"SWOOLE_ERROR_FILE_EMPTY", Long(702)),
    always(c"SWOOLE_ERROR_DIR_NOT_EXIST", Long(703)),
    always(c"SWOOLE_ERROR_DNSLOOKUP_DUPLICATE_REQUEST", Long(710)),
    always(c"SWOOLE_ERROR_DNSLOOKUP_RESOLVE_FAILED", Long(711)),
    always(c"SWOOLE_ERROR_DNSLOOKUP_RESOLVE_TIMEOUT", Long(712)),
    always(c"SWOOLE_ERROR_DNSLOOKUP_UNSUPPORTED", Long(713)),
    always(c"SWOOLE_ERROR_DNSLOOKUP_NO_SERVER", Long(714)),
    always(c"SWOOLE_ERROR_BAD_IPV6_ADDRESS", Long(720)),
    always(c"SWOOLE_ERROR_UNREGISTERED_SIGNAL", Long(721)),
    always(c"SWOOLE_ERROR_BAD_HOST_ADDR", Long(722)),
    always(c"SWOOLE_ERROR_BAD_PORT", Long(723)),
    always(c"SWOOLE_ERROR_BAD_SOCKET_TYPE", Long(724)),
    always(c"SWOOLE_ERROR_EVENT_REMOVE_FAILED", Long(800)),
    always(c"SWOOLE_ERROR_EVENT_ADD_FAILED", Long(801)),
    always(c"SWOOLE_ERROR_EVENT_UPDATE_FAILED", Long(802)),
    always(c"SWOOLE_ERROR_EVENT_UNKNOWN_DATA", Long(803)),
    always(c"SWOOLE_ERROR_SESSION_CLOSED_BY_SERVER", Long(1001)),
    always(c"SWOOLE_ERROR_SESSION_CLOSED_BY_CLIENT", Long(1002)),
    always(c"SWOOLE_ERROR_SESSION_CLOSING", Long(1003)),
    always(c"SWOOLE_ERROR_SESSION_CLOSED", Long(1004)),
    always(c"SWOOLE_ERROR_SESSION_NOT_EXIST", Long(1005)),
    always(c"SWOOLE_ERROR_SESSION_INVALID_ID", Long(1006)),
    always(c"SWOOLE_ERROR_SESSION_DISCARD_TIMEOUT_DATA", Long(1007)),
    always(c"SWOOLE_ERROR_SESSION_DISCARD_DATA", Long(1008)),
    always(c"SWOOLE_ERROR_OUTPUT_BUFFER_OVERFLOW", Long(1009)),
    always(c"SWOOLE_ERROR_OUTPUT_SEND_YIELD", Long(1010)),
    always(c"SWOOLE_ERROR_SSL_NOT_READY", Long(1011)),
    always(c"SWOOLE_ERROR_SSL_CANNOT_USE_SENFILE", Long(1012)),
    always(c"SWOOLE_ERROR_SSL_EMPTY_PEER_CERTIFICATE", Long(1013)),
    always(c"SWOOLE_ERROR_SSL_VERIFY_FAILED", Long(1014)),
    always(c"SWOOLE_ERROR_SSL_BAD_CLIENT", Long(1015)),
    always(c"SWOOLE_ERROR_SSL_BAD_PROTOCOL", Long(1016)),
    always(c"SWOOLE_ERROR_SSL_RESET", Long(1017)),
    always(c"SWOOLE_ERROR_SSL_HANDSHAKE_FAILED", Long(1018)),
    always(c"SWOOLE_ERROR_SSL_CREATE_CONTEXT_FAILED", Long(1019)),
    always(c"SWOOLE_ERROR_SSL_CREATE_SESSION_FAILED", Long(1020)),
    always(c"SWOOLE_ERROR_PACKAGE_LENGTH_TOO_LARGE", Long(1201)),
    always(c"SWOOLE_ERROR_PACKAGE_LENGTH_NOT_FOUND", Long(1202)),
    always(c"SWOOLE_ERROR_DATA_LENGTH_TOO_LARGE", Long(1203)),
    always(c"SWOOLE_ERROR_PACKAGE_MALFORMED_DATA", Long(1204)),
    always(c"SWOOLE_ERROR_TASK_PACKAGE_TOO_BIG", Long(2001)),
    always(c"SWOOLE_ERROR_TASK_DISPATCH_FAIL", Long(2002)),
    always(c"SWOOLE_ERROR_TASK_TIMEOUT", Long(2003)),
    always(c"SWOOLE_ERROR_HTTP2_STREAM_ID_TOO_BIG", Long(3001)),
    always(c"SWOOLE_ERROR_HTTP2_STREAM_NO_HEADER", Long(3002)),
    always(c"SWOOLE_ERROR_HTTP2_STREAM_NOT_FOUND", Long(3003)),
    always(c"SWOOLE_ERROR_HTTP2_STREAM_IGNORE", Long(3004)),
    always(c"SWOOLE_ERROR_HTTP2_SEND_CONTROL_FRAME_FAILED", Long(3005)),
    always(c"SWOOLE_ERROR_HTTP2_INTERNAL_ERROR", Long(3006)),
    always(c"SWOOLE_ERROR_AIO_BAD_REQUEST", Long(4001)),
    always(c"SWOOLE_ERROR_AIO_CANCELED", Long(4002)),
    always(c"SWOOLE_ERROR_AIO_TIMEOUT", Long(4003)),
    always(c"SWOOLE_ERROR_CLIENT_NO_CONNECTION", Long(5001)),
    always(c"SWOOLE_ERROR_SOCKET_CLOSED", Long(6001)),
    always(c"SWOOLE_ERROR_SOCKET_POLL_TIMEOUT", Long(6002)),
    always(c"SWOOLE_ERROR_SOCKET_NOT_EXISTS", Long(6003)),
    always(c"SWOOLE_ERROR_SOCKS5_UNSUPPORT_VERSION", Long(7001)),
    always(c"SWOOLE_ERROR_SOCKS5_UNSUPPORT_METHOD", Long(7002)),
    always(c"SWOOLE_ERROR_SOCKS5_AUTH_FAILED", Long(7003)),
    always(c"SWOOLE_ERROR_SOCKS5_SERVER_ERROR", Long(7004)),
    always(c"SWOOLE_ERROR_SOCKS5_HANDSHAKE_FAILED", Long(7005)),
    always(c"SWOOLE_ERROR_SOCKS5_CONNECT_FAILED", Long(7006)),
    always(c"SWOOLE_ERROR_HTTP_PROXY_HANDSHAKE_ERROR", Long(7101)),
    always(c"SWOOLE_ERROR_HTTP_INVALID_PROTOCOL", Long(7102)),
    always(c"SWOOLE_ERROR_HTTP_PROXY_HANDSHAKE_FAILED", Long(7103)),
    always(c"SWOOLE_ERROR_HTTP_PROXY_BAD_RESPONSE", Long(7104)),
    always(c"SWOOLE_ERROR_HTTP_CONFLICT_HEADER", Long(7105)),
    always(c"SWOOLE_ERROR_HTTP_CONTEXT_UNAVAILABLE", Long(7106)),
    always(c"SWOOLE_ERROR_HTTP_COOKIE_UNAVAILABLE", Long(7107)),
    always(c"SWOOLE_ERROR_WEBSOCKET_BAD_CLIENT", Long(8501)),
    always(c"SWOOLE_ERROR_WEBSOCKET_BAD_OPCODE", Long(8502)),
    always(c"SWOOLE_ERROR_WEBSOCKET_UNCONNECTED", Long(8503)),
    always(c"SWOOLE_ERROR_WEBSOCKET_HANDSHAKE_FAILED", Long(8504)),
    always(c"SWOOLE_ERROR_WEBSOCKET_PACK_FAILED", Long(8505)),
    always(c"SWOOLE_ERROR_WEBSOCKET_UNPACK_FAILED", Long(8506)),
    always(c"SWOOLE_ERROR_WEBSOCKET_INCOMPLETE_PACKET", Long(8507)),
    always(c"SWOOLE_ERROR_SERVER_MUST_CREATED_BEFORE_CLIENT", Long(9001)),
    always(c"SWOOLE_ERROR_SERVER_TOO_MANY_SOCKET", Long(9002)),
    always(c"SWOOLE_ERROR_SERVER_WORKER_TERMINATED", Long(9003)),
    always(c"SWOOLE_ERROR_SERVER_INVALID_LISTEN_PORT", Long(9004)),
    always(c"SWOOLE_ERROR_SERVER_TOO_MANY_LISTEN_PORT", Long(9005)),
    always(c"SWOOLE_ERROR_SERVER_PIPE_BUFFER_FULL", Long(9006)),
    always(c"SWOOLE_ERROR_SERVER_NO_IDLE_WORKER", Long(9007)),
    always(c"SWOOLE_ERROR_SERVER_ONLY_START_ONE", Long(9008)),
    always(c"SWOOLE_ERROR_SERVER_SEND_IN_MASTER", Long(9009)),
    always(c"SWOOLE_ERROR_SERVER_INVALID_REQUEST", Long(9010)),
    always(c"SWOOLE_ERROR_SERVER_CONNECT_FAIL", Long(9011)),
    always(c"SWOOLE_ERROR_SERVER_INVALID_COMMAND", Long(9012)),
    always(c"SWOOLE_ERROR_SERVER_IS_NOT_REGULAR_FILE", Long(9013)),
    always(c"SWOOLE_ERROR_SERVER_SEND_TO_WOKER_TIMEOUT", Long(9014)),
    always(c"SWOOLE_ERROR_SERVER_INVALID_CALLBACK", Long(9015)),
    always(c"SWOOLE_ERROR_SERVER_UNRELATED_THREAD", Long(9016)),
    always(c"SWOOLE_ERROR_SERVER_WORKER_EXIT_TIMEOUT", Long(9101)),
    always(c"SWOOLE_ERROR_SERVER_WORKER_ABNORMAL_PIPE_DATA", Long(9102)),
    always(c"SWOOLE_ERROR_SERVER_WORKER_UNPROCESSED_DATA", Long(9103)),
    always(c"SWOOLE_ERROR_CO_OUT_OF_COROUTINE", Long(10001)),
    always(c"SWOOLE_ERROR_CO_HAS_BEEN_BOUND", Long(10002)),
    always(c"SWOOLE_ERROR_CO_HAS_BEEN_DISCARDED", Long(10003)),
    always(c"SWOOLE_ERROR_CO_MUTEX_DOUBLE_UNLOCK", Long(10004)),
    always(c"SWOOLE_ERROR_CO_BLOCK_OBJECT_LOCKED", Long(10005)),
    always(c"SWOOLE_ERROR_CO_BLOCK_OBJECT_WAITING", Long(10006)),
    always(c"SWOOLE_ERROR_CO_YIELD_FAILED", Long(10007)),
    always(c"SWOOLE_ERROR_CO_GETCONTEXT_FAILED", Long(10008)),
    always(c"SWOOLE_ERROR_CO_SWAPCONTEXT_FAILED", Long(10009)),
    always(c"SWOOLE_ERROR_CO_MAKECONTEXT_FAILED", Long(10010)),
    always(c"SWOOLE_ERROR_CO_IOCPINIT_FAILED", Long(10011)),
    always(c"SWOOLE_ERROR_CO_PROTECT_STACK_FAILED", Long(10012)),
    always(c"SWOOLE_ERROR_CO_STD_THREAD_LINK_ERROR", Long(10013)),
    always(c"SWOOLE_ERROR_CO_DISABLED_MULTI_THREAD", Long(10014)),
    always(c"SWOOLE_ERROR_CO_CANNOT_CANCEL", Long(10015)),
    always(c"SWOOLE_ERROR_CO_NOT_EXISTS", Long(10016)),
    always(c"SWOOLE_ERROR_CO_CANCELED", Long(10017)),
    always(c"SWOOLE_ERROR_CO_TIMEDOUT", Long(10018)),
    always(c"SWOOLE_ERROR_CO_SOCKET_CLOSE_WAIT", Long(10019)),
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
/// # Safety
/// At MINIT, with the extension's module number.
pub(crate) unsafe fn register(module: i32, flags: u32) {
    #[expect(clippy::cast_possible_wrap, reason = "a small flag")]
    let persistent = CONST_PERSISTENT as i32;
    for (name, value) in selected(flags) {
        let (text, length) = (name.as_ptr(), name.count_bytes());
        match value {
            // SAFETY: per the caller; Zend copies the name.
            Long(long) => unsafe { register_long_constant(name, long, module) },
            // SAFETY: as above.
            Bool(on) => unsafe {
                zend_register_bool_constant(text, length, on, persistent, module);
            },
            // SAFETY: as above; Zend copies the value.
            Str(string) => unsafe {
                zend_register_string_constant(text, length, string.as_ptr(), persistent, module);
            },
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
        assert!(matches!(all[1].1, Long(60204)), "SWOOLE_VERSION_ID");
        let trace_all = all.iter().find(|(name, _)| *name == c"SWOOLE_TRACE_ALL");
        assert!(matches!(trace_all, Some((_, Long(i64::MAX)))));
        let last = all.last().map(|(name, _)| *name);
        assert_eq!(last, Some(c"SWOOLE_IOURING_SQPOLL"));
    }
}

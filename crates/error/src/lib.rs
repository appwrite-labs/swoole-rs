#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
// SPDX-License-Identifier: Apache-2.0
// Message text derived from Swoole v6.2.2 src/core/error.cc,
// Copyright Swoole contributors; original author Tianfeng Han <rango@swoole.com>.
//! Swoole error dispatch and ignored state; native code supplies OS messages.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

use std::ffi::c_char;

// The archive's ABI queries live here, not in crates/ffi, whose object holds
// swoole_init: a C consumer that asks for them links no lifecycle.
/// Return the supported C ABI version.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_abi_version() -> u32 {
    1
}

/// Return the migrated kernels: every module, as the bits `MODULES` in `tools/check_migration.py` names.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_feature_bits() -> u64 {
    (1 << 28) - 1
}

mod codes;
pub use codes::ErrorCode;
mod state;
pub use state::*;

// The aliases are generated inside the match, so the switch stays integer-based
// while every identifier's value has one definition in ErrorCode.
macro_rules! messages {
    ($($name:ident => $message:literal),* $(,)?) => {
        #[inline(always)]
        fn known_message(code: i32) -> *const c_char {
            $(const $name: i32 = ErrorCode::$name as i32;)*
            match code { $($name => $message,)* _ => return std::ptr::null() }.as_ptr()
        }
    };
}
messages! {
    SW_ERROR_MALLOC_FAIL => c"Malloc fail",
    SW_ERROR_SYSTEM_CALL_FAIL => c"System call fail",
    SW_ERROR_PHP_FATAL_ERROR => c"PHP fatal error",
    SW_ERROR_NAME_TOO_LONG => c"Name too long",
    SW_ERROR_INVALID_PARAMS => c"Invalid params",
    SW_ERROR_QUEUE_FULL => c"Queue full",
    SW_ERROR_OPERATION_NOT_SUPPORT => c"Operation not support",
    SW_ERROR_PROTOCOL_ERROR => c"Protocol error",
    SW_ERROR_WRONG_OPERATION => c"Wrong operation",
    SW_ERROR_PHP_RUNTIME_NOTICE => c"PHP runtime notice",
    SW_ERROR_FOR_TEST => c"For test",
    SW_ERROR_NO_PAYLOAD => c"No payload",
    SW_ERROR_UNDEFINED_BEHAVIOR => c"Undefined behavior",
    SW_ERROR_NOT_THREAD_SAFETY => c"Not thread safety",
    SW_ERROR_FILE_NOT_EXIST => c"File not exist",
    SW_ERROR_FILE_TOO_LARGE => c"File too large",
    SW_ERROR_FILE_EMPTY => c"File empty",
    SW_ERROR_DIR_NOT_EXIST => c"Dir not exist",
    SW_ERROR_DNSLOOKUP_DUPLICATE_REQUEST => c"DNS Lookup duplicate request",
    SW_ERROR_DNSLOOKUP_RESOLVE_FAILED => c"DNS Lookup resolve failed",
    SW_ERROR_DNSLOOKUP_RESOLVE_TIMEOUT => c"DNS Lookup resolve timeout",
    SW_ERROR_DNSLOOKUP_UNSUPPORTED => c"DNS Lookup unsupported",
    SW_ERROR_DNSLOOKUP_NO_SERVER => c"DNS Lookup no server",
    SW_ERROR_BAD_IPV6_ADDRESS => c"Bad ipv6 address",
    SW_ERROR_UNREGISTERED_SIGNAL => c"Unregistered signal",
    SW_ERROR_BAD_HOST_ADDR => c"Bad host addr",
    SW_ERROR_BAD_PORT => c"Bad port",
    SW_ERROR_BAD_SOCKET_TYPE => c"Bad socket type",
    SW_ERROR_EVENT_REMOVE_FAILED => c"Event remove failed",
    SW_ERROR_EVENT_ADD_FAILED => c"Event add failed",
    SW_ERROR_EVENT_UPDATE_FAILED => c"Event update failed",
    SW_ERROR_EVENT_UNKNOWN_DATA => c"Event unknown data",
    SW_ERROR_SESSION_CLOSED_BY_SERVER => c"Session closed by server",
    SW_ERROR_SESSION_CLOSED_BY_CLIENT => c"Session closed by client",
    SW_ERROR_SESSION_CLOSING => c"Session closing",
    SW_ERROR_SESSION_CLOSED => c"Session closed",
    SW_ERROR_SESSION_NOT_EXIST => c"Session not exist",
    SW_ERROR_SESSION_INVALID_ID => c"Session invalid id",
    SW_ERROR_SESSION_DISCARD_TIMEOUT_DATA => c"Session discard timeout data",
    SW_ERROR_SESSION_DISCARD_DATA => c"Session discard data",
    SW_ERROR_OUTPUT_BUFFER_OVERFLOW => c"Output buffer overflow",
    SW_ERROR_OUTPUT_SEND_YIELD => c"Output send yield",
    SW_ERROR_SSL_NOT_READY => c"SSL not ready",
    SW_ERROR_SSL_CANNOT_USE_SENFILE => c"SSL cannot use senfile",
    SW_ERROR_SSL_EMPTY_PEER_CERTIFICATE => c"SSL empty peer certificate",
    SW_ERROR_SSL_VERIFY_FAILED => c"SSL verify failed",
    SW_ERROR_SSL_BAD_CLIENT => c"SSL bad client",
    SW_ERROR_SSL_BAD_PROTOCOL => c"SSL bad protocol",
    SW_ERROR_SSL_RESET => c"SSL reset",
    SW_ERROR_SSL_HANDSHAKE_FAILED => c"SSL handshake failed",
    SW_ERROR_SSL_CREATE_CONTEXT_FAILED => c"SSL create context failed",
    SW_ERROR_SSL_CREATE_SESSION_FAILED => c"SSL create session failed",
    SW_ERROR_PACKAGE_LENGTH_TOO_LARGE => c"Package length too large",
    SW_ERROR_PACKAGE_LENGTH_NOT_FOUND => c"Package length not found",
    SW_ERROR_DATA_LENGTH_TOO_LARGE => c"Data length too large",
    SW_ERROR_PACKAGE_MALFORMED_DATA => c"Package malformed data",
    SW_ERROR_TASK_PACKAGE_TOO_BIG => c"Task package too big",
    SW_ERROR_TASK_DISPATCH_FAIL => c"Task dispatch fail",
    SW_ERROR_TASK_TIMEOUT => c"Task timeout",
    SW_ERROR_HTTP2_STREAM_ID_TOO_BIG => c"Http2 stream id too big",
    SW_ERROR_HTTP2_STREAM_NO_HEADER => c"Http2 stream no header",
    SW_ERROR_HTTP2_STREAM_NOT_FOUND => c"Http2 stream not found",
    SW_ERROR_HTTP2_STREAM_IGNORE => c"Http2 stream ignore",
    SW_ERROR_HTTP2_SEND_CONTROL_FRAME_FAILED => c"Http2 send control frame failed",
    SW_ERROR_HTTP2_INTERNAL_ERROR => c"Http2 internal error",
    SW_ERROR_AIO_BAD_REQUEST => c"Aio bad request",
    SW_ERROR_AIO_CANCELED => c"Aio canceled",
    SW_ERROR_AIO_TIMEOUT => c"Aio timeout",
    SW_ERROR_CLIENT_NO_CONNECTION => c"Client no connection",
    SW_ERROR_SOCKET_CLOSED => c"Socket closed",
    SW_ERROR_SOCKET_POLL_TIMEOUT => c"Socket poll timeout",
    SW_ERROR_SOCKET_NOT_EXISTS => c"Socket not exists",
    SW_ERROR_SOCKS5_UNSUPPORT_VERSION => c"Socks5 unsupport version",
    SW_ERROR_SOCKS5_UNSUPPORT_METHOD => c"Socks5 unsupport method",
    SW_ERROR_SOCKS5_AUTH_FAILED => c"Socks5 auth failed",
    SW_ERROR_SOCKS5_SERVER_ERROR => c"Socks5 server error",
    SW_ERROR_SOCKS5_HANDSHAKE_FAILED => c"Socks5 handshake failed",
    SW_ERROR_SOCKS5_CONNECT_FAILED => c"Socks5 connect failed",
    SW_ERROR_HTTP_PROXY_HANDSHAKE_ERROR => c"Http proxy handshake error",
    SW_ERROR_HTTP_INVALID_PROTOCOL => c"Http invalid protocol",
    SW_ERROR_HTTP_PROXY_HANDSHAKE_FAILED => c"Http proxy handshake failed",
    SW_ERROR_HTTP_PROXY_BAD_RESPONSE => c"Http proxy bad response",
    SW_ERROR_HTTP_CONFLICT_HEADER => c"Http conflict header",
    SW_ERROR_HTTP_CONTEXT_UNAVAILABLE => c"Http context unavailable",
    SW_ERROR_HTTP_COOKIE_UNAVAILABLE => c"Http cookie unavailable",
    SW_ERROR_WEBSOCKET_BAD_CLIENT => c"Websocket bad client",
    SW_ERROR_WEBSOCKET_BAD_OPCODE => c"Websocket bad opcode",
    SW_ERROR_WEBSOCKET_UNCONNECTED => c"Websocket unconnected",
    SW_ERROR_WEBSOCKET_HANDSHAKE_FAILED => c"Websocket handshake failed",
    SW_ERROR_WEBSOCKET_PACK_FAILED => c"Websocket pack failed",
    SW_ERROR_WEBSOCKET_UNPACK_FAILED => c"Websocket unpack failed",
    SW_ERROR_WEBSOCKET_INCOMPLETE_PACKET => c"Websocket incomplete packet",
    SW_ERROR_SERVER_MUST_CREATED_BEFORE_CLIENT => c"Server must created before client",
    SW_ERROR_SERVER_TOO_MANY_SOCKET => c"Server too many socket",
    SW_ERROR_SERVER_WORKER_TERMINATED => c"Server worker terminated",
    SW_ERROR_SERVER_INVALID_LISTEN_PORT => c"Server invalid listen port",
    SW_ERROR_SERVER_TOO_MANY_LISTEN_PORT => c"Server too many listen port",
    SW_ERROR_SERVER_PIPE_BUFFER_FULL => c"Server pipe buffer full",
    SW_ERROR_SERVER_NO_IDLE_WORKER => c"Server no idle worker",
    SW_ERROR_SERVER_ONLY_START_ONE => c"Server only start one",
    SW_ERROR_SERVER_SEND_IN_MASTER => c"Server send in master",
    SW_ERROR_SERVER_INVALID_REQUEST => c"Server invalid request",
    SW_ERROR_SERVER_CONNECT_FAIL => c"Server connect fail",
    SW_ERROR_SERVER_INVALID_COMMAND => c"Server invalid command",
    SW_ERROR_SERVER_IS_NOT_REGULAR_FILE => c"Server is not regular file",
    SW_ERROR_SERVER_SEND_TO_WOKER_TIMEOUT => c"Server send to woker timeout",
    SW_ERROR_SERVER_INVALID_CALLBACK => c"Server invalid callback",
    SW_ERROR_SERVER_UNRELATED_THREAD => c"Server unrelated thread",
    SW_ERROR_SERVER_WORKER_EXIT_TIMEOUT => c"Server worker exit timeout",
    SW_ERROR_SERVER_WORKER_ABNORMAL_PIPE_DATA => c"Server worker abnormal pipe data",
    SW_ERROR_SERVER_WORKER_UNPROCESSED_DATA => c"Server worker unprocessed data",
    SW_ERROR_CO_OUT_OF_COROUTINE => c"Coroutine out of coroutine",
    SW_ERROR_CO_HAS_BEEN_BOUND => c"Coroutine has been bound",
    SW_ERROR_CO_HAS_BEEN_DISCARDED => c"Coroutine has been discarded",
    SW_ERROR_CO_MUTEX_DOUBLE_UNLOCK => c"Coroutine mutex double unlock",
    SW_ERROR_CO_BLOCK_OBJECT_LOCKED => c"Coroutine block object locked",
    SW_ERROR_CO_BLOCK_OBJECT_WAITING => c"Coroutine block object waiting",
    SW_ERROR_CO_YIELD_FAILED => c"Coroutine yield failed",
    SW_ERROR_CO_GETCONTEXT_FAILED => c"Coroutine getcontext failed",
    SW_ERROR_CO_SWAPCONTEXT_FAILED => c"Coroutine swapcontext failed",
    SW_ERROR_CO_MAKECONTEXT_FAILED => c"Coroutine makecontext failed",
    SW_ERROR_CO_IOCPINIT_FAILED => c"Coroutine iocpinit failed",
    SW_ERROR_CO_PROTECT_STACK_FAILED => c"Coroutine protect stack failed",
    SW_ERROR_CO_STD_THREAD_LINK_ERROR => c"Coroutine std thread link error",
    SW_ERROR_CO_DISABLED_MULTI_THREAD => c"Coroutine disabled multi thread",
    SW_ERROR_CO_CANNOT_CANCEL => c"Coroutine cannot cancel",
    SW_ERROR_CO_NOT_EXISTS => c"Coroutine not exists",
    SW_ERROR_CO_CANCELED => c"Coroutine canceled",
    SW_ERROR_CO_TIMEDOUT => c"Coroutine timedout",
    SW_ERROR_CO_SOCKET_CLOSE_WAIT => c"Coroutine socket close wait",
}

/// Return an immutable process-lifetime message, never to be freed; unknown is null.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_error_message(code: i32) -> *const c_char {
    known_message(code)
}

/// Resolve known codes, invoking an optional nonthrowing native formatter only on misses.
/// The formatter must not enter Zend, retain Rust state, or unwind through this frame.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_error_resolve(
    code: i32,
    fallback: Option<extern "C" fn(i32) -> *const c_char>,
) -> *const c_char {
    let known = known_message(code);
    if known.is_null() {
        fallback.map_or(std::ptr::null(), |format| format(code))
    } else {
        known
    }
}

unsafe extern "C" {
    safe fn strerror(code: i32) -> *const c_char;
}

extern "C" fn system_error(code: i32) -> *const c_char {
    strerror(code)
}

/// `const char *swoole_strerror(int)`: system messages below 500, as upstream.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z15swoole_strerrori")
)]
pub extern "C" fn swoole_strerror(code: i32) -> *const c_char {
    // SAFETY: the formatter is libc's message table; unknown-message use is serialized as upstream.
    unsafe { swr_v1_error_strerror(code, Some(system_error)) }
}

/// `SW_ERROR_MSG_SIZE`: the bytes of each thread's last-error message.
const ERROR_MSG_SIZE: usize = 16384;

thread_local! {
    // Cells: C++ writes through the pointer while the thread can still reach the array.
    static LAST_ERROR: [std::cell::Cell<c_char>; ERROR_MSG_SIZE] =
        const { [const { std::cell::Cell::new(0) }; ERROR_MSG_SIZE] };
}

/// `sw_error`: this thread's last-error message buffer, `SW_ERROR_MSG_SIZE` bytes that
/// live as long as the thread. C++ formats log lines into it.
#[must_use]
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_error_buffer() -> *mut c_char {
    LAST_ERROR.with(|buffer| buffer.as_ptr().cast::<c_char>().cast_mut())
}

/// `sw_snprintf(sw_error, SW_ERROR_MSG_SIZE, "%s", text)`: `text`, truncated to the buffer
/// and NUL-terminated; returns the length kept.
#[must_use]
pub fn set_last_error_msg(text: &[u8]) -> usize {
    let length = text.len().min(ERROR_MSG_SIZE - 1);
    LAST_ERROR.with(|buffer| {
        for (cell, &byte) in buffer.iter().zip(text) {
            cell.set(c_char::from_ne_bytes([byte]));
        }
        if let Some(end) = buffer.get(length) {
            end.set(0);
        }
    });
    length
}

/// `const char *swoole_get_last_error_msg()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z25swoole_get_last_error_msgv")
)]
#[must_use]
pub extern "C" fn swoole_get_last_error_msg() -> *const c_char {
    swr_v1_error_buffer()
}

/// `void swoole_clear_last_error_msg()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z27swoole_clear_last_error_msgv")
)]
pub extern "C" fn swoole_clear_last_error_msg() {
    LAST_ERROR.with(|buffer| buffer[0].set(0));
}

/// `void swoole_throw_error(int)`. Upstream threw a `swoole::Exception`; the extension
/// catches none, so it terminated. This reports the code and aborts.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z18swoole_throw_errori")
)]
pub extern "C" fn swoole_throw_error(code: i32) -> ! {
    use std::io::Write;
    writeln!(std::io::stderr(), "swoole_throw_error({code})").ok();
    std::process::abort();
}

/// Resolve system, known and unknown errors without heap allocation.
/// # Safety
/// Calls which return the mutable unknown-message buffer must be externally
/// serialized, as in the original snprintf buffer. Known-message calls are pure.
/// The optional system formatter must not unwind, enter Zend or retain Rust state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_error_strerror(
    code: i32,
    system: Option<extern "C" fn(i32) -> *const c_char>,
) -> *const c_char {
    use std::io::Write;
    static mut UNKNOWN: [u8; 32] = [0; 32];
    if code < ErrorCode::SW_ERROR_BEGIN as i32 {
        return system.map_or(std::ptr::null(), |format| format(code));
    }
    let known = known_message(code);
    if !known.is_null() {
        return known;
    }
    // SAFETY: The caller serializes unknown-message use. No mutable reference
    // survives this call; the C-string pointer and storage lifetime match upstream.
    let buffer = unsafe { &mut *std::ptr::addr_of_mut!(UNKNOWN) };
    let mut remaining = &mut buffer[..31];
    let colon = if cfg!(target_vendor = "apple") {
        ":"
    } else {
        ""
    };
    // The longest prefix + int32 decimal representation is 26 bytes including NUL.
    if write!(remaining, "Unknown error{colon} {code}").is_err() {
        return std::ptr::null();
    }
    let used = 31 - remaining.len();
    let Some(end) = buffer.get_mut(used) else {
        return std::ptr::null();
    };
    *end = 0;
    buffer.as_ptr().cast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_error_buffer_is_per_thread_and_clears() {
        let buffer = swr_v1_error_buffer();
        unsafe { buffer.copy_from_nonoverlapping(c"sentinel".as_ptr(), 9) };
        assert_eq!(swoole_get_last_error_msg(), buffer.cast_const());
        assert_eq!(unsafe { std::ffi::CStr::from_ptr(buffer) }, c"sentinel");
        let other = std::thread::spawn(|| {
            let other = swr_v1_error_buffer();
            (other as usize, unsafe { *other })
        })
        .join()
        .unwrap();
        assert_ne!(other.0, buffer as usize, "each thread has its own buffer");
        assert_eq!(other.1, 0);
        swoole_clear_last_error_msg();
        assert_eq!(unsafe { *buffer }, 0);
    }
}

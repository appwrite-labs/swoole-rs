// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 include/swoole{,_config}.h (Swoole contributors).
//! Canonical legacy event, socket and configuration identifiers. Both native
//! headers and Rust operations use these definitions.

/// The socket TLS handshake state, shared by reactor storage and TLS operations.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[expect(non_camel_case_types, reason = "preserve the native SDK names")]
pub enum SslState {
    #[default]
    SW_SSL_STATE_HANDSHAKE = 0,
    SW_SSL_STATE_READY = 1,
    SW_SSL_STATE_WAIT_STREAM = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    non_camel_case_types,
    reason = "the stable native identifiers are preserved by generated bindings"
)]
pub enum ResultCode {
    SW_OK = 0,
    SW_ERR = -1,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    non_camel_case_types,
    reason = "the stable native identifiers are preserved by generated bindings"
)]
pub enum ReturnCode {
    SW_SUCCESS = 0,
    SW_CONTINUE = 1,
    SW_WAIT = 2,
    SW_CLOSE = 3,
    SW_ERROR = 4,
    SW_READY = 5,
    SW_INVALID = 6,
    SW_REDUCE_SIZE = 7,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    non_camel_case_types,
    reason = "the stable native identifiers are preserved by generated bindings"
)]
pub enum FdType {
    SW_FD_SESSION = 0,
    SW_FD_STREAM_SERVER = 1,
    SW_FD_DGRAM_SERVER = 2,
    SW_FD_PIPE = 3,
    SW_FD_STREAM = 4,
    SW_FD_AIO = 5,
    SW_FD_CO_SOCKET = 6,
    SW_FD_CO_POLL = 7,
    SW_FD_CO_EVENT = 8,
    SW_FD_SIGNAL = 9,
    SW_FD_DNS_RESOLVER = 10,
    SW_FD_CARES = 11,
    SW_FD_IOURING = 12,
    SW_FD_USER = 16,
    SW_FD_STREAM_CLIENT = 17,
    SW_FD_DGRAM_CLIENT = 18,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    non_camel_case_types,
    reason = "the stable native identifiers are preserved by generated bindings"
)]
pub enum SocketFlag {
    SW_SOCK_NONBLOCK = 4,
    SW_SOCK_CLOEXEC = 8,
    SW_SOCK_SSL = 512,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    non_camel_case_types,
    reason = "the stable native identifiers are preserved by generated bindings"
)]
pub enum SocketType {
    SW_SOCK_TCP = 1,
    SW_SOCK_UDP = 2,
    SW_SOCK_TCP6 = 3,
    SW_SOCK_UDP6 = 4,
    SW_SOCK_UNIX_STREAM = 5,
    SW_SOCK_UNIX_DGRAM = 6,
    SW_SOCK_RAW = 7,
    SW_SOCK_RAW6 = 8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    non_camel_case_types,
    reason = "the stable native identifiers are preserved by generated bindings"
)]
pub enum TimeoutType {
    SW_TIMEOUT_DNS = 1,
    SW_TIMEOUT_CONNECT = 2,
    SW_TIMEOUT_READ = 4,
    SW_TIMEOUT_WRITE = 8,
    SW_TIMEOUT_RDWR = 12,
    SW_TIMEOUT_ALL = 15,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    non_camel_case_types,
    reason = "the stable native identifiers are preserved by generated bindings"
)]
pub enum EventType {
    SW_EVENT_NULL = 0,
    SW_EVENT_DEAULT = 256,
    SW_EVENT_READ = 512,
    SW_EVENT_WRITE = 1024,
    SW_EVENT_RDWR = 1536,
    SW_EVENT_ERROR = 2048,
    SW_EVENT_ONCE = 4096,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    non_camel_case_types,
    reason = "the stable native identifiers are preserved by generated bindings"
)]
pub enum ForkType {
    SW_FORK_SPAWN = 0,
    SW_FORK_EXEC = 2,
    SW_FORK_DAEMON = 4,
    SW_FORK_PRECHECK = 8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    non_camel_case_types,
    reason = "the stable native identifiers are preserved by generated bindings"
)]
pub enum TraverseOperation {
    SW_TRAVERSE_KEEP = 0,
    SW_TRAVERSE_REMOVE = 1,
    SW_TRAVERSE_STOP = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    non_camel_case_types,
    reason = "the stable native identifiers are preserved by generated bindings"
)]
pub enum EventDataFlag {
    SW_EVENT_DATA_NORMAL = 0,
    SW_EVENT_DATA_PTR = 2,
    SW_EVENT_DATA_CHUNK = 4,
    SW_EVENT_DATA_BEGIN = 8,
    SW_EVENT_DATA_END = 16,
    SW_EVENT_DATA_OBJ_PTR = 32,
    SW_EVENT_DATA_POP_PTR = 64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    non_camel_case_types,
    reason = "the stable native identifiers are preserved by generated bindings"
)]
pub enum TaskFlag {
    SW_TASK_TMPFILE = 1,
    SW_TASK_SERIALIZE = 2,
    SW_TASK_NONBLOCK = 4,
    SW_TASK_CALLBACK = 8,
    SW_TASK_WAITALL = 16,
    SW_TASK_COROUTINE = 32,
    SW_TASK_PEEK = 64,
    SW_TASK_NOREPLY = 128,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    non_camel_case_types,
    reason = "the stable native identifiers are preserved by generated bindings"
)]
pub enum DNSLookupFlag {
    SW_DNS_LOOKUP_RANDOM = 2048,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    non_camel_case_types,
    reason = "the stable native identifiers are preserved by generated bindings"
)]
pub enum PipeType {
    SW_PIPE_WORKER = 0,
    SW_PIPE_MASTER = 1,
    SW_PIPE_NONBLOCK = 2,
}
pub const SW_PIPE_READ: PipeType = PipeType::SW_PIPE_WORKER;
pub const SW_PIPE_WRITE: PipeType = PipeType::SW_PIPE_MASTER;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    non_camel_case_types,
    reason = "the stable native identifiers are preserved by generated bindings"
)]
pub enum EventInitFlag {
    SW_EVENTLOOP_WAIT_EXIT = 1,
}

pub const SW_MAX_FDTYPE: u32 = 32;
pub const SW_MAX_HOOK_TYPE: u32 = 32;
pub const SW_ERROR_MSG_SIZE: u32 = 16384;
pub const SW_MAX_FILE_CONTENT: u32 = 67_108_864;
pub const SW_MAX_LISTEN_PORT: u32 = 60000;
pub const SW_MAX_CONNECTION: u32 = 100_000;
pub const SW_MAX_CONCURRENT_TASK: u32 = 1024;
pub const SW_STACK_BUFFER_SIZE: u32 = 65536;
pub const SW_MALLOC_TRIM_INTERVAL: u32 = 60;
pub const SW_MALLOC_TRIM_PAD: u32 = 0;
pub const SW_MAX_SOCKETS_DEFAULT: u32 = 1024;
pub const SW_SOCKET_BUFFER_SIZE: u32 = 8_388_608;
pub const SW_SOCKET_RETRY_COUNT: u32 = 10;
pub const SW_SOCKET_DEFAULT_DNS_TIMEOUT: u32 = 60;
pub const SW_SOCKET_DEFAULT_CONNECT_TIMEOUT: u32 = 10;
pub const SW_SOCKET_DEFAULT_READ_TIMEOUT: u32 = 60;
pub const SW_SOCKET_DEFAULT_WRITE_TIMEOUT: u32 = 60;
pub const SW_SOCKET_CORK_MIN_SIZE: u32 = 65536;
pub const SW_SYSTEMD_FDS_START: u32 = 3;
pub const SW_GLOBAL_MEMORY_PAGESIZE: u32 = 2_097_152;
pub const SW_MAX_THREAD_NCPU: u32 = 4;
pub const SW_MAX_WORKER_NCPU: u32 = 1000;
pub const SW_CLIENT_BUFFER_SIZE: u32 = 65536;
pub const SW_CLIENT_CONNECT_TIMEOUT: f64 = 0.5;
pub const SW_IPC_BUFFER_MAX_SIZE: u32 = 65536;
pub const SW_BUFFER_SIZE_STD: u32 = 8192;
pub const SW_BUFFER_SIZE_BIG: u32 = 65536;
pub const SW_BUFFER_SIZE_UDP: u32 = 65536;
pub const SW_SENDFILE_CHUNK_SIZE: u32 = 65536;
pub const SW_DATA_EOF: &str = "\r\n\r\n";
pub const SW_DATA_EOF_MAXLEN: u32 = 8;
pub const SW_TASKWAIT_TIMEOUT: f64 = 0.5;
pub const SW_AIO_THREAD_NUM_MULTIPLE: u32 = 8;
pub const SW_AIO_THREAD_MAX_IDLE_TIME: f64 = 1.0;
pub const SW_AIO_TASK_MAX_WAIT_TIME: f64 = 0.001;
pub const SW_AIO_EVENT_NUM: u32 = 128;
pub const SW_WORKER_MAX_WAIT_TIME: u32 = 3;
pub const SW_WORKER_MIN_REQUEST: u32 = 10;
pub const SW_WORKER_MAX_RECV_CHUNK_COUNT: u32 = 32;
pub const SW_REACTOR_MAXEVENTS: u32 = 4096;
pub const SW_SESSION_LIST_SIZE: u32 = 1_048_576;
pub const SW_MSGMAX: u32 = 65536;
pub const SW_MESSAGE_BOX_SIZE: u32 = 65536;
pub const SW_REACTOR_MAX_THREAD: u32 = 8;
pub const SW_INPUT_BUFFER_SIZE: u32 = 2_097_152;
pub const SW_BUFFER_MIN_SIZE: u32 = 65536;
pub const SW_SEND_BUFFER_SIZE: u32 = 65536;
pub const SW_BACKLOG: u32 = 512;
pub const SW_ACCEPT_MAX_COUNT: u32 = 64;
pub const SW_ACCEPT_RETRY_TIME: f64 = 1.0;
pub const SW_TCP_KEEPCOUNT: u32 = 5;
pub const SW_TCP_KEEPIDLE: u32 = 3600;
pub const SW_TCP_KEEPINTERVAL: u32 = 60;
pub const SW_TASK_TMP_PATH_SIZE: u32 = 256;
pub const SW_TASK_TMP_DIR: &str = "/tmp";
pub const SW_TASK_TMP_FILE: &str = "swoole.task.XXXXXX";
pub const SW_TABLE_CONFLICT_PROPORTION: f64 = 0.2;
pub const SW_TABLE_KEY_SIZE: u32 = 64;
pub const SW_SSL_BUFFER_SIZE: u32 = 16384;
pub const SW_SSL_CIPHER_LIST: &str = "EECDH+AESGCM:EDH+AESGCM:AES256+EECDH:AES256+EDH";
pub const SW_SSL_ECDH_CURVE: &str = "auto";
pub const SW_SPINLOCK_LOOP_N: u32 = 1024;
pub const SW_FILE_LOCK_DEFAULT_SECOND: f64 = 0.001;
pub const SW_FILE_LOCK_MAX_SECOND: f64 = 0.1;
pub const SW_SIGNO_MAX: u32 = 128;
pub const SW_UNREGISTERED_SIGNAL_FMT: &str = "Unable to find callback function for signal %s";
pub const SW_DNS_HOST_BUFFER_SIZE: u32 = 16;
pub const SW_DNS_SERVER_PORT: u32 = 53;
pub const SW_DNS_RESOLV_CONF: &str = "/etc/resolv.conf";
pub const SW_Z_BEST_SPEED: u32 = 1;
pub const SW_COMPRESSION_MIN_LENGTH_DEFAULT: u32 = 20;
pub const SW_IOV_MAX_ERROR_MSG: &str = "The maximum of iov count is %d";
pub const SW_IOURING_QUEUE_SIZE: u32 = 8192;
pub const SW_IOURING_SQE_BATCH_SIZE: u32 = 16;
pub const SW_HTTP_SERVER_SOFTWARE: &str = "swoole-http-server";
pub const SW_HTTP_SERVER_BOUNDARY_PREKEY: &str = "SwooleBoundary";
pub const SW_HTTP_SERVER_BOUNDARY_TOTAL_SIZE: u32 = 39;
pub const SW_HTTP_SERVER_PART_HEADER: u32 = 256;
pub const SW_HTTP_FORM_KEYLEN: u32 = 512;
pub const SW_HTTP_RESPONSE_INIT_SIZE: u32 = 65536;
pub const SW_HTTP_HEADER_MAX_SIZE: u32 = 65536;
pub const SW_HTTP_HEADER_KEY_SIZE: u32 = 128;
pub const SW_HTTP_UPLOAD_TMPDIR_SIZE: u32 = 256;
pub const SW_HTTP_DATE_FORMAT: &str = "D, d M Y H:i:s T";
pub const SW_HTTP_RFC1123_DATE_GMT: &str = "%a, %d %b %Y %T GMT";
pub const SW_HTTP_RFC1123_DATE_UTC: &str = "%a, %d %b %Y %T UTC";
pub const SW_HTTP_RFC850_DATE: &str = "%A, %d-%b-%y %T GMT";
pub const SW_HTTP_ASCTIME_DATE: &str = "%a %b %e %T %Y";
pub const SW_HTTP_UPLOAD_FILE: &str = "Swoole-Upload-File";
pub const SW_HTTP_CHUNK_EOF: &str = "0\r\n\r\n";
pub const SW_HTTP_DEFAULT_CONTENT_TYPE: &str = "text/html";
pub const SW_HTTP_MAX_APPEND_DATA: u32 = 16384;
pub const SW_HTTP_100_CONTINUE_PACKET: &str = "HTTP/1.1 100 Continue\r\n\r\n";
pub const SW_HTTP_BAD_REQUEST_PACKET: &str = "HTTP/1.1 400 Bad Request\r\n\r\n";
pub const SW_HTTP_REQUEST_ENTITY_TOO_LARGE_PACKET: &str =
    "HTTP/1.1 413 Request Entity Too Large\r\n\r\n";
pub const SW_HTTP_SERVICE_UNAVAILABLE_PACKET: &str = "HTTP/1.1 503 Service Unavailable\r\n\r\n";
pub const SW_HTTP_POWER_BY: &str = "<footer><i>Powered by Swoole</i></footer>";
pub const SW_HTTP2_DATA_BUFFER_SIZE: u32 = 8192;
pub const SW_HTTP2_DEFAULT_HEADER_TABLE_SIZE: u32 = 4096;
pub const SW_HTTP2_DEFAULT_MAX_CONCURRENT_STREAMS: u32 = 4_294_967_295_u32;
pub const SW_HTTP2_DEFAULT_ENABLE_PUSH: u32 = 0;
pub const SW_HTTP2_DEFAULT_MAX_FRAME_SIZE: u32 = 16384_u32;
pub const SW_HTTP2_DEFAULT_INIT_WINDOW_SIZE: u32 = 65535;
pub const SW_HTTP2_DEFAULT_MAX_HEADER_LIST_SIZE: u32 = 4_294_967_295_u32;
pub const SW_HTTP_CLIENT_BOUNDARY_PREKEY: &str = "----SwooleBoundary";
pub const SW_HTTP_CLIENT_BOUNDARY_TOTAL_SIZE: u32 = 39;
pub const SW_HTTP_FORM_RAW_DATA_FMT: &str =
    "--%.*s\r\nContent-Disposition: form-data; name=\"%.*s\"\r\n\r\n";
pub const SW_HTTP_FORM_RAW_DATA_FMT_LEN: u32 = 8;
pub const SW_HTTP_FORM_FILE_DATA_FMT: &str = "--%.*s\r\nContent-Disposition: form-data; name=\"%.*s\"; filename=\"%.*s\"\r\nContent-Type: %.*s\r\n\r\n";
pub const SW_HTTP_FORM_FILE_DATA_FMT_LEN: u32 = 16;
pub const SW_WEBSOCKET_VERSION: &str = "13";
pub const SW_WEBSOCKET_KEY_LENGTH: u32 = 16;
pub const SW_WEBSOCKET_EXTENSION_DEFLATE: &str =
    "permessage-deflate; client_no_context_takeover; server_no_context_takeover";
pub const SW_DEFAULT_C_STACK_SIZE: u32 = 2_097_152;
pub const SW_CORO_BAILOUT_EXIT_CODE: u32 = 1;
#[cfg(target_vendor = "apple")]
pub const SW_IPC_MAX_SIZE: u32 = 2048;
#[cfg(not(target_vendor = "apple"))]
pub const SW_IPC_MAX_SIZE: u32 = 8192;
/// `SW_IPC_MAX_SIZE - sizeof(DataHead)`: an `EventData`'s payload.
pub const SW_IPC_BUFFER_SIZE: usize = SW_IPC_MAX_SIZE as usize - 40;

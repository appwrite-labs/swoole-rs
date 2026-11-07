#ifndef SWOOLE_RS_PORTS_H
#define SWOOLE_RS_PORTS_H
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#ifdef __cplusplus
extern "C" {
#endif
enum { SWR_PORTS_DGRAM=1, SWR_PORTS_DTLS=2, SWR_PORTS_EOF=4, SWR_PORTS_LENGTH=8,
       SWR_PORTS_HTTP=16, SWR_PORTS_HTTP2=32, SWR_PORTS_WEBSOCKET=64, SWR_PORTS_MQTT=128, SWR_PORTS_REDIS=256 };
enum { SWR_PORTS_NONE, SWR_PORTS_READ_EOF, SWR_PORTS_LENGTH_DEFAULT, SWR_PORTS_HTTP_ONLY,
       SWR_PORTS_HTTP2_ONLY, SWR_PORTS_WEBSOCKET_ONLY, SWR_PORTS_HTTP_BOTH, SWR_PORTS_READ_MQTT,
       SWR_PORTS_READ_REDIS, SWR_PORTS_RAW, SWR_PORTS_LENGTH_CUSTOM };
typedef struct { const uint8_t *data; size_t len; } SwrPortsSpan;
typedef struct { int32_t index; SwrPortsSpan property; } SwrPortsEvent;
/* Getter uses Zend ASCII folding; registration's existing locale-sensitive ABI is unchanged.
 * Borrowed initialized bytes for this call. Returned property span is static immutable. */
SwrPortsSpan swr_v1_ports_callback_property(const uint8_t *, size_t);
/* Native binding IDs only; no callback address, execution, lifetime or mutable registry. */
enum { SWR_PORTS_HOOK_NONE, SWR_PORTS_HOOK_CONNECT, SWR_PORTS_HOOK_PACKET,
       SWR_PORTS_HOOK_CLOSE, SWR_PORTS_HOOK_BUFFER_FULL, SWR_PORTS_HOOK_BUFFER_EMPTY };
uint32_t swr_v1_ports_callback_hook(int32_t index);
uint32_t swr_v1_ports_protocol(uint32_t flags, uint8_t length_type);
const char *swr_v1_ports_protocol_name(uint32_t flags);

typedef struct { SwrPortsSpan name; int64_t value; uint32_t kind, is_private; } SwrPortsProperty;
enum { SWR_PORTS_PROPERTY_END, SWR_PORTS_PROPERTY_NULL, SWR_PORTS_PROPERTY_LONG, SWR_PORTS_PROPERTY_BOOL };
/* Immutable length-taking names/defaults in original declaration order. No NUL
 * promised or ownership transfer; each native Zend call follows Rust return. */
SwrPortsProperty swr_v1_ports_class_property(size_t index);
SwrPortsEvent swr_v1_ports_event(const uint8_t *, size_t, uint8_t native_casefold);
/* Readable callbacks (crates/ports/src/read.rs): Rust runs the whole callback, reads and
 * writes the current buffer and request in place through the SwrPortsShared that starts
 * the context, and asks native code for every other fact or effect through
 * call(context, op, arg). */
typedef struct { uint64_t length, size; int64_t offset; char *str; } SwrPortsString;
typedef struct {
    SwrPortsString *buffer;
    /* The connection's http_server::Request (crates/http/src/request.rs). */
    void *request;
    uint8_t websocket, upgraded, http2_stream, static_handler, trace;
} SwrPortsShared;
enum { SWR_PORTS_ON_READ_RAW=0, SWR_PORTS_ON_READ_LENGTH=1, SWR_PORTS_ON_READ_EOF=2, SWR_PORTS_ON_READ_REDIS=3,
       SWR_PORTS_ON_READ_HTTP=4, SWR_PORTS_OP_RECV=0, SWR_PORTS_OP_CATCH=1, SWR_PORTS_OP_WARN_RECV=2,
       SWR_PORTS_OP_CLOSE_ERRNO=3, SWR_PORTS_OP_CLOSE=4, SWR_PORTS_OP_SEND=5, SWR_PORTS_OP_RECV_BUFFER=6,
       SWR_PORTS_OP_RELEASE=7, SWR_PORTS_OP_RECV_LENGTH=8, SWR_PORTS_OP_RECV_EOF=9, SWR_PORTS_OP_RECV_REDIS=10,
       SWR_PORTS_OP_DISPATCH_TASK=11, SWR_PORTS_OP_DISPATCH_REQUEST=12, SWR_PORTS_OP_ACTIVE=13,
       SWR_PORTS_OP_WEBSOCKET_ACTIVATE=17, SWR_PORTS_OP_HTTP2_ENABLED=18, SWR_PORTS_OP_HTTP2_START=19,
       SWR_PORTS_OP_SKIP_RECV=20, SWR_PORTS_OP_MAX_LENGTH=21, SWR_PORTS_OP_UPLOADS=22, SWR_PORTS_OP_STATIC=23,
       SWR_PORTS_OP_BUF_REDUCE=36, SWR_PORTS_OP_BUF_EXTEND=38, SWR_PORTS_OP_BUF_HEADER_END=39, SWR_PORTS_OP_REQ_INIT=50,
       SWR_PORTS_OP_REQ_DESTROY=51, SWR_PORTS_OP_REQ_MULTIPART_INIT=63, SWR_PORTS_OP_CLIENT=89,
       SWR_PORTS_TRACE_HEADER=98, SWR_PORTS_TRACE_CHUNKED=99, SWR_PORTS_TRACE_POST_WAIT=100, SWR_PORTS_TRACE_CLOSE=101, SWR_PORTS_SEND_BAD_REQUEST=0,
       SWR_PORTS_SEND_TOO_LARGE=1, SWR_PORTS_SEND_UNAVAILABLE=2, SWR_PORTS_SEND_CONTINUE=3 };
typedef uint64_t (*SwrPortsCall)(void *context, uint32_t op, uint64_t arg);
/* `new http_server::Request()` on Rust's heap; ListenPort::destroy_http_request (Rust) frees it. */
void *swr_v1_http_request_new(uint64_t (*native)(uint32_t, void *, const uint8_t *, uint64_t, uint64_t));
int32_t swr_v1_ports_on_read(uint32_t kind, void *context, SwrPortsCall call);
/* TLS setup, SNI, import and close (crates/ports/src/lifecycle.rs), over the same call. */
enum { SWR_PORTS_RUN_SNI_ADD=0, SWR_PORTS_RUN_SSL_INIT=1, SWR_PORTS_RUN_SSL_CREATE=2, SWR_PORTS_RUN_CONTEXT_INIT=3,
       SWR_PORTS_RUN_SNI=4, SWR_PORTS_RUN_IMPORT=5, SWR_PORTS_RUN_CLOSE=6, SWR_PORTS_RUN_CONTEXT_CREATE=7, SWR_PORTS_LC_OP_CERT_EMPTY=0,
       SWR_PORTS_LC_OP_KEY_EMPTY=1, SWR_PORTS_LC_OP_LOG_REQUIRE_CERT=2, SWR_PORTS_LC_OP_OPEN_HTTP=3, SWR_PORTS_LC_OP_OPEN_HTTP2=4,
       SWR_PORTS_LC_OP_SET_HTTP=5, SWR_PORTS_LC_OP_SET_HTTP2=6, SWR_PORTS_LC_OP_CREATE=7, SWR_PORTS_LC_OP_WARN_CREATE=8,
       SWR_PORTS_LC_OP_SNI_EMPLACE=9, SWR_PORTS_LC_OP_HAS_SNI=10, SWR_PORTS_LC_OP_SET_SNI_CALLBACK=11, SWR_PORTS_LC_OP_SOCKET_SSL=12,
       SWR_PORTS_LC_OP_SET_CREATE_ERROR=13, SWR_PORTS_LC_OP_SET_EX_DATA=14, SWR_PORTS_LC_OP_WARN_EX_DATA=15, SWR_PORTS_LC_OP_ALLOW_EMPTY_CERT=16, SWR_PORTS_LC_OP_CONTEXT_NEW=20,
       SWR_PORTS_LC_OP_CONTEXT_FLAG=21, SWR_PORTS_LC_OP_CONTEXT_CIPHERS=22, SWR_PORTS_LC_OP_IS_DGRAM=23, SWR_PORTS_LC_OP_DTLS_SUPPORTED=24,
       SWR_PORTS_LC_OP_CONTEXT_DTLS=25, SWR_PORTS_LC_OP_WARN_NO_DTLS=26, SWR_PORTS_LC_OP_SERVER_NAME=30, SWR_PORTS_LC_OP_SSL_PORT=31,
       SWR_PORTS_LC_OP_SNI_EMPTY=32, SWR_PORTS_LC_OP_SNI_NEXT=33, SWR_PORTS_LC_OP_SNI_SELECT=34, SWR_PORTS_LC_OP_SOCKET_NEW=40,
       SWR_PORTS_LC_OP_GET_TYPE=41, SWR_PORTS_LC_OP_GET_NAME=42, SWR_PORTS_LC_OP_GET_ACCEPTCONN=43, SWR_PORTS_LC_OP_ACCEPTCONN=44,
       SWR_PORTS_LC_OP_WARN_IMPORT=45, SWR_PORTS_LC_OP_SOCKET_FAIL=46, SWR_PORTS_LC_OP_ADOPT=47, SWR_PORTS_LC_OP_SSL_ON=50,
       SWR_PORTS_LC_OP_HAS_CONTEXT=51, SWR_PORTS_LC_OP_RESET_CONTEXT=52, SWR_PORTS_LC_OP_DELETE_DTLS=53, SWR_PORTS_LC_OP_HAS_SOCKET=54,
       SWR_PORTS_LC_OP_FREE_SOCKET=55, SWR_PORTS_LC_OP_IS_UNIX=56, SWR_PORTS_LC_OP_UNLINK=57, SWR_PORTS_CONTEXT_PREFER_SERVER_CIPHERS=0,
       SWR_PORTS_CONTEXT_SESSION_TICKETS=1, SWR_PORTS_CONTEXT_STAPLING=2, SWR_PORTS_CONTEXT_STAPLING_VERIFY=3, SWR_PORTS_IMPORT_TYPE=0,
       SWR_PORTS_IMPORT_NAME=1, SWR_PORTS_IMPORT_ACCEPTCONN=2, SWR_PORTS_IMPORT_NOT_LISTENING=3 };
int32_t swr_v1_ports_run(uint32_t kind, void *context, SwrPortsCall call);
struct swr_v1_port_config;
int32_t swr_v1_ports_run_state(uint32_t kind, void *context, SwrPortsCall call, const struct swr_v1_port_config *config);
/* port.cc's runners, which the ListenPort methods crates/ports/src/methods.rs defines call:
 * one lifecycle kind over (port, target context, socket, SSL, fd or allow_empty_cert),
 * one readable callback kind, and one port fact (SWR_PORTS_FACT_*). */
enum { SWR_PORTS_FACT_REUSE_PORT = 0, SWR_PORTS_FACT_DTLS = 1 };
void swr_v1_ports_set_native(int32_t (*run)(uint32_t kind, void *port, void *target, void *sock, void *ssl, int number),
                             int32_t (*read)(void *reactor, void *port, void *event, uint32_t kind),
                             bool (*fact)(void *port, uint32_t which));
#ifdef __cplusplus
}
#endif
#endif

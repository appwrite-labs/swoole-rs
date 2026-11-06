#ifndef SWOOLE_RS_PORTS_H
#define SWOOLE_RS_PORTS_H
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#ifdef __cplusplus
extern "C" {
#endif
enum { SWR_PORTS_U16, SWR_PORTS_BUFFER, SWR_PORTS_U32, SWR_PORTS_I32, SWR_PORTS_U8 };
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
typedef struct { uint32_t count; int32_t first, last; } SwrPortsBufferSetting;
/* Signed clamped first store; optional second INT_MAX store. No pointers or state. */
SwrPortsBufferSetting swr_v1_ports_buffer_setting(int64_t value);
typedef struct { uint32_t value, status; size_t copy_len; } SwrPortsPacketSetting;
/* EOF: status1 empty after u8 publication,2 over-capacity,0 bounded copy.
 * Width/offset: status1 invalid/too-large. Native field publication precedes errors.
 * Width is the existing pure M19 helper result; no type table or pointer is copied. */
SwrPortsPacketSetting swr_v1_ports_packet_eof(size_t length, uint32_t capacity);
SwrPortsPacketSetting swr_v1_ports_packet_width(int32_t width);
SwrPortsPacketSetting swr_v1_ports_packet_offset(int64_t value, uint32_t limit);
enum { SWR_PORTS_OPTION_END, SWR_PORTS_OPTION_NUMBER, SWR_PORTS_OPTION_BUFFER,
       SWR_PORTS_OPTION_FLAG, SWR_PORTS_OPTION_SPECIAL };
enum { SWR_PORTS_CONVERT_LONG, SWR_PORTS_CONVERT_SIZE, SWR_PORTS_CONVERT_TRUTH, SWR_PORTS_CONVERT_STRING };
enum { SWR_PORTS_MISSING_SKIP, SWR_PORTS_MISSING_DEFAULT, SWR_PORTS_OPTION_ALWAYS };
enum {
    SWR_PORTS_NUMBER_BACKLOG = 1,
    SWR_PORTS_NUMBER_SOCKET_BUFFER = 2,
    SWR_PORTS_NUMBER_KERNEL_RECV = 3,
    SWR_PORTS_NUMBER_KERNEL_SEND = 4,
    SWR_PORTS_NUMBER_HEARTBEAT = 5,
    SWR_PORTS_NUMBER_HIGH_WATERMARK = 6,
    SWR_PORTS_NUMBER_LOW_WATERMARK = 7,
    SWR_PORTS_NUMBER_DEFER_ACCEPT = 8,
    SWR_PORTS_NUMBER_KEEP_IDLE = 9,
    SWR_PORTS_NUMBER_KEEP_INTERVAL = 10,
    SWR_PORTS_NUMBER_KEEP_COUNT = 11,
    SWR_PORTS_NUMBER_USER_TIMEOUT = 12,
    SWR_PORTS_NUMBER_MAX_LENGTH = 13,
};
enum {
    SWR_PORTS_SPECIAL_PACKET_EOF = 1,
    SWR_PORTS_SPECIAL_WEBSOCKET_OPTIONS = 2,
    SWR_PORTS_SPECIAL_MAX_IDLE = 3,
    SWR_PORTS_SPECIAL_PACKET_TYPE = 4,
    SWR_PORTS_SPECIAL_LENGTH_OFFSET = 5,
    SWR_PORTS_SPECIAL_BODY_OFFSET = 6,
    SWR_PORTS_SPECIAL_LENGTH_CALLBACK = 7,
};
typedef struct {
    SwrPortsSpan key, alias;
    uint32_t action, target, converter, number_kind, policy, reserved;
} SwrPortsOption;
/* Static immutable NUL-terminated key bytes, len excludes NUL. No PHP value/store.
 * action0 terminates; policy selects lazy missing/default/always handling.
 * Every instruction returns before native lookup/conversion/effects. */
SwrPortsOption swr_v1_ports_option(size_t index);
enum { SWR_PORTS_TLS_CONTEXT_FILES, SWR_PORTS_TLS_PORT };
enum { SWR_PORTS_TLS_CERT_FILE = 1, SWR_PORTS_TLS_KEY_FILE };
enum {
    SWR_PORTS_TLS_COMPRESS = 1,
    SWR_PORTS_TLS_PROTOCOLS = 2,
    SWR_PORTS_TLS_VERIFY_PEER = 3,
    SWR_PORTS_TLS_ALLOW_SELF_SIGNED = 4,
    SWR_PORTS_TLS_CLIENT_CERT = 5,
    SWR_PORTS_TLS_CAFILE = 6,
    SWR_PORTS_TLS_CAPATH = 7,
    SWR_PORTS_TLS_VERIFY_DEPTH = 8,
    SWR_PORTS_TLS_PREFER_CIPHERS = 9,
    SWR_PORTS_TLS_CIPHERS = 10,
    SWR_PORTS_TLS_ECDH_CURVE = 11,
    SWR_PORTS_TLS_DH_PARAM = 12,
};
/* group0: context file frontend, group1: Port method frontend before protected SNI.
 * Alias/number_kind/policy are unused zero/empty; no context or PHP value pointer. */
SwrPortsOption swr_v1_ports_tls_option(uint32_t group, size_t index);
/* Native-converted scalar for group1 only; native methods/crypto execute after return. */
int64_t swr_v1_ports_tls_value(uint32_t target, int64_t value);
typedef struct { SwrPortsSpan key; uint32_t status, reserved; } SwrPortsSniPlan;
/* stage0 static key; stage1 top-array admission; stage2 key-before-item-array admission.
 * status1 top not array,2 missing string key,3 item not array. No name/context pointer.
 * Native iteration/duplication/file/installation/matching execute outside Rust. */
SwrPortsSniPlan swr_v1_ports_sni_plan(uint32_t stage, uint32_t has_key, uint32_t is_array);
int64_t swr_v1_ports_number(uint32_t kind, int64_t value);
typedef struct {
    uint32_t object_id, length_type, length_size, body_offset, max_length, eof_length;
} SwrPortsDefaults;
/* Native build maximum and bounded EOF literal length; native copy/publication after return. */
SwrPortsDefaults swr_v1_ports_defaults(uint32_t max_length, uint32_t eof_length);
enum { SWR_PORTS_EOF_SETTINGS, SWR_PORTS_LENGTH_SETTINGS };
typedef struct {
    uint32_t kind, find_from_right;
    size_t length, capacity;
    uint32_t length_type;
    int32_t length_size;
    uint32_t length_offset, body_offset;
} SwrPortsFramingInput;
typedef struct {
    size_t length;
    uint32_t enabled, split, length_type, length_size, length_offset, body_offset;
} SwrPortsFramingSettings;
/* Native buffer/type-size facts in; initialized scalar field plan out. No pointers. */
SwrPortsFramingSettings swr_v1_ports_framing_settings(SwrPortsFramingInput);
/* Fixed network-u32 stream framing, native default-length callback binding. */
SwrPortsFramingSettings swr_v1_ports_stream_settings(void);
uint32_t swr_v1_ports_protocol(uint32_t flags, uint8_t length_type);
const char *swr_v1_ports_protocol_name(uint32_t flags);
/* The SWR_PORTS_* protocol flags of a ListenPort's settings, with SWR_PORTS_DGRAM. */
uint32_t swr_v1_ports_flags(void *port);
typedef struct {
    uint32_t flags, length_type, eof_len, eof_max, http2_header_size, websocket_header_size, mqtt_header_size;
} SwrPortsProtocolInput;
typedef struct { uint32_t kind, value; } SwrPortsProtocolAction;
typedef struct { uint32_t count; SwrPortsProtocolAction actions[6]; } SwrPortsProtocolPlan;
enum { SWR_PORTS_SET_EOF_LEN=1, SWR_PORTS_SET_READ, SWR_PORTS_SET_PACKAGE,
       SWR_PORTS_SET_LENGTH_FN, SWR_PORTS_SET_LENGTH_SIZE_FN, SWR_PORTS_SET_LENGTH_SIZE,
       SWR_PORTS_SET_LENGTH_OFFSET, SWR_PORTS_SET_BODY_OFFSET,
       SWR_PORTS_SET_NODELAY, SWR_PORTS_SET_KEEPALIVE, SWR_PORTS_SET_CHECK_EOF,
       SWR_PORTS_SET_SPLIT_EOF, SWR_PORTS_SET_HTTP_FLAG, SWR_PORTS_SET_WEBSOCKET_FLAG,
       SWR_PORTS_SET_HTTP2_FLAG, SWR_PORTS_SET_MQTT_FLAG, SWR_PORTS_SET_REDIS_FLAG,
       SWR_PORTS_SET_FASTOPEN, SWR_PORTS_SET_CHECK_LENGTH };
/* One native PHP conversion's scalar value/presence; no pointer or copied Port state.
 * At most two ordered writes, remaining actions zero. Missing NODELAY defaults true. */
SwrPortsProtocolPlan swr_v1_ports_flag_setting(uint32_t kind, int64_t value, uint32_t present);
/* Read bindings:1 EOF,2 length,3 HTTP,4 Redis,5 raw.
 * Package bindings:1 task,2 HTTP frame,3 websocket frame.
 * Length bindings:1 default,2 HTTP,3 HTTP2,4 websocket,5 MQTT. No address crosses Rust. */
SwrPortsProtocolPlan swr_v1_ports_protocol_plan(SwrPortsProtocolInput);

typedef struct { SwrPortsSpan name; int64_t value; uint32_t kind, is_private; } SwrPortsProperty;
enum { SWR_PORTS_PROPERTY_END, SWR_PORTS_PROPERTY_NULL, SWR_PORTS_PROPERTY_LONG, SWR_PORTS_PROPERTY_BOOL };
/* Immutable length-taking names/defaults in original declaration order. No NUL
 * promised or ownership transfer; each native Zend call follows Rust return. */
SwrPortsProperty swr_v1_ports_class_property(size_t index);
SwrPortsEvent swr_v1_ports_event(const uint8_t *, size_t, uint8_t native_casefold);
typedef struct { uint32_t phase; } SwrPortsListenState;
typedef struct {
    uint32_t capabilities, listening, keepalive, high_watermark;
    int32_t defer_accept, fastopen, keepidle, keepinterval, keepcount, user_timeout, result;
} SwrPortsListenInput;
typedef struct { uint32_t kind; int32_t value, status; } SwrPortsListenAction;
SwrPortsListenAction swr_v1_ports_listen_step(SwrPortsListenState *, const SwrPortsListenInput *);
typedef struct { uint32_t phase; } SwrPortsSocketState;
typedef struct { uint32_t capabilities, socket_present; int32_t result; } SwrPortsSocketInput;
typedef struct { uint32_t kind; int32_t status; } SwrPortsSocketAction;
SwrPortsSocketAction swr_v1_ports_socket_step(SwrPortsSocketState *, const SwrPortsSocketInput *);
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
 * and one readable callback kind. */
void swr_v1_ports_set_native(int32_t (*run)(uint32_t kind, void *port, void *target, void *sock, void *ssl, int number),
                             int32_t (*read)(void *reactor, void *port, void *event, uint32_t kind));
#ifdef __cplusplus
}
#endif
#endif

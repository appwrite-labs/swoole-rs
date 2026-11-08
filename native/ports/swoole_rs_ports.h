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
typedef uint64_t (*SwrPortsCall)(void *context, uint32_t op, uint64_t arg);
/* TLS setup, SNI, import and close (crates/ports/src/lifecycle.rs): Rust decides each
 * step and asks native code for every fact or effect through call(context, op, arg). */
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
/* master.cc's runners, which the ListenPort methods crates/ports/src/methods.rs defines call:
 * one lifecycle kind over (port, target context, socket, SSL, fd or allow_empty_cert),
 * and one port fact (SWR_PORTS_FACT_*). */
enum { SWR_PORTS_FACT_REUSE_PORT = 0, SWR_PORTS_FACT_DTLS = 1 };
void swr_v1_ports_set_native(int32_t (*run)(uint32_t kind, void *port, void *target, void *sock, void *ssl, int number),
                             bool (*fact)(void *port, uint32_t which));
#ifdef __cplusplus
}
#endif
#endif

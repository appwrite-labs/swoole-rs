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
#ifdef __cplusplus
}
#endif
#endif

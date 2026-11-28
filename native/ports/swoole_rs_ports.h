#ifndef SWOOLE_RS_PORTS_H
#define SWOOLE_RS_PORTS_H
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct { const uint8_t *data; size_t len; } SwrPortsSpan;
typedef struct { int32_t index; SwrPortsSpan property; } SwrPortsEvent;
/* Getter uses Zend ASCII folding; registration's existing locale-sensitive ABI is unchanged.
 * Borrowed initialized bytes for this call. Returned property span is static immutable. */
SwrPortsSpan swr_v1_ports_callback_property(const uint8_t *, size_t);
SwrPortsEvent swr_v1_ports_event(const uint8_t *, size_t, uint8_t native_casefold);
#ifdef __cplusplus
}
#endif
#endif

#ifndef SWOOLE_RS_PROTOCOLS_H
#define SWOOLE_RS_PROTOCOLS_H
#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#include <sys/types.h>
#include <sys/uio.h>
typedef struct swr_v1_core_proxy swr_v1_core_proxy;
#ifdef __cplusplus
extern "C" {
#endif
/* Borrowed byte ranges; no allocation, callbacks, retained pointers or I/O. */
int64_t swr_v1_mqtt_frame_length(const uint8_t *, size_t);
typedef struct { uint8_t state, action, error, reply; } swr_socks_transition;
int32_t swr_v1_socks_transition(uint8_t state, uint8_t method, const uint8_t *, size_t, swr_socks_transition *);
int64_t swr_v1_socks_negotiate(uint8_t has_user, uint8_t *method, uint8_t *output, size_t capacity);
int64_t swr_v1_socks_auth(const uint8_t *user, size_t user_length, const uint8_t *password, size_t password_length, uint8_t *output, size_t capacity);
/* -2: domain (type 3) over 255 bytes, before any write. */
int64_t swr_v1_socks_connect(uint8_t address_type, const uint8_t *address, size_t length, uint16_t port, uint8_t *output, size_t capacity);
/* Factory:0 rejected,1 no credentials,2 both credentials; max250 each. */
int32_t swr_v1_socks_config(size_t user_length, size_t password_length);
/* Static borrowed string; never free. */
const char *swr_v1_socks_strerror(int32_t code);
typedef struct { size_t offset, length; int32_t integer; uint32_t kind; } swr_resp_span;
/* All-zero state begins a borrowed walk. No callback/allocation in Rust.
 * 1 event,0 end,-1 invalid; invalid leaves state/span unchanged.
 * Event kind0 string parameter,1 integer,2 null,3 first string command. */
typedef struct { size_t position; int32_t length; uint8_t state; } swr_resp_cursor;
typedef struct { swr_resp_cursor cursor; uint8_t command_seen; } swr_resp_php_state;
int32_t swr_v1_resp_php_next(const uint8_t *, size_t, swr_resp_php_state *, swr_resp_span *, uint8_t *range);
/* Fixed64-byte NUL-terminated handler key; returns length,0 on invalid input.
 * Existing PHP raw-length validation remains native; empty input is allowed. */
size_t swr_v1_resp_handler_key(const uint8_t *, size_t, uint8_t *output);
int64_t swr_v1_resp_parse(const uint8_t *, size_t, swr_resp_span *, size_t capacity);
/* Optional range byte reports signed-long overflow without touching native errno. */
int64_t swr_v1_resp_parse_range(const uint8_t *, size_t, swr_resp_span *, size_t capacity, uint8_t *range);
/* Single scan: writes a bounded prefix, returns full required count; -1 on error.
 * Unlike parse, output may be partially written on error. */
int64_t swr_v1_resp_scan(const uint8_t *, size_t, swr_resp_span *, size_t capacity, uint8_t *range);
size_t swr_v1_resp_format(uint8_t kind, const uint8_t *, size_t, int64_t integer, uint8_t *, size_t);
typedef struct {
    uint8_t state;
    int32_t lines_total, lines_received, bytes_total, bytes_received, offset;
} swr_resp_request;
int32_t swr_v1_resp_receive(const uint8_t *, size_t length, size_t capacity, size_t maximum,
                          swr_resp_request *, uint8_t *set_offset);
int32_t swr_v1_resp_receive_range(const uint8_t *, size_t length, size_t capacity, size_t maximum,
                                swr_resp_request *, uint8_t *set_offset, uint8_t *range);
/* After receive action 1: new buffer size, or 0 to keep reading without growth. */
size_t swr_v1_resp_extend_size(size_t length, size_t size, size_t maximum, size_t page);
/* redis::recv_packet's loop (crates/protocols-ffi/src/resp_recv.rs) over the caller's
 * effects. buffer is a swoole::String's head; *request is null or from sw_malloc.
 * recv: bytes received, -1 to close (SW_ERR), -2 to stop (SW_OK).
 * dispatch: onPackage of the first length bytes: <0 failed, 1 socket removed, else 0. */
typedef struct {
    void *context;
    ssize_t (*recv)(void *context, uint8_t *at, size_t n);
    void (*extend)(void *context, size_t size);
    int (*dispatch)(void *context, size_t length);
} swr_resp_io;
int swr_v1_resp_recv(const swr_resp_io *, void *buffer, swr_resp_request **request, uint32_t maximum, size_t page);
/* MessageBus (crates/bus), for crates/process' send_message. */
typedef struct swr_bus swr_bus;
#ifdef __cplusplus
}
#endif
#endif

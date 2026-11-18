#ifndef SWOOLE_RS_REACTOR_H
#define SWOOLE_RS_REACTOR_H
#include <stdint.h>
#include <stdbool.h>

/* network::Socket's write-path fields, embedded in Socket in this order and with these
 * types (Socket static_asserts it). */
typedef struct {
    void *out_buffer;
    uint64_t send_timer;
    uint32_t buffer_size;
    bool close_wait;
    bool nonblock;
    bool ssl_send;
    bool send_wait;
} swr_v1_socket_output;
/* network::Socket's timeout group (dns_timeout, connect_timeout, read_timeout,
 * write_timeout), embedded in this order (Socket static_asserts it). */
typedef struct { double dns, connect, read, write; } swr_v1_socket_timeouts;
/* network::Socket's TLS state group (ssl_state, ssl_want_read, ssl_want_write),
 * embedded in this order and these types (Socket static_asserts it). The calls below
 * that take it set it. */
typedef struct {
    uint32_t state;
    bool want_read;
    bool want_write;
} swr_v1_socket_tls;

#endif

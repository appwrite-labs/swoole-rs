#ifndef SWOOLE_RS_REACTOR_H
#define SWOOLE_RS_REACTOR_H
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <sys/types.h>
#include <sys/socket.h>
#ifdef __APPLE__
#include <TargetConditionals.h>
#endif
#ifdef __cplusplus
namespace swoole { class Reactor; namespace network { struct Socket; } }
typedef int (*swr_v1_reactor_close_fn)(swoole::Reactor *, swoole::network::Socket *);
extern "C" {

#endif
#ifndef __cplusplus
typedef int (*swr_v1_reactor_close_fn)(void *, void *);
#endif

struct pollfd;
struct epoll_event;
struct kevent;

/* network::Socket's write-path fields, embedded in Socket in this order and with these
 * types (Socket static_asserts it). */
typedef struct swr_v1_buffer swr_v1_buffer;
typedef struct {
    void *out_buffer;
    uint64_t send_timer;
    uint32_t buffer_size;
    bool close_wait;
    bool nonblock;
    bool ssl_send;
    bool send_wait;
} swr_v1_socket_output;
/* Reactor handler tables (read/write/error per fd type, write and error defaults), which
 * only Rust reads, of Rust handlers. */
typedef struct swr_v1_reactor_handlers swr_v1_reactor_handlers;
/* The fd -> socket registry (Reactor::sockets_), which only Rust reads. */
typedef struct swr_v1_reactor_registry swr_v1_reactor_registry;
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

/* Rust's reactor state (crates/reactor's ReactorState), which begins with swr_v1_reactor_head:
 * a Reactor is that state. swoole_event_init/free and the Reactor's constructor and destructor
 * are Rust's; nothing in C or C++ allocates one. */
typedef struct swr_v1_reactor_state swr_v1_reactor_state;
/* Reactor's idle and future tasks (Swoole\Event::cycle): null until set. */
typedef struct { void (*callback)(void *); void *data; } swr_v1_reactor_task;
/* The state's own views of itself; fields points at the head's type_. */
typedef struct {
    swr_v1_reactor_state *owner; void *fields;
    swr_v1_reactor_registry *registry; swr_v1_reactor_handlers *handlers;
} swr_v1_reactor_prepared;
/* Rust's ReactorPrepared then the native prefix of ReactorFields (asserted on both sides). */
typedef struct {
    swr_v1_reactor_prepared state_;
    int type_; void *ptr; int native_handle; uint32_t max_event_num;
    bool ready_, running, start, once, wait_exit, destroyed, bailout, timed_out;
    int32_t timeout_msec; uint16_t id; uint32_t max_socket; time_t last_malloc_trim_time;
    swr_v1_reactor_close_fn close;
    swr_v1_reactor_task idle_task, future_task;
} swr_v1_reactor_head;
#ifdef __cplusplus
}
static_assert(offsetof(swr_v1_reactor_head, type_) == sizeof(swr_v1_reactor_prepared) &&
                  (sizeof(void *) != 8 || (sizeof(swr_v1_reactor_prepared) == 32 && sizeof(swr_v1_reactor_head) == 128)),
              "crates/reactor's ReactorState layout");
#endif
#endif

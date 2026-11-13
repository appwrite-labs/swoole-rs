// C entry points for the Rust cases in src/main.rs (upstream's core-tests/src/reactor/base.cpp):
// the Reactor's members and callback slots, the SocketPair classes and the
// C++-linkage event, hook, signal and timer APIs. Every assertion and callback body is Rust's.
// Compiled into system.py's lib-swoole by tests/modules/hooks/system.py.
#include <stddef.h>

#include "swoole_api.h"
#include "swoole_pipe.h"
#include "swoole_reactor.h"
#include "swoole_signal.h"

using namespace swoole;
using network::Socket;

typedef void (*Plain)(void *);
typedef void (*End)(Reactor *);

extern "C" {
// The error codes the cases expect, by their upstream names (extern: a namespace-scope const is
// otherwise internal).
extern const int swr_case_error_operation_not_support = SW_ERROR_OPERATION_NOT_SUPPORT;
extern const int swr_case_error_output_buffer_overflow = SW_ERROR_OUTPUT_BUFFER_OVERFLOW;
extern const int swr_case_error_package_length_too_large = SW_ERROR_PACKAGE_LENGTH_TOO_LARGE;
extern const int swr_case_error_socket_not_exists = SW_ERROR_SOCKET_NOT_EXISTS;
extern const int swr_case_error_event_remove_failed = SW_ERROR_EVENT_REMOVE_FAILED;
// network::Socket's field offsets, which src/main.rs's Socket mirrors.
extern const size_t swr_case_socket_offsets[] = {
    offsetof(swr_v1_socket, fd),          offsetof(swr_v1_socket, fd_type),  offsetof(swr_v1_socket, events),
    offsetof(swr_v1_socket, removed),     offsetof(swr_v1_socket, buffer_size), offsetof(swr_v1_socket, nonblock),
    offsetof(swr_v1_socket, object),      offsetof(swr_v1_socket, ssl)};

// The Reactor members the cases read.
struct swr_case_reactor_view {
    int type;
    uint32_t max_event_num;
    bool ready;
    bool timed_out;
    const void *write;
    const void *close;
    const void *defer_tasks;
    const void *default_write_handler;
    void *ptr;
};

Reactor *swr_case_tg_reactor() {
    return SwooleTG.reactor;
}
void swr_case_set_tg_reactor(Reactor *reactor) {
    SwooleTG.reactor = reactor;
}
swr_case_reactor_view swr_case_reactor_view_of(const Reactor *r) {
    return {r->type_,
            r->max_event_num,
            r->ready(),
            r->timed_out,
            reinterpret_cast<const void *>(r->write),
            reinterpret_cast<const void *>(r->close),
            r->defer_tasks,
            reinterpret_cast<const void *>(r->default_write_handler()),
            r->ptr};
}
// wait_exit, once and running take 0 or 1; -1 leaves the field.
void swr_case_reactor_flags(Reactor *r, int wait_exit, int once, int running) {
    if (wait_exit >= 0) r->wait_exit = wait_exit;
    if (once >= 0) r->once = once;
    if (running >= 0) r->running = running;
}
void swr_case_reactor_set_max_event_num(Reactor *r, uint32_t n) {
    r->max_event_num = n;
}
void swr_case_reactor_set_timeout_msec(Reactor *r, int msec) {
    r->set_timeout_msec(msec);
}
void swr_case_reactor_set_ptr(Reactor *r, void *ptr) {
    r->ptr = ptr;
}
// op: 0 add, 1 set, 2 del.
int swr_case_reactor_change(Reactor *r, Socket *socket, int op, int events) {
    return op == 0 ? r->add(socket, events) : op == 1 ? r->set(socket, events) : r->del(socket);
}
int swr_case_reactor_wait(Reactor *r) {
    return r->wait();
}
void swr_case_reactor_set_handler(Reactor *r, int fd_type, int event, void *handler) {
    r->set_handler(fd_type, event, reinterpret_cast<ReactorHandler>(handler));
}
bool swr_case_reactor_isset_handler(Reactor *r, int fd_type, int event) {
    return r->isset_handler(fd_type, event);
}
void *swr_case_reactor_get_handler(Reactor *r, int fd_type, int event) {
    return reinterpret_cast<void *>(r->get_handler(static_cast<FdType>(fd_type), static_cast<EventType>(event)));
}
bool swr_case_reactor_exists_fd(Reactor *r, int fd) {
    return r->exists(fd);
}
size_t swr_case_reactor_event_num(Reactor *r) {
    return r->get_event_num();
}
// End callbacks are plain functions; the one case that sets one runs it on its reactor.
static Reactor *end_reactor;
static End end_fn;
void swr_case_reactor_set_end_callback(Reactor *r, int id, End fn) {
    end_reactor = r;
    end_fn = fn;
    r->set_end_callback(static_cast<Reactor::EndCallback>(id), [] { end_fn(end_reactor); });
}
void swr_case_reactor_erase_end_callback(Reactor *r, int id) {
    r->erase_end_callback(static_cast<Reactor::EndCallback>(id));
}
void swr_case_reactor_defer(Reactor *r, Plain fn, void *data) {
    r->defer(fn, data);
}
// which: 0 future_task, 1 idle_task.
void swr_case_reactor_set_task(Reactor *r, int which, Plain fn, void *data) {
    swr_v1_reactor_task &task = which == 0 ? r->future_task : r->idle_task;
    task.callback = fn;
    task.data = data;
}
void swr_case_reactor_drain_write_buffer(Reactor *r, Socket *socket) {
    r->drain_write_buffer(socket);
}
Socket *swr_case_event_socket(const Event *event) {
    return event->socket;
}

int swr_case_event_init(int flags) {
    return swoole_event_init(flags);
}
int swr_case_event_free() {
    return swoole_event_free();
}
int swr_case_event_wait() {
    return swoole_event_wait();
}
int swr_case_event_add(Socket *socket, int events) {
    return swoole_event_add(socket, events);
}
int swr_case_event_del(Socket *socket) {
    return swoole_event_del(socket);
}
int swr_case_event_add_or_update(Socket *socket, int event) {
    return swoole_event_add_or_update(socket, event);
}
ssize_t swr_case_event_write(Socket *socket, const void *data, size_t len) {
    return swoole_event_write(socket, data, len);
}
ssize_t swr_case_event_writev(Socket *socket, const iovec *iov, size_t iovcnt) {
    return swoole_event_writev(socket, iov, iovcnt);
}
Socket *swr_case_event_get_socket(int fd) {
    return swoole_event_get_socket(fd);
}
void swr_case_event_set_handler(int fd_type, int event, void *handler) {
    swoole_event_set_handler(fd_type, event, reinterpret_cast<ReactorHandler>(handler));
}
bool swr_case_event_is_available() {
    return swoole_event_is_available();
}

// kind: 0 UnixSocket(blocking, protocol), 1 Pipe(blocking).
SocketPair *swr_case_pair_new(int kind, bool blocking, int protocol) {
    if (kind == 0) return new UnixSocket(blocking, protocol);
    return new Pipe(blocking);
}
void swr_case_pair_delete(SocketPair *pair) {
    delete pair;
}
bool swr_case_pair_ready(const SocketPair *pair) {
    return pair->ready();
}
Socket *swr_case_pair_socket(const SocketPair *pair, bool master) {
    return pair->get_socket(master);
}
ssize_t swr_case_pair_write(const SocketPair *pair, const void *data, size_t len) {
    return pair->write(data, len);
}
void swr_case_pair_set_blocking(SocketPair *pair, bool blocking) {
    pair->set_blocking(blocking);
}
bool swr_case_unix_socket_set_buffer_size(const SocketPair *pair, size_t size) {
    return static_cast<const UnixSocket *>(pair)->set_buffer_size(size);
}
bool swr_case_pair_close(SocketPair *pair, int which) {
    return pair->close(which);
}

// `network::Socket s;` and `network::Socket s{}` alike: a default-constructed record.
Socket *swr_case_socket_new() {
    return new Socket();
}
void swr_case_socket_delete(Socket *socket) {
    delete socket;
}
Socket *swr_case_make_socket(int fd, int fd_type) {
    return make_socket(fd, static_cast<FdType>(fd_type));
}
void swr_case_socket_free(Socket *socket) {
    socket->free();
}
int swr_case_socket_move_fd(Socket *socket) {
    return socket->move_fd();
}
uint32_t swr_case_socket_out_buffer_length(const Socket *socket) {
    return socket->get_out_buffer_length();
}
int swr_case_socket_wait_event(const Socket *socket, int timeout_ms, int events) {
    return socket->wait_event(timeout_ms, events);
}

void swr_case_add_hook(int type, Plain fn) {
    swoole_add_hook(static_cast<swGlobalHookType>(type), fn, 1);
}
void swr_case_clear_last_error() {
    swoole_clear_last_error();
}
void swr_case_set_print_backtrace_on_error(bool enable) {
    swoole_set_print_backtrace_on_error(enable);
}
void swr_case_signal_set(int signo, void (*handler)(int), int restart, int mask) {
    swoole_signal_set(signo, handler, restart, mask);
}
void swr_case_signal_block_all() {
    swoole_signal_block_all();
}
size_t swr_case_random_bytes(char *buf, size_t size) {
    return swoole_random_bytes(buf, size);
}
String *swr_case_tg_buffer() {
    return sw_tg_buffer();
}
void swr_case_string_grow(String *str, size_t n) {
    str->grow(n);
}
void swr_case_string_append(String *str, const char *data, size_t len) {
    str->append(data, len);
}
}

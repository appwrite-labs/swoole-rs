// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/system.cc (Swoole contributors).
// What crates/hooks can't express of System and coroutine::Socket (upstream's
// src/coroutine/system.cc is deleted): the host tables whose entries need C++ types or
// configure's macros. native/hooks/contract.md lists what stays and why.
#include "swoole_coroutine_system.h"
#include "swoole_rs_hooks.h"
#include "swoole_util.h"

#include "swoole_signal.h"
#include "swoole_rs_uring.h"
#include "swoole_rs_coroutine_socket.h"

namespace swoole {
namespace coroutine {

static_assert(SW_EVENT_READ == 1 << 9 && SW_EVENT_WRITE == 1 << 10, "swr_v1_hook_socket_* event bits");

// What the Rust operations can't do without C++ types (crates/hooks/src/co_io.rs): the
// timers (the packet gate fakes them), the yield's std::function cancel, the String
// buffers and the Protocol callback.
const swr_v1_hook_co_socket_host_t Socket::host_{
    [](swr_v1_co_socket *s, double timeout) -> void * {
        return swoole_timer_add(timeout, false, [](Timer *, TimerNode *tnode) {
            swr_v1_hook_co_socket_timer_fired(static_cast<Socket *>(tnode->data), tnode);
        }, static_cast<Socket *>(s));
    },
    [](void *node) { swoole_timer_del(static_cast<TimerNode *>(node)); },
    [](swr_v1_co_socket *s, void *co, uint32_t event) {
        auto *sock = static_cast<Socket *>(s);
        Coroutine::CancelFunc cancel_fn = [sock, event](Coroutine *) {
            return sock->cancel(static_cast<EventType>(event));
        };
        static_cast<Coroutine *>(co)->yield(&cancel_fn);
    },
    [](swr_v1_co_socket *s, char **buf, size_t n) {
        auto *sock = static_cast<Socket *>(s);
        if (!sock->zero_copy && n > 0 && *buf != sock->get_write_buffer()->str) {
            sock->write_buffer->clear();
            sock->write_buffer->append(*buf, n);
            *buf = sock->write_buffer->str;
        }
    },
    [](swr_v1_co_socket *s, swr_v1_hook_read_buffer *view, uint32_t action, size_t value) {
        auto *sock = static_cast<Socket *>(s);
        String *buffer = action == 0 ? sock->get_read_buffer() : sock->read_buffer;
        if (action != 0) {
            buffer->length = view->length;
            buffer->offset = view->offset;
            if (action == 2) { buffer->extend(value); }
            if (action == 3) { buffer->reduce(value); }
        }
        *view = {buffer->length, buffer->size, static_cast<int64_t>(buffer->offset), buffer->str};
    },
    [](swr_v1_co_socket *s, char *at, size_t n, bool all) {
        auto *sock = static_cast<Socket *>(s);
        return all ? sock->recv_all(at, n) : sock->recv(at, n);
    },
    [](swr_v1_co_socket *s, uint32_t size, uint32_t *header_len) {
        PacketLength pl;
        pl.header_len = 0;
        pl.buf = s->read_buffer->str;
        pl.buf_size = size;
        ssize_t length = s->protocol.get_package_length(&s->protocol, s->socket, &pl);
        swoole_trace_log(SW_TRACE_SOCKET, "packet_len=%ld, length=%u", length, size);
        *header_len = pl.header_len;
        return length;
    },
    [](swr_v1_co_socket *s, uint32_t kind, ssize_t length) {
        auto *sock = static_cast<Socket *>(s);
        if (kind == 0) {
            sock->set_err(SW_ERROR_PACKAGE_LENGTH_NOT_FOUND, "get package length failed");
        } else if (kind == 1) {
            swoole_log_as("Socket::recv_packet_with_length_protocol",
                          swoole_error_log(SW_LOG_WARNING, SW_ERROR_PACKAGE_LENGTH_TOO_LARGE,
                                           "packet length is too big, remote_addr=%s:%d, length=%zu",
                                           s->socket->get_addr(), s->socket->get_port(), length));
            sock->set_err(SW_ERROR_PACKAGE_LENGTH_TOO_LARGE, sw_error);
        } else {
            sock->set_err(SW_ERROR_PACKAGE_LENGTH_TOO_LARGE, "no package eof, package_max_length exceeded");
        }
    },
};
const bool Socket::host_installed_ = (swr_v1_hook_co_socket_host(&Socket::host_), true);

static_assert(SW_FD_CO_SOCKET == 6 && SW_SOCK_RAW == 7 && SW_BUFFER_SIZE_BIG == 65536 &&
                  SW_INPUT_BUFFER_SIZE == 2 * 1024 * 1024,
              "swr_v1_hook_co_socket constants");

}  // namespace coroutine
}  // namespace swoole

// hook.cc's socket functions are Rust (crates/hooks/src/hooked.rs), as are the hooked
// file, sleep and resolver functions (stdio.rs); this is what they need of native: the
// io_uring engine and configure's io_uring switches.
static const swr_v1_hook_host hook_host = {
#ifdef SW_USE_IOURING
    &swr_v1_uring_native,
#else
    nullptr,
#endif
#ifdef HAVE_IOURING_STATX
    true,
#else
    false,
#endif
#ifdef HAVE_IOURING_FTRUNCATE
    true,
#else
    false,
#endif
#ifdef HAVE_IOURING_FUTEX
    true,
#else
    false,
#endif
#ifdef SW_USE_URING_SOCKET
    true,
#else
    false,
#endif
};

[[maybe_unused]] static const bool hook_host_set = (swr_v1_hook_set_host(&hook_host), true);

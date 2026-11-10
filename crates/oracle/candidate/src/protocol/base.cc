/*
 +----------------------------------------------------------------------+
 | Swoole                                                               |
 +----------------------------------------------------------------------+
 | Copyright (c) 2012-2017 The Swoole Group                             |
 +----------------------------------------------------------------------+
 | This source file is subject to version 2.0 of the Apache license,    |
 | that is bundled with this package in the file LICENSE, and is        |
 | available through the world-wide-web at the following url:           |
 | http://www.apache.org/licenses/LICENSE-2.0.html                      |
 | If you did not receive a copy of the Apache2.0 license and are unable|
 | to obtain it through the world-wide-web, please send a note to       |
 | license@swoole.com so we can mail you a copy immediately.            |
 +----------------------------------------------------------------------+
 | Author: Tianfeng Han  <rango@swoole.com>                             |
 +----------------------------------------------------------------------+
 */

#include "swoole_string.h"
#include "swoole_socket.h"
#include "swoole_protocol.h"
#include "swoole_rs_framing.h"
#include <cstddef>
#include <cstdlib>
#include <exception>
static_assert(sizeof(int) == 4 && sizeof(size_t) == 8 && SW_BUFFER_SIZE_STD == 8192, "framing platform ABI");
namespace swoole {
#define proto_error(_sock, _error, _fmt, ...) \
    network::Address _remote_addr; \
    if (_sock->get_peer_name(&_remote_addr) < 0) { _remote_addr = _sock->info; } \
    swoole_error_log(SW_LOG_WARNING, _error, _fmt "<%s:%d>", ##__VA_ARGS__, _remote_addr.get_addr(), _remote_addr.get_port())
ssize_t Protocol::default_length_func(const Protocol *p, network::Socket *socket, PacketLength *pl) {
    uint16_t offset = p->package_length_offset;
    uint8_t size = p->get_package_length_size ? p->get_package_length_size(socket) : p->package_length_size;
    int64_t value = 0;
    int result = swr_v1_framing_default(p->package_length_type, (const uint8_t *) pl->buf, pl->buf_size,
                                      offset, size, p->package_body_offset, &value);
    if (result == -2) {
        swoole_error_log(SW_LOG_ERROR, SW_ERROR_INVALID_PARAMS, "Invalid format specifier '%c'", p->package_length_type);
    }
    if (result < 0) { return SW_ERR; }
    if (result == 0) { pl->header_len = value; return 0; }
    swoole_debug("length=%ld", value);
    pl->header_len = p->package_length_size;
    return value;
}
// Rust reads and writes the String head and these Protocol fields in place.
static_assert(offsetof(String, length) == 0 && offsetof(String, size) == 8 && offsetof(String, offset) == 16 &&
              offsetof(String, str) == 24 && sizeof(off_t) == 8, "SwrFramingIo buffer layout");
static_assert(offsetof(Protocol, split_by_eof) == 0 && offsetof(Protocol, package_eof) == 1 &&
              offsetof(Protocol, package_eof_len) == 9 && offsetof(Protocol, package_length_offset) == 12 &&
              offsetof(Protocol, package_max_length) == 16 && sizeof(bool) == 1, "SwrFramingIo protocol layout");
namespace {
struct Recv {
    SwrFramingIo io;
    const Protocol *p;
    network::Socket *socket;
    String *buffer;
    std::exception_ptr error;
};
uint32_t socket_flags(network::Socket *s) {
    return (s->removed ? SWR_FRAMING_REMOVED : 0) | (s->recv_wait ? SWR_FRAMING_WAIT : 0) | (s->ssl ? SWR_FRAMING_TLS : 0);
}
// The effects a receive rarely needs, kept out of `call`'s frame.
__attribute__((noinline)) int64_t rare(Recv *r, uint32_t op, uint64_t arg) {
    const Protocol *p = r->p; network::Socket *socket = r->socket; String *buffer = r->buffer;
    switch (op) {
    case SWR_FRAMING_OP_RECV:
        switch (socket->catch_read_error(errno)) {
        case SW_ERROR:
            if (r->io.mode == SWR_FRAMING_MODE_LENGTH) { swoole_sys_warning("recv(%d, %d) failed", socket->fd, (uint32_t) arg); }
            else { swoole_sys_warning("recv from socket#%d failed", socket->fd); }
            return SWR_FRAMING_RECV_DONE;
        case SW_CLOSE: return SWR_FRAMING_RECV_CLOSE;
        default: return SWR_FRAMING_RECV_DONE;
        }
    case SWR_FRAMING_OP_REDUCE: buffer->reduce(arg); return 0;
    case SWR_FRAMING_OP_SPLIT:
        return buffer->split(p->package_eof, p->package_eof_len, [&](const char *data, size_t len) {
            RecvData packet{}; packet.info.len = len; packet.data = data;
            return p->onPackage(p, socket, &packet) >= 0 && !socket->removed;
        });
    case SWR_FRAMING_OP_SSL_PENDING: return SSL_pending(socket->ssl);
    case SWR_FRAMING_OP_CLEAR_SKIP: socket->skip_recv = 0; return 0;
    case SWR_FRAMING_OP_PAGE: return swoole_pagesize();
    case SWR_FRAMING_OP_LOG_MALFORMED: {
        proto_error(socket, SW_ERROR_PACKAGE_MALFORMED_DATA, "received %zu bytes of malformed data, remote_addr=", buffer->length);
        return 0;
    }
    case SWR_FRAMING_OP_LOG_NO_LENGTH:
        swoole_error_log(SW_LOG_WARNING, SW_ERROR_PACKAGE_LENGTH_NOT_FOUND,
                         "bad request, no length found in %zu bytes, remote_addr=", buffer->length);
        return 0;
    case SWR_FRAMING_OP_LOG_TOO_LARGE: {
        if (r->io.mode == SWR_FRAMING_MODE_LENGTH) {
            proto_error(socket, SW_ERROR_PACKAGE_LENGTH_TOO_LARGE, "the received packet length %ld is too large, remote_addr=", (long) arg);
        } else {
            proto_error(socket, SW_ERROR_PACKAGE_LENGTH_TOO_LARGE, "The received data packet is too large, length=%lu", (unsigned long) buffer->length);
        }
        return 0;
    }
    default: abort();
    }
}
// One effect. Rust's recv_wait is applied before it and the socket's flags read back
// after it. Exceptions never unwind through Rust: the first one is kept and rethrown
// after it returns.
int64_t call(void *context, uint32_t op, uint64_t arg) noexcept {
    auto *r = static_cast<Recv *>(context);
    const Protocol *p = r->p; network::Socket *socket = r->socket; String *buffer = r->buffer;
    socket->recv_wait = (r->io.flags & SWR_FRAMING_WAIT) != 0;
    int64_t result = 0;
    try {
        switch (op) {
        case SWR_FRAMING_OP_RECV: {
            ssize_t n = socket->recv(buffer->str + buffer->length, arg, 0);
            result = n >= 0 ? n : rare(r, op, arg);
            break;
        }
        case SWR_FRAMING_OP_GET_LENGTH: {
            PacketLength pl{}; pl.buf = buffer->str + arg; pl.buf_size = buffer->length - arg;
            result = p->get_package_length(p, socket, &pl);
            break;
        }
        case SWR_FRAMING_OP_EXTEND: buffer->extend(arg); break;
        case SWR_FRAMING_OP_DISPATCH: {
            RecvData data{}; data.info.len = buffer->offset; data.data = buffer->str + arg;
            result = p->onPackage(p, socket, &data);
            break;
        }
        default: result = rare(r, op, arg);
        }
    } catch (...) { r->error = std::current_exception(); result = SWR_FRAMING_THROWN; }
    r->io.flags = socket_flags(socket);
    return result;
}
int receive(const Protocol *p, network::Socket *socket, String *buffer, uint32_t mode) {
    Recv r{{p, buffer, mode, 0, socket_flags(socket)}, p, socket, buffer, nullptr};
    if (mode == SWR_FRAMING_MODE_LENGTH) {
        r.io.head = p->get_package_length_size ? p->get_package_length_size(socket) : p->package_length_size;
        r.io.flags |= (socket->skip_recv ? SWR_FRAMING_SKIP : 0) | (p->get_package_length_size ? SWR_FRAMING_DYNAMIC : 0);
    }
    int status = swr_v1_framing_recv(&r.io, &r, call);
    socket->recv_wait = (r.io.flags & SWR_FRAMING_WAIT) != 0;
    if (r.error) { std::rethrow_exception(r.error); }
    return status;
}
}  // namespace
int Protocol::recv_split_by_eof(network::Socket *s, String *b) const { return receive(this, s, b, SWR_FRAMING_MODE_SPLIT); }
int Protocol::recv_with_length_protocol(network::Socket *s, String *b) const { return receive(this, s, b, SWR_FRAMING_MODE_LENGTH); }
int Protocol::recv_with_eof_protocol(network::Socket *s, String *b) const { return receive(this, s, b, SWR_FRAMING_MODE_EOF); }
}
int64_t swoole_unpack(char type, const void *data) {
    int64_t value = -1;
    if (swr_v1_framing_unpack(type, (const uint8_t *) data, swr_v1_framing_width(type), &value) == -1) {
        swoole_error_log(SW_LOG_ERROR, SW_ERROR_INVALID_PARAMS, "Invalid format specifier '%c'", type);
    }
    return value;
}
uint64_t swoole_hton64(uint64_t value) { return swr_v1_framing_hton64(value); }
uint64_t swoole_ntoh64(uint64_t value) { return swr_v1_framing_hton64(value); }

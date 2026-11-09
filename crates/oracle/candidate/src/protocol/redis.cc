/*
 +----------------------------------------------------------------------+
 | Swoole                                                               |
 +----------------------------------------------------------------------+
 | Copyright (c) 2012-2015 The Swoole Group                             |
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
#include "swoole_server.h"
#include "swoole_redis.h"
#include "swoole_rs_protocols.h"

namespace swoole {
namespace redis {

using Request = swr_resp_request;

const char *get_number(const char *p, int *_ret) {
    char *endptr;
    p++;
    int ret = strtol(p, &endptr, 10);
    if (strncmp(SW_CRLF, endptr, SW_CRLF_LEN) == 0) {
        p += (endptr - p) + SW_CRLF_LEN;
        *_ret = ret;
        return p;
    } else {
        return nullptr;
    }
}

int recv_packet(Protocol *protocol, Connection *conn, String *buffer) {
    auto *socket = conn->socket;
    auto *request = static_cast<Request *>(conn->object);
    if (!request) {
        request = static_cast<Request *>(sw_malloc(sizeof(Request)));
        if (!request) {
            swoole_warning("malloc(%ld) failed", sizeof(Request));
            return SW_ERR;
        }
        sw_memset_zero(request, sizeof(Request));
        conn->object = request;
    }
    for (;;) {
        const int received = socket->recv(buffer->str + buffer->length, buffer->size - buffer->length, 0);
        if (received < 0) {
            switch (socket->catch_read_error(errno)) {
            case SW_ERROR:
                swoole_sys_warning("recv from socket#%d failed", conn->fd);
                return SW_OK;
            case SW_CLOSE: return SW_ERR;
            default: return SW_OK;
            }
        }
        if (received == 0) return SW_ERR;
        buffer->length += received;
        uint8_t set_offset = 0;
        uint8_t range = 0;
        const int action = swr_v1_resp_receive_range(reinterpret_cast<const uint8_t *>(buffer->str), buffer->length,
                                              buffer->size, protocol->package_max_length, request, &set_offset, &range);
        if (range) errno = ERANGE;
        if (set_offset) buffer->offset = buffer->length;
        if (action == 1) {
            if (const size_t size = swr_v1_resp_extend_size(buffer->length, buffer->size,
                                                            protocol->package_max_length, swoole_pagesize())) {
                buffer->extend(size);
            }
            continue;
        }
        if (action == 2) return SW_OK;
        if (action == 3) {
            RecvData data{};
            data.info.len = buffer->length;
            data.data = buffer->str;
            if (protocol->onPackage(protocol, socket, &data) < 0) return SW_ERR;
            if (socket->removed) return SW_OK;
            buffer->clear();
            sw_memset_zero(request, sizeof(Request));
            return SW_OK;
        }
        if (action == 4) {
            swoole_warning("Package is too big. package_length=%zu", buffer->length);
        } else {
            swoole_warning("redis protocol error");
        }
        return SW_ERR;
    }
}

static void append_reply(String *buffer, uint8_t kind, const char *data, size_t length, int64_t integer = 0) {
    const auto *input = reinterpret_cast<const uint8_t *>(data);
    const size_t size = swr_v1_resp_format(kind, input, length, integer, nullptr, 0);
    if (!size || size > SIZE_MAX - buffer->length) return;
    if (buffer->length + size > buffer->size) buffer->reserve(buffer->length + size);
    if (swr_v1_resp_format(kind, input, length, integer,
                           reinterpret_cast<uint8_t *>(buffer->str + buffer->length), size) == size) {
        buffer->length += size;
    }
}

void format_nil(String *buffer) { append_reply(buffer, 7, nullptr, 0); }
void format(String *buffer, ReplyType type, const std::string &value) {
    append_reply(buffer, type, value.data(), value.size());
}
void format(String *buffer, ReplyType, long value) { append_reply(buffer, 8, nullptr, 0, value); }

std::vector<std::string> parse(const char *data, size_t length) {
    const auto *input = reinterpret_cast<const uint8_t *>(data);
    uint8_t range = 0;
    swr_resp_span local[16];
    auto *spans = local;
    const auto count = swr_v1_resp_scan(input, length, local, 16, &range);
    if (range) errno = ERANGE;
    if (count < 0) return {};
    std::vector<swr_resp_span> overflow;
    if (count > 16) {
        overflow.resize(count);
        spans = overflow.data();
        if (swr_v1_resp_scan(input, length, spans, count, &range) != count) return {};
    }
    std::vector<std::string> result;
    result.reserve(count);
    for (int64_t i = 0; i < count; ++i) {
        const auto &span = spans[i];
        if (span.kind == 1) result.push_back(std::to_string(span.integer));
        else result.emplace_back(data + span.offset, span.length);
    }
    return result;
}
}  // namespace redis
}  // namespace swoole

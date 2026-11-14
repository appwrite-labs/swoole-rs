// The stream receive loops from base.cc behind a C entry point, once per engine:
// build.rs copies the pinned bodies (upstream) or the bodies with the framing
// overlay patch applied (candidate, which calls the Rust exports) into
// SWR_STREAM_INC unchanged, and String's split and reduce from the pinned
// string.cc. The facade's Socket returns a fuzzer-chosen chunk per recv; the
// length function and onPackage are the facade's, the same for both engines.
#include <cassert>
#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <exception>
#include <functional>
#include <string>
#include <sys/types.h>
#include "swoole_rs_framing.h"

// One fuzz case: the protocol (mode 0 length, 1 EOF, 2 EOF split), the stream
// and the size of each chunk a recv may return.
extern "C" struct SwrOracleStream {
    uint8_t mode;
    uint8_t eof_len;
    uint8_t close_at_end;
    uint8_t length_size;
    uint16_t length_offset;
    uint16_t body_offset;
    uint32_t max_length;
    uint32_t buffer_size;
    const uint8_t *eof;
    const uint8_t *data;
    size_t length;
    const uint16_t *chunks;
    size_t chunk_count;
};

#define SWR_CAT2(a, b) a##_##b
#define SWR_CAT(a, b) SWR_CAT2(a, b)

namespace SWR_STREAM_ENGINE {
// As upstream include/swoole.h, swoole_config.h, swoole_error.h and swoole_log.h (logging off).
enum { SW_OK = 0, SW_ERR = -1 };
enum { SW_CONTINUE = 1, SW_WAIT = 2, SW_CLOSE = 3, SW_ERROR = 4 };
enum { SW_ERROR_PACKAGE_LENGTH_TOO_LARGE = 1203, SW_ERROR_PACKAGE_LENGTH_NOT_FOUND = 1204,
       SW_ERROR_PACKAGE_MALFORMED_DATA = 1205, SW_LOG_WARNING = 4 };
#define SW_BUFFER_SIZE_STD 8192
#define sw_unlikely(x) (x)
#define sw_likely(x) (x)
#define swoole_sys_warning(...) ((void) 0)
#define swoole_error_log(...) ((void) 0)
#define swoole_debug(...) ((void) 0)
#define swoole_trace_log(...) ((void) 0)
#define proto_error(...) ((void) 0)
using ulong_t = unsigned long;
static inline const char *swoole_strnstr(const char *haystack, uint32_t haystack_length, const char *needle,
                                         uint32_t needle_length) {
    assert(needle_length > 0);
    if (needle_length <= haystack_length) {
        for (uint32_t i = 0; i < haystack_length - needle_length + 1; i++) {
            if ((haystack[0] == needle[0]) && (0 == memcmp(haystack, needle, needle_length))) {
                return haystack;
            }
            haystack++;
        }
    }
    return nullptr;
}
static inline int swoole_pagesize() {
    return 4096;
}
__attribute__((unused)) static inline size_t swoole_size_align(size_t size, int pagesize) {
    return size + (pagesize - (size % pagesize));
}
static inline int SSL_pending(const void *) {
    return 0;
}

using StringExplodeHandler = std::function<bool(const char *, size_t)>;
struct String {
    size_t length = 0;
    size_t size = 0;
    off_t offset = 0;
    char *str = nullptr;
    void clear() {
        length = 0;
        offset = 0;
    }
    void extend(size_t new_size) {
        assert(new_size > size);
        str = static_cast<char *>(realloc(str, new_size));
        size = new_size;
    }
    void reduce(off_t _offset);
    ssize_t split(const char *delimiter, size_t delimiter_length, const StringExplodeHandler &handler);
};
#include "framing_stream_string.inc"

struct DataHead {
    uint32_t len;
};
struct RecvData {
    DataHead info;
    const char *data;
};
struct PacketLength {
    const char *buf;
    uint32_t buf_size;
    uint32_t header_len;
};

namespace network {
struct Address {
    const char *get_addr() const {
        return "";
    }
    int get_port() const {
        return 0;
    }
};
// The fields the receive loops read, and a recv that hands out the queued chunks.
struct Socket {
    int fd = 0;
    unsigned removed : 1;
    unsigned recv_wait : 1;
    unsigned skip_recv : 1;
    void *ssl = nullptr;
    Address info;
    std::deque<std::string> chunks;
    bool close_at_end = false;
    Socket() : removed(0), recv_wait(0), skip_recv(0) {}
    ssize_t recv(void *buffer, size_t length, int) {
        if (chunks.empty()) {
            if (close_at_end) {
                return 0;
            }
            errno = EAGAIN;
            return -1;
        }
        std::string &chunk = chunks.front();
        size_t n = length < chunk.size() ? length : chunk.size();
        memcpy(buffer, chunk.data(), n);
        chunk.erase(0, n);
        if (chunk.empty()) {
            chunks.pop_front();
        }
        return static_cast<ssize_t>(n);
    }
    int catch_read_error(int error) const {
        return error == EAGAIN ? SW_WAIT : SW_ERROR;
    }
    int get_peer_name(Address *) const {
        return 0;
    }
};
}  // namespace network

// Upstream's leading field order: the candidate reads them in place.
struct Protocol {
    bool split_by_eof;
    char package_eof[8];
    uint8_t package_eof_len;
    char package_length_type;
    uint8_t package_length_size;
    uint16_t package_length_offset;
    uint16_t package_body_offset;
    uint32_t package_max_length;
    void *private_data_1;
    uint8_t (*get_package_length_size)(network::Socket *);
    int (*onPackage)(const Protocol *, network::Socket *, const RecvData *);
    ssize_t (*get_package_length)(const Protocol *, network::Socket *, PacketLength *);
    int recv_split_by_eof(network::Socket *socket, String *buffer) const;
    int recv_with_length_protocol(network::Socket *socket, String *buffer) const;
    int recv_with_eof_protocol(network::Socket *socket, String *buffer) const;
};
#include SWR_STREAM_INC

// A big-endian length of package_length_size bytes at package_length_offset, plus
// package_body_offset: 0 until the header is in, -1 past the limit.
static ssize_t length_of(const Protocol *protocol, network::Socket *, PacketLength *pl) {
    size_t end = size_t(protocol->package_length_offset) + protocol->package_length_size;
    if (pl->buf_size < end) {
        return 0;
    }
    uint64_t value = 0;
    for (size_t i = protocol->package_length_offset; i < end; i++) {
        value = value << 8 | static_cast<uint8_t>(pl->buf[i]);
    }
    if (value > 0xffff) {
        return -1;
    }
    pl->header_len = protocol->package_length_size;
    return static_cast<ssize_t>(value + protocol->package_body_offset);
}

static std::string *events;
static int on_package(const Protocol *, network::Socket *, const RecvData *data) {
    *events += "P" + std::to_string(data->info.len) + ":";
    events->append(data->data, data->info.len);
    return 0;
}
}  // namespace SWR_STREAM_ENGINE

extern "C" size_t SWR_CAT(swr_oracle_framing_stream, SWR_STREAM_ENGINE)(const SwrOracleStream *stream, char *out,
                                                                      size_t capacity) {
    using namespace SWR_STREAM_ENGINE;
    std::string log;
    events = &log;
    Protocol protocol{};
    memcpy(protocol.package_eof, stream->eof, stream->eof_len);
    protocol.package_eof_len = stream->eof_len;
    protocol.split_by_eof = stream->mode == 2;
    protocol.package_length_size = stream->length_size;
    protocol.package_length_offset = stream->length_offset;
    protocol.package_body_offset = stream->body_offset;
    protocol.package_max_length = stream->max_length;
    protocol.onPackage = on_package;
    protocol.get_package_length = length_of;
    network::Socket socket;
    socket.close_at_end = stream->close_at_end;
    size_t at = 0;
    for (size_t i = 0; i < stream->chunk_count && at < stream->length; i++) {
        size_t n = stream->chunks[i] < stream->length - at ? stream->chunks[i] : stream->length - at;
        socket.chunks.emplace_back(reinterpret_cast<const char *>(stream->data) + at, n);
        at += n;
    }
    String buffer;
    buffer.extend(stream->mode == 0 ? stream->buffer_size : stream->max_length);
    for (int call = 0; call < 4096; call++) {
        bool drained = socket.chunks.empty();
        int result = stream->mode == 0 ? protocol.recv_with_length_protocol(&socket, &buffer)
                                       : protocol.recv_with_eof_protocol(&socket, &buffer);
        // The length loop reads greedily in the candidate, so how many calls a packet
        // takes and what is left buffered differ from upstream: compare the packets
        // and whether the stream ends in an error. The EOF loops compare every call.
        if (stream->mode != 0 || result == SW_ERR) {
            log += "R" + std::to_string(result) + ";";
        }
        if (result == SW_ERR || drained) {
            break;
        }
    }
    if (stream->mode != 0) {
        log += "L" + std::to_string(buffer.length) + "O" + std::to_string(buffer.offset);
    }
    free(buffer.str);
    size_t n = log.size() < capacity ? log.size() : capacity;
    memcpy(out, log.data(), n);
    return log.size();
}

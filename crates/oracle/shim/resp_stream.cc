// redis::recv_packet from the pinned redis.cc behind a C entry point: build.rs
// copies its body into resp_stream_upstream.inc unchanged, with its Request. The
// facade's Socket returns a fuzzer-chosen chunk per recv; onPackage is the facade's.
#include <cerrno>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <string>
#include <sys/types.h>

// One fuzz case: the stream, the size of each chunk a recv may return, the
// buffer's starting size and the protocol's package_max_length.
extern "C" struct SwrOracleResp {
    const uint8_t *data;
    size_t length;
    const uint16_t *chunks;
    size_t chunk_count;
    uint32_t buffer_size;
    uint32_t max_length;
    uint8_t close_at_end;
};

// Each shim's facade types get their own namespace: framing_stream.cc defines a
// different network::Socket, and inline members of same-named classes would merge.
namespace upstream {
namespace resp_stream {
// As upstream include/swoole.h, swoole_error.h and swoole_log.h (logging off).
enum { SW_OK = 0, SW_ERR = -1 };
enum { SW_CONTINUE = 1, SW_WAIT = 2, SW_CLOSE = 3, SW_ERROR = 4 };
#define SW_CRLF "\r\n"
#define SW_CRLF_LEN 2
#define sw_malloc malloc
#define sw_memset_zero(s, n) memset(s, '\0', n)
#define swoole_warning(...) ((void) 0)
#define swoole_sys_warning(...) ((void) 0)
#include "protocols_states.inc"
static inline int swoole_pagesize() {
    return 4096;
}
__attribute__((unused)) static inline size_t swoole_size_align(size_t size, int pagesize) {
    return size + (pagesize - (size % pagesize));
}

// The buffer keeps a byte past size, and each recv writes a NUL after what it
// hands out, so text past length reads as a NUL even after clear().
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
        str = static_cast<char *>(realloc(str, new_size + 1));
        memset(str + size, 0, new_size + 1 - size);
        size = new_size;
    }
};

struct DataHead {
    uint32_t len;
};
struct RecvData {
    DataHead info;
    const char *data;
};

namespace network {
// The fields recv_packet reads, and a recv that hands out the queued chunks.
struct Socket {
    unsigned removed : 1;
    std::deque<std::string> chunks;
    bool close_at_end = false;
    Socket() : removed(0) {}
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
        static_cast<char *>(buffer)[n] = '\0';
        chunk.erase(0, n);
        if (chunk.empty()) {
            chunks.pop_front();
        }
        return static_cast<ssize_t>(n);
    }
    int catch_read_error(int error) const {
        return error == EAGAIN ? SW_WAIT : SW_ERROR;
    }
};
}  // namespace network

struct Protocol {
    uint32_t package_max_length;
    int (*onPackage)(const Protocol *, network::Socket *, const RecvData *);
};
struct Connection {
    int fd;
    network::Socket *socket;
    void *object;
};

namespace redis {
#include "resp_stream_upstream.inc"
}  // namespace redis

static std::string *events;
static bool trace;
static int on_package(const Protocol *, network::Socket *, const RecvData *data) {
    const int error = errno;
    *events += "P" + std::to_string(data->info.len) + ":";
    events->append(data->data, data->info.len);
    if (trace) {
        *events += "E" + std::to_string(error);
    }
    return 0;
}
}  // namespace resp_stream
}  // namespace upstream

// Runs recv_packet until it fails or the chunks are drained, logging each call's
// result (R<n>), each package (P<len>:bytes) and, while the connection stays open,
// the request state (,<state>,<lines>/<total>,<bytes>/<total>) and the final buffer
// length and offset. On SW_ERR the server closes the connection, freeing both.
// A trace starts each call with errno at EDOM and also logs errno at each package
// and after each call (E<n>), the buffer's length and offset after each call, and
// the request state with its offset after a failed call too.
// Returns the log's full length; at most capacity bytes are written to out.
static size_t run(const SwrOracleResp *stream, char *out, size_t capacity, bool traced) {
    using namespace upstream::resp_stream;
    std::string log;
    events = &log;
    trace = traced;
    Protocol protocol{stream->max_length, on_package};
    network::Socket socket;
    socket.close_at_end = stream->close_at_end;
    size_t at = 0;
    for (size_t i = 0; i < stream->chunk_count && at < stream->length; i++) {
        size_t n = stream->chunks[i] < stream->length - at ? stream->chunks[i] : stream->length - at;
        socket.chunks.emplace_back(reinterpret_cast<const char *>(stream->data) + at, n);
        at += n;
    }
    Connection conn{0, &socket, nullptr};
    String buffer;
    buffer.extend(stream->buffer_size);
    for (int call = 0; call < 4096; call++) {
        bool drained = socket.chunks.empty();
        if (traced) {
            errno = EDOM;
        }
        int result = redis::recv_packet(&protocol, &conn, &buffer);
        const int error = errno;
        log += "R" + std::to_string(result);
        if (traced) {
            log += "E" + std::to_string(error) + "L" + std::to_string(buffer.length) + "O" +
                   std::to_string(buffer.offset);
        } else if (result == SW_ERR) {
            break;
        }
        if (conn.object) {
            const redis::Request &r = *static_cast<const redis::Request *>(conn.object);
            log += "," + std::to_string(r.state) + "," + std::to_string(r.n_lines_received) + "/" +
                   std::to_string(r.n_lines_total) + "," + std::to_string(r.n_bytes_received) + "/" +
                   std::to_string(r.n_bytes_total);
            if (traced) {
                log += "," + std::to_string(r.offset);
            }
        }
        log += ";";
        if (result == SW_ERR) {
            break;
        }
        if (drained) {
            log += "L" + std::to_string(buffer.length) + "O" + std::to_string(buffer.offset);
            break;
        }
    }
    free(conn.object);
    free(buffer.str);
    size_t n = log.size() < capacity ? log.size() : capacity;
    memcpy(out, log.data(), n);
    return log.size();
}

extern "C" size_t swr_oracle_resp_stream_upstream(const SwrOracleResp *stream, char *out, size_t capacity) {
    return run(stream, out, capacity, false);
}

extern "C" size_t swr_oracle_resp_trace_upstream(const SwrOracleResp *stream, char *out, size_t capacity) {
    return run(stream, out, capacity, true);
}

// Upstream's Server::select_static_handler behind a C entry point: build.rs copies the
// pinned body, and get_status_message from the pinned http.cc, into
// http_static_select.inc unchanged. The facade's StaticHandler, request, connection
// and server answer from the caller's scenario and trace each call, as the retired
// tests/modules/http/static_gate.py's oracle did.
#include <cerrno>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>
#include <sys/types.h>

extern "C" {
struct swr_oracle_static_text {
    const char *data;
    size_t length;
};
struct swr_oracle_static_task {
    int64_t offset;
    size_t length;
    char part_header[256];
};
struct swr_oracle_static_scenario {
    int serve, modified, index_ok, index, autoindex, directory, status;
    swr_oracle_static_text date, last_modified, mime, content_type, body, filename, end_part, modified_header;
    size_t filesize, content_length;
    const swr_oracle_static_task *tasks;
    size_t task_count;
    int throw_on_send, send_result;
    unsigned toggle_keep_on_send, toggle_method_on_send;
    int head, keep_alive;
};
typedef void (*swr_oracle_static_line)(void *out, const char *line, size_t length);
}

namespace upstream_static_select {
// As upstream include/swoole_config.h, swoole_http.h and swoole.h, and src/core/base.cc's sw_snprintf.
#define SW_HTTP_SERVER_SOFTWARE "swoole-http-server"
#define SW_HTTP_PAGE_CSS                                                                                               \
    "<style> \
body { padding: 0.5em; line-height: 2; } \
h1 { font-size: 1.5em; padding-bottom: 0.3em; border-bottom: 1px solid #ccc; } \
ul { list-style-type: disc; } \
footer { border-top: 1px solid #ccc; } \
a { color: #0969da; } \
</style>"
#define SW_HTTP_POWER_BY "<footer><i>Powered by Swoole</i></footer>"
#define SW_HTTP_PAGE_404 "<html><body>" SW_HTTP_PAGE_CSS "<h1>HTTP 404 Not Found</h1>" SW_HTTP_POWER_BY "</body></html>"
#define sw_unlikely(x) __builtin_expect(!!(x), 0)
enum { SW_HTTP_GET = 2, SW_HTTP_HEAD = 3 };
enum { SW_HTTP_PARTIAL_CONTENT = 206, SW_HTTP_NOT_FOUND = 404 };
enum ServerEventType { SW_SERVER_EVENT_SEND_DATA = 2, SW_SERVER_EVENT_SEND_FILE = 3, SW_SERVER_EVENT_CLOSE = 4 };
__attribute__((unused)) static size_t sw_snprintf(char *buf, size_t size, const char *format, ...) {
    va_list args;
    va_start(args, format);
    int retval = vsnprintf(buf, size, format, args);
    va_end(args);
    if (size == 0) {
        return retval;
    } else if (sw_unlikely(retval < 0)) {
        retval = 0;
        buf[0] = '\0';
    } else if (sw_unlikely(retval >= (int) size)) {
        retval = size - 1;
        buf[retval] = '\0';
    }
    return retval;
}
static void *sw_malloc(size_t size) {
    return malloc(size);
}
static void sw_free(void *ptr) {
    free(ptr);
}
namespace network {
struct SendfileTask {
    off_t offset;
    size_t length;
    char filename[0];
};
}  // namespace network
struct DataHead {
    long fd;
    uint32_t len;
    uint8_t type;
};
struct SendData {
    DataHead info;
    const char *data;
};
struct Buffer {
    std::string storage;
    char *str = nullptr;
    size_t length = 0;
    void clear() {
        storage.clear();
        str = &storage[0];
        length = 0;
    }
};
static thread_local Buffer tls_buffer;
static Buffer *sw_tg_buffer() {
    return &tls_buffer;
}
static thread_local std::vector<std::string> trace, request_trace, events;
struct Socket {
    void cork() {
        trace.emplace_back("cork");
    }
};
struct Connection {
    long session_id;
    Socket *socket;
};
struct Server;
namespace http_server {
#include "http_static_select_status.inc"
struct Request {
    Buffer *buffer_;
    size_t url_offset_ = 0, url_length_ = 7;
    int method = SW_HTTP_GET;
    bool keep_alive = true;
    std::string modified_header;
    std::string get_header(const char *key) const {
        request_trace.emplace_back(key);
        return std::string(key) == "If-Modified-Since" ? modified_header : std::string();
    }
};
struct Task {
    off_t offset;
    size_t length;
    char part_header[256];
};
struct StaticHandler {
    const swr_oracle_static_scenario *s;
    int status_code;
    std::vector<Task> tasks_;
    std::string mime_, content_type_, filename_, end_part_;
    StaticHandler(Server *server, const char *, size_t);
    bool try_serve() {
        trace.emplace_back("serve");
        return s->serve;
    }
    std::string get_date() {
        trace.emplace_back("date");
        return std::string(s->date.data, s->date.length);
    }
    std::string get_date_last_modified() {
        trace.emplace_back("modified-date");
        return std::string(s->last_modified.data, s->last_modified.length);
    }
    bool is_modified(const std::string &) {
        trace.emplace_back("is-modified");
        return s->modified;
    }
    bool try_serve_index_file() {
        trace.emplace_back("index");
        return s->index_ok;
    }
    bool has_index_file() const {
        return s->index;
    }
    bool is_enabled_auto_index() const {
        return s->autoindex;
    }
    bool is_dir() const {
        return s->directory;
    }
    size_t make_index_page(Buffer *buffer) {
        trace.emplace_back("index-page");
        buffer->storage.assign(s->body.data, s->body.length);
        buffer->str = &buffer->storage[0];
        buffer->length = buffer->storage.size();
        return buffer->length;
    }
    void parse_range(const char *, const char *) {
        trace.emplace_back("range");
    }
    const std::vector<Task> &get_tasks() const {
        return tasks_;
    }
    size_t get_filesize() const {
        return s->filesize;
    }
    size_t get_content_length() const {
        return s->content_length;
    }
    const std::string &get_mimetype() const {
        trace.emplace_back("mime");
        return mime_;
    }
    const std::string &get_content_type() {
        trace.emplace_back("content-type");
        return content_type_;
    }
    const char *get_filename() const {
        return filename_.c_str();
    }
    const std::string &get_end_part() const {
        return end_part_;
    }
};
}  // namespace http_server
struct Server {
    const swr_oracle_static_scenario *scenario;
    http_server::Request *mutable_request = nullptr;
    unsigned send_count = 0;
    bool select_static_handler(const http_server::Request *request, const Connection *conn);
    int send_to_connection(const SendData *response) {
        trace.emplace_back("send");
        if (scenario->throw_on_send) throw std::runtime_error("send failed");
        ++send_count;
        if (send_count == scenario->toggle_keep_on_send) mutable_request->keep_alive = !mutable_request->keep_alive;
        if (send_count == scenario->toggle_method_on_send)
            mutable_request->method = mutable_request->method == SW_HTTP_HEAD ? SW_HTTP_GET : SW_HTTP_HEAD;
        std::string payload;
        if (response->info.type == SW_SERVER_EVENT_SEND_FILE) {
            auto task = reinterpret_cast<const network::SendfileTask *>(response->data);
            payload = std::to_string(task->offset) + ":" + std::to_string(task->length) + ":" + task->filename + ":" +
                      std::to_string(response->info.len);
        } else if (response->info.len) {
            payload.assign(response->data, response->info.len);
        }
        events.push_back(std::to_string(response->info.fd) + ":" + std::to_string(response->info.type) + ":" + payload);
        return scenario->send_result;
    }
};
http_server::StaticHandler::StaticHandler(Server *server, const char *, size_t)
    : s(server->scenario), status_code(server->scenario->status), mime_(s->mime.data, s->mime.length),
      content_type_(s->content_type.data, s->content_type.length), filename_(s->filename.data, s->filename.length),
      end_part_(s->end_part.data, s->end_part.length) {
    for (size_t i = 0; i < s->task_count; ++i) {
        Task task{};
        task.offset = s->tasks[i].offset;
        task.length = s->tasks[i].length;
        memcpy(task.part_header, s->tasks[i].part_header, sizeof(task.part_header));
        tasks_.push_back(task);
    }
}
#include "http_static_select.inc"
}  // namespace upstream_static_select

// Runs the pinned select_static_handler on the scenario. Each line goes to `line`:
// "T:" server trace, "R:" request trace, "E:fd:type:payload" sends, then "K:" the
// request's keep-alive, "M:" its method and "errno:" errno (EINTR before the call).
// Returns the result, or 2 if it threw.
extern "C" int swr_oracle_static_select(const swr_oracle_static_scenario *scenario, swr_oracle_static_line line,
                                        void *out) {
    using namespace upstream_static_select;
    // The first throw's lazy unwinder lookup may change errno: take it before comparing.
    static thread_local bool unwinder_ready = false;
    if (!unwinder_ready) {
        try {
            throw std::runtime_error("initialize native unwinder");
        } catch (const std::runtime_error &) {
        }
        unwinder_ready = true;
    }
    trace.clear();
    request_trace.clear();
    events.clear();
    Server server{scenario};
    Socket socket;
    Connection conn{73, &socket};
    Buffer buffer;
    buffer.storage = "request";
    buffer.str = &buffer.storage[0];
    buffer.length = buffer.storage.size();
    http_server::Request request{};
    request.buffer_ = &buffer;
    request.keep_alive = scenario->keep_alive;
    request.method = scenario->head ? SW_HTTP_HEAD : SW_HTTP_GET;
    request.modified_header.assign(scenario->modified_header.data, scenario->modified_header.length);
    server.mutable_request = &request;
    int result;
    errno = EINTR;
    try {
        result = server.select_static_handler(&request, &conn);
    } catch (const std::runtime_error &) {
        result = 2;
    }
    int error = errno;
    auto emit = [&](const std::string &value) { line(out, value.data(), value.size()); };
    for (auto &entry : trace) emit("T:" + entry);
    for (auto &entry : request_trace) emit("R:" + entry);
    for (auto &entry : events) emit("E:" + entry);
    emit("K:" + std::to_string(request.keep_alive));
    emit("M:" + std::to_string(request.method));
    emit("errno:" + std::to_string(error));
    return result;
}

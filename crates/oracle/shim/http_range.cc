// Upstream's StaticHandler::parse_range behind a C entry point: build.rs copies the
// pinned body into SWR_RANGE_INC unchanged. The facade gives it upstream's
// StaticHandler fields, with the file size, MIME type, boundary and If-Range verdict
// chosen by the caller. The candidate is crates/http's static context (fuzz/src/lib.rs).
#include <cerrno>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <stdexcept>
#include <string>
#include <strings.h>
#include <vector>

#define SWR_CAT2(a, b) a##_##b
#define SWR_CAT(a, b) SWR_CAT2(a, b)

namespace SWR_RANGE_ENGINE {
// As upstream include/swoole.h, swoole_config.h and swoole_http.h, and src/core/base.cc's sw_snprintf.
#define SW_STRL(s) s, sizeof(s) - 1
#define SW_STR_ISTARTS_WITH(str, len, const_sub_str) swoole_str_istarts_with(str, len, SW_STRL(const_sub_str))
#define SW_HTTP_SERVER_PART_HEADER 256
#define sw_unlikely(x) __builtin_expect(!!(x), 0)
enum { SW_HTTP_OK = 200, SW_HTTP_PARTIAL_CONTENT = 206, SW_HTTP_RANGE_NOT_SATISFIABLE = 416 };
__attribute__((unused)) static inline unsigned int swoole_str_istarts_with(const char *pstr, size_t plen, const char *sstr, size_t slen) {
    return (plen >= slen) && (strncasecmp(pstr, sstr, slen) == 0);
}
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
struct StaticHandler {
    typedef struct {
        off_t offset;
        size_t length;
        char part_header[SW_HTTP_SERVER_PART_HEADER];
    } task_t;
    std::vector<task_t> tasks;
    std::string boundary;
    std::string end_part;
    std::string mime;
    size_t content_length = 0;
    size_t filesize = 0;
    bool modified = false;
    int status_code = SW_HTTP_OK;
    const std::string &get_boundary() { return boundary; }
    const std::string &get_mimetype() const { return mime; }
    size_t get_filesize() const { return filesize; }
    bool is_modified_range(const std::string &) const { return modified; }
    void parse_range(const char *range, const char *if_range);
};
#include SWR_RANGE_INC
}  // namespace SWR_RANGE_ENGINE

extern "C" {
using SWR_RANGE_ENGINE::StaticHandler;

// See swoole_rs_oracle::http_range::Outcome.
struct swr_oracle_http_range {
    size_t content_length;
    int32_t status_code, thrown;
    size_t tasks, end_part_length;
    char end_part[128];
};

// parse_range(range, if_range ? "" : nullptr) on a file of filesize bytes. Each task
// goes to tasks_out as offset, length and its NUL-terminated part header, up to
// capacity tasks. thrown is 1 if the body threw.
void SWR_CAT(swr_oracle_http_range, SWR_RANGE_ENGINE)(const char *range, uint64_t filesize, const char *mime,
                                                     const char *boundary, int if_range, int modified,
                                                     swr_oracle_http_range *out, void *tasks_out, size_t capacity) {
    StaticHandler handler;
    handler.filesize = filesize;
    handler.mime = mime;
    handler.boundary = boundary;
    handler.modified = modified;
    *out = {};
    try {
        handler.parse_range(range, if_range ? "" : nullptr);
    } catch (const std::exception &) {
        out->thrown = 1;
    }
    out->content_length = handler.content_length;
    out->status_code = handler.status_code;
    out->tasks = handler.tasks.size();
    out->end_part_length = handler.end_part.size();
    memcpy(out->end_part, handler.end_part.data(), std::min(handler.end_part.size(), sizeof(out->end_part)));
    auto *task = static_cast<char *>(tasks_out);
    for (size_t i = 0; i < handler.tasks.size() && i < capacity; i++) {
        const auto &t = handler.tasks[i];
        uint64_t offset = t.offset, length = t.length;
        memcpy(task, &offset, 8);
        memcpy(task + 8, &length, 8);
        memset(task + 16, 0, SW_HTTP_SERVER_PART_HEADER);
        memcpy(task + 16, t.part_header, strnlen(t.part_header, SW_HTTP_SERVER_PART_HEADER));
        task += 16 + SW_HTTP_SERVER_PART_HEADER;
    }
}
}

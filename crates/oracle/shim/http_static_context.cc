// Upstream's StaticHandler behind C entry points: build.rs copies the class from the
// pinned swoole_static_handler.h, its method bodies and Server::apply_rewrite_rules
// from the pinned static_handler.cc and url_decode from the pinned http.cc unchanged.
// The filesystem, MIME table and random strings answer from the caller's script and
// trace each call, and strptime, mktime and gmtime are libc's, recorded as calendar
// events, as the retired tests/modules/http/context_gate.py and conditional_gate.py
// oracles did.
#include <algorithm>
#include <cctype>
#include <cerrno>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <ctime>
#include <dirent.h>
#include <limits.h>
#include <memory>
#include <random>
#include <regex>
#include <set>
#include <sstream>
#include <string>
#include <strings.h>
#include <sys/stat.h>
#include <unordered_set>
#include <vector>

extern "C" {
typedef void (*swr_oracle_static_line)(void *out, const char *line, size_t length);
struct swr_oracle_static_rule {
    const char *pattern, *replacement;
    int is_regex;
};
struct swr_oracle_static_server {
    int autoindex, location, has_index;
    long timezone;
    const char *const *index;
    size_t index_count;
    const swr_oracle_static_rule *rules;
    size_t rule_count;
};
struct swr_oracle_static_fs {
    size_t file_size;
    int64_t file_time;
    const char *directory, *link, *canonical, *mime;
    const char *const *names;
    size_t name_count;
};
}

namespace {
thread_local std::vector<std::string> calendar_events;
std::string fields(const tm *value) {
    if (!value) return "null";
    std::ostringstream out;
    for (int field : {value->tm_sec, value->tm_min, value->tm_hour, value->tm_mday, value->tm_mon, value->tm_year,
                      value->tm_wday, value->tm_yday, value->tm_isdst})
        out << field << ',';
    return out.str();
}
}  // namespace

extern "C" {
bool swr_oracle_calendar_parse(const char *text, const char *format, void *value) {
    char *end = ::strptime(text, format, static_cast<tm *>(value));
    int saved = errno;
    calendar_events.push_back(std::string("parse:") + format + ":" + (end ? std::to_string(end - text) : "null") + ":" +
                              fields(static_cast<tm *>(value)) + ":" + std::to_string(saved));
    errno = saved;
    return end != nullptr;
}
int64_t swr_oracle_calendar_time(void *value) {
    time_t result = ::mktime(static_cast<tm *>(value));
    int saved = errno;
    calendar_events.push_back("make:" + std::to_string(result) + ":" + fields(static_cast<tm *>(value)) + ":" +
                              std::to_string(saved));
    errno = saved;
    return result;
}
void *swr_oracle_calendar_gmt(int64_t time) {
    const time_t value = time;
    tm *result = ::gmtime(&value);
    int saved = errno;
    calendar_events.push_back("gmt:" + std::to_string(value) + ":" + fields(result) + ":" + std::to_string(saved));
    errno = saved;
    return result;
}
}

namespace upstream_static_context {
namespace http_server = ::upstream_static_context;
// As upstream include/swoole.h, swoole_config.h, swoole_http.h, swoole_util.h and
// swoole_string.h, and src/core/base.cc's sw_snprintf and intersection.
#define SW_STRL(s) s, sizeof(s) - 1
#define SW_STR_ISTARTS_WITH(str, len, const_sub_str) swoole_str_istarts_with(str, len, SW_STRL(const_sub_str))
#define SW_MEM_ALIGNED_SIZE(size) (((size) + (sizeof(unsigned long) - 1LL)) & ~(sizeof(unsigned long) - 1LL))
#define SW_HTTP_SERVER_BOUNDARY_PREKEY "SwooleBoundary"
#define SW_HTTP_SERVER_BOUNDARY_TOTAL_SIZE 39
#define SW_HTTP_SERVER_PART_HEADER 256
#define SW_HTTP_RFC1123_DATE_GMT "%a, %d %b %Y %T GMT"
#define SW_HTTP_RFC1123_DATE_UTC "%a, %d %b %Y %T UTC"
#define SW_HTTP_RFC850_DATE "%A, %d-%b-%y %T GMT"
#define SW_HTTP_ASCTIME_DATE "%a %b %e %T %Y"
#define SW_HTTP_PAGE_CSS                                                                                               \
    "<style> \
body { padding: 0.5em; line-height: 2; } \
h1 { font-size: 1.5em; padding-bottom: 0.3em; border-bottom: 1px solid #ccc; } \
ul { list-style-type: disc; } \
footer { border-top: 1px solid #ccc; } \
a { color: #0969da; } \
</style>"
#define SW_HTTP_POWER_BY "<footer><i>Powered by Swoole</i></footer>"
#define sw_unlikely(x) __builtin_expect(!!(x), 0)
enum { SW_HTTP_OK = 200, SW_HTTP_PARTIAL_CONTENT = 206, SW_HTTP_NOT_FOUND = 404, SW_HTTP_RANGE_NOT_SATISFIABLE = 416 };
static inline unsigned int swoole_str_starts_with(const char *pstr, size_t plen, const char *sstr, size_t slen) {
    return (plen >= slen) && (strncmp(pstr, sstr, slen) == 0);
}
static inline unsigned int swoole_str_istarts_with(const char *pstr, size_t plen, const char *sstr, size_t slen) {
    return (plen >= slen) && (strncasecmp(pstr, sstr, slen) == 0);
}
static inline const char *swoole_strnstr(const char *haystack, uint32_t haystack_length, const char *needle,
                                         uint32_t needle_length) {
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
static inline ssize_t swoole_strnpos(const char *haystack, uint32_t haystack_length, const char *needle,
                                     uint32_t needle_length) {
    const char *pos = swoole_strnstr(haystack, haystack_length, needle, needle_length);
    return pos == nullptr ? -1 : pos - haystack;
}
static inline bool starts_with(const std::string &str, const std::string &prefix) {
    if (prefix.size() > str.size()) {
        return false;
    }
    return std::equal(prefix.begin(), prefix.end(), str.begin());
}
static size_t sw_snprintf(char *buf, size_t size, const char *format, ...) {
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
static std::string intersection(const std::vector<std::string> &vec1, std::set<std::string> &vec2) {
    for (const auto &vec1_item : vec1) {
        if (vec2.find(vec1_item) != vec2.end()) {
            return vec1_item;
        }
    }
    return "";
}
// upstream's String, starting with "prefix" so make_index_page is seen appending.
struct String {
    enum FormatFlag { FORMAT_APPEND = 1, FORMAT_GROW = 2 };
    std::vector<char> storage;
    char *str;
    size_t size, length = 6;
    explicit String(size_t capacity) : storage(std::max<size_t>(capacity, 6)), str(storage.data()), size(storage.size()) {
        memcpy(str, "prefix", 6);
    }
    void reserve(size_t new_size) {
        storage.resize(SW_MEM_ALIGNED_SIZE(new_size));
        str = storage.data();
        size = storage.size();
    }
    void append(const char *append_str, size_t _length) {
        size_t new_size = length + _length;
        if (new_size > size) {
            reserve(new_size);
        }
        memcpy(str + length, append_str, _length);
        length += _length;
    }
    template <typename... Args>
    size_t format_impl(int flags, const char *format, Args... args) {
        size_t _size = sw_snprintf(nullptr, 0, format, args...);
        if (_size == 0) {
            return 0;
        }
        ++_size;
        size_t new_size = (flags & FORMAT_APPEND) ? length + _size : _size;
        if (flags & FORMAT_GROW) {
            size_t align_size = SW_MEM_ALIGNED_SIZE(size * 2);
            while (align_size < new_size) {
                align_size *= 2;
            }
            new_size = align_size;
        }
        size_t n;
        if (flags & FORMAT_APPEND) {
            if (_size > size - length) {
                reserve(new_size);
            }
            n = sw_snprintf(str + length, size - length, format, args...);
            length += n;
        } else {
            if (_size > size) {
                reserve(new_size);
            }
            n = sw_snprintf(str, size, format, args...);
            length = n;
        }
        return n;
    }
};

// The scripted filesystem, MIME table and random strings.
thread_local std::vector<std::string> trace;
thread_local struct {
    size_t file_size;
    int64_t file_time;
    std::string directory, link, canonical;
    std::vector<std::string> names;
} script;
struct ScriptedDir {
    size_t next = 0;
    struct dirent entry {};
};
static int scripted_lstat(const char *path, struct stat *value) {
    trace.emplace_back(std::string("stat:") + path);
    memset(value, 0, sizeof(*value));
    value->st_size = script.file_size;
    value->st_mode = script.link == path ? S_IFLNK : script.directory == path ? S_IFDIR : S_IFREG;
#ifdef __MACH__
    value->st_mtimespec.tv_sec = script.file_time;
#else
    value->st_mtim.tv_sec = script.file_time;
#endif
    return 0;
}
static char *scripted_realpath(const char *input, char *output) {
    trace.emplace_back(std::string("canonical:") + input);
    strcpy(output, script.canonical.empty() ? input : script.canonical.c_str());
    return output;
}
static ScriptedDir *scripted_opendir(const char *filename) {
    trace.emplace_back(std::string("open:") + filename);
    return new ScriptedDir();
}
static struct dirent *scripted_readdir(ScriptedDir *dir) {
    if (dir->next == script.names.size()) {
        trace.emplace_back("end");
        return nullptr;
    }
    const std::string &name = script.names[dir->next++];
    trace.emplace_back("read:" + name);
    memcpy(dir->entry.d_name, name.c_str(), name.size() + 1);
    return &dir->entry;
}
static int scripted_closedir(ScriptedDir *dir) {
    trace.emplace_back("close");
    delete dir;
    return 0;
}
static void swoole_random_string(std::string &value, size_t count) {
    trace.emplace_back("random:" + std::to_string(count));
    value.append(count, 'x');
}
namespace mime_type {
thread_local std::string current;
static bool exists(const std::string &path) {
    trace.emplace_back("mime-exists:" + path);
    return true;
}
static const std::string &get(const std::string &path) {
    trace.emplace_back("mime:" + path);
    return current;
}
}  // namespace mime_type
#define lstat scripted_lstat
#define realpath scripted_realpath
#define DIR ScriptedDir
#define opendir scripted_opendir
#define readdir scripted_readdir
#define closedir scripted_closedir
#define strptime(text, format, value) (char *) (swr_oracle_calendar_parse(text, format, value) ? text : nullptr)
#define mktime(value) ((time_t) swr_oracle_calendar_time(value))
#define gmtime(value) ((tm *) swr_oracle_calendar_gmt(*(value)))

class StaticHandler;
struct RewriteRule;
struct Server {
    std::string root = "/root";
    bool http_autoindex = false;
    long timezone_ = 0;
    std::shared_ptr<std::unordered_set<std::string>> locations = std::make_shared<std::unordered_set<std::string>>();
    std::shared_ptr<std::vector<std::string>> http_index_files;
    std::shared_ptr<std::vector<RewriteRule>> rewrite_rules;
    const std::string &get_document_root() const {
        return root;
    }
    bool apply_rewrite_rules(StaticHandler *);
};
#include "http_static_context_class.inc"
#include "http_static_context.inc"
}  // namespace upstream_static_context

using namespace upstream_static_context;
namespace {
struct Handle {
    Server server;
    StaticHandler handler;
    Handle(const swr_oracle_static_server *config, const char *url, size_t length) : handler(&server, url, length) {
        server.http_autoindex = config->autoindex;
        server.timezone_ = config->timezone;
        if (config->location) server.locations->insert("/");
        if (config->has_index)
            server.http_index_files =
                std::make_shared<std::vector<std::string>>(config->index, config->index + config->index_count);
        if (config->rule_count) {
            server.rewrite_rules = std::make_shared<std::vector<RewriteRule>>();
            for (size_t i = 0; i < config->rule_count; i++)
                server.rewrite_rules->push_back(
                    {config->rules[i].pattern, config->rules[i].replacement, config->rules[i].is_regex != 0});
        }
    }
};
void emit(swr_oracle_static_line line, void *out, const std::string &text) {
    line(out, text.data(), text.size());
}
}  // namespace

extern "C" {
// The scripted filesystem's answers until the next call; the effect trace starts empty.
void swr_oracle_static_script(const swr_oracle_static_fs *fs) {
    script.file_size = fs->file_size;
    script.file_time = fs->file_time;
    script.directory = fs->directory;
    script.link = fs->link;
    script.canonical = fs->canonical;
    script.names.assign(fs->names, fs->names + fs->name_count);
    mime_type::current = fs->mime;
    trace.clear();
    calendar_events.clear();
}
// Whether this compiler evaluates parse_range's sw_snprintf arguments
// (`get_boundary().c_str(), get_mimetype().c_str()`) left to right. C++ leaves
// the order unspecified: clang goes left to right, GCC right to left.
int swr_oracle_static_arguments_left_to_right() {
    std::string order;
    auto first = [&]() -> const std::string & { return order += 'b', order; };
    auto second = [&]() -> const std::string & { return order += 'm', order; };
    char buffer[8];
    sw_snprintf(buffer, sizeof(buffer), "%s%s", first().c_str(), second().c_str());
    return order[0] == 'b';
}
// `count` outputs of std::mt19937(seed).
void swr_oracle_mt19937(uint32_t seed, uint32_t *out, size_t count) {
    std::mt19937 rng(seed);
    for (size_t i = 0; i < count; i++) out[i] = rng();
}
// Each effect traced since the last call, in order.
void swr_oracle_static_trace(swr_oracle_static_line line, void *out) {
    for (const auto &entry : trace) emit(line, out, entry);
    trace.clear();
}
// Each calendar call since the last call (or script), in order.
void swr_oracle_calendar_events(swr_oracle_static_line line, void *out) {
    for (const auto &entry : calendar_events) emit(line, out, entry);
    calendar_events.clear();
}
void *swr_oracle_static_new(const swr_oracle_static_server *config, const char *url, size_t length) {
    return new Handle(config, url, length);
}
void swr_oracle_static_free(void *handle) {
    delete static_cast<Handle *>(handle);
}
int swr_oracle_static_try_serve(void *handle) {
    return static_cast<Handle *>(handle)->handler.try_serve();
}
int swr_oracle_static_try_serve_index_file(void *handle) {
    return static_cast<Handle *>(handle)->handler.try_serve_index_file();
}
int swr_oracle_static_get_dir_files(void *handle) {
    return static_cast<Handle *>(handle)->handler.get_dir_files();
}
void swr_oracle_static_parse_range(void *handle, const char *range, const char *if_range) {
    static_cast<Handle *>(handle)->handler.parse_range(range, if_range);
}
int swr_oracle_static_is_modified(void *handle, const char *date, size_t length) {
    return static_cast<Handle *>(handle)->handler.is_modified(std::string(date, length));
}
int swr_oracle_static_is_modified_range(void *handle, const char *date, size_t length) {
    return static_cast<Handle *>(handle)->handler.is_modified_range(std::string(date, length));
}
void swr_oracle_static_filename(void *handle, swr_oracle_static_line line, void *out) {
    emit(line, out, static_cast<Handle *>(handle)->handler.get_filename());
}
// make_index_page into a String of `capacity` that already holds `prefix` and
// earlier pages: each call emits the bytes it appended.
void *swr_oracle_static_buffer_new(size_t capacity) {
    return new String(capacity);
}
void swr_oracle_static_buffer_free(void *buffer) {
    delete static_cast<String *>(buffer);
}
size_t swr_oracle_static_index_page(void *handle, void *buffer, swr_oracle_static_line line, void *out) {
    auto *string = static_cast<String *>(buffer);
    size_t before = string->length;
    size_t length = static_cast<Handle *>(handle)->handler.make_index_page(string);
    line(out, string->str + before, string->length - before);
    return length - before;
}
// The handler's tasks, content type, If-Modified-Since verdict, end part, filename,
// last-modified date, content length and status, and errno, one line each. Returns
// whether get_mimetype() is the global MIME reference and get_content_type() a
// scoped one (multipart) or that same reference (one task).
int swr_oracle_static_snapshot(void *handle, swr_oracle_static_line line, void *out) {
    auto &handler = static_cast<Handle *>(handle)->handler;
    std::vector<std::string> lines;
    for (const auto &task : handler.get_tasks())
        lines.push_back("task:" + std::to_string(task.offset) + ":" + std::to_string(task.length) + ":" +
                        std::string(task.part_header, strnlen(task.part_header, sizeof(task.part_header))));
    const std::string &type = handler.get_content_type();
    bool content_identity =
        handler.get_tasks().size() > 1 ? &type != &mime_type::current : &type == &mime_type::current;
    lines.push_back("type:" + type);
    lines.push_back("modified:" + std::to_string(handler.is_modified("Tue, 14 Nov 2023 22:13:20 GMT")));
    lines.push_back("end:" + handler.get_end_part());
    lines.push_back(std::string("filename:") + handler.get_filename());
    lines.push_back("date:" + handler.get_date_last_modified());
    lines.push_back("length:" + std::to_string(handler.get_content_length()));
    lines.push_back("status:" + std::to_string(handler.status_code));
    bool mime_identity = &handler.get_mimetype() == &mime_type::current;
    lines.push_back("errno:" + std::to_string(errno));
    for (const auto &entry : lines) emit(line, out, entry);
    return mime_identity && content_identity;
}
}

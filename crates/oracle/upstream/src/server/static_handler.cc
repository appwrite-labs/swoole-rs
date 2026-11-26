/*
 +----------------------------------------------------------------------+
 | Swoole                                                               |
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

#include "swoole_static_handler.h"
#include "swoole_util.h"

#include <string>
#include <dirent.h>
#include <algorithm>
#include <sstream>
#include <regex>

namespace swoole {
namespace http_server {
static bool parse_http_date(const std::string &value, time_t &result) {
    if (value.empty() || value.length() > 63 || value.find('\0') != std::string::npos) {
        return false;
    }
    static const char *formats[] = {
        SW_HTTP_RFC1123_DATE_GMT, SW_HTTP_RFC1123_DATE_UTC, SW_HTTP_RFC850_DATE, SW_HTTP_ASCTIME_DATE};
    for (const auto *format : formats) {
        tm parsed{};
        const char *end = strptime(value.c_str(), format, &parsed);
        if (!end || *end) {
            continue;
        }
        result = make_utc_time(parsed);
        const tm check = get_utc_tm(result);
        // Reject dates normalized by the calendar conversion (for example February 31).
        if (check.tm_year == parsed.tm_year && check.tm_mon == parsed.tm_mon && check.tm_mday == parsed.tm_mday &&
            check.tm_hour == parsed.tm_hour && check.tm_min == parsed.tm_min && check.tm_sec == parsed.tm_sec) {
            return true;
        }
    }
    return false;
}

std::string StaticHandler::get_etag() const {
    // Metadata-based validator, like common static servers; no file-content hash
    // or per-request reads are needed. Nanoseconds distinguish subsecond updates.
    long nanoseconds = 0;
#ifdef __MACH__
    nanoseconds = file_stat.st_mtimespec.tv_nsec;
#elif !defined(_WIN32)
    nanoseconds = file_stat.st_mtim.tv_nsec;
#endif
    char value[96];
    const int length = snprintf(value,
                                sizeof(value),
                                "\"%llx-%llx-%lx\"",
                                static_cast<unsigned long long>(get_file_mtime()),
                                static_cast<unsigned long long>(get_filesize()),
                                static_cast<unsigned long>(nanoseconds));
    return {value, static_cast<size_t>(length)};
}

bool StaticHandler::is_modified(const std::string &date_if_modified_since) const {
    time_t date;
    return parse_http_date(date_if_modified_since, date) && date >= get_file_mtime();
}

bool StaticHandler::is_modified_range(const std::string &date_range) const {
    if (!date_range.empty() && date_range.front() == '"') {
        return date_range != get_etag();
    }
    time_t date;
    // If-Range requires a strong entity tag or an exactly matching HTTP-date.
    return !parse_http_date(date_range, date) || date != get_file_mtime();
}

std::string StaticHandler::get_date() {
    char date_[64];
    time_t now = ::time(nullptr);
    tm tm1 = get_utc_tm(now);
    // HTTP dates require a literal GMT suffix, independent of libc's UTC zone name.
    strftime(date_, sizeof(date_), SW_HTTP_RFC1123_DATE_GMT, &tm1);
    return date_;
}

std::string StaticHandler::get_date_last_modified() const {
    char date_last_modified[64];
    time_t file_mtime = get_file_mtime();
    tm tm2 = get_utc_tm(file_mtime);
    strftime(date_last_modified, sizeof(date_last_modified), SW_HTTP_RFC1123_DATE_GMT, &tm2);
    return date_last_modified;
}

bool StaticHandler::get_absolute_path() {
    char abs_path[PATH_MAX];
    if (!realpath(filename, abs_path)) {
        return false;
    }

    size_t abs_path_len = strlen(abs_path);
    if (abs_path_len >= PATH_MAX) {
        return false;
    }
    memcpy(filename, abs_path, abs_path_len + 1);
    l_filename = abs_path_len;
    return true;
}

bool StaticHandler::try_serve() {
    serv->apply_rewrite_rules(this);

    char *p = filename;
    const char *url = request_url.c_str();
    size_t url_length = request_url.length();
    /**
     * discard the url parameter
     * [/test.jpg?version=1#position] -> [/test.jpg]
     */
    auto params = (char *) memchr(url, '?', url_length);
    if (params == nullptr) {
        params = (char *) memchr(url, '#', url_length);
    }
    size_t n = params ? params - url : url_length;

    const std::string &document_root = serv->get_document_root();
    const size_t l_document_root = document_root.length();

    memcpy(p, document_root.c_str(), l_document_root);
    p += l_document_root;

    if (!serv->locations->empty()) {
        for (const auto &i : *serv->locations) {
            if (swoole_str_istarts_with(url, url_length, i.c_str(), i.size())) {
                last = true;
            }
        }
        if (!last) {
            return false;
        }
    }

    if (l_document_root + n >= PATH_MAX) {
        return catch_error();
    }

    memcpy(p, url, n);
    p += n;
    *p = '\0';
    if (!dir_path.empty()) {
        dir_path.clear();
    }
    dir_path = std::string(url, n);

    l_filename = url_decode(filename, p - filename);
    filename[l_filename] = '\0';

    // The file does not exist
    if (lstat(filename, &file_stat) < 0) {
        return catch_error();
    }

    // The filename is relative path, allows for the resolution of symbolic links.
    // This path is formed by concatenating the document root and that is permitted for access.
    if (is_absolute_path()) {
        if (is_link()) {
            // Use the realpath function to resolve a symbolic link to its actual path.
            if (!get_absolute_path()) {
                return catch_error();
            }
            if (lstat(filename, &file_stat) < 0) {
                return catch_error();
            }
        }
    } else {
        if (!get_absolute_path() || !is_located_in_document_root()) {
            return catch_error();
        }
    }

    if (serv->http_index_files && !serv->http_index_files->empty() && is_dir()) {
        return true;
    }

    if (serv->http_autoindex && is_dir()) {
        return true;
    }

    if (!mime_type::exists(filename) && !last) {
        return false;
    }

    if (!is_file()) {
        return false;
    }

    return true;
}

bool StaticHandler::try_serve_index_file() {
    if (serv->http_index_files && !serv->http_index_files->empty() && is_dir()) {
        if (!get_dir_files()) {
            return false;
        }
        index_file = intersection(*serv->http_index_files, dir_files);

        if (has_index_file() && !set_filename(index_file)) {
            return false;
        }
        if (!has_index_file() && !is_enabled_auto_index()) {
            return false;
        }
    }
    return true;
}

size_t StaticHandler::make_index_page(String *buffer) {
    get_dir_files();

    if (dir_path.back() != '/') {
        dir_path.append("/");
    }

    buffer->format_impl(String::FORMAT_APPEND | String::FORMAT_GROW,
                        "<html>\n"
                        "<head>\n"
                        "\t<meta charset='UTF-8'>\n<title>Index of %s</title>"
                        "</head>\n"
                        "<body>\n" SW_HTTP_PAGE_CSS "<h1>Index of %s</h1>"
                        "\t<ul>\n",
                        dir_path.c_str(),
                        dir_path.c_str());

    for (const auto &dir_file : dir_files) {
        if (dir_file == "." || (dir_path == "/" && dir_file == "..")) {
            continue;
        }
        buffer->format_impl(String::FORMAT_APPEND | String::FORMAT_GROW,
                            "\t\t<li><a href=%s%s>%s</a></li>\n",
                            dir_path.c_str(),
                            dir_file.c_str(),
                            dir_file.c_str());
    }

    buffer->append(SW_STRL("\t</ul>\n" SW_HTTP_POWER_BY "</body>\n</html>\n"));

    return buffer->length;
}

bool StaticHandler::get_dir_files() {
    if (!dir_files.empty()) {
        return true;
    }

    if (!is_dir()) {
        return false;
    }

    DIR *dir = opendir(filename);
    if (dir == nullptr) {
        return false;
    }

    struct dirent *ptr;
    while ((ptr = readdir(dir)) != nullptr) {
        dir_files.insert(ptr->d_name);
    }

    closedir(dir);

    return true;
}

bool StaticHandler::set_filename(const std::string &_filename) {
    char *p = filename + l_filename;

    if (*p != '/') {
        *p = '/';
        p += 1;
    }

    memcpy(p, _filename.c_str(), _filename.length());
    p += _filename.length();
    *p = 0;

    if (lstat(filename, &file_stat) < 0) {
        return false;
    }

    if (!is_file()) {
        return false;
    }

    return true;
}

void StaticHandler::parse_range(const char *range, const char *if_range) {
    // Publish a plan only after the complete header has been validated. A malformed
    // later range must never leave executable tasks or multipart lengths behind.
    tasks.clear();
    end_part.clear();
    content_length = 0;
    status_code = SW_HTTP_OK;
    const size_t file_size = get_filesize();
    const auto full_response = [&]() {
        task_t task{};
        task.length = content_length = file_size;
        tasks.clear();
        tasks.push_back(task);
        status_code = SW_HTTP_OK;
    };
    if (!range || !*range || !SW_STR_ISTARTS_WITH(range, strlen(range), "bytes=") || file_size == 0) {
        full_response();
        return;
    }

    if (if_range && *if_range && is_modified_range(if_range)) {
        full_response();
        return;
    }

    static constexpr size_t max_ranges = 16;
    size_t range_count = 0;
    size_t selected_bytes = 0;
    bool excessive = false;
    std::vector<task_t> ranges;
    const char *p = range + 6;
    const auto skip_space = [&]() {
        while (*p == ' ' || *p == '\t') {
            ++p;
        }
    };
    const auto number = [&](size_t &value) {
        if (*p < '0' || *p > '9') {
            return false;
        }
        value = 0;
        do {
            const size_t digit = *p++ - '0';
            if (value > (SIZE_MAX - digit) / 10) {
                return false;
            }
            value = value * 10 + digit;
        } while (*p >= '0' && *p <= '9');
        return true;
    };
    for (;;) {
        skip_space();
        size_t start = 0, end = 0;
        if (*p == '-') {
            ++p;
            if (!number(end)) {
                status_code = SW_HTTP_RANGE_NOT_SATISFIABLE;
                return;
            }
            start = end < file_size ? file_size - end : 0;
            end = file_size;
        } else {
            if (!number(start)) {
                status_code = SW_HTTP_RANGE_NOT_SATISFIABLE;
                return;
            }
            skip_space();
            if (*p != '-') {
                status_code = SW_HTTP_RANGE_NOT_SATISFIABLE;
                return;
            }
            ++p;
            skip_space();
            if (*p == ',' || *p == '\0') {
                end = file_size;
            } else if (!number(end) || start > end) {
                status_code = SW_HTTP_RANGE_NOT_SATISFIABLE;
                return;
            } else {
                end = end >= file_size ? file_size : end + 1;
            }
        }
        skip_space();
        if (*p != ',' && *p != '\0') {
            status_code = SW_HTTP_RANGE_NOT_SATISFIABLE;
            return;
        }
        if (++range_count > max_ranges) {
            excessive = true;
        }
        if (start < end) {
            if (end - start > file_size - selected_bytes) {
                excessive = true;
            } else {
                selected_bytes += end - start;
            }
            task_t task{};
            task.offset = start;
            task.length = end - start;
            if (!excessive) {
                ranges.push_back(task);
            }
        }
        if (!*p) {
            break;
        }
        ++p;
    }
    if (excessive) {
        full_response();
        return;
    }
    if (ranges.empty()) {
        status_code = SW_HTTP_RANGE_NOT_SATISFIABLE;
        return;
    }

    size_t length = 0;
    if (ranges.size() == 1) {
        length = ranges.front().length;
    } else {
        for (auto &task : ranges) {
            const int n = snprintf(task.part_header,
                                   sizeof(task.part_header),
                                   "%s--%s\r\nContent-Type: %s\r\n"
                                   "Content-Range: bytes %zu-%zu/%zu\r\n\r\n",
                                   &task == &ranges.front() ? "" : "\r\n",
                                   get_boundary().c_str(),
                                   get_mimetype().c_str(),
                                   static_cast<size_t>(task.offset),
                                   static_cast<size_t>(task.offset) + task.length - 1,
                                   file_size);
            if (n < 0 || static_cast<size_t>(n) >= sizeof(task.part_header) ||
                task.length > SIZE_MAX - static_cast<size_t>(n) ||
                length > SIZE_MAX - task.length - static_cast<size_t>(n)) {
                status_code = SW_HTTP_RANGE_NOT_SATISFIABLE;
                return;
            }
            length += task.length + n;
        }
        const std::string terminator = "\r\n--" + get_boundary() + "--\r\n";
        if (length > SIZE_MAX - terminator.length()) {
            status_code = SW_HTTP_RANGE_NOT_SATISFIABLE;
            return;
        }
        end_part = terminator;
        length += end_part.length();
    }
    tasks = std::move(ranges);
    content_length = length;
    status_code = SW_HTTP_PARTIAL_CONTENT;
}

}  // namespace http_server

void Server::add_rewrite_rule(const std::string &pattern, const std::string &replacement) {
    if (rewrite_rules == nullptr) {
        rewrite_rules = std::make_shared<std::vector<http_server::RewriteRule>>();
    }

    http_server::RewriteRule rule;
    rule.replacement = replacement;

    if (pattern.length() >= 2 && pattern.at(0) == '~' && pattern.at(pattern.length() - 1) == '~') {
        rule.pattern = pattern.substr(1, pattern.length() - 2);
        rule.is_regex = true;
    } else {
        rule.pattern = pattern;
        rule.is_regex = false;
    }
    rewrite_rules->emplace_back(rule);
}

void Server::add_static_handler_location(const std::string &location) {
    if (locations == nullptr) {
        locations = std::make_shared<std::unordered_set<std::string>>();
    }
    locations->emplace(location);
}

void Server::add_static_handler_index_files(const std::string &file) {
    if (http_index_files == nullptr) {
        http_index_files = std::make_shared<std::vector<std::string>>();
    }

    auto iter = std::find(http_index_files->begin(), http_index_files->end(), file);
    if (iter == http_index_files->end()) {
        http_index_files->emplace_back(file);
    }
}

bool Server::select_static_handler(const http_server::Request *request, const Connection *conn) {
    const char *url = request->buffer_->str + request->url_offset_;
    size_t url_length = request->url_length_;

    http_server::StaticHandler handler(this, url, url_length);
    if (!handler.try_serve()) {
        return false;
    }

    char header_buffer[1024];
    SendData response;
    response.info.fd = conn->session_id;
    response.info.type = SW_SERVER_EVENT_SEND_DATA;

    if (handler.status_code == SW_HTTP_NOT_FOUND) {
        response.info.len = sw_snprintf(header_buffer,
                                        sizeof(header_buffer),
                                        "HTTP/1.1 %s\r\n"
                                        "Server: " SW_HTTP_SERVER_SOFTWARE "\r\n"
                                        "Content-Length: %zu\r\n"
                                        "\r\n%s",
                                        http_server::get_status_message(SW_HTTP_NOT_FOUND),
                                        sizeof(SW_HTTP_PAGE_404) - 1,
                                        SW_HTTP_PAGE_404);
        response.data = header_buffer;
        send_to_connection(&response);

        return true;
    }

    auto date_str = handler.get_date();
    auto date_str_last_modified = handler.get_date_last_modified();

    std::string date_if_modified_since = request->get_header("If-Modified-Since");
    if (!date_if_modified_since.empty() && handler.is_modified(date_if_modified_since)) {
        response.info.len = sw_snprintf(header_buffer,
                                        sizeof(header_buffer),
                                        "HTTP/1.1 304 Not Modified\r\n"
                                        "Connection: %s\r\n"
                                        "Date: %s\r\n"
                                        "Last-Modified: %s\r\n"
                                        "Server: %s\r\n\r\n",
                                        request->keep_alive ? "keep-alive" : "close",
                                        date_str.c_str(),
                                        date_str_last_modified.c_str(),
                                        SW_HTTP_SERVER_SOFTWARE);
        response.data = header_buffer;
        send_to_connection(&response);

        return true;
    }

    /**
     * if http_index_files is enabled, need to search the index file first.
     * if the index file is found, set filename to index filename.
     */
    if (!handler.try_serve_index_file()) {
        return false;
    }

    /**
     * the index file was not found in the current directory,
     * if http_autoindex is enabled, should show the list of files in the current directory.
     */
    if (!handler.has_index_file() && handler.is_enabled_auto_index() && handler.is_dir()) {
        sw_tg_buffer()->clear();
        size_t body_length = handler.make_index_page(sw_tg_buffer());

        response.info.len = sw_snprintf(header_buffer,
                                        sizeof(header_buffer),
                                        "HTTP/1.1 200 OK\r\n"
                                        "Connection: %s\r\n"
                                        "Content-Length: %ld\r\n"
                                        "Content-Type: text/html\r\n"
                                        "Date: %s\r\n"
                                        "Last-Modified: %s\r\n"
                                        "Server: %s\r\n\r\n",
                                        request->keep_alive ? "keep-alive" : "close",
                                        static_cast<long>(body_length),
                                        date_str.c_str(),
                                        date_str_last_modified.c_str(),
                                        SW_HTTP_SERVER_SOFTWARE);
        response.data = header_buffer;
        send_to_connection(&response);

        response.info.len = body_length;
        response.data = sw_tg_buffer()->str;
        send_to_connection(&response);
        return true;
    }

    handler.parse_range(request->get_header("Range").c_str(), request->get_header("If-Range").c_str());
    auto tasks = handler.get_tasks();

    std::stringstream header_stream;
    if (1 == tasks.size()) {
        if (SW_HTTP_PARTIAL_CONTENT == handler.status_code) {
            header_stream << "Content-Range: bytes " << tasks[0].offset << "-"
                          << (tasks[0].length + tasks[0].offset - 1) << "/" << handler.get_filesize() << "\r\n";
        } else {
            header_stream << "Accept-Ranges: bytes\r\n";
        }
    }

    response.info.len = sw_snprintf(
        header_buffer,
        sizeof(header_buffer),
        "HTTP/1.1 %s\r\n"
        "Connection: %s\r\n"
        "Content-Length: %ld\r\n"
        "Content-Type: %s\r\n"
        "%s"
        "Date: %s\r\n"
        "Last-Modified: %s\r\n"
        "Server: %s\r\n\r\n",
        http_server::get_status_message(handler.status_code),
        request->keep_alive ? "keep-alive" : "close",
        SW_HTTP_HEAD == request->method ? 0 : handler.get_content_length(),
        SW_HTTP_HEAD == request->method ? handler.get_mimetype().c_str() : handler.get_content_type().c_str(),
        header_stream.str().c_str(),
        date_str.c_str(),
        date_str_last_modified.c_str(),
        SW_HTTP_SERVER_SOFTWARE);

    response.data = header_buffer;

    // Use tcp_nopush to improve sending efficiency
    conn->socket->cork();

    // Send HTTP header
    send_to_connection(&response);

    // Send HTTP body
    if (SW_HTTP_HEAD != request->method) {
        if (!tasks.empty()) {
            size_t task_size = sizeof(network::SendfileTask) + strlen(handler.get_filename()) + 1;
            auto task = static_cast<network::SendfileTask *>(sw_malloc(task_size));
            strcpy(task->filename, handler.get_filename());
            if (tasks.size() > 1) {
                for (const auto &i : tasks) {
                    response.info.type = SW_SERVER_EVENT_SEND_DATA;
                    response.info.len = strlen(i.part_header);
                    response.data = i.part_header;
                    send_to_connection(&response);

                    task->offset = i.offset;
                    task->length = i.length;
                    response.info.type = SW_SERVER_EVENT_SEND_FILE;
                    response.info.len = task_size;
                    response.data = reinterpret_cast<char *>(task);
                    send_to_connection(&response);
                }

                response.info.type = SW_SERVER_EVENT_SEND_DATA;
                response.info.len = handler.get_end_part().length();
                response.data = handler.get_end_part().c_str();
                send_to_connection(&response);
            } else if (tasks[0].length > 0) {
                task->offset = tasks[0].offset;
                task->length = tasks[0].length;
                response.info.type = SW_SERVER_EVENT_SEND_FILE;
                response.info.len = task_size;
                response.data = reinterpret_cast<char *>(task);
                send_to_connection(&response);
            }
            sw_free(task);
        }
    }

    // Close the connection if keepalive is not used
    if (!request->keep_alive) {
        response.info.type = SW_SERVER_EVENT_CLOSE;
        response.info.len = 0;
        response.data = nullptr;
        send_to_connection(&response);
    }

    return true;
}

bool Server::apply_rewrite_rules(http_server::StaticHandler *handler) {
    if (!rewrite_rules || rewrite_rules->empty()) {
        return false;
    }

    bool rewritten = false;
    auto current_url = handler->get_request_url();

    for (const auto &rule : *rewrite_rules) {
        if (rule.is_regex) {
            try {
                std::regex pattern(rule.pattern);
                std::string rewritten_url;

                if (std::regex_search(current_url, pattern)) {
                    rewritten_url = std::regex_replace(current_url, pattern, rule.replacement);
                    if (rewritten_url != current_url) {
                        handler->set_request_url(rewritten_url);
                        rewritten = true;
                        break;
                    }
                }
            } catch (const std::regex_error &e) {
                continue;
            }
        } else {
            if (starts_with(current_url, rule.pattern)) {
                std::string rewritten_url = rule.replacement + current_url.substr(rule.pattern.length());
                handler->set_request_url(rewritten_url);
                rewritten = true;
                break;
            }
        }
    }

    return rewritten;
}
}  // namespace swoole

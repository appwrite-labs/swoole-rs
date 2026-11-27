// Swoole v6.3.0, src/protocol/http.cc (e0d606594), unchanged.
int Request::get_protocol() {
    char *p = buffer_->str;
    char *pe = p + buffer_->length;
    char state = 0;

    if (buffer_->length < (sizeof("GET / HTTP/1.x\r\n") - 1)) {
        return SW_ERR;
    }

    // HTTP Method
    SW_LOOP_N(sizeof(method_strings) / sizeof(char *) - 1) {
        auto method_str = method_strings[i];
        auto n = strlen(method_str);
        if (memcmp(p, method_str, n) == 0) {
            method = i + 1;
            p += n;
            goto _found_method;
        }
    }
    // HTTP2 Connection Preface
    if (memcmp(p, SW_STRL("PRI")) == 0) {
        method = SW_HTTP_PRI;
        if (buffer_->length >= (sizeof(SW_HTTP2_PRI_STRING) - 1) && memcmp(p, SW_STRL(SW_HTTP2_PRI_STRING)) == 0) {
            buffer_->offset = sizeof(SW_HTTP2_PRI_STRING) - 1;
            return SW_OK;
        } else {
            goto _excepted;
        }
    } else {
    _excepted:
        excepted = 1;
        return SW_ERR;
    }

    // HTTP Version
_found_method:
    for (; p < pe; p++) {
        switch (state) {
        case 0:
            if (*p == ' ') {
                continue;
            }
            state = 1;
            url_offset_ = p - buffer_->str;
            break;
        case 1:
            if (*p == ' ') {
                state = 2;
                url_length_ = p - buffer_->str - url_offset_;
                continue;
            } else if (*p == '\r' || *p == '\n') {
                goto _excepted;
            }
            break;
        case 2:
            if (*p == ' ') {
                continue;
            }
            if ((size_t) (pe - p) < (sizeof("HTTP/1.x") - 1)) {
                method = 0;
                return SW_ERR;
            }
            if (memcmp(p, SW_STRL("HTTP/1.1")) == 0) {
                version = SW_HTTP_VERSION_11;
                goto _end;
            } else if (memcmp(p, SW_STRL("HTTP/1.0")) == 0) {
                version = SW_HTTP_VERSION_10;
                goto _end;
            } else {
                goto _excepted;
            }
        default:
            break;
        }
    }
    // the request line is incomplete, parse it again when more data arrives
    method = 0;
    return SW_ERR;
_end:
    p += sizeof("HTTP/1.x") - 1;
    request_line_length_ = buffer_->offset = p - buffer_->str;
    return SW_OK;
}

bool Request::has_expect_header() const {
    char *buf = buffer_->str;
    size_t len = buffer_->length;

    char *pe = buf + len;

    for (char *p = buf; p < pe; p++) {
        if (*p == '\r' && (size_t) (pe - p) > sizeof("\r\nExpect")) {
            p += 2;
            if (SW_STR_ISTARTS_WITH(p, pe - p, "Expect:")) {
                p += sizeof("Expect:") - 1;
                while (p < pe && (*p == ' ' || *p == '\t')) {
                    p++;
                }
                if (SW_STR_ISTARTS_WITH(p, pe - p, "100-continue")) {
                    return true;
                } else {
                    return false;
                }
            } else {
                p++;
            }
        }
    }
    return false;
}

// Swoole v6.3.0, include/swoole_http.h (e0d606594), unchanged.
static inline size_t rtrim_ows(const char *str, size_t len) {
    while (len > 0 && (str[len - 1] == ' ' || str[len - 1] == '\t')) {
        len--;
    }
    return len;
}

// Swoole v6.3.0, src/protocol/http.cc (e0d606594), unchanged.
std::string Request::get_header(const char *name) const {
    size_t name_len = strlen(name);
    char *p = buffer_->str + url_offset_ + url_length_ + 10;
    char *pe = buffer_->str + header_length_;

    const char *buffer = nullptr;
    char *colon = nullptr;

    int state = 0;
    int i = 0;

    bool is_error_header_name = false;

    for (; p < pe; p++) {
        switch (state) {
        case 0:
            if (SW_STR_ISTARTS_WITH(p, pe - p, "\r\n")) {
                i = 0;
                is_error_header_name = false;
                break;
            }

            if (!is_error_header_name && swoole_str_istarts_with(p, pe - p, name, name_len)) {
                colon = p + name_len;
                if (colon[0] != ':' || i > 1) {
                    is_error_header_name = true;
                    break;
                }

                p += name_len;
                state = 1;
            }

            i++;
            break;
        case 1:
            if (!isspace(*p)) {
                buffer = p;
                state = 2;
            }
            break;
        case 2:
            if (SW_STR_ISTARTS_WITH(p, pe - p, "\r\n")) {
                return {buffer, rtrim_ows(buffer, static_cast<size_t>(p - buffer))};
            }
            break;
        default:
            break;
        }
    }

    return {};
}

// Swoole v6.3.0, src/protocol/http.cc (e0d606594), unchanged.
void Request::parse_header_info() {
    // header field start
    char *p = buffer_->str + request_line_length_ + (sizeof("\r\n") - 1);
    // point-end: start + strlen(all-header) without strlen("\r\n\r\n")
    char *pe = buffer_->str + header_length_ - (sizeof("\r\n\r\n") - 1);
    bool transfer_encoding = false;
    bool chunked_seen = false;
    bool after_chunked = false;

    for (; p < pe; p++) {
        if (*(p - 1) == '\n' && *(p - 2) == '\r') {
            if (SW_STR_ISTARTS_WITH(p, pe - p, "Content-Length:")) {
                // strlen("Content-Length:")
                p += (sizeof("Content-Length:") - 1);
                // skip spaces
                while (*p == ' ') {
                    p++;
                }
                content_length_ = strtoull(p, nullptr, 10);
                known_length = 1;
            } else if (SW_STR_ISTARTS_WITH(p, pe - p, "Connection:")) {
                // strlen("Connection:")
                p += (sizeof("Connection:") - 1);
                // skip OWS
                while (*p == ' ' || *p == '\t') {
                    p++;
                }
                if (SW_STR_ISTARTS_WITH(p, pe - p, "keep-alive")) {
                    keep_alive = 1;
                }
            } else if (SW_STR_ISTARTS_WITH(p, pe - p, "Transfer-Encoding:")) {
                // strlen("Transfer-Encoding:")
                p += (sizeof("Transfer-Encoding:") - 1);
                char *value_end = (char *) memchr(p, '\r', pe - p + 1);
                transfer_encoding = true;
                while (true) {
                    while (p < value_end && (*p == ' ' || *p == '\t')) {
                        p++;
                    }
                    char *coding = p;
                    while (p < value_end && *p != ',') {
                        p++;
                    }
                    const size_t coding_length = rtrim_ows(coding, p - coding);
                    if (chunked_seen) {
                        after_chunked = true;
                    }
                    if (SW_STRCASEEQ(coding, coding_length, "chunked")) {
                        chunked_seen = true;
                    }
                    if (p == value_end) {
                        break;
                    }
                    p++;
                }
                p = value_end;
            } else if (SW_STR_ISTARTS_WITH(p, pe - p, "Content-Type:")) {
                delete form_data_;
                form_data_ = nullptr;
                p += (sizeof("Content-Type:") - 1);
                while (*p == ' ' || *p == '\t') {
                    p++;
                }
                if (SW_STR_ISTARTS_WITH(p, pe - p, "multipart/form-data")) {
                    form_data_ = new FormData();
                    form_data_->multipart_boundary_buf = p + (sizeof("multipart/form-data") - 1);
                    form_data_->multipart_boundary_len = rtrim_ows(
                        form_data_->multipart_boundary_buf, strchr(p, '\r') - form_data_->multipart_boundary_buf);
                }
            }
        }
    }

    if (transfer_encoding) {
        chunked = chunked_seen && !after_chunked;
        if (!chunked) {
            excepted = 1;
        }
    }
    header_parsed = 1;
}

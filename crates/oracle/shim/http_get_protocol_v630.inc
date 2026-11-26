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

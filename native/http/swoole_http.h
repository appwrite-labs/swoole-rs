/* SPDX-License-Identifier: Apache-2.0 */
// http_server::Request and the HTTP helpers: what the compiled core-tests still use. crates/http and crates/ports define the functions under their
// upstream symbols; crates/http-ffi/src/request.rs owns Request's state.
#pragma once

#include "swoole_protocol.h"
#include "swoole_rs_http.h"

enum swHttpVersion {
    SW_HTTP_VERSION_10 = 1,
    SW_HTTP_VERSION_11,
    SW_HTTP_VERSION_2,
    SW_HTTP_VERSION_3,
};

// crates/http's method table numbers methods in this order, from 1.
enum swHttpMethod {
    SW_HTTP_DELETE = 1,
    SW_HTTP_GET,
    SW_HTTP_HEAD,
    SW_HTTP_POST,
    SW_HTTP_PUT,
    SW_HTTP_PATCH,
    SW_HTTP_CONNECT,
    SW_HTTP_OPTIONS,
    SW_HTTP_TRACE,
    SW_HTTP_COPY,
    SW_HTTP_LOCK,
    SW_HTTP_MKCOL,
    SW_HTTP_MOVE,
    SW_HTTP_PROPFIND,
    SW_HTTP_PROPPATCH,
    SW_HTTP_UNLOCK,
    SW_HTTP_REPORT,
    SW_HTTP_MKACTIVITY,
    SW_HTTP_CHECKOUT,
    SW_HTTP_MERGE,
    SW_HTTP_MSEARCH,
    SW_HTTP_NOTIFY,
    SW_HTTP_SUBSCRIBE,
    SW_HTTP_UNSUBSCRIBE,
    SW_HTTP_PURGE,
    SW_HTTP_QUERY,
    SW_HTTP_PRI,
};

enum swHttpStatusCode {
    SW_HTTP_SWITCHING_PROTOCOLS = 101,
    SW_HTTP_OK = 200,
};

namespace swoole {
namespace http_server {
struct Request;
struct FormData;
}  // namespace http_server
}  // namespace swoole

namespace swoole {
namespace http_server {
// crates/http/src/request.rs mirrors this layout; one-byte flags, not bit-fields.
struct Request {
    uint8_t method;
    uint8_t version;
    bool excepted;
    bool too_large;
    bool unavailable;

    bool header_parsed;
    bool tried_to_dispatch;
    bool multipart_header_parsed;

    bool known_length;
    bool keep_alive;
    bool chunked;
    uint8_t chunk_state;

    uint32_t url_offset_;
    uint32_t url_length_;

    uint32_t max_length_;
    uint32_t request_line_length_;
    uint32_t header_length_;
    uint64_t content_length_;

    FormData *form_data_;
    String *buffer_;
    uint64_t (*native_)(uint32_t, void *, const uint8_t *, uint64_t, uint64_t);
    size_t chunk_length_;
    size_t chunk_offset_;
    size_t chunk_scan_offset_;
};
static_assert(offsetof(Request, url_offset_) == 12 && offsetof(Request, content_length_) == 32 &&
                  offsetof(Request, form_data_) == 40 && offsetof(Request, buffer_) == 48 &&
                  offsetof(Request, native_) == 56 && sizeof(Request) == 88,
              "crates/http/src/request.rs mirrors Request");

int get_method(const char *method_str, size_t method_len);
const char *get_method_string(int method);
const char *get_status_message(int code);
extern int list_of_status_code[128];
// Decodes in place.
size_t url_decode(char *str, size_t len);
// The caller frees the result.
char *url_encode(char const *str, size_t len);
bool parse_multipart_boundary(
    const char *at, size_t length, size_t offset, char **out_boundary_str, int *out_boundary_len);
ssize_t get_package_length(const Protocol *protocol, network::Socket *conn, PacketLength *pl);
uint8_t get_package_length_size(network::Socket *conn);
}  // namespace http_server
}  // namespace swoole

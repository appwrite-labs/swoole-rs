// SPDX-License-Identifier: Apache-2.0
// The C layouts crates/http/src/request.rs mirrors: http_server::Request, and the head
// of the third-party multipart parser it drives.
#include "swoole_http.h"

// Relative to src/, where the build copies this file: not every consumer puts the tree root on -I.
#include "../thirdparty/multipart_parser.h"

using swoole::http_server::Request;

static_assert(offsetof(Request, url_offset_) == 12 && offsetof(Request, content_length_) == 32 &&
                  offsetof(Request, form_data_) == 40 && offsetof(Request, buffer_) == 48 &&
                  offsetof(Request, native_) == 56 && sizeof(Request) == 64,
              "crates/http/src/request.rs mirrors Request");
static_assert(offsetof(multipart_parser, boundary_length) == 24 && offsetof(multipart_parser, fp) == 32 &&
                  offsetof(multipart_parser, data) == 40 && offsetof(multipart_parser, boundary) == 50 &&
                  sizeof(multipart_parser_settings) == 7 * sizeof(void *),
              "crates/http/src/request.rs: Parser is multipart_parser's head");

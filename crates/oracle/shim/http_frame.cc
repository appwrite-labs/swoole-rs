// Request framing from the pinned http.cc behind C entry points: build.rs copies the
// bodies into SWR_FRAME_INC unchanged. The facade gives them upstream's Request
// fields and String's str/length/offset.
#include <cctype>
#include <cerrno>
#include <cstdint>
#include <cstring>
#include <cstdlib>
#include <string>
#include <strings.h>
#include <sys/types.h>
#include "http_frame_flags.inc"

#define SWR_CAT2(a, b) a##_##b
#define SWR_CAT(a, b) SWR_CAT2(a, b)

namespace SWR_FRAME_ENGINE {
using uchar = unsigned char;
// swResultCode and the macros it uses, as the pinned headers define them (build.rs copies
// them from upstream/include); tracing is off.
#include "http_frame_pinned.inc"
#define swoole_trace_log(what, str, ...)
__attribute__((unused)) static inline unsigned int swoole_str_istarts_with(const char *pstr, size_t plen, const char *sstr, size_t slen) {
    return (plen >= slen) && (strncasecmp(pstr, sstr, slen) == 0);
}
__attribute__((unused)) static inline unsigned int swoole_strcaseeq(const char *str1, size_t len1, const char *str2, size_t len2) {
    return (len1 == len2) && (strncasecmp(str1, str2, len1) == 0);
}
#include "http_frame_enums.inc"
struct String {
    char *str;
    size_t length;
    off_t offset;
};
struct FormData {
    const char *multipart_boundary_buf;
    uint32_t multipart_boundary_len;
};
struct Request {
    uint8_t method;
    uint8_t version;
    uchar excepted : 1;
    uchar too_large : 1;
    uchar known_length : 1;
    uchar keep_alive : 1;
    uchar chunked : 1;
    uchar header_parsed : 1;
    uint32_t url_offset_;
    uint32_t url_length_;
    uint32_t max_length_;
    uint32_t request_line_length_;
    uint32_t header_length_;
    uint64_t content_length_;
    uint8_t chunk_state_;
    size_t chunk_length_;
    size_t chunk_offset_;
    size_t chunk_scan_offset_;
    String *buffer_;
    FormData *form_data_;
    void parse_header_info();
    int get_protocol();
    int get_header_length();
    int get_chunked_body_length();
    bool has_expect_header() const;
    std::string get_header(const char *name) const;
};
#include SWR_FRAME_INC
}  // namespace SWR_FRAME_ENGINE

extern "C" {
using SWR_FRAME_ENGINE::Request;
using SWR_FRAME_ENGINE::String;
extern const uint8_t swr_oracle_http_versions[] = {
    SWR_FRAME_ENGINE::SW_HTTP_VERSION_10, SWR_FRAME_ENGINE::SW_HTTP_VERSION_11, SWR_FRAME_ENGINE::SW_HTTP_VERSION_2,
};

// See swoole_rs_oracle::http_frame::State.
struct swr_oracle_http_request {
    size_t offset;
    uint64_t content_length;
    uint32_t method, version, excepted, known_length, url_offset, url_length, line_length, header_length;
    uint32_t max_length, too_large, chunk_state;
    size_t chunk_length, chunk_offset, chunk_scan_offset;
};

// operation 0 get_protocol, 1 get_header_length, 2 get_chunked_body_length; data
// has a spare NUL after length, as a received String keeps spare capacity.
int SWR_CAT(swr_oracle_http_frame, SWR_FRAME_ENGINE)(uint32_t operation, char *data, size_t length,
                                                     swr_oracle_http_request *state) {
    String buffer{data, length, static_cast<off_t>(state->offset)};
    Request request{};
    request.method = state->method;
    request.version = state->version;
    request.excepted = state->excepted;
    request.known_length = state->known_length;
    request.url_offset_ = state->url_offset;
    request.url_length_ = state->url_length;
    request.request_line_length_ = state->line_length;
    request.header_length_ = state->header_length;
    request.content_length_ = state->content_length;
    request.max_length_ = state->max_length;
    request.too_large = state->too_large;
    request.chunk_state_ = state->chunk_state;
    request.chunk_length_ = state->chunk_length;
    request.chunk_offset_ = state->chunk_offset;
    request.chunk_scan_offset_ = state->chunk_scan_offset;
    request.buffer_ = &buffer;
    int result = operation == 0 ? request.get_protocol()
                 : operation == 1 ? request.get_header_length()
                                  : request.get_chunked_body_length();
    *state = {static_cast<size_t>(buffer.offset), request.content_length_, request.method, request.version,
              request.excepted, request.known_length, request.url_offset_, request.url_length_,
              request.request_line_length_, request.header_length_, request.max_length_,
              request.too_large, request.chunk_state_, request.chunk_length_, request.chunk_offset_,
              request.chunk_scan_offset_};
    return result;
}

// See swoole_rs_oracle::http_frame::HeaderInfo.
struct swr_oracle_http_header_info {
    uint64_t content_length;
    uint32_t flags, error;
    int64_t boundary_offset, boundary_length;
};

// parse_header_info over length bytes and a NUL, with this request line and header
// length, from errno 0. flags: build.rs's HTTP_FRAME_FLAGS. The boundary offset is -1
// without multipart.
void SWR_CAT(swr_oracle_http_header_info, SWR_FRAME_ENGINE)(char *data, size_t length, uint32_t line_length,
                                                           uint32_t header_length, swr_oracle_http_header_info *out) {
    String buffer{data, length, 0};
    Request request{};
    request.request_line_length_ = line_length;
    request.header_length_ = header_length;
    request.buffer_ = &buffer;
    errno = 0;
    request.parse_header_info();
    out->error = errno;
    out->content_length = request.content_length_;
    out->flags = (request.known_length ? SWR_HTTP_FLAG_KNOWN_LENGTH : 0) |
                 (request.keep_alive ? SWR_HTTP_FLAG_KEEP_ALIVE : 0) | (request.chunked ? SWR_HTTP_FLAG_CHUNKED : 0) |
                 (request.header_parsed ? SWR_HTTP_FLAG_HEADER_PARSED : 0) |
                 (request.excepted ? SWR_HTTP_FLAG_EXCEPTED : 0);
    out->boundary_offset = request.form_data_ ? request.form_data_->multipart_boundary_buf - data : -1;
    out->boundary_length = request.form_data_ ? request.form_data_->multipart_boundary_len : 0;
    delete request.form_data_;
}

bool SWR_CAT(swr_oracle_http_expect, SWR_FRAME_ENGINE)(char *data, size_t length) {
    String buffer{data, length, 0};
    Request request{};
    request.buffer_ = &buffer;
    return request.has_expect_header();
}

// Copies get_header(name) into output (capacity length); returns its size.
size_t SWR_CAT(swr_oracle_http_header, SWR_FRAME_ENGINE)(char *data, size_t length, uint32_t url_offset,
                                                        uint32_t url_length, uint32_t header_length,
                                                        const char *name, char *output) {
    String buffer{data, length, 0};
    Request request{};
    request.url_offset_ = url_offset;
    request.url_length_ = url_length;
    request.header_length_ = header_length;
    request.buffer_ = &buffer;
    std::string value = request.get_header(name);
    memcpy(output, value.data(), value.size());
    return value.size();
}

const char *SWR_CAT(swr_oracle_http_method, SWR_FRAME_ENGINE)(int method) {
    return SWR_FRAME_ENGINE::get_method_string(method);
}
}

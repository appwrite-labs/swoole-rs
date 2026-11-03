#ifndef SWOOLE_RS_HTTP_REQUEST_VIEW_H
#define SWOOLE_RS_HTTP_REQUEST_VIEW_H
#include "swoole_http.h"
#include "swoole_server.h"
namespace swoole {
namespace http_server {
// Pointer-only identity adapter. Every getter observes the same captured object
// at its call point; no scalar/text/resource snapshot extends across effects.
template<class T>
class LegacyRequestView {
    T *request_;
  public:
    explicit LegacyRequestView(T *request) : request_(request) {}
    T *get() const { return request_; }
    auto buffer() const { return request_->buffer_; }
    void set_buffer(decltype(request_->buffer_) value) { request_->buffer_ = value; }
    uint8_t method() const { return request_->method; }
    uint32_t url_offset() const { return request_->url_offset_; }
    uint32_t url_length() const { return request_->url_length_; }
    uint32_t header_length() const { return request_->header_length_; }
    uint32_t max_length() const { return request_->max_length_; }
    uint64_t content_length() const { return request_->content_length_; }
    bool has_form() const { return request_->form_data_ != nullptr; }
    bool excepted() const { return request_->excepted; }
    bool too_large() const { return request_->too_large; }
    bool unavailable() const { return request_->unavailable; }
    bool header_parsed() const { return request_->header_parsed; }
    bool tried_to_dispatch() const { return request_->tried_to_dispatch; }
    bool multipart_header_parsed() const { return request_->multipart_header_parsed; }
    bool known_length() const { return request_->known_length; }
    bool keep_alive() const { return request_->keep_alive; }
    bool chunked() const { return request_->chunked; }
    bool nobody_chunked() const { return request_->nobody_chunked; }
    void set_max_length(uint32_t value) { request_->max_length_ = value; }
    void advance_header_length(uint32_t value) { request_->header_length_ += value; }
    void mark_dispatched() { request_->tried_to_dispatch = 1; }
    void discard_form() { delete request_->form_data_; request_->form_data_ = nullptr; }
    void clean() { request_->clean(); }
    int get_protocol() { return request_->get_protocol(); }
    int get_header_length() { return request_->get_header_length(); }
    int get_chunked_body_length() { return request_->get_chunked_body_length(); }
    void parse_header_info() { request_->parse_header_info(); }
    bool parse_multipart_data(decltype(request_->buffer_) buffer) { return request_->parse_multipart_data(buffer); }
    bool init_multipart_parser(const Server *server) { return request_->init_multipart_parser(server); }
    void destroy_multipart_parser() { request_->destroy_multipart_parser(); }
    std::string get_header(const char *name) const { return request_->get_header(name); }
    bool has_expect_header() const { return request_->has_expect_header(); }
    bool select_static_handler(Server *server, const Connection *connection) const {
        return server->select_static_handler(request_,connection);
    }
};
using SelectedRequest = Request;
using SelectedRequestView = LegacyRequestView<SelectedRequest>;
static_assert(sizeof(LegacyRequestView<Request>)==sizeof(Request *),"Request view must retain only typed identity");
static_assert(sizeof(SelectedRequestView)==sizeof(SelectedRequest *),"Selected request view must retain only typed identity");
}
}
#endif

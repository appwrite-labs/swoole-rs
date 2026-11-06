#pragma once
#include "swoole_server.h"
#include "swoole_static_handler.h"
#include "swoole_rs_http.h"
#include <sstream>
#include <stdexcept>
#include <tuple>
struct StaticScenario {
    bool serve=true, modified=false, index_ok=true, index=false, autoindex=false, directory=false;
    int status=SW_HTTP_OK;
    std::string date="Mon, 01 Jan 2024 00:00:00 GMT", last_modified="Sun, 31 Dec 2023 00:00:00 GMT", mime="text/plain", content_type="text/plain";
    std::string body="directory body", filename="/static/file.txt", end_part="--boundary--\r\n";
    size_t filesize=12345, content_length=12345;
    struct Task { off_t offset; size_t length; char part_header[SW_HTTP_SERVER_PART_HEADER]{}; };
    std::vector<Task> tasks{{0,12345,{}}};
    bool throw_on_send=false; int send_result=0; unsigned toggle_keep_on_send=0, toggle_method_on_send=0;
};
#define STATIC_ORACLE(ns) namespace ns { \
namespace network { using SendfileTask=::swoole::network::SendfileTask; } \
using SendData=::swoole::SendData; using ::swoole::SW_SERVER_EVENT_SEND_DATA; using ::swoole::SW_SERVER_EVENT_SEND_FILE; using ::swoole::SW_SERVER_EVENT_CLOSE; \
struct Server; \
struct Buffer { std::string storage="request"; char *str=storage.data(); size_t length=storage.size(); void clear() { storage.clear();str=storage.data();length=0; } }; \
inline Buffer tls_buffer; inline Buffer *sw_tg_buffer() { return &tls_buffer; } \
inline void *sw_malloc(size_t n) { return std::malloc(n); } inline void sw_free(void *p) { std::free(p); } \
struct Socket { std::vector<std::string> *trace; void cork() { trace->push_back("cork"); } }; \
struct Connection { int session_id=73; Socket *socket; }; \
namespace http_server { \
inline const char *get_status_message(int status) { return status==SW_HTTP_NOT_FOUND ? "404 Not Found" : status==SW_HTTP_PARTIAL_CONTENT ? "206 Partial Content" : status==SW_HTTP_RANGE_NOT_SATISFIABLE ? "416 Range Not Satisfiable" : "200 OK"; } \
struct Request { Buffer *buffer_; size_t url_offset_=0,url_length_=7; int method=SW_HTTP_GET; bool keep_alive=true; std::string modified_header="",range="",if_range=""; mutable std::vector<std::string> trace; \
    std::string get_header(const char *key) const { trace.emplace_back(key); return std::string(key)=="If-Modified-Since" ? modified_header : std::string(key)=="Range" ? range : if_range; } \
}; \
struct StaticHandler { Server *server; int status_code; StaticHandler(Server *,const char *,size_t); \
    bool try_serve(); std::string get_date(); std::string get_date_last_modified(); bool is_modified(const std::string &); bool try_serve_index_file(); \
    bool has_index_file() const; bool is_enabled_auto_index() const; bool is_dir() const; size_t make_index_page(Buffer *); \
    void parse_range(const char *,const char *); const std::vector<StaticScenario::Task> &get_tasks() const; std::vector<StaticScenario::Task> copy_tasks() const; \
    size_t get_filesize() const; size_t get_content_length() const; const std::string &get_mimetype() const; const std::string &get_content_type(); \
    const char *get_filename() const; const std::string &get_end_part() const; int32_t get_status()const{return status_code;} void copy_filename(char *,size_t)const; size_t copy_end_part(char *,size_t)const; const std::string &get_content_type(std::string &scratch){(void)scratch;return get_content_type();} \
}; } \
struct Server { StaticScenario scenario; std::vector<std::string> trace; std::vector<std::tuple<int,int,std::string>> events; http_server::Request *mutable_request=nullptr; unsigned send_count=0; \
    bool select_static_handler(const http_server::Request *,const Connection *); \
    int send_to_connection(const SendData *response) { trace.push_back("send"); if(scenario.throw_on_send) throw std::runtime_error("send failed"); ++send_count; \
        if(mutable_request && send_count==scenario.toggle_keep_on_send) mutable_request->keep_alive=!mutable_request->keep_alive; \
        if(mutable_request && send_count==scenario.toggle_method_on_send) mutable_request->method=mutable_request->method==SW_HTTP_HEAD ? SW_HTTP_GET : SW_HTTP_HEAD; \
        std::string payload; if(response->info.type==SW_SERVER_EVENT_SEND_FILE) { auto task=reinterpret_cast<const network::SendfileTask *>(response->data); payload=std::to_string(task->offset)+":"+std::to_string(task->length)+":"+task->filename; } \
        else if(response->info.len) payload.assign(response->data,response->info.len); \
        events.emplace_back(response->info.fd,response->info.type,payload); return scenario.send_result; } \
}; \
inline http_server::StaticHandler::StaticHandler(Server *s,const char *,size_t):server(s),status_code(s->scenario.status) {} \
inline void http_server::StaticHandler::copy_filename(char *output,size_t capacity)const{if(server->scenario.filename.size()>=capacity)throw std::length_error("filename scratch");memcpy(output,server->scenario.filename.c_str(),server->scenario.filename.size()+1);} \
inline size_t http_server::StaticHandler::copy_end_part(char *output,size_t capacity)const{if(server->scenario.end_part.size()>capacity)throw std::length_error("end scratch");memcpy(output,server->scenario.end_part.data(),server->scenario.end_part.size());return server->scenario.end_part.size();} \
inline bool http_server::StaticHandler::try_serve() { server->trace.push_back("serve");return server->scenario.serve; } \
inline std::string http_server::StaticHandler::get_date() { server->trace.push_back("date");return server->scenario.date; } \
inline std::string http_server::StaticHandler::get_date_last_modified() { server->trace.push_back("modified-date");return server->scenario.last_modified; } \
inline bool http_server::StaticHandler::is_modified(const std::string &) { server->trace.push_back("is-modified");return server->scenario.modified; } \
inline bool http_server::StaticHandler::try_serve_index_file() { server->trace.push_back("index");return server->scenario.index_ok; } \
inline bool http_server::StaticHandler::has_index_file() const { return server->scenario.index; } \
inline bool http_server::StaticHandler::is_enabled_auto_index() const { return server->scenario.autoindex; } \
inline bool http_server::StaticHandler::is_dir() const { return server->scenario.directory; } \
inline size_t http_server::StaticHandler::make_index_page(Buffer *buffer) { server->trace.push_back("index-page");buffer->storage=server->scenario.body;buffer->str=buffer->storage.data();buffer->length=buffer->storage.size();return buffer->length; } \
inline void http_server::StaticHandler::parse_range(const char *,const char *) { server->trace.push_back("range"); } \
inline const std::vector<StaticScenario::Task> &http_server::StaticHandler::get_tasks() const { return server->scenario.tasks; } \
inline std::vector<StaticScenario::Task> http_server::StaticHandler::copy_tasks() const { return server->scenario.tasks; } \
inline size_t http_server::StaticHandler::get_filesize() const { return server->scenario.filesize; } \
inline size_t http_server::StaticHandler::get_content_length() const { return server->scenario.content_length; } \
inline const std::string &http_server::StaticHandler::get_mimetype() const { server->trace.push_back("mime");return server->scenario.mime; } \
inline const std::string &http_server::StaticHandler::get_content_type() { server->trace.push_back("content-type");return server->scenario.content_type; } \
inline const char *http_server::StaticHandler::get_filename() const { return server->scenario.filename.c_str(); } \
inline const std::string &http_server::StaticHandler::get_end_part() const { return server->scenario.end_part; } \
}
STATIC_ORACLE(upstream_static)
STATIC_ORACLE(candidate_static)
#undef STATIC_ORACLE

// Generated from crates/phphttp/src/state.rs; native borrowed views only.
#ifndef SWR_PHPHTTP_NATIVE_RECORD_CONTEXT
#include "php_swoole_cxx.h"
#include "php_swoole_http.h"
#else
#ifndef SWOOLE_RS_HTTP_RECORDS_H
#define SWOOLE_RS_HTTP_RECORDS_H
struct Request {
    int version;
    char * path;
    uint32_t path_len;
    const char * ext;
    uint32_t ext_len;
    uint8_t post_form_urlencoded;
    zval zdata;
    const char * body_at;
    size_t body_length;
    void * h2_data_;
    zval *zobject;
    zval _zobject;
    zval *zserver;
    zval _zserver;
    zval *zheader;
    zval _zheader;
    zval *zget;
    zval _zget;
    zval *zpost;
    zval _zpost;
    zval *zcookie;
    zval _zcookie;
    zval *zfiles;
    zval _zfiles;
    zval *ztmpfiles;
    zval _ztmpfiles;
};
struct Response {
    enum llhttp_method method;
    int version;
    int status;
    char * reason;
    zval *zobject;
    zval _zobject;
    zval *zheader;
    zval _zheader;
    zval *zcookie;
    zval _zcookie;
    zval *ztrailer;
    zval _ztrailer;
};
struct ContextState {
    SessionId fd;
    Request request;
    llhttp_t parser;
    multipart_parser * mt_parser;
    uint8_t completed;
    uint8_t keepalive;
    uint8_t recv_chunked;
    uint8_t websocket;
    uint8_t parse_cookie;
    uint8_t parse_body;
    uint8_t parse_files;
    uint8_t enable_compression;
    uint8_t accept_compression;
    int8_t compression_method;
    alignas(8) unsigned char parse_[SWR_PHPHTTP_PARSE_SIZE];
    Response response;
    uint32_t compression_min_length;
    int8_t compression_level;
    uint8_t send_chunked;
    uint8_t upgrade;
    uint8_t send_header_;
    uint8_t content_compressed;
    uint8_t end_;
    uint8_t send_trailer_;
    uint8_t websocket_compression;
    uint8_t detached;
    uint8_t http2;
    zval zsocket;
    uint32_t stream_id;
    void * private_data;
    void * write_buffer_;
    void * compressed_;
    void * callback;
    uint8_t before_request;
    uint8_t after_response;
    uint8_t websocket_settings;
    void * upload_tmp_dir_;
    void * compression_types_;
    void * ws_buffer_;
};
#endif
#endif

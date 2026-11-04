#pragma once
#include "swoole_server.h"
#include "swoole_websocket.h"
#include "swoole_http2.h"
#include "swoole_rs_http.h"
#include <set>
#include <stdexcept>
#define DISPATCH_ORACLE(ns) namespace ns { \
struct HttpProxy { std::string host, username, password; int port = 0; \
    static inline HttpProxy *last_created = nullptr; HttpProxy() { last_created=this; } \
    static HttpProxy *create(const std::string &,int,const std::string &,const std::string &); }; \
struct Protocol {}; struct PacketLength { int seen=0; }; struct RecvData {}; \
struct Connection { uint8_t websocket_status=0,http2_stream=0; }; \
struct Socket { void *object; }; inline unsigned ws_calls=0,h2_calls=0,error_calls=0,dispatch_calls=0; \
inline const Protocol *seen_proto=nullptr; inline Socket *seen_socket=nullptr; inline const RecvData *seen_data=nullptr; \
inline void reset() { ws_calls=h2_calls=error_calls=dispatch_calls=0;seen_proto=nullptr;seen_socket=nullptr;seen_data=nullptr; } \
namespace websocket { constexpr int STATUS_HANDSHAKE=2; \
inline ssize_t get_package_length(const Protocol *p,Socket *s,PacketLength *pl){++ws_calls;seen_proto=p;seen_socket=s;pl->seen=11;return -11;} \
inline int dispatch_frame(const Protocol *p,Socket *s,const RecvData *d){++ws_calls;seen_proto=p;seen_socket=s;seen_data=d;return -22;} } \
namespace http2 { inline ssize_t get_frame_length(const Protocol *p,Socket *s,PacketLength *pl){++h2_calls;seen_proto=p;seen_socket=s;pl->seen=33;return 44;} } \
inline void protocol_status_error(Socket *s,const Connection *) { ++error_calls;seen_socket=s; } \
struct Server { std::shared_ptr<std::vector<std::string>> http_index_files; bool http_autoindex=false; \
static int dispatch_task(const Protocol *p,Socket *s,const RecvData *d){++dispatch_calls;seen_proto=p;seen_socket=s;seen_data=d;return 55;} }; \
ssize_t get_package_length(const Protocol *,Socket *,PacketLength *);uint8_t get_package_length_size(Socket *);int dispatch_frame(const Protocol *,Socket *,const RecvData *); \
}
DISPATCH_ORACLE(upstream_dispatch)
DISPATCH_ORACLE(candidate_dispatch)
#undef DISPATCH_ORACLE

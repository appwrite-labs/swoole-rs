#pragma once
#include "swoole_server.h"
#include "swoole_rs_http.h"
#include "swoole_proxy.h"
#include <stdexcept>
size_t base64_encode(const unsigned char *, size_t, char *);
namespace upstream_dispatch {
struct HttpProxy {
    std::string host, username, password;
    int port = 0;
    static inline HttpProxy *last_created = nullptr;
    HttpProxy() { last_created = this; }
    static HttpProxy *create(const std::string &, int, const std::string &, const std::string &);
    std::string get_auth_str() const;
};
}
#ifdef SWOOLE_RS_PROXY_OWNER
namespace candidate_dispatch { using HttpProxy = swoole::HttpProxy; }
#endif

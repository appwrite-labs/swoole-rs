#pragma once
#include "swoole_http.h"
#include "swoole_proxy.h"
#include "swoole_rs_http.h"
#include <cstdarg>
#include <cstdio>
#include <stdexcept>
size_t base64_encode(const unsigned char *, size_t, char *);
#define PROXY_ORACLE(ns) namespace ns { \
struct String { char *str; size_t size, length; }; \
inline int last_error = 0; \
inline void proxy_set_last_error(int value) { last_error = value; } \
struct HttpProxy { std::string username, password, target_host; int target_port; \
    std::string get_auth_str() const; size_t pack(const String *, const std::string &) const; \
    static bool handshake(const String *); \
}; }
PROXY_ORACLE(upstream_proxy)
#ifdef SWOOLE_RS_PROXY_OWNER
namespace candidate_proxy {
struct String { char *str; size_t size, length; };
inline int last_error = 0;
inline void proxy_set_last_error(int value) { last_error = value; }
struct HttpProxy : swoole::HttpProxy {
    HttpProxy(const std::string &user, const std::string &pass, const std::string &target, int port) {
        username=user;password=pass;target_host=target;target_port=port;
    }
    std::string get_auth_str() const;
    size_t pack(const String *, const std::string &) const;
    static bool handshake(const String *);
};
}
#endif
#undef PROXY_ORACLE
// Route the unchanged body to the counted native error facade, avoiding enum ADL.
#define swoole_set_last_error proxy_set_last_error

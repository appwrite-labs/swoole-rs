#pragma once
#include "swoole_rs_httpbindings.h"
#include <cstdarg>
#include <cstdio>
#include <cstdint>
#include <cstring>
#include <cerrno>
#include <functional>
#include <map>
#include <set>
#include <string>
#include <vector>
#include <unordered_map>
#include <stdexcept>
#include <strings.h>
#define SW_STRL(s) s,sizeof(s)-1
#define sw_unlikely(x) (x)
using zend_long=long;
struct AcceptServer;
struct Exit{};
struct Oracle {
 size_t fail=SIZE_MAX;long cid=0;int accept_error=0;bool accept_connection=false;AcceptServer*accept_server=nullptr;std::vector<std::string>trace;
 void event(const std::string&s){trace.push_back(s);if(trace.size()-1==fail)throw Exit{};}
};
extern thread_local Oracle*oracle;
struct zval{std::string value;};
namespace zend{struct Callable{int id;};}
inline void emit_log(const char*prefix,const char*fmt,va_list args){char b[512];vsnprintf(b,sizeof(b),fmt,args);oracle->event(std::string(prefix)+b);}
struct RouteContext{struct{const char*path;size_t path_len;}request;};
#define HANDLER_DECL(ns) namespace ns{struct HandlerServer{zend::Callable*default_handler=nullptr;std::unordered_map<std::string,zend::Callable*>handlers;zend::Callable*get_handler(const RouteContext*)const;};}
HANDLER_DECL(upstream_server)
HANDLER_DECL(candidate_server)

#define SW_ERROR_SSL_BAD_CLIENT 4001
#define SW_ACCEPT_RETRY_TIME 1
#define E_WARNING 2
#define ZEND_THIS nullptr
#define RETURN_FALSE do{oracle->event("return-false");return;}while(0)
struct AcceptSocket{int errCode=0;const char*errMsg="fake-error";AcceptSocket*accept();};
struct AcceptServer{bool running=true;};
inline AcceptSocket*AcceptSocket::accept(){oracle->event("accept");oracle->accept_server->running=false;errCode=oracle->accept_error;return oracle->accept_connection?this:nullptr;}
struct Callback{zval value;zval*ptr(){return &value;}};
struct PHPCoroutine{static long create(void*,int,zval*,zval*){oracle->event("create-coroutine");return oracle->cid;}};
struct System{static void sleep(int){oracle->event("sleep");}};
inline void php_swoole_init_socket_object(zval*,AcceptSocket*){oracle->event("socket-object");}
inline void zval_dtor(zval*){oracle->event("socket-dtor");}
// The hybrid's bailout trap (php_swoole_http.h): no bailout without Zend.
template<class F>bool swr_http_host_try(F&&body){body();return false;}
inline void http_server_coro_set_error(void*,AcceptSocket*){oracle->event("set-error");}
inline void php_swoole_fatal_error(int,const char*fmt,...){va_list args;va_start(args,fmt);emit_log("warning:",fmt,args);va_end(args);}
namespace upstream_server{void call_accept(AcceptSocket*,AcceptServer*);}
namespace candidate_server{void call_accept(AcceptSocket*,AcceptServer*);}

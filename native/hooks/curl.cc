// SPDX-License-Identifier: Apache-2.0
// What crates/curl's coroutine multi engine borrows from C++: the reactor's
// Socket, handlers and events, timers, the current Coroutine's yield and
// resume, and the bundled php_curl handle (thirdparty/php/curl) that
// curl_easy_perform and the curl_* functions reach it through.
#include "swoole_rs_phphost.h"
#include "swoole_rs_curl.h"
#include "swoole_socket.h"

#ifdef SW_USE_CURL

// The cURL sockets' reactor fd type, upstream's PHP_SWOOLE_FD_CO_CURL.
static constexpr int PHP_SWOOLE_FD_CO_CURL = SW_FD_STREAM_CLIENT + 10;

using swoole::Coroutine;
using swoole::Event;
using swoole::FdType;
using swoole::Reactor;
using swoole::Timer;
using swoole::TimerNode;

static int socket_event(Event *event, int bitmask) {
    swr_v1_curl_socket_event(event->socket->object, bitmask);
    return 0;
}

static int cb_readable(Reactor *reactor, Event *event) {
    return socket_event(event, CURL_CSELECT_IN);
}

static int cb_writable(Reactor *reactor, Event *event) {
    return socket_event(event, CURL_CSELECT_OUT);
}

static int cb_error(Reactor *reactor, Event *event) {
    return socket_event(event, CURL_CSELECT_ERR);
}

static const swr_v1_curl_ops ops = {
    []() { return swoole_event_is_available(); },
    []() {
        if (sw_unlikely(!swoole_event_isset_handler(PHP_SWOOLE_FD_CO_CURL, SW_EVENT_READ))) {
            swoole_event_set_handler(PHP_SWOOLE_FD_CO_CURL, SW_EVENT_READ, cb_readable);
            swoole_event_set_handler(PHP_SWOOLE_FD_CO_CURL, SW_EVENT_WRITE, cb_writable);
            swoole_event_set_handler(PHP_SWOOLE_FD_CO_CURL, SW_EVENT_ERROR, cb_error);
        }
    },
    [](int fd, void *object) -> void * {
        auto socket = new swoole::network::Socket();
        socket->fd = fd;
        socket->removed = 1;
        socket->fd_type = static_cast<FdType>(PHP_SWOOLE_FD_CO_CURL);
        socket->object = object;
        return socket;
    },
    [](void *socket, bool *removed) {
        auto sock = static_cast<swoole::network::Socket *>(socket);
        if (removed) {
            *removed = sock->removed;
        }
        return static_cast<int>(sock->events);
    },
    [](void *socket, int events) { return swoole_event_add(static_cast<swoole::network::Socket *>(socket), events); },
    [](void *socket, int events) { return swoole_event_set(static_cast<swoole::network::Socket *>(socket), events); },
    [](void *socket) { return swoole_event_del(static_cast<swoole::network::Socket *>(socket)); },
    [](void *socket) {
        auto sock = static_cast<swoole::network::Socket *>(socket);
        if (sock->events && sw_likely(swoole_event_is_available())) {
            sock->silent_remove = 1;
            swoole_event_del(sock);
        }
        sock->fd = -1;
        sock->free();
    },
    []() { return swoole_timer_is_available(); },
    [](long ms, void *multi) -> void * {
        return swoole_timer_add(
            ms,
            false,
            [](Timer *, TimerNode *tnode) { swr_v1_curl_multi_on_timer(static_cast<swr_v1_curl_multi *>(tnode->data)); },
            multi);
    },
    [](void *timer) { swoole_timer_del(static_cast<TimerNode *>(timer)); },
    []() -> void * { return Coroutine::get_current(); },
    [](void *co, double timeout) {
        auto coroutine = static_cast<Coroutine *>(co);
        coroutine->yield_ex(timeout);
        return coroutine->is_canceled();
    },
    [](void *co) { static_cast<Coroutine *>(co)->resume(); },
    [](void *multi) {
        swoole_event_defer([](void *data) { swr_v1_curl_multi_deferred(static_cast<swr_v1_curl_multi *>(data)); },
                           multi);
    },
    []() { swoole_set_last_error(SW_ERROR_CO_CANCELED); },
    []() { swoole_log_as("Multi::read_info", swoole_warning("CURLMSG default")); },
};

swr_v1_curl_multi *swoole_curl_multi_new() {
    return swr_v1_curl_multi_new(&ops);
}

void *swoole_curl_check_bound_co(swr_v1_curl_multi *multi) {
    auto co = static_cast<Coroutine *>(swr_v1_curl_multi_co(multi));
    if (co) {
        Coroutine::set_socket_bound_cid(co->get_cid());
        swoole_fatal_error(SW_ERROR_CO_HAS_BEEN_BOUND,
                           "This cURL handle is currently executing in coroutine#%ld, cannot be operated",
                           co->get_cid());
        return nullptr;
    }
    return Coroutine::get_current_safe();
}

CURLcode swoole_curl_easy_perform(CURL *cp) {
    auto handle = swr_v1_curl_get_handle(cp);
    if (!handle->easy_multi) {
        handle->easy_multi = swoole_curl_multi_new();
    }
    int result = swr_v1_curl_multi_exec(handle->easy_multi, handle);
    if (result < 0) {
        swoole_curl_check_bound_co(handle->easy_multi);
    }
    return static_cast<CURLcode>(result);
}

php_curl *swoole_curl_get_handle(zval *zid, bool exclusive, bool required) {
    php_curl *ch = Z_CURL_P(zid);
    if (SWOOLE_G(req_status) == PHP_SWOOLE_RSHUTDOWN_END) {
        exclusive = false;
    }
    if (exclusive && swoole_coroutine_is_in()) {
        auto handle = swr_v1_curl_get_handle(ch->cp);
        if (required && !handle) {
            php_swoole_fatal_error(E_WARNING, "The given handle is not initialized in coroutine");
            return nullptr;
        }
        if (handle && handle->multi && swoole_curl_check_bound_co(handle->multi) == nullptr) {
            return nullptr;
        }
    }
    return ch;
}

#endif

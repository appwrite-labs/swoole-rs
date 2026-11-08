// SPDX-License-Identifier: Apache-2.0
// What the resolver needs from C++ (upstream's src/network/dns.cc is deleted):
// the std::string signatures of swoole.h's DNS API and the name resolver lookup
// (the resolver list is crates/corebase/src/name_resolver.rs), the C++ locale's space table for the hosts file, the
// coroutine Socket and Coroutine calls, and the ResolverOwner exception paths.
// The resolver itself is crates/clients (dns.rs, cares.rs).
#include "swoole_rs_coroutine_socket.h"
#include "swoole_coroutine_system.h"
#include "swoole_coroutine_api.h"
#include "swoole_async.h"
#include "swoole_rs_clients.h"
#include "swoole_rs_phpclients.h"

#include <locale>
#include <string>
#include <vector>

#ifdef SW_USE_CARES
#include <ares.h>
#include "swoole_rs_cares.h"
#endif

using swoole::NameResolver;
using swoole::coroutine::Socket;
using swoole::coroutine::System;

static void assign_text(void *out, const uint8_t *text, size_t length) {
    static_cast<std::string *>(out)->assign(reinterpret_cast<const char *>(text), length);
}

static void push_text(void *out, const uint8_t *text, size_t length) {
    static_cast<std::vector<std::string> *>(out)->emplace_back(reinterpret_cast<const char *>(text), length);
}

static swoole::DnsServer dns_server() {
    swoole::DnsServer server;
    server.port = swr_v1_clients_dns_server(&server.host, assign_text);
    return server;
}

SW_API bool swoole_load_resolv_conf() {
    return swr_v1_clients_load_resolv_conf(SwooleG.dns_resolvconf_path.c_str()) > 0;
}

SW_API void swoole_set_dns_server(const std::string &server) {
    swr_v1_clients_set_dns_server(reinterpret_cast<const uint8_t *>(server.data()), server.size());
}

SW_API swoole::DnsServer swoole_get_dns_server() {
    if (dns_server().host.empty()) {
        swoole_load_resolv_conf();
    }
    return dns_server();
}

SW_API void swoole_set_hosts_path(const std::string &hosts_file) {
    swr_v1_clients_set_hosts_path(reinterpret_cast<const uint8_t *>(hosts_file.data()), hosts_file.size());
}

SW_API std::string swoole_name_resolver_lookup(const std::string &host_name, NameResolver::Context *ctx) {
    std::string result;
    if (swr_v1_core_name_resolver_resolve(host_name.data(), host_name.size(), ctx, &result,
                                          [](void *out, const char *text, size_t length) {
                                              static_cast<std::string *>(out)->assign(text, length);
                                          })) {
        return result;
    }
    if (swoole_coroutine_is_in()) {
        return System::gethostbyname(host_name, ctx->type, ctx->timeout);
    }
    return swoole::network::gethostbyname(ctx->type, host_name);
}

// Here, not in name_resolver.cc, so Zend-free callers (core-tests' Rust cases) reach it too.
void swoole_name_resolver_lookup_run(void *call) {
    auto *c = static_cast<swoole_name_resolver_lookup_call *>(call);
    auto result = swoole_name_resolver_lookup(std::string(c->name, c->length), static_cast<NameResolver::Context *>(c->context));
    c->assign(c->out, result.data(), result.size());
}

namespace swoole {
namespace coroutine {

std::string get_ip_by_hosts(const std::string &search_domain) {
    // std::istringstream originally captured the current C++ locale for each line.
    const auto &facet = std::use_facet<std::ctype<char>>(std::locale());
    uint8_t spaces[256];
    for (unsigned i = 0; i < 256; ++i) {
        spaces[i] = facet.is(std::ctype_base::space, static_cast<char>(i));
    }
    std::string address;
    swr_v1_clients_hosts_lookup(
        reinterpret_cast<const uint8_t *>(search_domain.data()), search_domain.size(), spaces, &address, assign_text);
    return address;
}

static const swr_v1_clients_dns_socket dns_socket = {
    [](double timeout) -> void * {
        auto sock = new Socket(SW_SOCK_UDP);
        if (timeout > 0) {
            sock->set_timeout(timeout);
        }
        return sock;
    },
    [](void *sock, const char *host, int port, const uint8_t *data, size_t length) {
        return static_cast<Socket *>(sock)->sendto(host, port, reinterpret_cast<const char *>(data), length) != 0;
    },
    [](void *socket, uint8_t *data, size_t capacity) -> long {
        auto sock = static_cast<Socket *>(socket);
        const ssize_t n = sock->recv(data, capacity);
        return n <= 0 && sock->errCode == ECANCELED ? -ECANCELED : n;
    },
    [](void *sock) { delete static_cast<Socket *>(sock); },
};

std::vector<std::string> dns_lookup_impl_with_socket(const char *domain, int family, double timeout) {
    std::vector<std::string> result;
    swr_v1_clients_dns_lookup(
        domain, family, timeout, SwooleG.dns_resolvconf_path.c_str(), &dns_socket, &result, push_text);
    return result;
}

#ifdef SW_USE_CARES
static const swr_v1_clients_cares_ops cares_ops = {
    []() -> void * { return Coroutine::get_current_safe(); },
    [](void *co) -> uint8_t { return static_cast<Coroutine *>(co)->is_suspending(); },
    [](void *co, double timeout) { static_cast<Coroutine *>(co)->yield_ex(timeout); },
    [](void *co) -> uint8_t { return static_cast<Coroutine *>(co)->is_canceled(); },
    [](void *co) -> uint8_t { return static_cast<Coroutine *>(co)->is_timedout(); },
    [](void *co) { static_cast<Coroutine *>(co)->resume(); },
    [](uint8_t kind, int32_t value) {
        swoole_log_as("coroutine::dns_lookup_impl_with_cares", switch (kind) {
        case 0: swoole_warning("error events, fd=%d", value); break;
        case 1: swoole_warning("ares_init_options() failed, Error: %s[%d]", ares_strerror(value), value); break;
        case 2: swoole_warning("not support to set port of dns server"); break;
        case 3: swoole_warning("not support to set dns server"); break;
        });
    },
    [](uint8_t kind, int32_t a, int32_t b, size_t c) {
        swoole_log_as("coroutine::dns_lookup_impl_with_cares", switch (kind) {
        case 0: swoole_trace_log(SW_TRACE_CARES, "[event callback] readable event, fd=%d", a); break;
        case 1: swoole_trace_log(SW_TRACE_CARES, "[event callback] writable event, fd=%d", a); break;
        case 2: swoole_trace_log(SW_TRACE_CARES, "[sock_state_cb], fd=%d, readable=%d, writable=%d", a, b, int(c)); break;
        case 3: swoole_trace_log(SW_TRACE_CARES, "[del event], fd=%d", a); break;
        case 4: swoole_trace_log(SW_TRACE_CARES, "[set event] fd=%d, events=%d", a, b); break;
        case 5: swoole_trace_log(SW_TRACE_CARES, "[add event] fd=%d, events=%d", a, b); break;
        case 6: swoole_trace_log(SW_TRACE_CARES, "[cares callback] status=%d, timeouts=%d", a, b); break;
        case 7: swoole_trace_log(SW_TRACE_CARES, "lookup success, result_count=%lu", c); break;
        });
    },
};

std::vector<std::string> dns_lookup_impl_with_cares(const char *domain, int family, double timeout) {
    std::vector<std::string> result;
    const auto server = dns_server();
    swr_v1_clients_cares_lookup(
        &cares_ops, domain, family, timeout, server.host.c_str(), server.port, &result, push_text);
    return result;
}
#endif

std::vector<std::string> dns_lookup(const char *domain, int family, double timeout) {
    family = family == AF_INET6 ? AF_INET6 : AF_INET;  // only support IPv4 and IPv6
#ifdef SW_USE_CARES
    return dns_lookup_impl_with_cares(domain, family, timeout);
#else
    return dns_lookup_impl_with_socket(domain, family, timeout);
#endif
}

}  // namespace coroutine

namespace network {

int gethostbyname(int flags, const char *name, char *addr) {
    return swr_v1_clients_gethostbyname(flags, name, addr);
}

std::string gethostbyname(int type, const std::string &name) {
    char addr[sizeof(in6_addr)];
    if (gethostbyname(type, name.c_str(), addr) == SW_OK) {
        return Address::addr_str(type, addr);
    }
    swoole_set_last_error(SW_ERROR_DNSLOOKUP_RESOLVE_FAILED);
    return {};
}

int getaddrinfo(swr_v1_core_resolver *request) {
    std::exception_ptr exception;
    const int status = swr_v1_core_resolver_lookup(
        request,
        [](void *context, int family) {
            try {
                swoole_warning("unknown socket family[%d]", family);
                return true;
            } catch (...) {
                *static_cast<std::exception_ptr *>(context) = std::current_exception();
                return false;
            }
        },
        &exception);
    if (status == -3) std::rethrow_exception(exception);
    if (status < -1) ResolverOwner::check(status);
    return status;
}

int gethostbyname(GethostbynameRequest *req) {
    const int status = swr_v1_core_resolver_host(req->state_handle());
    if (status < -1) req->addr.check(status);
    if (status == -1) swoole_set_last_error(SW_ERROR_DNSLOOKUP_RESOLVE_FAILED);
    return status;
}
}  // namespace network

void GetaddrinfoRequest::parse_result(std::vector<std::string> &retval) const {
    size_t cursor = 0;
    while (true) {
        auto address = swr_v1_core_resolver_next(state_.owner, &cursor);
        if (!address.valid) break;
        retval.emplace_back(address.bytes);
    }
}
}  // namespace swoole

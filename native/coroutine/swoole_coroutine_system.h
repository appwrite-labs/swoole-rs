/* SPDX-License-Identifier: Apache-2.0 */
/* Swoole's include/swoole_coroutine_system.h, owned here: the System members C++ still
 * calls. sleep, init_reactor and the waitpids are Rust under upstream's symbols
 * (crates/hooks, crates/process); write_file, gethostbyname and
 * gethostbyname_impl_with_async are native/hooks/system.cc. */
#pragma once

#include "swoole_coroutine.h"

namespace swoole {
namespace coroutine {
class System {
  public:
    static void init_reactor(Reactor *reactor);
    static int sleep(double sec);
    static ssize_t write_file(const char *file, const char *buf, size_t length, bool lock = false, int flags = 0);
    static std::string gethostbyname(const std::string &hostname, int domain, double timeout = -1);
    static pid_t waitpid(pid_t _pid, int *_stat_loc, int _options, double timeout = -1);
    static pid_t waitpid_safe(pid_t _pid, int *_stat_loc, int _options);
};
std::string gethostbyname_impl_with_async(const std::string &hostname, int domain, double timeout = -1);
}  // namespace coroutine
struct DnsServer {
    std::string host;
    int port;
};
}  // namespace swoole

/* Upstream swoole.h's DNS API, native/clients/dns.cc's. */
void swoole_set_dns_server(const std::string &server);
void swoole_set_hosts_path(const std::string &hosts_file);
swoole::DnsServer swoole_get_dns_server();
bool swoole_load_resolv_conf();
void swoole_name_resolver_add(const swoole::NameResolver &resolver, bool append = true);
void swoole_name_resolver_each(
    const std::function<swTraverseOperation(const std::list<swoole::NameResolver>::iterator &iter)> &fn);
std::string swoole_name_resolver_lookup(const std::string &host_name, swoole::NameResolver::Context *ctx);

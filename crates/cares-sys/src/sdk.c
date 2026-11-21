/* SPDX-License-Identifier: Apache-2.0 */
/* c-ares's constants and options. Stays C (docs/zend.md, What stays C): version
 * (the server API and ares_options members depend on ARES_VERSION, and the
 * status and option values are macros of the installed headers). */
#include <ares.h>
#include <ares_version.h>
#include <arpa/inet.h>
#include <string.h>

_Static_assert(ARES_SOCKET_BAD == -1, "POSIX invalid socket sentinel");
_Static_assert(sizeof(ares_socket_t) == sizeof(int), "Swoole uses POSIX integer descriptors");
struct metadata {
    int library_all, success, cancelled, timeout, timeout_ms, tries, socket_state, lookups, server_api;
};
const struct metadata *swrs_cares_metadata(void) {
    static const struct metadata metadata = {
        ARES_LIB_INIT_ALL, ARES_SUCCESS, ARES_ECANCELLED, ARES_ETIMEOUT,
        ARES_OPT_TIMEOUTMS, ARES_OPT_TRIES, ARES_OPT_SOCK_STATE_CB, ARES_OPT_LOOKUPS,
#if ARES_VERSION >= 0x010b00
        2
#elif ARES_VERSION >= 0x010701
        1
#else
        0
#endif
    };
    return &metadata;
}
int swrs_cares_init(ares_channel *channel, const char *lookups, int timeout, int tries,
                   ares_sock_state_cb callback, void *data, int mask) {
    struct ares_options options;
    memset(&options, 0, sizeof(options));
    options.lookups = (char *) lookups;
    options.timeout = timeout;
    options.tries = tries;
    options.sock_state_cb = callback;
    options.sock_state_cb_data = data;
    return ares_init_options(channel, &options, mask);
}
void swrs_cares_server(ares_channel channel, const char *host, unsigned short port) {
#if ARES_VERSION >= 0x010b00
    struct ares_addr_port_node server;
    memset(&server, 0, sizeof(server));
    server.family = AF_INET;
    inet_pton(AF_INET, host, &server.addr.addr4);
    server.udp_port = port;
    ares_set_servers_ports(channel, &server);
#elif ARES_VERSION >= 0x010701
    struct ares_addr_node server;
    memset(&server, 0, sizeof(server));
    server.family = AF_INET;
    inet_pton(AF_INET, host, &server.addr.addr4);
    ares_set_servers(channel, &server);
#endif
}

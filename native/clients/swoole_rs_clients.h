/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_CLIENTS_H
#define SWOOLE_RS_CLIENTS_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>
#ifdef __cplusplus
extern "C" {
#endif
const char *swr_v1_clients_address_string(int family, const void *address);
bool swr_v1_clients_address_verify(int family, const char *host);
const char *swr_v1_clients_address_host(const void *address);
int swr_v1_clients_address_port(const void *address);
void swr_v1_clients_address_set_port(void *address, int port);
bool swr_v1_clients_address_empty(const void *address);
const char *swr_v1_clients_address_type(int kind);
bool swr_v1_clients_address_loopback(const void *address);
int swr_v1_clients_address_assign(void *address, uint32_t *length, int kind, const char *host, size_t host_length, int port, int (*resolve)(int family, const char *name, char *address));
int swr_v1_clients_dns_query(const char *domain, int family, uint16_t request_id, void *context, bool (*send)(void *context, const uint8_t *data, size_t length), long (*recv)(void *context, uint8_t *data, size_t capacity), void (*push)(void *context, const uint8_t *text, size_t length), bool signed_char);
int swr_v1_clients_gethostbyname(int flags, const char *name, char *addr);
/* The resolver's configuration (SwooleG.dns_server and dns_hosts_path upstream). */
void swr_v1_clients_set_dns_server(const uint8_t *server, size_t length);
int swr_v1_clients_dns_server(void *context, void (*push)(void *context, const uint8_t *text, size_t length));
int swr_v1_clients_load_resolv_conf(const char *path);
void swr_v1_clients_set_hosts_path(const uint8_t *path, size_t length);
bool swr_v1_clients_hosts_lookup(const uint8_t *domain, size_t domain_length, const uint8_t *spaces, void *context, void (*push)(void *context, const uint8_t *text, size_t length));
typedef struct { int error; int count; } swr_v1_clients_resolved;
swr_v1_clients_resolved swr_v1_clients_getaddrinfo(const char *host, const char *service, int family, int socktype, int protocol, void *results, size_t capacity, void (*unknown)(int family));
struct swr_v1_clients_ip_plan { uint8_t bytes[16]; uint32_t length; };
struct swr_v1_clients_ip_plan swr_v1_clients_ip(uint8_t version, const uint8_t *data, size_t length, uint8_t allow_zeroes);
uint8_t swr_v1_clients_loopback(uint8_t version, const uint8_t *address);
struct swr_v1_clients_ip_text_plan { uint8_t bytes[40]; uint32_t length; };
struct swr_v1_clients_ip_text_plan swr_v1_clients_ip_text(uint8_t version, const uint8_t *address);
/* Address::assign(url): outcome 0 with the type, host span and port to assign, or
 * INVALID (native logs the notice) or PORT_RANGE (std::stoi's out_of_range). */
enum { SWR_CLIENTS_URL_INVALID = -1, SWR_CLIENTS_URL_PORT_RANGE = -2 };
struct swr_v1_clients_url_plan { int outcome, kind; size_t host_offset, host_length; int port; };
struct swr_v1_clients_url_plan swr_v1_clients_url(const uint8_t *data, size_t length);
uint8_t swr_v1_clients_port(int32_t port, uint8_t for_connect);
struct swr_v1_clients_host_match { uint32_t found; size_t offset, length; };
/* Line already has its first '#' replaced by NUL, as in the pinned caller.
 * spaces points to256 native locale classification bytes; no pointer retained. */
struct swr_v1_clients_host_match swr_v1_clients_hosts_match(const uint8_t *, size_t, const uint8_t *, size_t, const uint8_t *spaces);
struct swr_v1_clients_cookie_plan { size_t key_length, value_offset, value_length; };
struct swr_v1_clients_cookie_plan swr_v1_clients_cookie(const uint8_t *, size_t);
/* WebSocket client handshake: 1 if base64(SHA-1(key + GUID)) equals accept.
 * Zend header extraction and the Upgrade/Connection token checks stay native. */
uint8_t swr_v1_clients_ws_accept_valid(const uint8_t *key, size_t key_len, const uint8_t *accept, size_t accept_len);
/* Caller-owned bounded DNS codec buffers; no native execution or retained state. */
int32_t swr_v1_clients_domain_encode(const uint8_t *, size_t, uint8_t *, size_t);
uint8_t swr_v1_clients_domain_decode(uint8_t *, size_t, uint8_t signed_char);
/* network::Client: its state, every operation, its callbacks, reactor handlers and connect
 * timer are Rust's (crates/clients/src/client.rs). The C++ Client holds the state and its
 * members of C++ types, and gives Rust one static table of what it needs of them,
 * swr_v1_clients_native. Most entries take the Client *; those over the record alone are
 * Rust's own, declared after the table. */
typedef struct swr_v1_clients swr_v1_clients;
struct swr_v1_socket;
enum {
    SWR_CLIENTS_ACTIVE = 0,
    SWR_CLIENTS_CLOSED = 1,
    SWR_CLIENTS_ASYNC = 2,
    SWR_CLIENTS_SHUTDOWN_READ = 3,
    SWR_CLIENTS_SHUTDOWN_WRITE = 4,
    SWR_CLIENTS_ASYNC_CONNECT = 5,
    SWR_CLIENTS_WAIT_DNS = 6,
    SWR_CLIENTS_ONERROR_CALLED = 7,
    SWR_CLIENTS_OPEN_SSL = 8,
};
/* connect: connect_async (SW_READY, SW_WAIT or SW_ERROR), connect_sync, connect(2) (errno kept). */
enum { SWR_CLIENTS_CONNECT_ASYNC, SWR_CLIENTS_CONNECT_SYNC, SWR_CLIENTS_CONNECT_RAW };
enum { SWR_CLIENTS_EVENT_ADD, SWR_CLIENTS_EVENT_SET, SWR_CLIENTS_EVENT_DEL, SWR_CLIENTS_EVENT_ADD_OR_UPDATE };
/* ssl: HAS_CONTEXT 1/0; PREPARE creates the context unless ready; SESSION is ssl_create and
 * the SNI name; VERIFY_PEER is verify_peer | allow_self_signed << 1; VERIFY is ssl_verify(arg);
 * DTLS makes the context's protocols DTLS, or fails without DTLS support. */
enum {
    SWR_CLIENTS_SSL_HAS_CONTEXT = 1, SWR_CLIENTS_SSL_PREPARE = 2, SWR_CLIENTS_SSL_SESSION = 3,
    SWR_CLIENTS_SSL_CONNECT = 4, SWR_CLIENTS_SSL_VERIFY_PEER = 5, SWR_CLIENTS_SSL_VERIFY = 6,
    SWR_CLIENTS_SSL_CLOSE = 7, SWR_CLIENTS_SSL_DTLS = 8,
};
enum { SWR_CLIENTS_PROXY_SOCKS5, SWR_CLIENTS_PROXY_HTTP };
struct swr_v1_clients_record;
typedef struct {
    struct swr_v1_socket *(*make_socket)(int kind, int fd_type, int flags);
    void (*kernel_timeout)(struct swr_v1_clients_record *record, double timeout);
    bool (*assign)(struct swr_v1_clients_record *record, const char *host, int port, bool resolve);
    int (*connect)(struct swr_v1_clients_record *record, int mode);
    int (*event)(struct swr_v1_clients_record *record, int op, int events);
    int (*socket_error)(struct swr_v1_clients_record *record, int *error);
    int (*dns_lookup)(void *cli, const char *host, int domain);
    int (*ssl)(void *cli, int op, int arg);
    /* -1 when the client has no such proxy. */
    int (*proxy_state)(void *cli, int proxy);
    void (*set_proxy_state)(void *cli, int proxy, int state);
    /* With the proxy set: host and port become its target; returns its host and *port. */
    const char *(*proxy_route)(void *cli, int proxy, const char *host, int port, int *proxy_port);
    ssize_t (*socks5_negotiate)(void *cli, const char **data);
    bool (*socks5_handshake)(void *cli, const char *data, size_t length);
    size_t (*http_pack)(void *cli, const char **data);
    /* Of the shared buffer (its length set to length) or the input buffer (grown by length). */
    bool (*http_handshake)(void *cli, bool shared, size_t length);
} swr_v1_clients_native;
struct swr_v1_socket *swr_v1_clients_make_socket(int kind, int fd_type, int flags);
void swr_v1_clients_kernel_timeout(struct swr_v1_clients_record *record, double timeout);
bool swr_v1_clients_assign(struct swr_v1_clients_record *record, const char *host, int port, bool resolve);
int swr_v1_clients_connect_socket(struct swr_v1_clients_record *record, int mode);
int swr_v1_clients_event(struct swr_v1_clients_record *record, int op, int events);
int swr_v1_clients_socket_error(struct swr_v1_clients_record *record, int *error);
/* Client::Client: the state, with cli->socket made (null when socket() failed, errno kept). */
swr_v1_clients *swr_v1_clients_new(void *cli, const swr_v1_clients_native *native, struct swr_v1_clients_record *record, int kind, bool is_async);
/* Client::~Client, then the state's release. */
void swr_v1_clients_free(swr_v1_clients *client);
int swr_v1_clients_get_peer_name(const swr_v1_clients *client, void *address);
bool swr_v1_clients_flag(const swr_v1_clients *client, uint32_t name);
void swr_v1_clients_set_flag(swr_v1_clients *client, uint32_t name, bool value);
/* buffer_high_watermark (high) or buffer_low_watermark, for the C++ Client's reference views. */
uint32_t *swr_v1_clients_watermark(swr_v1_clients *client, bool high);
int swr_v1_clients_connect(swr_v1_clients *client, const char *host, int port, double timeout, int flags);
ssize_t swr_v1_clients_send(swr_v1_clients *client, const char *data, size_t length, int flags);
int swr_v1_clients_sendfile(swr_v1_clients *client, const char *file, int64_t offset, size_t length);
ssize_t swr_v1_clients_recv(swr_v1_clients *client, char *data, size_t length, int flags);
int swr_v1_clients_close(swr_v1_clients *client);
int swr_v1_clients_shutdown(swr_v1_clients *client, int how);
int swr_v1_clients_sleep(swr_v1_clients *client);
int swr_v1_clients_wakeup(swr_v1_clients *client);
/* After the C++ half made the TLS context: open_ssl, and DTLS for a datagram client. */
int swr_v1_clients_enable_ssl(swr_v1_clients *client);
int swr_v1_clients_ssl_handshake(swr_v1_clients *client);
/* Client::bind and Client::sendto; the callback logs, named as the calling method. */
int swr_v1_clients_bind(swr_v1_clients *client, const char *host, size_t host_length, int port, void (*reuse_failed)(int fd));
int swr_v1_clients_sendto(swr_v1_clients *client, const char *host, size_t host_length, int port, const char *data, size_t length, void (*not_datagram)(int fd));
/* Client::init_reactor: the client sockets' read, write and error handlers. */
void swr_v1_clients_init_reactor(void *reactor);
/* The DNS completion. */
void swr_v1_clients_on_resolved(swr_v1_clients *client, int error, const char *address);
/* The callbacks: listener(object, event, data, length) for each event in `events`
 * (1 << SWOOLE_CLIENT_ON_*); `object` becomes the client's. For SWOOLE_CLIENT_ON_ERROR,
 * `length` is swoole_get_last_error(). */
enum {
    SWOOLE_CLIENT_ON_CONNECT = 1,
    SWOOLE_CLIENT_ON_RECEIVE = 2,
    SWOOLE_CLIENT_ON_CLOSE = 3,
    SWOOLE_CLIENT_ON_ERROR = 4,
    SWOOLE_CLIENT_ON_BUFFER_FULL = 5,
    SWOOLE_CLIENT_ON_BUFFER_EMPTY = 6,
};
typedef void (*swoole_client_listener)(void *object, int event, const char *data, size_t length);
void swr_v1_clients_listen(swr_v1_clients *client, void *object, uint32_t events, swoole_client_listener listener);
struct swr_v1_clients_resolv_line { uint32_t kind; size_t offset, length; };
struct swr_v1_clients_resolv_line swr_v1_clients_resolv_line(const uint8_t *, size_t);
struct swr_v1_clients_dns_endpoint { uint32_t has_port; size_t host_length, port_offset; };
struct swr_v1_clients_dns_endpoint swr_v1_clients_dns_endpoint(const uint8_t *, size_t);
/* Sole private pool; binary keys, FIFO opaque native tokens. Allocation returns
 * 0 success/1 OOM/2 invalid; caller throws only after return. Never destroys Client.
 * All calls require live owner/no concurrent mutation; key storage is native. */
void *swr_v1_clients_pool_create(void);
void swr_v1_clients_pool_destroy(void *);
uint32_t swr_v1_clients_pool_push(void *, const uint8_t *, size_t, void *);
struct swr_v1_clients_pool_state { uint32_t kind; size_t length; };
struct swr_v1_clients_pool_state swr_v1_clients_pool_state(const void *, const uint8_t *, size_t);
void *swr_v1_clients_pool_take(void *, const uint8_t *, size_t);
uint8_t swr_v1_clients_pool_remove_empty(void *, const uint8_t *, size_t);
struct swr_v1_clients_format_span { const uint8_t *data; size_t length; };
struct swr_v1_clients_format_result { size_t written; uint32_t error; };
/* Output is disjoint native-owned writable bytes, possibly uninitialized.
 * Required inputs initialized.0 success,1 invalid,2 printf-count overflow. */
struct swr_v1_clients_format_result swr_v1_clients_http_format(uint32_t kind, struct swr_v1_clients_format_span, struct swr_v1_clients_format_span, struct swr_v1_clients_format_span, struct swr_v1_clients_format_span, size_t number, uint8_t *output, size_t capacity);
/* network::Client for its C callers (crates/phpclients, the Rust core-tests cases), defined by
 * native/clients/network_client.cc. create: new Client(type, async), or null with errno in
 * `error` when it has no socket. */
void *swoole_client_create(int type, bool async, int *error);
void swoole_client_free(void *cli);
/* The client's Rust state (crates/clients' ClientState). */
void *swoole_client_state(void *cli);
/* The client's SSLContext owner, which crates/tls configures; NULL while TLS is off. */
const struct swr_v1_tls_owner *swoole_client_ssl_context(void *cli);
/* set_http_proxy (http) or set_socks5_proxy. */
void swoole_client_proxy(void *cli,
                         bool http,
                         const char *host,
                         size_t host_length,
                         int64_t port,
                         const char *user,
                         size_t user_length,
                         const char *password,
                         size_t password_length);
#ifdef __cplusplus
}
#endif
#endif

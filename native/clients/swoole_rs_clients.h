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
int swr_v1_clients_address_assign(void *address, uint32_t *length, int kind, const char *host, size_t host_length, int port, int (*resolve)(int family, const char *name, char *address));
bool swr_v1_clients_verify_ip(int family, const char *host, void *destination);
int swr_v1_clients_dns_query(const char *domain, int family, uint16_t request_id, void *context, bool (*send)(void *context, const uint8_t *data, size_t length), long (*recv)(void *context, uint8_t *data, size_t capacity), void (*push)(void *context, const uint8_t *text, size_t length), bool signed_char);
int swr_v1_clients_gethostbyname(int flags, const char *name, char *addr);
int swr_v1_clients_resolv_conf(const char *path, char *server);
bool swr_v1_clients_hosts_lookup(const char *path, const uint8_t *domain, size_t domain_length, const uint8_t *spaces, void *context, void (*push)(void *context, const uint8_t *text, size_t length));
typedef struct { int error; int count; } swr_v1_clients_resolved;
swr_v1_clients_resolved swr_v1_clients_getaddrinfo(const char *host, const char *service, int family, int socktype, int protocol, void *results, size_t capacity, void (*unknown)(int family));
struct swr_v1_clients_ip_plan { uint8_t bytes[16]; uint32_t length; };
struct swr_v1_clients_ip_plan swr_v1_clients_ip(uint8_t version, const uint8_t *data, size_t length, uint8_t allow_zeroes);
uint8_t swr_v1_clients_loopback(uint8_t version, const uint8_t *address);
struct swr_v1_clients_ip_text_plan { uint8_t bytes[40]; uint32_t length; };
struct swr_v1_clients_ip_text_plan swr_v1_clients_ip_text(uint8_t version, const uint8_t *address);
struct swr_v1_clients_url_plan {
    uint32_t kind, datagram;
    size_t host_offset, host_length, port_offset, port_length;
};
/* Kinds0 invalid,1 IPv4/hostname,2 IPv6,3 Unix. Digit classification stays native. */
struct swr_v1_clients_url_plan swr_v1_clients_url(const uint8_t *, size_t);
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
/* HTTP response decompression over a per-Client zlib stream (gzip/deflate; brotli
 * and zstd are disabled in every build). The handle owns the z_stream and the
 * raw-deflate retry; native appends each chunk's produced bytes to its body. */
struct swr_v1_clients_http_inflate_view { const uint8_t *data; size_t length; };
void *swr_v1_clients_http_inflate_new(bool gzip);
bool swr_v1_clients_http_inflate_chunk(void *handle, const uint8_t *input, size_t len, struct swr_v1_clients_http_inflate_view *view);
void swr_v1_clients_http_inflate_free(void *handle);
/* Caller-owned bounded DNS codec buffers; no native execution or retained state. */
int32_t swr_v1_clients_domain_encode(const uint8_t *, size_t, uint8_t *, size_t);
uint8_t swr_v1_clients_domain_decode(uint8_t *, size_t, uint8_t signed_char);
/* network::Client: its state and every operation are Rust's (crates/clients/src/client.rs).
 * The C++ Client holds the state and its members of C++ types, and gives Rust one static
 * table of what it needs of them, swr_v1_clients_native; each entry takes the Client *. */
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
enum {
    SWR_CLIENTS_ON_CONNECT, SWR_CLIENTS_ON_ERROR, SWR_CLIENTS_ON_RECEIVE,
    SWR_CLIENTS_ON_CLOSE, SWR_CLIENTS_ON_BUFFER_FULL, SWR_CLIENTS_ON_BUFFER_EMPTY,
};
enum {
    SWR_CLIENTS_WARN_CALLBACKS = 1, SWR_CLIENTS_WARN_ON_RECEIVE = 2, SWR_CLIENTS_WARN_ON_ERROR = 3,
    SWR_CLIENTS_WARN_BIND = 5, SWR_CLIENTS_WARN_READ = 6, SWR_CLIENTS_WARN_GETSOCKOPT = 7,
};
/* connect: connect_async (SW_READY, SW_WAIT or SW_ERROR), connect_sync, connect(2) (errno kept). */
enum { SWR_CLIENTS_CONNECT_ASYNC, SWR_CLIENTS_CONNECT_SYNC, SWR_CLIENTS_CONNECT_RAW };
/* io: SENDTO sends to server_addr, the RECVFROMs read into remote_addr. */
enum {
    SWR_CLIENTS_IO_RECV, SWR_CLIENTS_IO_RECV_SYNC, SWR_CLIENTS_IO_SEND_SYNC,
    SWR_CLIENTS_IO_SENDTO, SWR_CLIENTS_IO_RECVFROM, SWR_CLIENTS_IO_RECVFROM_SYNC,
};
enum { SWR_CLIENTS_EVENT_ADD, SWR_CLIENTS_EVENT_SET, SWR_CLIENTS_EVENT_DEL, SWR_CLIENTS_EVENT_ADD_OR_UPDATE };
/* ssl: HAS_CONTEXT 1/0; PREPARE creates the context unless ready; SESSION is ssl_create and
 * the SNI name; VERIFY_PEER is verify_peer | allow_self_signed << 1; VERIFY is ssl_verify(arg);
 * DTLS makes the context's protocols DTLS, or warns and fails without DTLS support. */
enum {
    SWR_CLIENTS_SSL_HAS_CONTEXT = 1, SWR_CLIENTS_SSL_PREPARE = 2, SWR_CLIENTS_SSL_SESSION = 3,
    SWR_CLIENTS_SSL_CONNECT = 4, SWR_CLIENTS_SSL_VERIFY_PEER = 5, SWR_CLIENTS_SSL_VERIFY = 6,
    SWR_CLIENTS_SSL_CLOSE = 7, SWR_CLIENTS_SSL_DTLS = 8,
};
enum { SWR_CLIENTS_PROXY_SOCKS5, SWR_CLIENTS_PROXY_HTTP };
/* buffer: the input buffer (INIT creates it), or SHARED, sw_tg_buffer(), for VIEW. */
enum {
    SWR_CLIENTS_BUFFER_INIT, SWR_CLIENTS_BUFFER_VIEW, SWR_CLIENTS_BUFFER_GROW,
    SWR_CLIENTS_BUFFER_CLEAR, SWR_CLIENTS_BUFFER_FREE, SWR_CLIENTS_BUFFER_SHARED,
};
typedef struct { char *str; size_t length, size; } swr_v1_clients_buffer_view;
typedef struct {
    struct swr_v1_socket *(*make_socket)(void *cli, int type, int fd_type, int flags);
    void (*free_socket)(void *cli, bool deferred);
    bool (*has)(void *cli, int which);
    /* false when the callback threw; the client may be gone afterwards. */
    bool (*call)(void *cli, int which, const char *data, size_t length);
    int (*last_error)(void);
    void (*set_last_error)(int code);
    void (*warn)(void *cli, int which);
    void (*kernel_timeout)(void *cli, double timeout);
    bool (*assign)(void *cli, const char *host, int port, bool resolve);
    int (*connect)(void *cli, int mode);
    ssize_t (*io)(void *cli, int op, char *data, size_t length, int flags);
    /* 0, or -1 with the last error set to errno. */
    int (*sendfile)(void *cli, const char *file, int64_t offset, size_t length, bool is_async);
    int (*event)(void *cli, int op, int events);
    int (*event_write)(void *cli, const char *data, size_t length);
    uint32_t (*out_buffer_length)(void *cli);
    int (*writable)(void *reactor, void *event);
    int (*socket_error)(void *cli, int *error);
    void *(*timer_add)(void *cli, double timeout);
    void (*timer_del)(void *timer);
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
    void (*buffer)(void *cli, int op, size_t length, swr_v1_clients_buffer_view *view);
    /* With open_eof_check or open_length_check: the protocol's receive into *received (0 once
     * a callback released the client or threw) and true. */
    bool (*recv_framed)(void *cli, ssize_t *received);
} swr_v1_clients_native;
/* Client::Client: the state, with cli->socket made (null when socket() failed, errno kept). */
swr_v1_clients *swr_v1_clients_new(void *cli, const swr_v1_clients_native *native, int type, bool is_async);
/* Client::~Client, then the state's release. */
void swr_v1_clients_free(swr_v1_clients *client);
bool swr_v1_clients_flag(const swr_v1_clients *client, uint32_t flag);
void swr_v1_clients_set_flag(swr_v1_clients *client, uint32_t flag, bool value);
void swr_v1_clients_set_watermark(swr_v1_clients *client, bool high, uint32_t value);
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
/* The reactor handlers, the connect timer and the DNS completion. */
int swr_v1_clients_on_read(swr_v1_clients *client);
int swr_v1_clients_on_dgram_read(swr_v1_clients *client);
int swr_v1_clients_on_write(swr_v1_clients *client, void *reactor, void *event);
int swr_v1_clients_on_error(swr_v1_clients *client, void *reactor, void *event);
void swr_v1_clients_on_timeout(swr_v1_clients *client);
void swr_v1_clients_on_resolved(swr_v1_clients *client, int error, const char *address);
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
enum swr_v1_clients_resolver_action {
    SWR_CLIENTS_RESOLVER_LOOKUP=1, SWR_CLIENTS_RESOLVER_COUNT=2,
    SWR_CLIENTS_RESOLVER_STORE_CLUSTER=3, SWR_CLIENTS_RESOLVER_STRING=4,
    SWR_CLIENTS_RESOLVER_RESET=5, SWR_CLIENTS_RESOLVER_RELEASE=6,
    SWR_CLIENTS_RESOLVER_POP=7, SWR_CLIENTS_RESOLVER_FIND_HOST=8,
    SWR_CLIENTS_RESOLVER_COPY_HOST=9, SWR_CLIENTS_RESOLVER_COLON_PORT=10,
    SWR_CLIENTS_RESOLVER_APPEND_PORT=11, SWR_CLIENTS_RESOLVER_DTOR=12,
    SWR_CLIENTS_RESOLVER_RESULT=13, SWR_CLIENTS_RESOLVER_EMPTY=14
};
struct swr_v1_clients_resolver_plan { uint32_t action, stage; };
struct swr_v1_clients_resolver_plan swr_v1_clients_resolver_next(uint32_t stage, uint32_t facts);
struct swr_v1_clients_format_span { const uint8_t *data; size_t length; };
struct swr_v1_clients_format_result { size_t written; uint32_t error; };
/* Output is disjoint native-owned writable bytes, possibly uninitialized.
 * Required inputs initialized.0 success,1 invalid,2 printf-count overflow. */
struct swr_v1_clients_format_result swr_v1_clients_http_format(uint32_t kind, struct swr_v1_clients_format_span, struct swr_v1_clients_format_span, struct swr_v1_clients_format_span, struct swr_v1_clients_format_span, size_t number, uint8_t *output, size_t capacity);
#ifdef __cplusplus
}
#endif
#endif

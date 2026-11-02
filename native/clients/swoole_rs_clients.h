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
enum swr_v1_clients_object_action {
    SWR_CLIENTS_OBJECT_RETURN=0, SWR_CLIENTS_OBJECT_FD_STREAM=1,
    SWR_CLIENTS_OBJECT_FD_DGRAM=2, SWR_CLIENTS_OBJECT_MAKE=3,
    SWR_CLIENTS_OBJECT_INITIALIZE=4, SWR_CLIENTS_OBJECT_WARN=5,
    SWR_CLIENTS_OBJECT_STREAM=6, SWR_CLIENTS_OBJECT_DGRAM=7,
    SWR_CLIENTS_OBJECT_ASYNC=8, SWR_CLIENTS_OBJECT_SYNC=9,
    SWR_CLIENTS_OBJECT_DOMAIN=10, SWR_CLIENTS_OBJECT_PROTOCOL=11,
    SWR_CLIENTS_OBJECT_ASSERT=12, SWR_CLIENTS_OBJECT_CLOSE=13,
    SWR_CLIENTS_OBJECT_BUFFER=14, SWR_CLIENTS_OBJECT_ASYNC_FREE=15,
    SWR_CLIENTS_OBJECT_ADVANCE=16, SWR_CLIENTS_OBJECT_SYNC_DELETE=17
};
struct swr_v1_clients_object_plan { uint32_t action, stage; };
struct swr_v1_clients_object_plan swr_v1_clients_object_next(uint32_t operation, uint32_t stage, uint8_t fact);
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
/* network::Client's connection state, Rust-owned behind an opaque handle (#439).
 * Flags for swr_v1_clients_flag/set_flag. */
typedef struct swr_v1_clients_state swr_v1_clients_state;
enum {
    SWR_CLIENTS_ACTIVE = 0,
    SWR_CLIENTS_CLOSED = 1,
    SWR_CLIENTS_ASYNC = 2,
    SWR_CLIENTS_SHUTDOWN_READ = 3,
    SWR_CLIENTS_SHUTDOWN_WRITE = 4,
    SWR_CLIENTS_ASYNC_CONNECT = 5,
    SWR_CLIENTS_WAIT_DNS = 6,
    SWR_CLIENTS_ONERROR_CALLED = 7,
};
swr_v1_clients_state *swr_v1_clients_state_new(bool is_async);
void swr_v1_clients_state_free(swr_v1_clients_state *state);
bool swr_v1_clients_flag(const swr_v1_clients_state *state, uint32_t flag);
void swr_v1_clients_set_flag(swr_v1_clients_state *state, uint32_t flag, bool value);
/* Client::shutdown(how): 0, -1, or -2 for native to set EINVAL. request is how as 0 SHUT_RD,
 * 1 SHUT_WR, 2 SHUT_RDWR, other invalid; live is socket != nullptr; syscall is shutdown(2),
 * called with fd and how unchanged. */
int32_t swr_v1_clients_shutdown(swr_v1_clients_state *state, bool live, int32_t fd, int32_t how, uint32_t request,
                                int32_t (*syscall)(int32_t, int32_t));
/* Client::close(). on_close returns false when it threw; the client may be gone after it,
 * so only orphaned (over the socket saved by socket()) and close_fd run afterwards. */
typedef struct {
    void *context;
    swr_v1_clients_state *state;
    void *(*socket)(void *);
    void (*release)(void *);
    void (*detach)(void *);
    bool (*on_close)(void *);
    bool (*orphaned)(void *);
    int32_t (*close_fd)(void *);
} swr_v1_clients_close_ops;
int32_t swr_v1_clients_close(const swr_v1_clients_close_ops *ops);
/* Client_tcp_connect_sync: every fact is read afresh by its callback. connect_nonblocking
 * returns SWR_CLIENTS_CONNECT_READY or SWR_CLIENTS_CONNECT_WAIT (native SW_READY/SW_WAIT), or
 * anything else on failure. Rust sets active and async_connect itself. */
enum { SWR_CLIENTS_CONNECT_READY = 1, SWR_CLIENTS_CONNECT_WAIT = 2 };
typedef struct {
    void *context;
    swr_v1_clients_state *state;
    double timeout;
    bool nonblock;
    void (*set_timeout)(void *);
    void (*kernel_timeout)(void *);
    int32_t (*address)(void *);
    int32_t (*connect_nonblocking)(void *);
    int32_t (*connect_blocking)(void *);
    void (*activate)(void *);
    bool (*has_socks)(void *);
    ssize_t (*socks_send)(void *);
    void (*socks_publish)(void *);
    ssize_t (*receive)(void *);
    bool (*socks_handshake)(void *, ssize_t);
    bool (*socks_ready)(void *);
    bool (*has_http)(void *);
    ssize_t (*http_send)(void *);
    bool (*http_handshake)(void *, ssize_t);
    bool (*open_ssl)(void *);
    int32_t (*ssl_handshake)(void *);
} swr_v1_clients_connect_ops;
int32_t swr_v1_clients_connect(const swr_v1_clients_connect_ops *ops);
/* Client_tcp_connect_async. connect sets errno when it fails; fail may destroy the client and
 * returns false when onError threw (native rethrows after Rust returns). */
typedef struct {
    void *context;
    swr_v1_clients_state *state;
    double timeout;
    bool (*callbacks_set)(void *);
    void (*warn_callbacks)(void *);
    void (*set_timeout)(void *);
    void (*buffer)(void *);
    void (*watermark)(void *);
    int32_t (*address)(void *);
    int32_t (*dns_lookup)(void *);
    int32_t (*connect)(void *);
    bool (*add_timer)(void *);
    int32_t (*add_event)(void *);
    bool (*fail)(void *);
} swr_v1_clients_async_connect_ops;
int32_t swr_v1_clients_connect_async(const swr_v1_clients_async_connect_ops *ops);
enum swr_v1_clients_persistent_action {
    SWR_CLIENTS_PERSISTENT_ADVANCE=0, SWR_CLIENTS_PERSISTENT_FIND=1,
    SWR_CLIENTS_PERSISTENT_TAKE=2, SWR_CLIENTS_PERSISTENT_REUSE=3,
    SWR_CLIENTS_PERSISTENT_DROP_STALE=4, SWR_CLIENTS_PERSISTENT_CREATE=5,
    SWR_CLIENTS_PERSISTENT_CREATE_FAILED=6, SWR_CLIENTS_PERSISTENT_SET_IDENTITY=7,
    SWR_CLIENTS_PERSISTENT_PUBLISH_SOCKET=8, SWR_CLIENTS_PERSISTENT_KEEP=9,
    SWR_CLIENTS_PERSISTENT_ENABLE_SSL=10, SWR_CLIENTS_PERSISTENT_RETURN=11,
    SWR_CLIENTS_PERSISTENT_DELETE_TIMER=12, SWR_CLIENTS_PERSISTENT_FREE_CALLABLE=13,
    SWR_CLIENTS_PERSISTENT_ERASE_POOL=14, SWR_CLIENTS_PERSISTENT_DELETE_CLIENT=15,
    SWR_CLIENTS_PERSISTENT_FREE_SOCKET_OBJECT=16, SWR_CLIENTS_PERSISTENT_DETACH=17
};
struct swr_v1_clients_persistent_plan { uint32_t action, stage; };
struct swr_v1_clients_persistent_plan swr_v1_clients_persistent_next(uint32_t stage, uint32_t facts);
struct swr_v1_clients_resolv_line { uint32_t kind; size_t offset, length; };
struct swr_v1_clients_resolv_line swr_v1_clients_resolv_line(const uint8_t *, size_t);
struct swr_v1_clients_dns_endpoint { uint32_t has_port; size_t host_length, port_offset; };
struct swr_v1_clients_dns_endpoint swr_v1_clients_dns_endpoint(const uint8_t *, size_t);
/* Client_udp_connect and Client_tcp_send_async over the client's state. Every fact is read
 * afresh by its callback; a callback that runs user code returns false when it threw (native
 * rethrows after Rust returns). */
typedef struct {
    void *context;
    swr_v1_clients_state *state;
    double timeout;
    int32_t udp_connect;
    void (*set_timeout)(void *);
    void (*kernel_timeout)(void *);
    void (*publish_flags)(void *);
    int32_t (*address)(void *);
    bool (*has_on_receive)(void *);
    void (*warn_receive)(void *);
    bool (*has_on_error)(void *);
    void (*warn_error)(void *);
    int32_t (*dns_lookup)(void *);
    void (*activate)(void *);
    void (*socket_timeout)(void *);
    bool (*is_unix)(void *);
    int32_t (*bind_unix)(void *);
    void (*warn_bind)(void *);
    bool (*open_ssl)(void *);
    bool (*enable_dtls)(void *);
    int32_t (*connect)(void *);
    void (*clean)(void *);
    void (*fail_close)(void *);
    bool (*on_error)(void *);
    bool (*has_on_connect)(void *);
    int32_t (*add_event)(void *);
    bool (*on_connect)(void *);
    int32_t (*ssl_handshake)(void *);
} swr_v1_clients_udp_connect_ops;
int32_t swr_v1_clients_udp_connect(const swr_v1_clients_udp_connect_ops *ops);
/* Client_onStreamRead and Client_onWrite. Every fact is read afresh by its callback; a user
 * callback ends the handler, so nothing touches the client after one. */
enum { SWR_CLIENTS_FAIL_HTTP = 0, SWR_CLIENTS_FAIL_SOCKS = 1, SWR_CLIENTS_FAIL_SSL = 2 };
enum { SWR_CLIENTS_READ_ERROR = 0, SWR_CLIENTS_READ_CLOSE = 1 };
typedef struct {
    void *context;
    swr_v1_clients_state *state;
    bool ssl_requested;
    bool (*http_pending)(void *);
    ssize_t (*receive)(void *);
    bool (*http_handshake)(void *, ssize_t);
    void (*http_ready)(void *);
    bool (*socks_pending)(void *);
    bool (*socks_handshake)(void *, ssize_t);
    bool (*socks_ready)(void *);
    void (*clear_buffer)(void *);
    bool (*ssl_pending)(void *);
    ssize_t (*ssl_handshake)(void *);
    bool (*ssl_ready)(void *);
    bool (*framed)(void *);
    ssize_t (*protocol_receive)(void *);
    bool (*sleep_requested)(void *);
    void (*sleep)(void *);
    int32_t (*catch_error)(void *);
    void (*warn)(void *);
    int32_t (*close)(void *);
    void (*on_receive)(void *, ssize_t);
    void (*on_connect)(void *);
    void (*fail)(void *, uint32_t);
} swr_v1_clients_read_ops;
int32_t swr_v1_clients_on_read(const swr_v1_clients_read_ops *ops);
typedef struct {
    void *context;
    swr_v1_clients_state *state;
    bool (*ssl_waiting)(void *);
    int32_t (*ssl_handshake)(void *);
    bool (*ssl_ready)(void *);
    bool (*ssl_wants_read)(void *);
    void (*read_interest)(void *);
    int32_t (*writable)(void *);
    bool (*drained)(void *);
    void (*on_drained)(void *);
    int32_t (*get_option)(void *);
    void (*warn_option)(void *);
    bool (*connected)(void *);
    bool (*socks_waiting)(void *);
    int32_t (*socks_send)(void *);
    bool (*http_waiting)(void *);
    int32_t (*http_send)(void *);
    bool (*open_ssl)(void *);
    void (*ssl_wait)(void *);
    void (*on_connect)(void *);
    void (*fail)(void *, bool);
} swr_v1_clients_write_ops;
int32_t swr_v1_clients_on_write(const swr_v1_clients_write_ops *ops);
typedef struct {
    void *context;
    int32_t (*write)(void *);
    bool (*overflowed)(void *);
    void (*mark_high)(void *);
    bool (*full)(void *);
    bool (*on_buffer_full)(void *);
} swr_v1_clients_send_ops;
ssize_t swr_v1_clients_send_async(const swr_v1_clients_send_ops *ops, ssize_t length);
intptr_t swr_v1_clients_datagram_send(intptr_t result, intptr_t length);
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
struct swr_v1_clients_async_meta { uint32_t slot; const uint8_t *name; size_t length; };
struct swr_v1_clients_async_meta swr_v1_clients_async_meta(uint32_t slot);
uint32_t swr_v1_clients_async_event(const uint8_t *folded, size_t length, size_t original_length);
struct swr_v1_clients_async_plan { uint32_t action, stage; };
struct swr_v1_clients_async_plan swr_v1_clients_async_next(uint32_t operation, uint32_t stage, uint32_t facts);
#ifdef __cplusplus
}
#endif
#endif

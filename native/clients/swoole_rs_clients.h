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
/* network::Client is Rust's (crates/clients/src/client.rs): its state, every operation,
 * its callbacks, reactor handlers, connect timer, TLS context, proxies and DNS lookup. The
 * state starts with its public record, swr_v1_clients_record. These are the calls
 * native/clients/client.cc makes; crates/phpclients and the core-tests port call Rust
 * directly. */
typedef struct swr_v1_clients swr_v1_clients;
/* delete client. */
void swr_v1_clients_free(swr_v1_clients *client);
int swr_v1_clients_connect(swr_v1_clients *client, const char *host, int port, double timeout, int flags);
ssize_t swr_v1_clients_send(swr_v1_clients *client, const char *data, size_t length, int flags);
int swr_v1_clients_sendfile(swr_v1_clients *client, const char *file, int64_t offset, size_t length);
ssize_t swr_v1_clients_recv(swr_v1_clients *client, char *data, size_t length, int flags);
int swr_v1_clients_close(swr_v1_clients *client);
int swr_v1_clients_shutdown(swr_v1_clients *client, int how);
int swr_v1_clients_sleep(swr_v1_clients *client);
int swr_v1_clients_wakeup(swr_v1_clients *client);
/* enable_ssl_encrypt: a TLS context, open_ssl, and DTLS for a datagram client; -1 when TLS
 * was already on. */
int swr_v1_clients_enable_ssl(swr_v1_clients *client);
int swr_v1_clients_ssl_handshake(swr_v1_clients *client);
int swr_v1_clients_ssl_verify(swr_v1_clients *client, int allow_self_signed);
/* Client::sendto; the callback logs. */
int swr_v1_clients_sendto(swr_v1_clients *client, const char *host, size_t host_length, int port, const char *data, size_t length, void (*not_datagram)(int fd));
/* Client::init_reactor: the client sockets' read, write and error handlers. */
void swr_v1_clients_init_reactor(void *reactor);
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
#ifdef __cplusplus
}
#endif
#endif

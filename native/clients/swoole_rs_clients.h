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
/* network::Client is Rust's (crates/clients/src/client.rs), and crates/phpclients
 * calls it directly. Client::init_reactor: the client sockets' read, write and error
 * handlers. */
void swr_v1_clients_init_reactor(void *reactor);
struct swr_v1_clients_format_span { const uint8_t *data; size_t length; };
struct swr_v1_clients_format_result { size_t written; uint32_t error; };
/* Output is disjoint native-owned writable bytes, possibly uninitialized.
 * Required inputs initialized.0 success,1 invalid,2 printf-count overflow. */
struct swr_v1_clients_format_result swr_v1_clients_http_format(uint32_t kind, struct swr_v1_clients_format_span, struct swr_v1_clients_format_span, struct swr_v1_clients_format_span, struct swr_v1_clients_format_span, size_t number, uint8_t *output, size_t capacity);
#ifdef __cplusplus
}
#endif
#endif

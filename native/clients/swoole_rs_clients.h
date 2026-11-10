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
/* Address::assign(url): outcome 0 with the type, host span and port to assign, or
 * INVALID (native logs the notice) or PORT_RANGE (std::stoi's out_of_range). */
enum { SWR_CLIENTS_URL_INVALID = -1, SWR_CLIENTS_URL_PORT_RANGE = -2 };
struct swr_v1_clients_url_plan { int outcome, kind; size_t host_offset, host_length; int port; };
struct swr_v1_clients_url_plan swr_v1_clients_url(const uint8_t *data, size_t length);
uint8_t swr_v1_clients_port(int32_t port, uint8_t for_connect);
struct swr_v1_clients_format_span { const uint8_t *data; size_t length; };
struct swr_v1_clients_format_result { size_t written; uint32_t error; };
/* Output is disjoint native-owned writable bytes, possibly uninitialized.
 * Required inputs initialized.0 success,1 invalid,2 printf-count overflow. */
struct swr_v1_clients_format_result swr_v1_clients_http_format(uint32_t kind, struct swr_v1_clients_format_span, struct swr_v1_clients_format_span, struct swr_v1_clients_format_span, struct swr_v1_clients_format_span, size_t number, uint8_t *output, size_t capacity);
#ifdef __cplusplus
}
#endif
#endif

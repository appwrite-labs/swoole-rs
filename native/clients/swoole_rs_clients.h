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
bool swr_v1_clients_address_verify(int family, const char *host);
const char *swr_v1_clients_address_host(const void *address);
int swr_v1_clients_address_port(const void *address);
void swr_v1_clients_address_set_port(void *address, int port);
bool swr_v1_clients_address_empty(const void *address);
const char *swr_v1_clients_address_type(int kind);
bool swr_v1_clients_address_loopback(const void *address);
int swr_v1_clients_gethostbyname(int flags, const char *name, char *addr);
uint8_t swr_v1_clients_port(int32_t port, uint8_t for_connect);
#ifdef __cplusplus
}
#endif
#endif

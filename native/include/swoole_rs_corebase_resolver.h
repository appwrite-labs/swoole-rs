/* SPDX-License-Identifier: Apache-2.0 */
#pragma once
#include "swoole_rs_corebase_string_slot.h"
#include <stddef.h>
#include <sys/socket.h>
#include <netinet/in.h>
struct swr_v1_core_resolver;
struct swr_v1_core_resolver_fields { int family, socktype, protocol, error, count; };
struct swr_v1_core_resolver_prepared {
    swr_v1_core_resolver *owner;
    swr_v1_core_resolver_fields *fields;
    const swr_v1_core_string *name, *service, *address;
};
struct swr_v1_core_resolver_addresses { sockaddr_in6 *data; size_t length, capacity; };
struct swr_v1_core_resolver_text { char bytes[46]; bool valid; };
struct swr_v1_core_resolver_completion { int error; bool failed; };
extern "C" {
swr_v1_core_resolver_prepared swr_v1_core_resolver_new(int family, int socktype, int protocol);
swr_v1_core_resolver_prepared swr_v1_core_resolver_create(int family, int socktype, int protocol, const char *name, size_t name_length, const char *service, size_t service_length);
void swr_v1_core_resolver_free(swr_v1_core_resolver *owner);
swr_v1_core_resolver_addresses swr_v1_core_resolver_span(const swr_v1_core_resolver *owner);
int swr_v1_core_resolver_resize(const swr_v1_core_resolver *owner, size_t length);
int swr_v1_core_resolver_assign(const swr_v1_core_resolver *owner, const sockaddr_in6 *data, size_t length);
int swr_v1_core_resolver_lookup(const swr_v1_core_resolver *owner, bool (*unknown)(void *, int), void *context);
int swr_v1_core_resolver_host(const swr_v1_core_resolver *owner);
swr_v1_core_resolver_text swr_v1_core_resolver_next(const swr_v1_core_resolver *owner, size_t *cursor);
int swr_v1_clients_gethostbyname(int flags, const char *name, char *addr);
swr_v1_core_resolver_completion swr_v1_core_resolver_finish(ssize_t retval, int event_error, const int *request_error, int timeout_error, int resolution_timeout);
}

static_assert(sizeof(swr_v1_core_resolver_fields) == 20 && sizeof(swr_v1_core_resolver_prepared) == 40, "DNS record ABI");
static_assert(sizeof(swr_v1_core_resolver_addresses) == 24 && sizeof(swr_v1_core_resolver_text) == 47 &&
              sizeof(swr_v1_core_resolver_completion) == 8, "DNS result ABI");

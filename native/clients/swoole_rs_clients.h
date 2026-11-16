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
/* Rust's Text (crates/corebase/src/sys.rs): a NUL-terminated string, live and unchanged
 * for the call, never null. */
typedef const char *SwrText;
bool swr_v1_clients_address_verify(int family, SwrText host);
#ifdef __cplusplus
}
#endif
#endif

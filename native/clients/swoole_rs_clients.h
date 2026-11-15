/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_CLIENTS_H
#define SWOOLE_RS_CLIENTS_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>
#include "swoole_rs_utilities.h"
#ifdef __cplusplus
extern "C" {
#endif
bool swr_v1_clients_address_verify(int family, SwrText host);
#ifdef __cplusplus
}
#endif
#endif

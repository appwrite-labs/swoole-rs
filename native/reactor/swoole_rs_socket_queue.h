/* SPDX-License-Identifier: Apache-2.0 */
#pragma once
#include "swoole_rs_reactor.h"
#include "swoole_rs_buffer_typed.h"
#ifdef __cplusplus
extern "C" {
#endif
struct swr_v1_socket;
typedef struct {
    swr_v1_socket_output_result result;
    int32_t status;
    swr_v1_buffer_typed_detached detached;
} swr_v1_socket_owned_output_result;
swr_v1_socket_owned_output_result swr_v1_socket_handle_send_owned(struct swr_v1_socket *, swr_v1_buffer *, const swr_v1_socket_output_hooks *);
#ifdef __cplusplus
}
#endif

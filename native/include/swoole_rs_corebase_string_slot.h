/* SPDX-License-Identifier: Apache-2.0 */
#ifndef SWOOLE_RS_COREBASE_STRING_SLOT_H
#define SWOOLE_RS_COREBASE_STRING_SLOT_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct swr_v1_core_string swr_v1_core_string;
typedef struct {
  const char *data;
  size_t length;
} swr_v1_core_string_view_t;
swr_v1_core_string_view_t swr_v1_core_string_view(const swr_v1_core_string *);
int32_t swr_v1_core_string_set(const swr_v1_core_string *, const char *,
                               size_t);
const swr_v1_core_string *swr_v1_core_string_clone(const swr_v1_core_string *);
void swr_v1_core_string_free(const swr_v1_core_string *);
#ifdef __cplusplus
}
#endif
#endif

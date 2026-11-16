/* SPDX-License-Identifier: Apache-2.0 */
#pragma once
#include "swoole_rs_corebase_constants.h"
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>
#define SWR_CORE_ALLOCATOR_FIELDS                                              \
  void *(*malloc)(size_t);                                                     \
  void *(*calloc)(size_t, size_t);                                             \
  void *(*realloc)(void *, size_t);                                            \
  void (*free)(void *)
#ifdef __cplusplus
namespace swoole {
struct Allocator {
  SWR_CORE_ALLOCATOR_FIELDS;
};
class String;
class Reactor;
class Timer;
class MessageBus;
class AsyncThreads;
} // namespace swoole
typedef swoole::Allocator swr_v1_core_allocator;
extern "C" {
#else
typedef struct {
  SWR_CORE_ALLOCATOR_FIELDS;
} swr_v1_core_allocator;
#endif
#undef SWR_CORE_ALLOCATOR_FIELDS
size_t swr_v1_core_format_result(char *, size_t, int formatted, bool zero_query);
/* The thread's last error is Rust's (crates/corebase/src/log.rs). */
void swr_v1_core_set_last_error(int);
#ifdef __cplusplus
}
static_assert(sizeof(swr_v1_core_allocator) == 4 * sizeof(void *),
              "allocator callback ABI");
#endif

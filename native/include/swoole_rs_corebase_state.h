/* SPDX-License-Identifier: Apache-2.0 */
#pragma once
#include "swoole_rs_corebase_constants.h"
#include "swoole_rs_corebase_string_slot.h"
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
// Standalone corebase consumers do not apply the integration logging overlay.
// Keep the same origin scope for the two native diagnostic leaves.
#ifndef swoole_log_as
static constexpr const char *sw_log_origin = nullptr;
#undef __SW_FUNC__
#define __SW_FUNC__ (sw_log_origin ? sw_log_origin : swoole::Logger::get_pretty_name(__PRETTY_FUNCTION__).c_str())
#define swoole_log_as(origin, ...) do { const char *const sw_log_origin = origin; (void) sw_log_origin; __VA_ARGS__; } while (0)
#endif
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
/* swoole_set_task_tmpdir under its lock, with upstream's warnings. */
bool swr_v1_core_set_task_tmpdir(const char *dir);
/* The thread's last error is Rust's (crates/corebase/src/log.rs). */
int swr_v1_core_last_error(void);
void swr_v1_core_set_last_error(int);
#ifdef __cplusplus
}
namespace swoole {
// SwooleTG's layout: Rust holds each thread's (crates/corebase/src/lifecycle.rs,
// which asserts the same layout).
struct ThreadGlobal {
  uint16_t id;
  uint8_t type;
  bool main_thread;
  String *buffer_stack;
  Reactor *reactor;
  Timer *timer;
  MessageBus *message_bus;
  AsyncThreads *async_threads;
};
static_assert(offsetof(ThreadGlobal, main_thread) == 3 &&
                  offsetof(ThreadGlobal, buffer_stack) == 8 &&
                  offsetof(ThreadGlobal, async_threads) == 40 &&
                  sizeof(ThreadGlobal) == 48,
              "crates/corebase/src/lifecycle.rs's ThreadGlobal");
static_assert(sizeof(swr_v1_core_allocator) == 4 * sizeof(void *),
              "allocator callback ABI");
} // namespace swoole
extern "C" swoole::ThreadGlobal *swr_v1_core_thread_global(void);
#endif

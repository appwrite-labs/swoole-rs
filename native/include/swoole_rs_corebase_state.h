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
class Iouring;
} // namespace swoole
typedef swoole::Allocator swr_v1_core_allocator;
extern "C" {
#else
typedef struct {
  SWR_CORE_ALLOCATOR_FIELDS;
} swr_v1_core_allocator;
#endif
#undef SWR_CORE_ALLOCATOR_FIELDS
typedef struct swr_v1_core_config swr_v1_core_config;
typedef struct {
  uint32_t max_sockets;
  double dns_cache_refresh_time;
  swr_v1_core_allocator std_allocator;
} swr_v1_core_config_fields;
typedef struct {
  swr_v1_core_config *owner;
  swr_v1_core_config_fields *fields;
  const swr_v1_core_string *strings[2];
} swr_v1_core_config_prepared;
typedef struct {
  int32_t status;
  uint32_t index;
} swr_v1_core_config_copy_result;
size_t swr_v1_core_format_result(char *, size_t, int formatted, bool zero_query);
int32_t swr_v1_corebase_mkdir_complete(const uint8_t *, size_t, uint8_t *, size_t);
typedef struct { int32_t status, mkdir_status; } swr_v1_core_task_path_result;
swr_v1_core_task_path_result swr_v1_core_config_set_task_path(swr_v1_core_config *, const uint8_t *, size_t, uint8_t *, size_t);
swr_v1_core_config_prepared swr_v1_core_config_new(void);
void swr_v1_core_config_free(swr_v1_core_config *);
void swr_v1_core_config_allocator(swr_v1_core_config *);
bool swr_v1_core_config_max_sockets(swr_v1_core_config *, uint32_t limit);
swr_v1_core_config_copy_result
swr_v1_core_config_copy_values(swr_v1_core_config *,
                               const swr_v1_core_config *);
swr_v1_core_string_view_t
swr_v1_core_config_task_path(const swr_v1_core_config *);
/* The thread's last error is Rust's (crates/corebase/src/log.rs). */
int swr_v1_core_last_error(void);
void swr_v1_core_set_last_error(int);
/* Native's log macros, registered once: kind 0 swoole_sys_warning (errno = code),
 * 1 swoole_warning, 2 swoole_notice, 3 swoole_error_log(level, code); origin is __SW_FUNC__. */
typedef void (*swr_v1_core_log_host)(int kind, int level, int code, const char *origin, const char *message);
void swr_v1_core_set_log_host(swr_v1_core_log_host);
/* base.cc's C++ functions Rust calls (crates/corebase/src/native.rs), registered
 * once with the log macros, so the Rust archive links without base.cc. */
typedef struct {
    pid_t (*fork)(int flags);
    bool (*get_env)(const char *name, int *value);
} swr_v1_core_native_host;
void swr_v1_core_set_native_host(swr_v1_core_native_host);
#ifdef __cplusplus
}
namespace swoole {
class CoreConfigOwner {
protected:
  swr_v1_core_config_prepared config_ = swr_v1_core_config_new();
  CoreConfigOwner() = default;
  CoreConfigOwner(const CoreConfigOwner &) = delete;
  CoreConfigOwner &operator=(const CoreConfigOwner &) = delete;
  ~CoreConfigOwner() { swr_v1_core_config_free(config_.owner); }
};
// Trivial, so `thread_local SwooleTG = {}` is constant-initialized and other
// translation units reach it without a TLS init wrapper.
struct ThreadGlobal {
  uint16_t id;
  uint8_t type;
  bool main_thread;
  String *buffer_stack;
  Reactor *reactor;
  Timer *timer;
  MessageBus *message_bus;
  AsyncThreads *async_threads;
#ifdef SW_USE_IOURING
  Iouring *iouring;
#endif
  bool signal_blocking_all;
};
static_assert(sizeof(swr_v1_core_allocator) == 4 * sizeof(void *),
              "allocator callback ABI");
} // namespace swoole
#endif

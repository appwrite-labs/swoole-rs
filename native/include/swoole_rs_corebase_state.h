/* SPDX-License-Identifier: Apache-2.0 */
#pragma once
#include "swoole_rs_corebase_constants.h"
#include "swoole_rs_corebase_string_slot.h"
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
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
typedef swoole::String swr_v1_core_string_object;
typedef swoole::Reactor swr_v1_core_reactor_object;
typedef swoole::Timer swr_v1_core_timer_object;
typedef swoole::MessageBus swr_v1_core_bus_object;
typedef swoole::AsyncThreads swr_v1_core_async_object;
typedef swoole::Iouring swr_v1_core_uring_object;
extern "C" {
#else
typedef struct {
  SWR_CORE_ALLOCATOR_FIELDS;
} swr_v1_core_allocator;
typedef void swr_v1_core_string_object;
typedef void swr_v1_core_reactor_object;
typedef void swr_v1_core_timer_object;
typedef void swr_v1_core_bus_object;
typedef void swr_v1_core_async_object;
typedef void swr_v1_core_uring_object;
#endif
#undef SWR_CORE_ALLOCATOR_FIELDS
typedef struct swr_v1_core_config swr_v1_core_config;
typedef struct {
  uint32_t max_sockets;
  double dns_cache_refresh_time;
  swr_v1_core_allocator std_allocator;
  void *hooks[SW_MAX_HOOK_TYPE];
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
void swr_v1_core_config_copy_hooks(swr_v1_core_config *,
                                   const swr_v1_core_config *);
swr_v1_core_string_view_t
swr_v1_core_config_task_path(const swr_v1_core_config *);
typedef struct {
  uint16_t id;
  uint8_t kind;
  bool main_thread;
  swr_v1_core_string_object *buffer_stack;
  swr_v1_core_reactor_object *reactor;
  swr_v1_core_timer_object *timer;
  swr_v1_core_bus_object *message_bus;
  swr_v1_core_async_object *async_threads;
  swr_v1_core_uring_object *iouring;
  bool signal_blocking_all;
} swr_v1_core_thread_fields;
swr_v1_core_thread_fields *swr_v1_core_thread_new(void);
void swr_v1_core_thread_free(swr_v1_core_thread_fields *);
/* The thread's last error is Rust's (crates/corebase/src/log.rs). */
int swr_v1_core_last_error(void);
void swr_v1_core_set_last_error(int);
/* Native's log macros, registered once: kind 0 swoole_sys_warning (errno = code),
 * 1 swoole_warning, 2 swoole_notice, 3 swoole_error_log(level, code); origin is __SW_FUNC__. */
typedef void (*swr_v1_core_log_host)(int kind, int level, int code, const char *origin, const char *message);
void swr_v1_core_set_log_host(swr_v1_core_log_host);
void swr_v1_core_thread_copy(swr_v1_core_thread_fields *,
                             const swr_v1_core_thread_fields *);
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
class CoreThreadOwner {
protected:
  swr_v1_core_thread_fields *thread_ = swr_v1_core_thread_new();
  CoreThreadOwner() = default;
  CoreThreadOwner(const CoreThreadOwner &) = delete;
  CoreThreadOwner &operator=(const CoreThreadOwner &) = delete;
  ~CoreThreadOwner() { swr_v1_core_thread_free(thread_); }
};
struct ThreadGlobal : private CoreThreadOwner {
  ThreadGlobal() = default;
  ThreadGlobal(const ThreadGlobal &source) : ThreadGlobal() {
    swr_v1_core_thread_copy(thread_, source.thread_);
  }
  ThreadGlobal &operator=(const ThreadGlobal &source) {
    swr_v1_core_thread_copy(thread_, source.thread_);
    return *this;
  }
  uint16_t &id = thread_->id;
  uint8_t &type = thread_->kind;
  bool &main_thread = thread_->main_thread;
  String *&buffer_stack = thread_->buffer_stack;
  Reactor *&reactor = thread_->reactor;
  Timer *&timer = thread_->timer;
  MessageBus *&message_bus = thread_->message_bus;
  AsyncThreads *&async_threads = thread_->async_threads;
#ifdef SW_USE_IOURING
  Iouring *&iouring = thread_->iouring;
#endif
  bool &signal_blocking_all = thread_->signal_blocking_all;
};
static_assert(sizeof(swr_v1_core_allocator) == 4 * sizeof(void *),
              "allocator callback ABI");
static_assert(offsetof(swr_v1_core_thread_fields, buffer_stack) == 8,
              "thread scalar ABI");
} // namespace swoole
#endif

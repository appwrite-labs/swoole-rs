#ifndef SWOOLE_RS_COREBASE_THREAD_H
#define SWOOLE_RS_COREBASE_THREAD_H

/* Generated from crates/corebase by cbindgen (tools/headers.sh); do not edit. */

#include <stdbool.h>
#include <stdint.h>
#include <stddef.h>
#include <pthread.h>

typedef struct CoreThread {
  pthread_t handle;
  int exit_status;
  uint8_t living;
} CoreThread;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

bool swr_v1_core_thread_joinable(const struct CoreThread *record);

bool swr_v1_core_thread_alive(const struct CoreThread *record);

int swr_v1_core_thread_status(const struct CoreThread *record);

void swr_v1_core_thread_enter(const struct CoreThread *record);

void swr_v1_core_thread_exit(const struct CoreThread *record, int code);

bool swr_v1_core_thread_move(struct CoreThread *destination, struct CoreThread *source);

pthread_t swr_v1_core_thread_id(const struct CoreThread *record);

int swr_v1_core_thread_start(struct CoreThread *record, void *(*entry)(void*), void *argument);

int swr_v1_core_thread_join(struct CoreThread *record);

int swr_v1_core_thread_detach(struct CoreThread *record);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus



#endif  /* SWOOLE_RS_COREBASE_THREAD_H */

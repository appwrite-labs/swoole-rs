#ifndef SWOOLE_RS_COREBASE_WORKER_REGISTRY_H
#define SWOOLE_RS_COREBASE_WORKER_REGISTRY_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
void *swr_v1_process_worker_registry_find_array(void *array, uint32_t count, size_t stride, size_t pid_offset, int pid);
#define SWR_WORKER_EVENT 1
#define SWR_WORKER_USER 2
typedef struct swr_v1_process_worker_registry swr_v1_process_worker_registry;
swr_v1_process_worker_registry *swr_v1_process_worker_registry_new(void);
void swr_v1_process_worker_registry_free(swr_v1_process_worker_registry *);
void swr_v1_process_worker_registry_insert(swr_v1_process_worker_registry *, int pid, void *worker, int category);
void swr_v1_process_worker_registry_register(swr_v1_process_worker_registry *, int pid, void *worker, int category);
bool swr_v1_process_worker_registry_unregister(swr_v1_process_worker_registry *, int pid);
void *swr_v1_process_worker_registry_lookup_category(const swr_v1_process_worker_registry *, int, int);
void *swr_v1_process_worker_registry_lookup(const swr_v1_process_worker_registry *, int pid);
void swr_v1_process_worker_registry_detach(swr_v1_process_worker_registry *, int pid);
bool swr_v1_process_worker_registry_reap_detached(swr_v1_process_worker_registry *, int pid);
typedef struct swr_v1_worker_snapshot { void **data; size_t length; size_t capacity; } swr_v1_worker_snapshot;
bool swr_v1_process_worker_registry_snapshot(const swr_v1_process_worker_registry *, int category, swr_v1_worker_snapshot *);
void swr_v1_process_worker_registry_snapshot_free(swr_v1_worker_snapshot *);
/* Send signo to every worker of category, then wait for each. kill_fn/wait_fn are
 * swoole_kill / swoole_waitpid; warn_fn reports a wait failure. */
void swr_v1_process_worker_registry_terminate(const swr_v1_process_worker_registry *, int category, int signo,
                                              int (*kill_fn)(int pid, int signo),
                                              int (*wait_fn)(int pid, int *status, int options),
                                              void (*warn_fn)(int pid));
#ifdef __cplusplus
}
#endif
#endif

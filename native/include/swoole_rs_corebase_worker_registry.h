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
size_t swr_v1_process_worker_registry_pending_add(swr_v1_process_worker_registry *, void *);
size_t swr_v1_process_worker_registry_pending_count(const swr_v1_process_worker_registry *);
void *const *swr_v1_process_worker_registry_pending_data(const swr_v1_process_worker_registry *);
typedef struct swr_v1_worker_snapshot { void **data; size_t length; size_t capacity; } swr_v1_worker_snapshot;
bool swr_v1_process_worker_registry_snapshot(const swr_v1_process_worker_registry *, int category, swr_v1_worker_snapshot *);
void swr_v1_process_worker_registry_snapshot_free(swr_v1_worker_snapshot *);
#ifdef __cplusplus
}
#endif
#endif

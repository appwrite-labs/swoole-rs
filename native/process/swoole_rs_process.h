#ifndef SWOOLE_RS_PROCESS_H
#define SWOOLE_RS_PROCESS_H
#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>
#include "swoole_rs_corebase_worker_registry.h"
#ifdef __cplusplus
extern "C" {
#endif
typedef struct swr_v1_process_reload_task swr_v1_process_reload_task;
/* ProcessPool's IPC (crates/process/src/pool.rs). */
typedef struct swr_v1_process_pool_ipc swr_v1_process_pool_ipc;
#ifdef __cplusplus
}
#endif
#endif

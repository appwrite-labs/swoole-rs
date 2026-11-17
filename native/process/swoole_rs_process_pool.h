/* SPDX-License-Identifier: Apache-2.0 */
/* Workers and process pools: the C++ layouts of the Rust records (corebase::process,
 * swoole_rs_process_views.h), whose operations are Rust. Only the hooks/system fixture
 * includes it, to size them for its Rust mirrors.
 * Was include/swoole_process_pool.h, then the process half of swoole_server.h. */
#pragma once

#include "swoole_socket.h"
#include "swoole_protocol.h"
typedef struct swr_v1_process_worker_registry swr_v1_process_worker_registry;
typedef struct swr_v1_process_pool_ipc swr_v1_process_pool_ipc;
typedef struct swr_v1_process_reload_task swr_v1_process_reload_task;
typedef struct swr_v1_os_msg_queue swr_v1_os_msg_queue;
#include "swoole_rs_process_views.h"
// Worker and ProcessPool's fields are the Rust records' (corebase::process), which
// tools/process_records.py writes into swoole_rs_process_views.h; tools/headers.sh
// --check fails when that header is stale. #629.

namespace swoole {
class UnixSocket;
// The record's protocol_type, a c_int (corebase::process's Protocol codes).
typedef int ProtocolType;
struct ProcessPool;

struct Worker {
    SWR_WORKER_RECORD_FIELDS
};

struct ProcessPool {
    SWR_POOL_RECORD_FIELDS
};
}  // namespace swoole

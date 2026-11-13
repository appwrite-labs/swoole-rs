/* SPDX-License-Identifier: Apache-2.0 */
/* Workers and process pools: the C++ layouts of the Rust records (crates/process,
 * swoole_rs_process_views.h), whose operations are Rust. Only the hooks/system fixture
 * includes it, to size them for its Rust mirrors.
 * Was include/swoole_process_pool.h, then the process half of swoole_server.h. */
#pragma once

#include "swoole_pipe.h"
#include "swoole_protocol.h"
// Worker and ProcessPool's fields are the Rust records' (crates/process), which
// tools/process_records.py writes into swoole_rs_process_views.h; tools/headers.sh
// --check fails when that header is stale. #629.

namespace swoole {
// The record's protocol_type, a c_int (crates/process's Protocol codes).
typedef int ProtocolType;
class ExitStatus;
struct ProcessPool;

struct Worker {
    SWR_WORKER_RECORD_FIELDS
};

struct ProcessPool {
    SWR_POOL_RECORD_FIELDS
};
}  // namespace swoole

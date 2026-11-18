// What the Rust cases in src/ can't express, compiled into system.py's lib-swoole by
// tests/modules/hooks/system.py (--fixture): the C++ layouts of the process records,
// which process_pool.rs's mirrors are checked against. The witnesses of the resolver
// record's C++ views (formerly tests/modules/hooks/dns_request_owner.cpp) went with those
// views; what they checked of the record is tests/modules/corebase/resolver.rs.
#include "swoole_rs_process_pool.h"

#include <cstddef>

using namespace swoole;

extern "C" {
// os/process_pool.cpp (process_pool.rs): the record sizes its mirrors check.
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Winvalid-offsetof"
extern const size_t swr_case_process_layout[] = {sizeof(ProcessPool),
                                                 offsetof(ProcessPool, main_loop),
                                                 offsetof(ProcessPool, ptr),
                                                 sizeof(Worker),
                                                 offsetof(Worker, start_time),
                                                 SW_IPC_MAX_SIZE};
#pragma GCC diagnostic pop
}

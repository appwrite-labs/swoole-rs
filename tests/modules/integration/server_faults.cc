// SPDX-License-Identifier: Apache-2.0
#include "swoole_rs_corebase_native_exception.h"
#include <algorithm>
#include <cassert>
#include <cstdio>
#include <cstring>
#include <stdexcept>
#include <vector>

#ifdef WRAP_FREE
static void *watched;
static bool released;
extern "C" void __real_free(void *);
extern "C" void __wrap_free(void *pointer) {
    if (watched && pointer == watched) released = true;
    __real_free(pointer);
}
#endif
struct Marker : std::runtime_error {
    explicit Marker(int value) : std::runtime_error("native marker"), value(value) {}
    int value;
};
struct Context {
    swoole_rs::NativeException exception;
    std::vector<uint32_t> trace;
    uint32_t fail = UINT32_MAX;
    int kind = 0;
    bool nested = false;
};
static ServerReply effect(void *, uint32_t, uint64_t);
static int64_t invoke(Context &context, uint32_t operation) {
    return context.exception.finish(swr_v1_server_run_guarded(operation, 0, &context, effect, nullptr));
}
static ServerReply effect(void *opaque, uint32_t op, uint64_t arg) {
    auto &context = *static_cast<Context *>(opaque);
    return context.exception.capture([&]() -> uint64_t {
        context.trace.push_back(op);
        if (op == context.fail) {
#ifdef WRAP_FREE
            if (op == SWR_SERVER_OP_TASK_POOL_LISTEN) {
                watched = reinterpret_cast<void *>(arg);
                released = false;
                assert(std::strncmp(static_cast<const char *>(watched), "/tmp/swoole.task.", 17) == 0);
            }
#endif
            if (context.nested) {
                Context child;
                child.fail = SWR_SERVER_OP_ON_SHUTDOWN;
                child.kind = context.kind;
                invoke(child, SWR_SERVER_RUN_DESTROY);
                assert(false);
            }
            if (context.kind) throw 739;
            throw Marker(739);
        }
        if (op == SWR_SERVER_OP_MODE) return 2;
        if (op == SWR_SERVER_OP_FIELD) {
            if (arg == SWR_SERVER_FIELD_TASK_IPC_MODE) return 4;
            return 1;
        }
        switch (op) {
        case SWR_SERVER_OP_INIT_LOCAL_ADDRS:
        case SWR_SERVER_OP_ALLOC_SESSIONS:
        case SWR_SERVER_OP_ALLOC_PORT_GS:
        case SWR_SERVER_OP_ALLOC_WORKERS:
        case SWR_SERVER_OP_CREATE_FACTORY:
        case SWR_SERVER_OP_ALLOC_TASK_RESULTS:
        case SWR_SERVER_OP_INIT_TASK_WORKERS:
        case SWR_SERVER_OP_TASK_PIPE:
        case SWR_SERVER_OP_IS_STARTED: return 1;
        case SWR_SERVER_OP_MAX_CONNECTION:
        case SWR_SERVER_OP_MAX_SOCKETS: return 1024;
        case SWR_SERVER_OP_PORT_COUNT: return 2;
        case SWR_SERVER_OP_TASK_POOL_CREATE: return 1;
        case SWR_SERVER_OP_MASTER_PID: return 177;
        default: return 0;
        }
    });
}
int main() {
    static_assert(sizeof(ServerReply) == 16 && offsetof(ServerReply, fault) == 8, "native reply layout");
    size_t trials = 0;
    for (uint32_t operation : {SWR_SERVER_RUN_CREATE, SWR_SERVER_RUN_DESTROY}) {
        Context baseline;
        assert(invoke(baseline, operation) == 0);
        assert(!baseline.trace.empty());
        for (size_t index = 0; index < baseline.trace.size(); index++) {
            // Use first occurrence: repeated effects fail at their first call.
            auto op = baseline.trace[index];
            size_t first = 0;
            while (baseline.trace[first] != op) first++;
            if (first != index) continue;
            for (int kind : {0, 1}) for (bool nested : {false, true}) {
                Context context;
                context.fail = op;
                context.kind = kind;
                context.nested = nested;
                bool caught = false;
                try { invoke(context, operation); }
                catch (const Marker &marker) { assert(!kind && marker.value == 739); caught = true; }
                catch (int marker) { assert(kind && marker == 739); caught = true; }
                assert(caught && context.trace.size() == index + 1);
                assert(std::equal(context.trace.begin(), context.trace.end(), baseline.trace.begin()));
#ifdef WRAP_FREE
                if (op == SWR_SERVER_OP_TASK_POOL_LISTEN) {
                    assert(watched && released); // actual production Rust format String dropped before rethrow
                    watched = nullptr;
                }
#endif
                trials++;
            }
        }
    }
    std::printf("native Server failure propagation: %zu trials PASS\n", trials);
}

#include "swoole_rs.h"
#include <algorithm>
#include <cassert>
#include <cstdio>
#include <cstring>
#include <vector>
#ifdef WRAP_FREE
static void *watched;
static bool released;
extern "C" void __real_free(void *);
extern "C" void __wrap_free(void *pointer) {
    if (watched && watched == pointer) released = true;
    __real_free(pointer);
}
#endif
struct Context { std::vector<uint32_t> trace; uint32_t fail = UINT32_MAX, kind = 0; };
static ServerReply effect(void *opaque, uint32_t op, uint64_t arg) {
    auto &context = *static_cast<Context *>(opaque);
    context.trace.push_back(op);
    if (op == context.fail) {
#ifdef WRAP_FREE
        if (op == SWR_SERVER_OP_TASK_POOL_LISTEN) {
            watched = reinterpret_cast<void *>(arg);
            released = false;
            assert(std::strncmp(static_cast<const char *>(watched), "/tmp/swoole.task.", 17) == 0);
        }
#endif
        return {0, context.kind};
    }
    if (op == SWR_SERVER_OP_MODE) return {2, 0};
    if (op == SWR_SERVER_OP_FIELD) return {arg == SWR_SERVER_FIELD_TASK_IPC_MODE ? 4u : 1u, 0};
    switch (op) {
    case SWR_SERVER_OP_INIT_LOCAL_ADDRS:
    case SWR_SERVER_OP_ALLOC_SESSIONS:
    case SWR_SERVER_OP_ALLOC_PORT_GS:
    case SWR_SERVER_OP_ALLOC_WORKERS:
    case SWR_SERVER_OP_CREATE_FACTORY:
    case SWR_SERVER_OP_ALLOC_TASK_RESULTS:
    case SWR_SERVER_OP_INIT_TASK_WORKERS:
    case SWR_SERVER_OP_TASK_PIPE:
    case SWR_SERVER_OP_IS_STARTED:
    case SWR_SERVER_OP_TASK_POOL_CREATE: return {1, 0};
    case SWR_SERVER_OP_MAX_CONNECTION:
    case SWR_SERVER_OP_MAX_SOCKETS: return {1024, 0};
    case SWR_SERVER_OP_PORT_COUNT: return {2, 0};
    case SWR_SERVER_OP_MASTER_PID: return {177, 0};
    default: return {0, 0};
    }
}
int main() {
    size_t trials = 0, drops = 0;
    for (uint32_t operation : {SWR_SERVER_RUN_CREATE, SWR_SERVER_RUN_DESTROY}) {
        Context baseline;
        auto value = swr_v1_server_run_guarded(operation, 0, &baseline, effect, nullptr);
        assert(value.fault == 0 && value.value == 0 && !baseline.trace.empty());
        for (size_t index = 0; index < baseline.trace.size(); ++index) {
            auto op = baseline.trace[index];
            auto first = std::find(baseline.trace.begin(), baseline.trace.end(), op);
            if (static_cast<size_t>(first - baseline.trace.begin()) != index) continue;
            for (uint32_t kind : {1u, 2u, 3u, 4u}) {
                Context context;
                context.fail = op;
                context.kind = kind;
                auto result = swr_v1_server_run_guarded(operation, 0, &context, effect, nullptr);
                assert(result.fault == kind && context.trace.size() == index + 1);
                assert(std::equal(context.trace.begin(), context.trace.end(), baseline.trace.begin()));
#ifdef WRAP_FREE
                if (op == SWR_SERVER_OP_TASK_POOL_LISTEN) {
                    assert(watched && released);
                    watched = nullptr;
                    ++drops;
                }
#endif
                ++trials;
            }
        }
    }
    std::printf("production Server typed prefix propagation: %zu trials, %zu Rust String drops PASS\n", trials, drops);
}

// The pinned coroutine::async, async(fn), System::sleep, gethostbyname_impl_with_async
// and System::getaddrinfo behind one C entry point. build.rs copies them from
// upstream/src/coroutine/system.cc unchanged into hooks_async.inc; they compile over
// the deterministic facade below, which records every effect in order. The Rust side
// drives the hybrid's offload through the same facade (crates/hooks/src/offload/ported.rs).
#include <cassert>
#include <cerrno>
#include <cstdint>
#include <cstring>
#include <functional>
#include <limits>
#include <memory>
#include <string>
#include <vector>
#include <sys/socket.h>
#include <sys/types.h>

#define SW_USE_IOURING 0
#define SW_TIMER_MIN_SEC .001
#define SW_ERROR_AIO_TIMEOUT 1779
#define SW_ERROR_DNSLOOKUP_RESOLVE_TIMEOUT 1780
#define SW_OK 0
#define SW_ERR -1

namespace oracle_async {
static std::vector<std::string> trace;
static void record(std::string text) {
    trace.push_back(std::move(text));
}
struct AsyncRequest {
    virtual ~AsyncRequest() {
        record("request:delete");
    }
};
struct GethostbynameRequest : AsyncRequest {
    std::string addr;
    GethostbynameRequest(const std::string &name, int domain) {
        record("host-request:" + name + ":" + std::to_string(domain));
    }
};
struct GetaddrinfoRequest : AsyncRequest {
    int error = 0;
    bool ready = false;
    GetaddrinfoRequest(const std::string &name, int family, int kind, int protocol, const std::string &service) {
        record("addr-request:" + name + ":" + std::to_string(family) + ":" + std::to_string(kind) + ":" +
               std::to_string(protocol) + ":" + service);
    }
    void parse_result(std::vector<std::string> &result) {
        record("addr:parse");
        if (ready) result.push_back("native-result");
    }
};
struct AsyncEvent {
    size_t task_id = 0;
    uint8_t canceled = 0;
    int error = 0;
    std::shared_ptr<AsyncRequest> data;
    ssize_t retval = 0;
    void *pipe_socket = nullptr;
    double timestamp = 0;
    void *object = nullptr;
    void (*handler)(AsyncEvent *) = nullptr;
    void (*callback)(AsyncEvent *) = nullptr;
};
static bool dispatch_ok, yield_ok, context_canceled, queued, worker_canceled;
static int worker_error, request_error, last_error;
static ssize_t worker_result;
static AsyncEvent worker;
// A double's bits, so both sides print it exactly.
static std::string number(double value) {
    uint64_t bits;
    memcpy(&bits, &value, sizeof bits);
    return std::to_string(bits);
}
static int swoole_get_last_error() {
    record("last:get:" + std::to_string(last_error));
    return last_error;
}
static void swoole_set_last_error(int value) {
    last_error = value;
    record("last:set:" + std::to_string(value));
}
static void complete_worker() {
    if (!queued) return;
    queued = false;
    if (worker.handler && !worker.canceled) worker.handler(&worker);
    if (worker.callback) worker.callback(&worker);
}
struct Coroutine {
    static Coroutine *current;
    static Coroutine *get_current_safe() {
        record("current");
        return current;
    }
    bool yield_ex(double seconds) {
        record("yield-ex:" + number(seconds));
        if (yield_ok) complete_worker();
        return yield_ok;
    }
    void yield() {
        record("yield");
        complete_worker();
    }
    bool is_canceled() {
        record("canceled:" + std::to_string(context_canceled));
        return context_canceled;
    }
    void resume() {
        record("resume");
    }
};
Coroutine *Coroutine::current = nullptr;
namespace async {
using Handler = void (*)(AsyncEvent *);
static AsyncEvent *dispatch(const AsyncEvent *event) {
    record("dispatch:" + std::to_string(event->handler != nullptr) + ":" + std::to_string(event->callback != nullptr));
    if (!dispatch_ok) return nullptr;
    worker = *event;
    worker.canceled = worker_canceled;
    worker.error = worker_error;
    worker.retval = worker_result;
    queued = true;
    return &worker;
}
static void handler_gethostbyname(AsyncEvent *event) {
    record("handler:host");
    static_cast<GethostbynameRequest *>(event->data.get())->addr = "127.0.0.1";
    event->retval = worker_result;
    event->error = worker_error;
}
static void handler_getaddrinfo(AsyncEvent *event) {
    record("handler:addr");
    auto *request = static_cast<GetaddrinfoRequest *>(event->data.get());
    request->ready = true;
    request->error = request_error;
    event->retval = worker_result;
    event->error = worker_error;
}
}  // namespace async
namespace coroutine {
struct System {
    static int sleep(double seconds);
    static std::vector<std::string> getaddrinfo(const std::string &, int, int, int, const std::string &, double);
};
bool async(async::Handler, AsyncEvent &, double);
#include "hooks_async.inc"
}  // namespace coroutine
static void typed_handler(AsyncEvent *event) {
    record("handler:typed");
    event->error = worker_error;
    event->retval = worker_result;
}

// The gate's 426 scenarios: 96 typed async, 6 async(fn), 36 sleep, 96 gethostbyname
// and 192 getaddrinfo, each a different mix of dispatch refusal, timeout or cancel,
// worker cancel, errors and results.
static void run(int scenario) {
    trace.clear();
    errno = E2BIG;
    dispatch_ok = yield_ok = true;
    context_canceled = queued = worker_canceled = false;
    worker_error = EIO;
    request_error = 0;
    last_error = ECANCELED;
    worker_result = 1;
    Coroutine co;
    Coroutine::current = &co;
    worker = AsyncEvent{};
    if (scenario < 96) {
        int k = scenario;
        dispatch_ok = k & 1;
        yield_ok = k & 2;
        worker_canceled = k & 4;
        int errors[] = {0, ETIMEDOUT, EIO};
        ssize_t results[] = {-1, 0, 1, 123};
        worker_error = errors[(k / 8) % 3];
        worker_result = results[k / 24];
        AsyncEvent event;
        bool result = coroutine::async(typed_handler, event, .25);
        record("result:" + std::to_string(result));
        record("caller-canceled:" + std::to_string(event.canceled));
        record("caller-retval:" + std::to_string(event.retval));
        record("caller-error:" + std::to_string(event.error));
        record("worker-canceled:" + std::to_string(worker.canceled));
        record("errno:" + std::to_string(errno));
        if (queued) {
            record("late-completion");
            complete_worker();
        }
    } else if (scenario < 102) {
        int k = scenario - 96;
        dispatch_ok = k & 1;
        int errors[] = {0, EIO, EINVAL};
        worker_error = errors[(k / 2) % 3];
        bool result = coroutine::async([&]() {
            record("lambda:execute");
            errno = worker_error;
        });
        record("result:" + std::to_string(result));
        record("errno:" + std::to_string(errno));
    } else if (scenario < 138) {
        int k = scenario - 102;
        double seconds[] = {-std::numeric_limits<double>::infinity(), -1, -0.0, 0, .0001, .001, 1,
                            std::numeric_limits<double>::quiet_NaN(), std::numeric_limits<double>::infinity()};
        context_canceled = (k / 9) % 2;
        yield_ok = k / 18;
        record("sleep:" + std::to_string(coroutine::System::sleep(seconds[k % 9])));
        record("errno:" + std::to_string(errno));
    } else if (scenario < 234) {
        int k = scenario - 138;
        ssize_t results[] = {-1, 0, 1};
        int errors[] = {0, SW_ERROR_AIO_TIMEOUT, EIO, EINVAL};
        worker_result = results[k % 3];
        worker_error = errors[(k / 3) % 4];
        dispatch_ok = (k / 12) % 2;
        yield_ok = (k / 24) % 2;
        auto value = coroutine::gethostbyname_impl_with_async("private.test", k / 48 ? AF_INET6 : AF_INET, .5);
        record("host:" + value);
        record("errno:" + std::to_string(errno));
        record("last:" + std::to_string(last_error));
        if (queued) {
            record("late-completion");
            complete_worker();
        }
    } else {
        int k = scenario - 234;
        ssize_t results[] = {-1, 0, 1};
        int errors[] = {0, SW_ERROR_AIO_TIMEOUT, EIO, EINVAL};
        worker_result = results[k % 3];
        worker_error = errors[(k / 3) % 4];
        dispatch_ok = (k / 12) % 2;
        yield_ok = (k / 24) % 2;
        request_error = (k / 48) % 2;
        auto values = coroutine::System::getaddrinfo(
            "private.test", k / 96 ? AF_INET6 : AF_INET, SOCK_STREAM, 0, "443", .5);
        record("addresses:" + std::to_string(values.size()));
        if (!values.empty()) record(values[0]);
        record("errno:" + std::to_string(errno));
        record("last:" + std::to_string(last_error));
        if (queued) {
            record("late-completion");
            complete_worker();
        }
    }
    worker = AsyncEvent{};
}
}  // namespace oracle_async

extern "C" {
// Runs one scenario (0..426) through the pinned bodies and hands each recorded
// effect, in order, to line(context, text, length).
void swr_oracle_hooks_async(int scenario, void (*line)(void *, const char *, size_t), void *context) {
    oracle_async::run(scenario);
    for (auto &text : oracle_async::trace) line(context, text.data(), text.size());
}
}

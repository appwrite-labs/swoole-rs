// What the Rust cases in src/ can't express, compiled into the hybrid tree's lib-swoole by
// tests/modules/hooks/system.py (--fixture): the std::function arguments of
// coroutine::run, Coroutine::bailout, swoole_event_defer and the timer, and the repo-owned witnesses
// that check C++ copies and std::bad_alloc containment, which only C++ can throw (formerly
// tests/modules/coroutine/lifetime.cpp, tests/modules/hooks/socket_scopes.cpp and
// dns_request_owner.cpp, unchanged but for these macros in place of GoogleTest's), and the
// Server worker-retirement witness (formerly tests/modules/process/worker_retirement.cc).
#include "swoole_coroutine.h"
#include "swoole_coroutine_system.h"
#include "swoole_rs_coroutine_socket.h"
#include "swoole_async.h"
#include "swoole_timer.h"
#include "swoole_server.h"

#include <arpa/inet.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <new>
#include <unistd.h>

using namespace swoole;
using swoole::coroutine::ProtocolSwitch;
using swoole::coroutine::Socket;

// A failed assertion ends the case's process, which the runner reports as failed.
#define CHECK(c)                                                                                                       \
    do {                                                                                                               \
        if (!(c)) {                                                                                                    \
            fprintf(stderr, "%s:%d: assertion failed: %s\n", __FILE__, __LINE__, #c);                                  \
            _exit(1);                                                                                                  \
        }                                                                                                              \
    } while (0)
#define ASSERT_TRUE(c) CHECK(c)
#define ASSERT_FALSE(c) CHECK(!(c))
#define ASSERT_EQ(a, b) CHECK((a) == (b))
#define ASSERT_NE(a, b) CHECK((a) != (b))
#define ASSERT_GT(a, b) CHECK((a) > (b))
#define EXPECT_THROW(statement, type)                                                                                  \
    do {                                                                                                               \
        bool thrown = false;                                                                                           \
        try {                                                                                                          \
            statement;                                                                                                 \
        } catch (const type &) {                                                                                       \
            thrown = true;                                                                                             \
        }                                                                                                              \
        CHECK(thrown);                                                                                                 \
    } while (0)

// The fault oracle: between begin(n) and end(), this thread's nth operator new throws.
namespace allocation {
thread_local bool active = false;
thread_local size_t count = 0, fail = 0;
void begin(size_t nth) {
    count = 0;
    fail = nth;
    active = true;
}
void end() {
    active = false;
}
}  // namespace allocation
void *operator new(size_t size) {
    if (allocation::active && ++allocation::count == allocation::fail) {
        throw std::bad_alloc();
    }
    if (void *p = std::malloc(size ? size : 1)) {
        return p;
    }
    throw std::bad_alloc();
}
// What native/clients/dns.cc defined over the request views; production resolves in
// Rust (crates/hooks/src/resolve.rs), so these witnesses keep the views' copies and
// exception paths.
static int gethostbyname(GethostbynameRequest *req) {
    const int status = swr_v1_core_resolver_host(req->state_handle());
    if (status < -1) req->addr.check(status);
    return status;
}
static int getaddrinfo(swr_v1_core_resolver *request) {
    const int status = swr_v1_core_resolver_lookup(
        request, [](void *, int) { return true; }, nullptr);
    if (status < -1) ResolverOwner::check(status);
    return status;
}
static void parse_result(const GetaddrinfoRequest &request, std::vector<std::string> &retval) {
    size_t cursor = 0;
    while (true) {
        auto address = swr_v1_core_resolver_next(request.state_handle(), &cursor);
        if (!address.valid) break;
        retval.emplace_back(address.bytes);
    }
}
typedef void (*swr_put)(void *out, const char *data, size_t length);
extern "C" bool swr_v1_system_gethostbyname(const char *, size_t, long, double, swr_put, void *);
extern "C" bool swr_v1_system_getaddrinfo(const char *, long, long, long, const char *, double, swr_put, void *);

void operator delete(void *p) noexcept {
    std::free(p);
}
void *operator new[](size_t n) {
    return ::operator new(n);
}
void operator delete[](void *p) noexcept {
    ::operator delete(p);
}
void operator delete(void *p, size_t) noexcept {
    ::operator delete(p);
}
void operator delete[](void *p, size_t) noexcept {
    ::operator delete(p);
}
// Keep nothrow allocations in the same malloc/free domain as the fault oracle.
void *operator new(size_t n, const std::nothrow_t &) noexcept {
    try {
        return ::operator new(n);
    } catch (...) {
        return nullptr;
    }
}
void *operator new[](size_t n, const std::nothrow_t &) noexcept {
    try {
        return ::operator new[](n);
    } catch (...) {
        return nullptr;
    }
}
void operator delete(void *p, const std::nothrow_t &) noexcept {
    ::operator delete(p);
}
void operator delete[](void *p, const std::nothrow_t &) noexcept {
    ::operator delete[](p);
}

struct ThrowingCancelCopy {
    bool *fail;
    explicit ThrowingCancelCopy(bool *value) : fail(value) {}
    ThrowingCancelCopy(const ThrowingCancelCopy &other) : fail(other.fail) {
        if (*fail) {
            throw std::bad_alloc();
        }
    }
    bool operator()(Coroutine *co) const {
        co->resume();
        return true;
    }
};

extern "C" {
// `swoole::coroutine::run(fn, arg)`.
bool swr_case_coroutine_run(void (*fn)(void *), void *arg) {
    return coroutine::run(fn, arg);
}

// `Coroutine::bailout(fn)`, or `bailout(nullptr)` when `fn` is null.
void swr_case_bailout(void (*fn)(void *), void *data) {
    Coroutine::bailout(fn ? Coroutine::BailoutCallback([fn, data]() { fn(data); }) : nullptr);
}

// `swoole_event_defer(fn, data)`.
void swr_case_event_defer(void (*fn)(void *), void *data) {
    swoole_event_defer(fn, data);
}

// SwooleTG.timer, a C++ thread_local (timer.rs).
Timer *swr_case_timer_exchange(Timer *timer) {
    Timer *previous = SwooleTG.timer;
    SwooleTG.timer = timer;
    return previous;
}
size_t swr_case_timer_count() {
    return sw_timer()->count();
}
void swr_case_timer_reinit() {
    sw_timer()->reinit();
}
bool swr_case_timer_remove(long id) {
    return sw_timer()->remove(id);
}

// os/process_pool.cpp (process_pool.rs): the record sizes its mirrors check, and what its
// TESTs reach that isn't an exported symbol: sw_mem_pool(), the inline swoole_set_isolation,
// SwooleWG and sw_worker().
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Winvalid-offsetof"
extern const size_t swr_case_process_layout[] = {sizeof(ProcessPool),
                                                 offsetof(ProcessPool, main_loop),
                                                 offsetof(ProcessPool, ptr),
                                                 sizeof(Worker),
                                                 offsetof(Worker, start_time),
                                                 sizeof(EventData)};
#pragma GCC diagnostic pop
void *swr_case_mem_pool_alloc(uint32_t size) {
    return sw_mem_pool()->alloc(size);
}
void swr_case_set_isolation_user(const char *user) {
    swoole_set_isolation("", user, "");
}
uint32_t swr_case_worker_max_request() {
    return SwooleWG.max_request;
}
Worker *swr_case_current_worker() {
    return sw_worker();
}

// rust_worker_retirement.abort_and_reset (formerly tests/modules/process/worker_retirement.cc):
// first-match PID lookup, the session bitfields abort_worker clears (including the last slot
// and the reserved bits) and the worker/shared concurrency reset, over a real Server.
void swr_case_worker_retirement() {
    Server server(Server::MODE_BASE);
    server.worker_num = 2;
    ASSERT_TRUE(server.add_port(SW_SOCK_TCP, "127.0.0.1", 0));
    ASSERT_EQ(server.create(), SW_OK);
    server.get_event_worker_pool()->workers = server.workers;
    auto *worker = &server.workers[0];
    auto *other = &server.workers[1];
    worker->pid = other->pid = 79;
    ASSERT_EQ(server.get_worker_by_pid(79), worker);
    worker->pid = 80;
    ASSERT_EQ(server.get_worker_by_pid(79), other);
    other->pid = -7;
    ASSERT_EQ(server.get_worker_by_pid(-7), other);
    ASSERT_EQ(server.get_worker_by_pid(79), nullptr);

    worker->id = 1;
    worker->concurrency = 3;
    worker->request_count = 7;
    worker->response_count = 8;
    worker->dispatch_count = 9;
    server.gs->concurrency = 7;
    const size_t last = SW_SESSION_LIST_SIZE - 1;
    for (size_t index = 0; index < SW_SESSION_LIST_SIZE; ++index) {
        auto *slot = server.get_session(index);
        slot->id = index + 41;
        slot->fd = 23;
        slot->reactor_id = 2;
        slot->reserve_ = 0x5a1234;
    }
    server.get_session(0)->reactor_id = 1;
    server.get_session(last)->reactor_id = 1;
    server.abort_worker(worker);
    ASSERT_TRUE(worker->concurrency == 0 && server.gs->concurrency == 4);
    ASSERT_TRUE(worker->request_count == 0 && worker->response_count == 0 && worker->dispatch_count == 0);
    for (size_t index = 0; index < SW_SESSION_LIST_SIZE; ++index) {
        const auto *slot = server.get_session(index);
        ASSERT_TRUE(slot->id == static_cast<SessionId>(index + 41));
        ASSERT_TRUE(slot->reserve_ == 0x5a1234);
        ASSERT_TRUE(slot->reactor_id == (index == 0 || index == last ? 1 : 2));
        ASSERT_TRUE(slot->fd == (index == 0 || index == last ? 0 : 23));
    }
    worker->concurrency = 9;
    server.gs->concurrency = 3;
    server.reset_worker_counter(worker);
    ASSERT_TRUE(worker->concurrency == 0 && server.gs->concurrency == 0);
    server.destroy();
}

// rust_coroutine_lifetime.cancellation_copy_failure_preserves_wait, in a coroutine.
void swr_case_cancellation_copy_failure_preserves_wait() {
    bool fail = false;
    bool resumed_canceled = false;
    long cid = Coroutine::create([&](void *) {
        Coroutine::CancelFunc cancel = ThrowingCancelCopy(&fail);
        Coroutine *co = Coroutine::get_current();
        co->yield(&cancel);
        resumed_canceled = co->is_canceled();
    });
    Coroutine *co = Coroutine::get_by_cid(cid);
    ASSERT_TRUE(co->is_suspending());
    fail = true;
    EXPECT_THROW(co->cancel(), std::bad_alloc);
    ASSERT_TRUE(co->is_suspending());
    ASSERT_FALSE(co->is_canceled());
    fail = false;
    ASSERT_TRUE(co->cancel());
    // cancel's callback has resumed and deleted co; never read it again.
    ASSERT_EQ(Coroutine::get_by_cid(cid), nullptr);
    ASSERT_TRUE(resumed_canceled);
}

// rust_socket_scopes.protocol_snapshot_and_copy_restore, in a coroutine.
void swr_case_protocol_snapshot_and_copy_restore() {
    Socket socket(SW_SOCK_TCP);
    socket.open_eof_check = true;
    socket.open_length_check = false;
    std::memset(&socket.protocol, 0, sizeof(socket.protocol));
    socket.protocol.package_max_length = 117;
    socket.protocol.package_eof[0] = '\0';
    socket.protocol.package_eof[1] = 'z';
    const auto original = socket.protocol;
    {
        ProtocolSwitch scope(&socket);
        auto copy = scope;
        socket.open_eof_check = false;
        socket.open_length_check = true;
        std::memset(&socket.protocol, 1, sizeof(socket.protocol));
    }
    ASSERT_TRUE(socket.open_eof_check);
    ASSERT_FALSE(socket.open_length_check);
    ASSERT_EQ(std::memcmp(&original, &socket.protocol, sizeof(original)), 0);
    {
        ProtocolSwitch first(&socket);
        socket.protocol.package_max_length = 222;
        {
            ProtocolSwitch nested(&socket);
            socket.protocol.package_max_length = 333;
        }
        ASSERT_EQ(socket.protocol.package_max_length, 222u);
    }
    ASSERT_EQ(socket.protocol.package_max_length, 117u);
}

// rust_socket_scopes.error_text_publication, in a coroutine. The error text is the
// socket's Rust string; a Socket has no C++ copy of it to cache or throw from.
void swr_case_error_text_publication() {
    Socket socket(SW_SOCK_TCP);
    swoole_set_last_error(0);
    errno = EINVAL;
    socket.set_err();
    ASSERT_EQ(socket.errCode, EINVAL);
    ASSERT_EQ(swoole_get_last_error(), 0);
    ASSERT_EQ(errno, EINVAL);
    swoole_set_last_error(EACCES);
    socket.set_err();
    ASSERT_EQ(socket.errCode, EACCES);
    ASSERT_EQ(errno, EINVAL);
    const char *borrowed = "borrowed";
    socket.set_err(EIO, borrowed);
    ASSERT_EQ(socket.errMsg, borrowed);
    ASSERT_EQ(errno, EIO);
    ASSERT_EQ(swoole_get_last_error(), EIO);
    const std::string first(100, 'a');
    const std::string second(600, 'b');
    socket.set_err(ERANGE, first);
    ASSERT_EQ(std::string(socket.errMsg), first);
    socket.set_err(ENOSPC, second);
    ASSERT_EQ(socket.errCode, ENOSPC);
    ASSERT_EQ(errno, ENOSPC);
    ASSERT_EQ(swoole_get_last_error(), ENOSPC);
    ASSERT_EQ(std::string(socket.errMsg), second);
    socket.set_err(0, std::string("a\0b", 3));
    ASSERT_EQ(std::string(socket.errMsg, 3), std::string("a\0b", 3));
    ASSERT_EQ(socket.errMsg[2], 'b');
}

// rust_dns_request.numeric_host_owner_and_independent_copies.
void swr_case_numeric_host_owner_and_independent_copies() {
    GethostbynameRequest host("127.0.0.1", AF_INET);
    ASSERT_EQ(gethostbyname(&host), 0);
    ASSERT_EQ(host.addr, "127.0.0.1");
    auto copy = host;
    copy.name = "::1";
    copy.family = AF_INET6;
    ASSERT_EQ(gethostbyname(&copy), 0);
    ASSERT_EQ(copy.addr, "::1");
    ASSERT_EQ(host.name, "127.0.0.1");
    ASSERT_EQ(host.addr, "127.0.0.1");
    host = copy;
    copy.name = "changed";
    ASSERT_EQ(host.name, "::1");
    ASSERT_EQ(host.family, AF_INET6);
    host = host;
    ASSERT_EQ(host.addr, "::1");
}

// rust_dns_request.hints_vectors_and_failure_publication.
void swr_case_hints_vectors_and_failure_publication() {
    GetaddrinfoRequest request("127.0.0.1", AF_INET, SOCK_STREAM, 0, "");
    ASSERT_EQ(request.count, 0);
    ASSERT_EQ(request.error, 0);
    int *family_address = &request.family;
    ASSERT_EQ(getaddrinfo(request.state_handle()), 0);
    ASSERT_GT(request.count, 0);
    ASSERT_EQ(request.results.size(), static_cast<size_t>(request.count));
    auto copy = request;
    auto snapshot = copy.results;
    const auto saved_count = request.count;
    const std::vector<sockaddr_in6> saved = request.results;
    request.family = -123;
    ASSERT_EQ(getaddrinfo(request.state_handle()), -1);
    ASSERT_NE(request.error, 0);
    ASSERT_EQ(request.count, saved_count);
    ASSERT_EQ(request.results.size(), saved.size());
    ASSERT_EQ(std::memcmp(request.results.data(), saved.data(), saved.size() * sizeof(sockaddr_in6)), 0);
    ASSERT_EQ(&request.family, family_address);
    auto *address = reinterpret_cast<sockaddr_in *>(&copy.results[0]);
    ASSERT_EQ(inet_pton(AF_INET, "127.0.0.2", &address->sin_addr), 1);
    ASSERT_NE(std::memcmp(copy.results.data(), snapshot.data(), sizeof(sockaddr_in6)), 0);
    std::vector<std::string> output{"keep"};
    parse_result(copy, output);
    ASSERT_EQ(output.front(), "keep");
    ASSERT_EQ(output[1], "127.0.0.2");
    request = copy;
    copy.hostname = "changed";
    copy.results.clear();
    ASSERT_EQ(request.hostname, "127.0.0.1");
    ASSERT_FALSE(request.results.empty());
    request = request;
    ASSERT_EQ(&request.family, family_address);
    request.results.resize(0);
    ASSERT_TRUE(request.results.empty());
}

// rust_dns_request.native_output_append_exception_is_contained.
void swr_case_native_output_append_exception_is_contained() {
    GetaddrinfoRequest request("::1", AF_INET6, SOCK_STREAM, 0, "");
    request.results.resize(3);
    const char *text = "2001:db8:1111:2222:3333:4444:5555:6666";
    for (auto &address : request.results) {
        address = sockaddr_in6{};
        ASSERT_EQ(inet_pton(AF_INET6, text, &address.sin6_addr), 1);
    }
    std::vector<std::string> output{"keep"};
    output.reserve(4);
    allocation::begin(2);
    bool caught = false;
    try {
        parse_result(request, output);
    } catch (const std::bad_alloc &) {
        caught = true;
    }
    allocation::end();
    ASSERT_TRUE(caught);
    ASSERT_EQ(output.size(), 2u);
    ASSERT_EQ(output[0], "keep");
    ASSERT_EQ(output[1], text);
    ASSERT_EQ(request.results.size(), 3u);
    output.clear();
    parse_result(request, output);
    ASSERT_EQ(output.size(), 3u);
}

// rust_dns_request.actual_async_numeric_results, in a coroutine.
void swr_case_actual_async_numeric_results() {
    std::string host;
    ASSERT_TRUE(swr_v1_system_gethostbyname("127.0.0.1", 9, AF_INET, 2, [](void *out, const char *data, size_t length) {
        static_cast<std::string *>(out)->assign(data, length);
    }, &host));
    ASSERT_EQ(host, "127.0.0.1");
    std::vector<std::string> addresses;
    ASSERT_TRUE(swr_v1_system_getaddrinfo("127.0.0.1", AF_INET, SOCK_STREAM, 0, "", 2, [](void *out, const char *data, size_t length) {
        static_cast<std::vector<std::string> *>(out)->emplace_back(data, length);
    }, &addresses));
    ASSERT_FALSE(addresses.empty());
    for (const auto &address : addresses) ASSERT_EQ(address, "127.0.0.1");
}

// rust_dns_request.owned_address_cache_exception.
void swr_case_owned_address_cache_exception() {
    const std::string text = "2001:db8:1111:2222:3333:4444:5555:6666";
    GethostbynameRequest request(text, AF_INET6);
    request.addr = "previous";
    const std::string &cache = request.addr;
    const std::string *identity = &cache;
    allocation::begin(1);
    bool caught = false;
    try {
        gethostbyname(&request);
    } catch (const std::bad_alloc &) {
        caught = true;
    }
    allocation::end();
    ASSERT_TRUE(caught);
    ASSERT_EQ(cache, "previous");
    ASSERT_EQ(&static_cast<const std::string &>(request.addr), identity);
    ASSERT_EQ(gethostbyname(&request), 0);
    ASSERT_EQ(request.addr, text);
}
}

// What the Rust cases in src/ can't express, compiled into system.py's lib-swoole by
// tests/modules/hooks/system.py (--fixture): swoole_event_defer and the timer, which have C++
// linkage, and the repo-owned witnesses that check C++ copies and std::bad_alloc containment,
// which only C++ can throw (formerly tests/modules/hooks/dns_request_owner.cpp, unchanged
// but for these macros in place of GoogleTest's).
#include "swoole_coroutine.h"
#include "swoole_coroutine_system.h"
#include "swoole_rs_coroutine_socket.h"
#include "swoole_async.h"
#include "swoole_timer.h"
#include "swoole_rs_process_pool.h"

#include <arpa/inet.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <new>
#include <unistd.h>

using namespace swoole;

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

extern "C" {
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
// TESTs reach that isn't an exported symbol: the inline swoole_set_isolation, SwooleWG and
// sw_worker().
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Winvalid-offsetof"
extern const size_t swr_case_process_layout[] = {sizeof(ProcessPool),
                                                 offsetof(ProcessPool, main_loop),
                                                 offsetof(ProcessPool, ptr),
                                                 sizeof(Worker),
                                                 offsetof(Worker, start_time),
                                                 sizeof(EventData)};
#pragma GCC diagnostic pop
void swr_case_set_isolation_user(const char *user) {
    swoole_set_isolation("", user, "");
}
// `SwooleWG`: crates/process's WorkerGlobalRecord, up to the fields read here.
struct CaseWorkerGlobal {
    uint32_t id;
    uint8_t type;
    pid_t pid;
    bool shutdown;
    bool running;
    uint32_t max_request;
    Worker *worker;
};
static_assert(offsetof(CaseWorkerGlobal, max_request) == 16, "WorkerGlobalRecord::max_request");
static_assert(offsetof(CaseWorkerGlobal, worker) == 24, "WorkerGlobalRecord::worker");
extern "C" CaseWorkerGlobal *swr_v1_worker_globals();

uint32_t swr_case_worker_max_request() {
    return swr_v1_worker_globals()->max_request;
}
Worker *swr_case_current_worker() {
    return swr_v1_worker_globals()->worker;
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
}

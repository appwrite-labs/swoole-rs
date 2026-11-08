// What the Rust cases in src/ can't express, compiled into the hybrid tree's lib-swoole by
// tests/modules/hooks/system.py (--fixture): the std::function arguments of
// coroutine::run, Coroutine::bailout and swoole_event_defer, and the repo-owned witnesses
// that check C++ copies and std::bad_alloc containment, which only C++ can throw (formerly
// tests/modules/coroutine/lifetime.cpp, tests/modules/hooks/socket_scopes.cpp and
// dns_request_owner.cpp, unchanged but for these macros in place of GoogleTest's).
#include "swoole_coroutine.h"
#include "swoole_coroutine_system.h"
#include "swoole_rs_coroutine_socket.h"
#include "swoole_async.h"

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

// rust_socket_scopes.error_text_publication_and_cache_exception, in a coroutine.
void swr_case_error_text_publication_and_cache_exception() {
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
    const std::string &cache = socket.errString;
    const std::string *identity = &cache;
    const char *message = socket.errMsg;
    allocation::begin(1);
    bool caught = false;
    try {
        socket.set_err(ENOSPC, second);
    } catch (const std::bad_alloc &) {
        caught = true;
    }
    allocation::end();
    ASSERT_TRUE(caught);
    ASSERT_EQ(socket.errCode, ENOSPC);
    ASSERT_EQ(errno, ENOSPC);
    ASSERT_EQ(swoole_get_last_error(), ENOSPC);
    ASSERT_EQ(socket.errMsg, message);
    ASSERT_EQ(cache, first);
    ASSERT_EQ(&static_cast<const std::string &>(socket.errString), identity);
    socket.set_err(0, std::string("a\0b", 3));
    ASSERT_EQ(static_cast<const std::string &>(socket.errString), std::string("a\0b", 3));
    ASSERT_EQ(socket.errMsg[2], 'b');
}

// rust_dns_request.numeric_host_owner_and_independent_copies.
void swr_case_numeric_host_owner_and_independent_copies() {
    GethostbynameRequest host("127.0.0.1", AF_INET);
    ASSERT_EQ(network::gethostbyname(&host), 0);
    ASSERT_EQ(host.addr, "127.0.0.1");
    auto copy = host;
    copy.name = "::1";
    copy.family = AF_INET6;
    ASSERT_EQ(network::gethostbyname(&copy), 0);
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
    ASSERT_EQ(network::getaddrinfo(&request), 0);
    ASSERT_GT(request.count, 0);
    ASSERT_EQ(request.results.size(), static_cast<size_t>(request.count));
    auto copy = request;
    auto snapshot = copy.results;
    const auto saved_count = request.count;
    const std::vector<sockaddr_in6> saved = request.results;
    request.family = -123;
    ASSERT_EQ(network::getaddrinfo(&request), -1);
    ASSERT_NE(request.error, 0);
    ASSERT_EQ(request.count, saved_count);
    ASSERT_EQ(request.results.size(), saved.size());
    ASSERT_EQ(std::memcmp(request.results.data(), saved.data(), saved.size() * sizeof(sockaddr_in6)), 0);
    ASSERT_EQ(&request.family, family_address);
    auto *address = reinterpret_cast<sockaddr_in *>(&copy.results[0]);
    ASSERT_EQ(inet_pton(AF_INET, "127.0.0.2", &address->sin_addr), 1);
    ASSERT_NE(std::memcmp(copy.results.data(), snapshot.data(), sizeof(sockaddr_in6)), 0);
    std::vector<std::string> output{"keep"};
    copy.parse_result(output);
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
        request.parse_result(output);
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
    request.parse_result(output);
    ASSERT_EQ(output.size(), 3u);
}

// rust_dns_request.actual_async_numeric_results, in a coroutine.
void swr_case_actual_async_numeric_results() {
    const auto host = swoole::coroutine::gethostbyname_impl_with_async("127.0.0.1", AF_INET, 2);
    ASSERT_EQ(host, "127.0.0.1");
    // What System::getaddrinfo did before it folded into swr_v1_system_getaddrinfo.
    AsyncEvent ev{};
    auto req = new GetaddrinfoRequest("127.0.0.1", AF_INET, SOCK_STREAM, 0, "");
    ev.data = std::shared_ptr<AsyncRequest>(req);
    ASSERT_TRUE(swoole::coroutine::async(async::handler_getaddrinfo, ev, 2));
    std::vector<std::string> addresses;
    req->parse_result(addresses);
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
        network::gethostbyname(&request);
    } catch (const std::bad_alloc &) {
        caught = true;
    }
    allocation::end();
    ASSERT_TRUE(caught);
    ASSERT_EQ(cache, "previous");
    ASSERT_EQ(&static_cast<const std::string &>(request.addr), identity);
    ASSERT_EQ(network::gethostbyname(&request), 0);
    ASSERT_EQ(request.addr, text);
}
}

#include "test_coroutine.h"
#include "swoole_async.h"
#include "swoole_rs_test_allocation.h"
#include <arpa/inet.h>
#include <cstring>
using namespace swoole;

TEST(rust_dns_request, numeric_host_owner_and_independent_copies) {
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

TEST(rust_dns_request, hints_vectors_and_failure_publication) {
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

TEST(rust_dns_request, native_output_append_exception_is_contained) {
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
    try { request.parse_result(output); }
    catch (const std::bad_alloc &) { caught = true; }
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

TEST(rust_dns_request, actual_async_numeric_results) {
    swoole::test::coroutine::run([](void *) {
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
    });
}

TEST(rust_dns_request, owned_address_cache_exception) {
    const std::string text = "2001:db8:1111:2222:3333:4444:5555:6666";
    GethostbynameRequest request(text, AF_INET6);
    request.addr = "previous";
    const std::string &cache = request.addr;
    const std::string *identity = &cache;
    allocation::begin(1);
    bool caught = false;
    try { network::gethostbyname(&request); }
    catch (const std::bad_alloc &) { caught = true; }
    allocation::end();
    ASSERT_TRUE(caught);
    ASSERT_EQ(cache, "previous");
    ASSERT_EQ(&static_cast<const std::string &>(request.addr), identity);
    ASSERT_EQ(network::gethostbyname(&request), 0);
    ASSERT_EQ(request.addr, text);
}

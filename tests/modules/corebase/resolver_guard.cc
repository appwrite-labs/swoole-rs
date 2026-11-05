#include "swoole_rs_corebase_resolver.h"
#include <cassert>
#include <cstring>
#include <cstdint>
#include <exception>
#include <netdb.h>
#include <new>
#include <iostream>
static addrinfo entry;
static sockaddr_storage address;
static bool freed;
extern "C" int getaddrinfo(const char *, const char *, const addrinfo *, addrinfo **result) {
    entry = addrinfo{};
    entry.ai_family = 12345;
    entry.ai_addr = reinterpret_cast<sockaddr *>(&address);
    *result = &entry;
    return 0;
}
extern "C" void freeaddrinfo(addrinfo *result) { assert(result == &entry); freed = true; }
struct Diagnostic { std::exception_ptr exception; unsigned calls = 0; bool refuse = true; };
static bool diagnostic(void *pointer, int family) {
    auto &state = *static_cast<Diagnostic *>(pointer);
    assert(family == 12345);
    ++state.calls;
    if (!state.refuse) return true;
    try { throw std::bad_alloc(); }
    catch (...) { state.exception = std::current_exception(); return false; }
}
int main() {
    const auto failed_await = swr_v1_core_resolver_finish(-1, 4003, reinterpret_cast<const int *>(uintptr_t{1}), 4003, 712);
    assert(failed_await.failed && failed_await.error == 712);
    auto request = swr_v1_core_resolver_create(AF_INET, SOCK_STREAM, 0, "127.0.0.1", 9, "", 0);
    assert(request.owner);
    request.fields->error = 55;
    request.fields->count = 77;
    assert(swr_v1_core_resolver_resize(request.owner, 1) == 0);
    auto before = swr_v1_core_resolver_span(request.owner);
    std::memset(before.data, 0x5a, sizeof(sockaddr_in6));
    Diagnostic state;
    assert(swr_v1_core_resolver_lookup(request.owner, diagnostic, &state) == -3);
    assert(freed && state.calls == 1 && state.exception);
    assert(request.fields->error == 55 && request.fields->count == 77);
    auto after = swr_v1_core_resolver_span(request.owner);
    assert(after.data == before.data && after.length == 1);
    for (size_t i = 0; i < sizeof(sockaddr_in6); ++i) assert(reinterpret_cast<unsigned char *>(after.data)[i] == 0x5a);
    bool caught = false;
    try { std::rethrow_exception(state.exception); } catch (const std::bad_alloc &) { caught = true; }
    assert(caught);
    freed = false; state.refuse = false;
    assert(swr_v1_core_resolver_lookup(request.owner, diagnostic, &state) == 0);
    assert(freed && state.calls == 2 && request.fields->error == 0 && request.fields->count == 1);
    after = swr_v1_core_resolver_span(request.owner);
    for (size_t i = 0; i < sizeof(sockaddr_in6); ++i) assert(reinterpret_cast<unsigned char *>(after.data)[i] == 0);
    swr_v1_core_resolver_free(request.owner);
    std::cout << "{\"diagnostic_cleanup\":true,\"contained_exception\":true,\"unknown_slot\":true}\n";
}

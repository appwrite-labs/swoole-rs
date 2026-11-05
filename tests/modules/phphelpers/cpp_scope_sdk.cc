#include "swoole_rs_native_cpp_scope.h"
#include "swoole_rs_native_effect.h"
#include "swoole_rs_zend_guard.h"
#include <cassert>
#include <stdexcept>
using swoole_rs::NativeCppScope;
extern NativeCppScope *other_scope();
extern "C" int cpp_scope_test_owner(int (*)(void *), void *, void (*)(const unsigned char *, size_t));
static size_t drops, bytes, checksum;
static void dropped(const unsigned char *data, size_t length) {
    ++drops; bytes += length;
    for (size_t i = 0; i < length; ++i) checksum += data[i];
}
static int capture(void *argument) {
    auto *scope = static_cast<NativeCppScope *>(argument);
    return scope->capture([]() { throw std::runtime_error("ordinary native catch"); }) ? 0 : 1;
}
static void *key;
static void *context() { return key; }
int main() {
    static_assert(SWR_ZEND_NATIVE_EXCEPTION == 3, "additive native custody kind");
    assert(!swoole_rs::native_effect_provider());
    assert(!other_scope());
    NativeCppScope outer;
    assert(other_scope() == &outer);
    bool continued = false;
    {
        NativeCppScope inner;
        assert(other_scope() == &inner);
        assert(cpp_scope_test_owner(capture, &inner, dropped) == 1);
        assert(drops == 1 && bytes == 4096 && checksum == 4096 * 42);
        try {
            auto exception = inner.take();
            inner.leave();
            std::rethrow_exception(exception);
        } catch (const std::runtime_error &) { continued = true; }
    }
    assert(continued && other_scope() == &outer);
    int a, b;
    swoole_rs::NativeCppRuntime runtime{context, nullptr, nullptr, nullptr};
    key = &a; NativeCppScope first{runtime};
    key = &b; NativeCppScope second{runtime};
    assert(NativeCppScope::current(runtime) == &second);
    key = &a; assert(NativeCppScope::current(runtime) == &first);
    first.leave(); assert(!NativeCppScope::current(runtime));
    key = &b; assert(NativeCppScope::current(runtime) == &second);
    second.leave(); assert(!NativeCppScope::current(runtime));
    assert(other_scope() == &outer);
}

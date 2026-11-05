#include "php.h"
#include "swoole_rs_zend_guard.h"
#include "swoole_rs_native_cpp_scope.h"
#include <memory>
#include <stdexcept>
#ifdef ZTS
ZEND_TSRMLS_CACHE_DEFINE()
#endif

extern "C" int swr_exit_guard_test_owner(void (*)(void *), void *, void (*)(const unsigned char *, size_t));
static const swoole_rs::NativeCppRuntime runtime{
    swr_zend_guard_context, swr_zend_guard_pending, swr_zend_guard_mark_cpp, swr_zend_guard_clear};
struct Frame;
static thread_local Frame *frames;
struct Frame {
    swoole_rs::NativeCppScope scope{runtime};
    void *context = swr_zend_guard_context();
    Frame *previous = frames;
    size_t owners = 0, bytes = 0, checksum = 0, tokens = 0, token_owner_count = 0, publications = 0;
    bool control, caught = false, linked = true;
    explicit Frame(bool control) : control(control) { frames = this; }
    void leave() {
        if (!linked) return;
        scope.leave();
        auto **slot = &frames;
        while (*slot && *slot != this) slot = &(*slot)->previous;
        if (!*slot) std::abort();
        *slot = previous;
        linked = false;
    }
    ~Frame() { leave(); }
};
static Frame *current_frame() {
    auto *context = swr_zend_guard_context();
    for (auto *frame = frames; frame; frame = frame->previous) {
        if (frame->context == context) return frame;
    }
    std::abort();
}
struct Token {
    Frame *owner;
    explicit Token(Frame *owner) : owner(owner) {}
    ~Token() { ++owner->tokens; owner->token_owner_count = owner->owners; }
};
struct Failure : std::runtime_error {
    std::shared_ptr<Token> token;
    explicit Failure(Frame *frame) : std::runtime_error("native captured failure"), token(std::make_shared<Token>(frame)) {}
};
static void dropped(const unsigned char *data, size_t length) {
    auto *frame = current_frame();
    ++frame->owners;
    frame->bytes += length;
    for (size_t i = 0; i < length; ++i) frame->checksum += data[i];
}
static void capture_failure(void *) {
    auto *frame = current_frame();
    auto *scope = swoole_rs::NativeCppScope::current(runtime);
    if (!scope) std::abort();
    scope->capture([&]() { throw Failure(frame); });
    zend_bailout();
}
PHP_FUNCTION(cpp_native_catch) {
    ZEND_PARSE_PARAMETERS_NONE();
    auto *frame = current_frame();
    if (frame->control) {
        int relay = 0;
        try {
            // This native entry owns only its completed inner Rust boundary.
            swoole_rs::NativeCppScope inner{runtime};
            int outcome = swr_exit_guard_test_owner(capture_failure, nullptr, dropped);
            auto exception = inner.take();
            inner.leave();
            if (exception) std::rethrow_exception(exception);
            relay = outcome;
        } catch (const Failure &) {
            frame->caught = true;
        }
        // No native custody object remains live across a nonlocal resume.
        if (relay) swr_zend_guard_resume();
    } else {
        zend_try {
            int outcome = swr_exit_guard_test_owner(capture_failure, nullptr, dropped);
            if (outcome) swr_zend_guard_resume();
        } zend_catch {
            // A plain native Zend catch cannot acknowledge C++ custody.
        } zend_end_try();
    }
    RETURN_NULL();
}
PHP_FUNCTION(cpp_publish) {
    ZEND_PARSE_PARAMETERS_NONE();
    ++current_frame()->publications;
    RETURN_NULL();
}
struct Call { zval *callback; zval result{}; };
static void invoke(void *argument) {
    auto *call = static_cast<Call *>(argument);
    call_user_function(EG(function_table), nullptr, call->callback, &call->result, 0, nullptr);
    zval_ptr_dtor(&call->result);
}
PHP_FUNCTION(cpp_chain) {
    zval *callback;
    bool control = false;
    ZEND_PARSE_PARAMETERS_START(1, 2)
        Z_PARAM_ZVAL(callback)
        Z_PARAM_OPTIONAL
        Z_PARAM_BOOL(control)
    ZEND_PARSE_PARAMETERS_END();
    zend_long values[7]{};
    bool caught;
    {
        auto *original = EG(bailout);
        auto *target = swr_zend_guard_target();
        auto *previous = swoole_rs::NativeCppScope::current(runtime);
        Frame frame{control};
        Call call{callback};
        int outcome = swr_exit_guard_test_owner(invoke, &call, dropped);
        bool cleaned = frame.owners == 2 && frame.bytes == 8192 && frame.checksum == 8192 * 42 &&
            EG(bailout) == original && swr_zend_guard_target() == target;
        try {
            auto exception = frame.scope.take();
            if (exception) std::rethrow_exception(exception);
        } catch (const Failure &failure) {
            frame.caught = std::string(failure.what()) == "native captured failure";
        }
        values[0] = outcome;
        values[1] = frame.publications;
        values[2] = frame.owners;
        values[3] = frame.bytes;
        values[4] = frame.checksum;
        values[5] = frame.tokens;
        values[6] = frame.token_owner_count;
        frame.leave();
        caught = cleaned && frame.caught && swoole_rs::NativeCppScope::current(runtime) == previous &&
            swr_zend_guard_pending() == 0;
    }
    // Custody and registry nodes have ended before PHP result allocation.
    array_init(return_value);
    for (auto value : values) add_next_index_long(return_value, value);
    add_next_index_bool(return_value, caught);
}
ZEND_BEGIN_ARG_INFO_EX(arginfo_none, 0, 0, 0)
ZEND_END_ARG_INFO()
ZEND_BEGIN_ARG_INFO_EX(arginfo_chain, 0, 0, 1)
    ZEND_ARG_INFO(0, callback)
    ZEND_ARG_INFO(0, native_catch)
ZEND_END_ARG_INFO()
static const zend_function_entry functions[] = {
    PHP_FE(cpp_native_catch, arginfo_none)
    PHP_FE(cpp_publish, arginfo_none)
    PHP_FE(cpp_chain, arginfo_chain)
    PHP_FE_END
};
static PHP_RINIT_FUNCTION(swr_cpp_guard_test) {
#ifdef ZTS
    ZEND_TSRMLS_CACHE_UPDATE();
#endif
    return SUCCESS;
}
extern "C" {
zend_module_entry swr_cpp_guard_test_module_entry = {
    STANDARD_MODULE_HEADER, "swr_cpp_guard_test", functions, nullptr, nullptr,
    PHP_RINIT(swr_cpp_guard_test), nullptr, nullptr, "1", STANDARD_MODULE_PROPERTIES
};
ZEND_GET_MODULE(swr_cpp_guard_test)
}

#include "php.h"
#include "swoole_rs_zend_guard.h"
#include <exception>
#include <memory>
#include <stdexcept>
#if defined(__GLIBCXX__)
#include <cxxabi.h>
#endif
#ifdef ZTS
ZEND_TSRMLS_CACHE_DEFINE()
#endif

extern "C" int swr_exit_guard_test_owner(void (*)(void *), void *, void (*)(const unsigned char *, size_t));
static size_t owners, bytes, checksum, tokens;
struct Token { ~Token() { ++tokens; } };
struct Failure : std::runtime_error {
    std::shared_ptr<Token> token = std::make_shared<Token>();
    Failure() : std::runtime_error("native captured failure") {}
};
struct Frame { std::exception_ptr failure; size_t publications = 0; };
static thread_local Frame *active;
static void dropped(const unsigned char *data, size_t length) {
    ++owners;
    bytes += length;
    for (size_t i = 0; i < length; ++i) checksum += data[i];
}
static void capture_failure(void *) {
    try { throw Failure();
#if defined(__GLIBCXX__)
    } catch (const __cxxabiv1::__forced_unwind &) { throw;
#endif
    } catch (...) { active->failure = std::current_exception(); }
    // The native holder outlives both Rust owners. A plain native Zend catch
    // cannot acknowledge this custody or allow PHP publication afterward.
    swr_zend_guard_mark_cpp();
    zend_bailout();
}
PHP_FUNCTION(cpp_native_catch) {
    ZEND_PARSE_PARAMETERS_NONE();
    zend_try {
        int outcome = swr_exit_guard_test_owner(capture_failure, nullptr, dropped);
        if (outcome) swr_zend_guard_resume();
    } zend_catch {
        // A native PHP leaf can legitimately contain an ordinary Zend bailout.
    } zend_end_try();
    RETURN_NULL();
}
PHP_FUNCTION(cpp_publish) {
    ZEND_PARSE_PARAMETERS_NONE();
    ++active->publications;
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
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_ZVAL(callback)
    ZEND_PARSE_PARAMETERS_END();
    Frame frame;
    auto *previous = active;
    auto *original = EG(bailout);
    active = &frame;
    Call call{callback};
    int outcome = swr_exit_guard_test_owner(invoke, &call, dropped);
    active = previous;
    bool cleaned = owners == 2 && bytes == 8192 && checksum == 8192 * 42 &&
        EG(bailout) == original && swr_zend_guard_target() == nullptr;
    swr_zend_guard_clear();
    bool caught = false;
    try { if (frame.failure) std::rethrow_exception(frame.failure); }
    catch (const Failure &failure) { caught = cleaned && std::string(failure.what()) == "native captured failure"; }
    frame.failure = nullptr;
    array_init(return_value);
    add_next_index_long(return_value, outcome);
    add_next_index_long(return_value, frame.publications);
    add_next_index_long(return_value, owners);
    add_next_index_long(return_value, bytes);
    add_next_index_long(return_value, checksum);
    add_next_index_long(return_value, tokens);
    add_next_index_bool(return_value, caught);
}
ZEND_BEGIN_ARG_INFO_EX(arginfo_none, 0, 0, 0)
ZEND_END_ARG_INFO()
ZEND_BEGIN_ARG_INFO_EX(arginfo_chain, 0, 0, 1)
    ZEND_ARG_INFO(0, callback)
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

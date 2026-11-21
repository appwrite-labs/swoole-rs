#include "php.h"
#include "swoole_rs_zend_guard.h"
#include <atomic>
#include <exception>
#include <stdexcept>
#ifdef ZTS
ZEND_TSRMLS_CACHE_DEFINE()
#endif

// The guard ABI as the Cargo-built swoole.so exports it (crates/ext/src/lib.rs).
extern "C" int swr_v1_zend_try(void (*body)(void *), void *argument);
extern "C" void *swr_v1_zend_guard_target(void);
extern "C" void swr_v1_zend_guard_resume(void);
extern "C" int swr_exit_guard_test_owner(void (*body)(void *), void *, void (*dropped)(const unsigned char *, size_t));
static std::atomic<size_t> entered{0}, dropped{0}, returned{0}, bytes{0}, checksum{0}, zend_bailouts{0}, thread_exits{0};
struct Call { zval *callback; zval result{}; };
static void invoke(void *argument) {
    auto *call = static_cast<Call *>(argument);
    call_user_function(EG(function_table), nullptr, call->callback, &call->result, 0, nullptr);
    zval_ptr_dtor(&call->result);
}
static void owner_dropped(const unsigned char *data, size_t length) {
    size_t sum = 0;
    for (size_t i = 0; i < length; ++i) sum += data[i];
    checksum.fetch_add(sum);
    dropped.fetch_add(1);
    bytes.fetch_add(length);
}
PHP_FUNCTION(swr_guard_owned) {
    zval *callback;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_ZVAL(callback)
    ZEND_PARSE_PARAMETERS_END();
    Call call{callback};
    entered.fetch_add(1);
    int outcome = swr_exit_guard_test_owner(invoke, &call, owner_dropped);
    returned.fetch_add(1);
    if (outcome == SWR_ZEND_BAILOUT) zend_bailouts.fetch_add(1);
    else if (outcome == SWR_ZEND_THREAD_EXIT) thread_exits.fetch_add(1);
    else if (outcome != SWR_ZEND_RETURN) std::abort();
    if (outcome) swr_v1_zend_guard_resume();
    RETURN_NULL();
}
static void throw_native(void *) { throw std::runtime_error("native probe"); }
static void nested_native(void *) { swr_v1_zend_try(throw_native, nullptr); }
static void capture_native(void *argument) {
    auto *exception = static_cast<std::exception_ptr *>(argument);
    try { nested_native(nullptr); } catch (...) { *exception = std::current_exception(); }
}
PHP_FUNCTION(swr_guard_cpp_probe) {
    ZEND_PARSE_PARAMETERS_NONE();
    auto *original = EG(bailout);
    bool caught = false;
    try { swr_v1_zend_try(nested_native, nullptr); }
    catch (const std::runtime_error &error) { caught = std::string(error.what()) == "native probe"; }
    bool clean = caught && EG(bailout) == original && swr_v1_zend_guard_target() == nullptr;
    std::exception_ptr exception;
    entered.fetch_add(1);
    int outcome = swr_exit_guard_test_owner(capture_native, &exception, owner_dropped);
    returned.fetch_add(1);
    clean = clean && outcome == 0 && exception && EG(bailout) == original &&
        swr_v1_zend_guard_target() == nullptr && dropped.load() == 1 && bytes.load() == 4096 &&
        checksum.load() == 4096 * 42;
    caught = false;
    try { if (exception) std::rethrow_exception(exception); }
    catch (const std::runtime_error &error) { caught = std::string(error.what()) == "native probe"; }
    RETURN_BOOL(clean && caught);
}
PHP_FUNCTION(swr_guard_counts) {
    ZEND_PARSE_PARAMETERS_NONE();
    array_init(return_value);
    add_next_index_long(return_value, entered.load());
    add_next_index_long(return_value, dropped.load());
    add_next_index_long(return_value, returned.load());
    add_next_index_long(return_value, bytes.load());
    add_next_index_long(return_value, checksum.load());
    add_next_index_long(return_value, zend_bailouts.load());
    add_next_index_long(return_value, thread_exits.load());
}
ZEND_BEGIN_ARG_INFO_EX(arginfo_owned, 0, 0, 1)
    ZEND_ARG_INFO(0, callback)
ZEND_END_ARG_INFO()
ZEND_BEGIN_ARG_INFO_EX(arginfo_counts, 0, 0, 0)
ZEND_END_ARG_INFO()
static const zend_function_entry functions[] = {
    PHP_FE(swr_guard_owned, arginfo_owned)
    PHP_FE(swr_guard_counts, arginfo_counts)
    PHP_FE(swr_guard_cpp_probe, arginfo_counts)
    PHP_FE_END
};
static PHP_RINIT_FUNCTION(swr_exit_guard_test) {
#ifdef ZTS
    ZEND_TSRMLS_CACHE_UPDATE();
#endif
    return SUCCESS;
}
extern "C" {
zend_module_entry swr_exit_guard_test_module_entry = {
    STANDARD_MODULE_HEADER, "swr_exit_guard_test", functions, nullptr, nullptr,
    PHP_RINIT(swr_exit_guard_test), nullptr, nullptr, "1", STANDARD_MODULE_PROPERTIES
};
ZEND_GET_MODULE(swr_exit_guard_test)
}

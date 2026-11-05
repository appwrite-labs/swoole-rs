#include "php.h"
#include "swoole_rs_zend_guard.h"
#include <atomic>
#ifdef ZTS
ZEND_TSRMLS_CACHE_DEFINE()
#endif

extern "C" int swr_exit_guard_test_owner(void (*body)(void *), void *, void (*dropped)(size_t));
static std::atomic<size_t> entered{0}, dropped{0}, returned{0}, bytes{0};
struct Call { zval *callback; zval result{}; };
static void invoke(void *argument) {
    auto *call = static_cast<Call *>(argument);
    call_user_function(EG(function_table), nullptr, call->callback, &call->result, 0, nullptr);
    zval_ptr_dtor(&call->result);
}
static void owner_dropped(size_t length) {
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
    if (outcome) swr_zend_guard_resume();
    RETURN_NULL();
}
PHP_FUNCTION(swr_guard_counts) {
    ZEND_PARSE_PARAMETERS_NONE();
    array_init(return_value);
    add_next_index_long(return_value, entered.load());
    add_next_index_long(return_value, dropped.load());
    add_next_index_long(return_value, returned.load());
    add_next_index_long(return_value, bytes.load());
}
ZEND_BEGIN_ARG_INFO_EX(arginfo_owned, 0, 0, 1)
    ZEND_ARG_INFO(0, callback)
ZEND_END_ARG_INFO()
ZEND_BEGIN_ARG_INFO_EX(arginfo_counts, 0, 0, 0)
ZEND_END_ARG_INFO()
static const zend_function_entry functions[] = {
    PHP_FE(swr_guard_owned, arginfo_owned)
    PHP_FE(swr_guard_counts, arginfo_counts)
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

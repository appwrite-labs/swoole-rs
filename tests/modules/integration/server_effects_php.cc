#include "php.h"
#include "swoole_server.h"
#include "swoole_api.h"
#include "php_swoole_cxx.h"
#ifdef ZTS
ZEND_TSRMLS_CACHE_DEFINE()
#endif
static thread_local zval *callback;
static void on_shutdown(void *) {
    if (!callback) return;
    zval result;
    ZVAL_UNDEF(&result);
    call_user_function(EG(function_table), nullptr, callback, &result, 0, nullptr);
    if (Z_TYPE(result) == IS_OBJECT) {
        // The returned object has one owner when the native Variable facade
        // frees it. Its PHP destructor may exit through variable_free.
        zend::Variable owned(&result);
        zval_ptr_dtor(&result);
        ZVAL_UNDEF(&result);
    } else {
        zval_ptr_dtor(&result);
    }
}
PHP_FUNCTION(swr_actual_server_guard) {
    zval *function;
    ZEND_PARSE_PARAMETERS_START(1, 1)
        Z_PARAM_ZVAL(function)
    ZEND_PARSE_PARAMETERS_END();
    auto *server = new swoole::Server(swoole::Server::MODE_BASE);
    server->worker_num = 1;
    if (!server->add_port(SW_SOCK_TCP, "127.0.0.1", 0) || server->create() != SW_OK) {
        delete server;
        zend_throw_error(nullptr, "SDK Server preparation failed");
        RETURN_THROWS();
    }
    swoole_add_hook(SW_GLOBAL_HOOK_AFTER_SERVER_SHUTDOWN, on_shutdown, 0);
    callback = function;
    server->destroy();
    callback = nullptr;
    delete server;
    RETURN_NULL();
}
ZEND_BEGIN_ARG_INFO_EX(arginfo_guard, 0, 0, 1)
    ZEND_ARG_INFO(0, callback)
ZEND_END_ARG_INFO()
static const zend_function_entry functions[] = {
    PHP_FE(swr_actual_server_guard, arginfo_guard)
    PHP_FE_END
};
PHP_RINIT_FUNCTION(server_guard_probe) {
#ifdef ZTS
    ZEND_TSRMLS_CACHE_UPDATE();
#endif
    return SUCCESS;
}
static zend_module_entry module = {
    STANDARD_MODULE_HEADER, "server_guard_probe", functions, nullptr, nullptr,
    PHP_RINIT(server_guard_probe), nullptr, nullptr, "1", STANDARD_MODULE_PROPERTIES
};
extern "C" ZEND_DLEXPORT zend_module_entry *get_module() { return &module; }

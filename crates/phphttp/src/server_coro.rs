//! `Swoole\Coroutine\Http\Server`'s method table. The class and its handlers
//! stay C++ (`swoole_http_server_coro.cc`), which registers it with this table.
use swoole_rs_phpstubs::http_server_coro;
use swoole_rs_zend::{method_table, sys as z};

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_http_server_coro___construct, &http_server_coro::arginfo_class_Swoole_Coroutine_Http_Server___construct, z::ZEND_ACC_PUBLIC;
        c"set" => zim_swoole_http_server_coro_set, &http_server_coro::arginfo_class_Swoole_Coroutine_Http_Server_set, z::ZEND_ACC_PUBLIC;
        c"handle" => zim_swoole_http_server_coro_handle, &http_server_coro::arginfo_class_Swoole_Coroutine_Http_Server_handle, z::ZEND_ACC_PUBLIC;
        c"onAccept" => zim_swoole_http_server_coro_onAccept, &http_server_coro::arginfo_class_Swoole_Coroutine_Http_Server_onAccept, z::ZEND_ACC_PRIVATE;
        c"start" => zim_swoole_http_server_coro_start, &http_server_coro::arginfo_class_Swoole_Coroutine_Http_Server_start, z::ZEND_ACC_PUBLIC;
        c"shutdown" => zim_swoole_http_server_coro_shutdown, &http_server_coro::arginfo_class_Swoole_Coroutine_Http_Server_shutdown, z::ZEND_ACC_PUBLIC;
    ];
}

// The handlers, which stay C++.
unsafe extern "C" {
    fn zim_swoole_http_server_coro___construct(
        frame: *mut z::zend_execute_data,
        value: *mut z::zval,
    );
    fn zim_swoole_http_server_coro_handle(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zim_swoole_http_server_coro_onAccept(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zim_swoole_http_server_coro_set(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zim_swoole_http_server_coro_shutdown(frame: *mut z::zend_execute_data, value: *mut z::zval);
    fn zim_swoole_http_server_coro_start(frame: *mut z::zend_execute_data, value: *mut z::zval);
}

/// The method table `php_swoole_http_server_coro_minit` registers the class with.
#[unsafe(no_mangle)]
extern "C" fn swr_v1_phphttp_server_coro_methods() -> *const z::zend_function_entry {
    METHODS.as_ptr()
}

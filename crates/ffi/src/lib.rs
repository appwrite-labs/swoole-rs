//! The hybrid's Rust archive: every module crate, and `swoole_init` (src/init.rs).
//! The C ABI queries are crates/error-ffi's.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

mod init;

pub use swoole_rs_error::*;
pub use swoole_rs_error_ffi::*;

pub use swoole_rs_websocket::*;
pub use swoole_rs_websocket_ffi::*;

pub use swoole_rs_buffer::*;

pub use swoole_rs_buffer::string::*;

pub use swoole_rs_timer::*;

pub use swoole_rs_server::*;

pub use swoole_rs_protocols_ffi::*;

pub use swoole_rs_bus::swr_v1_bus_write;

pub use swoole_rs_memory::*;

pub use swoole_rs_http::*;
pub use swoole_rs_http_ffi::*;

pub use swoole_rs_utilities::*;

pub use swoole_rs_locks::*;

#[cfg(feature = "php")]
pub use swoole_rs_phplocks::*;

#[cfg(feature = "php")]
pub use swoole_rs_phptimer::*;

#[cfg(feature = "php")]
pub use swoole_rs_phpswoole::*;

#[cfg(feature = "php")]
pub use swoole_rs_phpredis::*;

pub use swoole_rs_reactor::*;

pub use swoole_rs_corebase::*;

pub use swoole_rs_coroutine::*;

pub use swoole_rs_hooks::*;
pub use swoole_rs_phpruntime as phpruntime;

pub use swoole_rs_ossupport::*;

// Reactor's Socket patch calls the TLS reads and writes in crates/tls.
pub use swoole_rs_tls::*;

pub use swoole_rs_process::*;

#[cfg(feature = "php")]
pub use swoole_rs_phpprocess::*;

#[cfg(feature = "php")]
pub use swoole_rs_phpthread::*;

pub use swoole_rs_clients as clients;
#[cfg(feature = "php")]
pub use swoole_rs_phpclients::*;

// Keep local Rust type names scoped; C exports retain their no_mangle names.
pub use swoole_rs_httpbindings as httpbindings;

#[cfg(feature = "php")]
pub use swoole_rs_phphttp::*;

pub use swoole_rs_http2 as http2;
#[cfg(feature = "php")]
pub use swoole_rs_phphttp2::*;

pub use swoole_rs_ports as ports;

#[cfg(feature = "php")]
pub use swoole_rs_phpserver::*;

pub use swoole_rs_phpstdext as phpstdext;

pub use swoole_rs_phpcollections as phpcollections;

pub use swoole_rs_phpevents as phpevents;

pub use swoole_rs_phpcoroutines as phpcoroutines;

#[cfg(feature = "pgsql")]
pub use swoole_rs_pgsql as pgsql;

#[cfg(feature = "odbc")]
pub use swoole_rs_odbc as odbc;

#[cfg(feature = "firebird")]
pub use swoole_rs_firebird as firebird;

#[cfg(feature = "sqlite")]
pub use swoole_rs_sqlite as sqlite;

#[cfg(feature = "oci")]
pub use swoole_rs_oci as oci;

#[cfg(feature = "curl")]
pub use swoole_rs_curl as curl;

#[cfg(feature = "ssh2")]
pub use swoole_rs_ssh2 as ssh2;

// crates/ssh2 doesn't depend on corebase, nor crates/phphttp on error: their copies are
// checked here.
#[cfg(feature = "ssh2")]
const _: () = assert!(
    ssh2::READ == swoole_rs_corebase::constants::EventType::SW_EVENT_READ as std::ffi::c_int
        && ssh2::WRITE
            == swoole_rs_corebase::constants::EventType::SW_EVENT_WRITE as std::ffi::c_int,
    "crates/ssh2's event bits"
);
#[cfg(feature = "php")]
const _: () = assert!(
    swoole_rs_phphttp::SSL_BAD_CLIENT == ErrorCode::SW_ERROR_SSL_BAD_CLIENT as std::ffi::c_int,
    "crates/phphttp's SW_ERROR_SSL_BAD_CLIENT"
);

/// crates/ext's `get_module()` entry: phpswoole's, whose phases run the crates
/// above it, which only this one links together.
#[cfg(feature = "php")]
#[must_use]
pub fn phpswoole_module() -> *mut std::ffi::c_void {
    swoole_rs_phpserver::register_http(&HTTP_HOOKS);
    swoole_rs_phpswoole::module_entry(&FAMILIES).cast()
}

#[cfg(feature = "php")]
static HTTP_HOOKS: swoole_rs_phpserver::HttpHooks = swoole_rs_phpserver::HttpHooks {
    on_receive: swoole_rs_phphttp::http_server_on_receive,
    on_close: swoole_rs_phphttp::http_server_on_close,
    can_cache_session_addr: swoole_rs_phphttp::can_cache_session_addr,
};

/// `php_swoole_reactor_init()`: 0, or -1 after an error (crates/phpevents).
#[cfg(feature = "php")]
#[unsafe(no_mangle)]
pub extern "C" fn php_swoole_reactor_init() -> i32 {
    swoole_rs_phpevents::reactor_init()
}

/// `php_swoole_convert_to_fd(value)` (crates/phpevents): C lends the zval
/// as a `Value`, which wraps it transparently.
#[cfg(feature = "php")]
#[unsafe(no_mangle)]
pub extern "C" fn php_swoole_convert_to_fd(value: Option<&swoole_rs_zend::Value>) -> i32 {
    swoole_rs_phpevents::convert_to_fd(value)
}

/// `Swoole\Event::wait()`'s native lifecycle effect (crates/phpevents).
#[cfg(feature = "php")]
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpevents_wait() {
    swoole_rs_phpevents::wait_and_raise();
}

#[cfg(feature = "php")]
static FAMILIES: swoole_rs_phpswoole::Families = {
    use swoole_rs_phpclients as clients;
    use swoole_rs_phpcoroutines as co;
    use swoole_rs_phphttp as http;
    use swoole_rs_phpthread as thread;
    swoole_rs_phpswoole::Families {
        coroutine_minit: [
            co::php_swoole_coroutine_minit,
            co::php_swoole_coroutine_system_minit,
            co::php_swoole_coroutine_scheduler_minit,
        ],
        server_minit: swoole_rs_phpserver::php_swoole_server_minit,
        locks_minit: [
            swoole_rs_phplocks::atomic_minit,
            swoole_rs_phplocks::lock_minit,
            swoole_rs_phplocks::coroutine_lock_minit,
            swoole_rs_phplocks::thread_locks_minit,
        ],
        event_minit: swoole_rs_phpevents::minit,
        collections_minit: [
            swoole_rs_phpcollections::table_minit,
            swoole_rs_phpcollections::channel_minit,
        ],
        timer_minit: swoole_rs_phptimer::minit,
        timer_rshutdown: swoole_rs_phptimer::rshutdown,
        redis_minit: swoole_rs_phpredis::redis_minit,
        redis_rshutdown: swoole_rs_phpredis::redis_rshutdown,
        tracer_minit: co::tracer_minit,
        thread_minit: thread::thread_minit,
        thread_classes_minit: [
            thread::barrier_minit,
            thread::queue_minit,
            thread::map_minit,
            thread::arraylist_minit,
        ],
        coroutine_rinit: co::php_swoole_coroutine_rinit,
        thread_rinit: thread::thread_rinit,
        tracer_rinit: co::tracer_rinit,
        server_rshutdown: swoole_rs_phpserver::php_swoole_server_rshutdown,
        server_mshutdown: swoole_rs_phpserver::php_swoole_server_mshutdown,
        coroutine_rshutdown: [
            co::php_swoole_coroutine_rshutdown,
            co::php_swoole_coroutine_scheduler_rshutdown,
        ],
        thread_rshutdown: thread::thread_rshutdown,
        tracer_rshutdown: co::tracer_rshutdown,
        thread_bailout: thread::thread_bailout,
        print_backtrace: co::print_coroutine_backtrace,
        hook_flags: co::hook_flags,
        set_hook_flags: co::set_hook_flags,
        active_threads: thread::sw_active_thread_count,
        server_class: swoole_rs_phpserver::server_class_entry,
        client_minit: [
            clients::socket_minit,
            clients::php_swoole_client_minit,
            clients::php_swoole_client_async_minit,
            clients::client_coro_minit,
            clients::http_client_minit,
        ],
        name_resolver_minit: clients::php_swoole_name_resolver_minit,
        socket_settings: clients::socket_settings,
        name_resolver_add: clients::php_swoole_name_resolver_add,
        name_resolver_remove: clients::swoole_name_resolver_php_remove,
        socket_class: clients::socket,
        client_class: clients::client,
        http: swoole_rs_phpswoole::Http {
            minit: [
                http::request_minit,
                http::response_minit,
                http::php_swoole_http_cookie_minit,
                http::server_minit,
                http::server_coro_minit,
                http::websocket_minit,
                swoole_rs_phphttp2::client_minit,
            ],
            server_rinit: http::server_rinit,
            server_rshutdown: http::server_rshutdown,
            response_rshutdown: http::response_rshutdown,
            http2_on_close: swoole_rs_phphttp2::server_on_close,
            server_class: http::server_class,
            websocket_class: http::websocket_class,
        },
    }
};

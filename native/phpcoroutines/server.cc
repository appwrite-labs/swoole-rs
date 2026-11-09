// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 ext-src/swoole_server.cc (Swoole contributors).
// What crates/phpserver needs of C++ that stable Rust can't express: the build's #ifdefs
// (sockets, thread mode, the features set() checks), the C++ objects a Server holds
// (`new`/`delete Server`, its MessageBus), the variadic PHP error reporters and the
// zend_try frames around calls that bail out (start(), addProcess's worker).
#include "swoole_rs_phphost.h"
#include "swoole_server.h"
#include "thirdparty/llhttp/llhttp.h"
#include "swoole_rs_phpserver_server.h"
#include "swoole_rs_phpserver_settings.h"
#include "swoole_rs_phpserver_dispatch.h"
#include "swoole_rs_phpserver_lifecycle.h"
#include "swoole_rs_phpserver.h"
#include "swoole_rs_phpprocess.h"
#include "swoole_rs_phpruntime.h"
#include "swoole_rs.h"

using namespace swoole;

zend_class_entry *swoole_server_ce;
zend_class_entry *swoole_server_port_ce;
zend_object_handlers swoole_server_handlers;

namespace {
Server *as_server(void *server) {
    return static_cast<Server *>(server);
}
ListenPort *as_port(void *port) {
    return static_cast<ListenPort *>(port);
}

void server_invalid(zend_object *object) {
    php_swoole_fatal_error(E_ERROR, "Invalid instance of %s", ZSTR_VAL(object->ce->name));
}
void port_socket(void *port, zval *return_value) {
#ifdef SWOOLE_SOCKETS_SUPPORT
    int fd = dup(as_port(port)->get_fd());
    if (fd < 0) {
        RETURN_FALSE;
    }
    // A Socket whatever the import's result, as upstream's php_swoole_convert_to_socket.
    swr_cpp_phpthread_php_socket_create(return_value, fd);
    zval *zsocket = sw_zval_dup(return_value);
    Z_TRY_ADDREF_P(zsocket);
#else
    (void) port;
    RETURN_FALSE;
#endif
}

void *dispatch_message_bus(void *server) {
    return as_server(server)->get_worker_message_bus();
}
void dispatch_worker_start(void *server) {
    Server *serv = as_server(server);
    if (serv->is_task_worker() && !serv->task_enable_coroutine && !serv->is_thread_mode()) {
        swr_v1_phpruntime_enable_hook(0);
    }
    serv->get_worker_message_bus()->set_allocator(sw_zend_string_allocator());
}

int64_t settings_get(void *, uint32_t id) {
    switch (id) {
#if defined(__linux__) and defined(HAVE_REUSEPORT)
    case SWR_PHPSERVER_SET_HAVE_REUSE_PORT: return 1;
#endif
#ifdef SW_HAVE_COMPRESSION
    case SWR_PHPSERVER_SET_HAVE_COMPRESSION: return 1;
#endif
#ifdef SW_HAVE_ZLIB
    case SWR_PHPSERVER_SET_HAVE_ZLIB: return 1;
#endif
#ifdef HAVE_MSGQUEUE
    case SWR_PHPSERVER_SET_HAVE_MSGQUEUE: return 1;
#endif
#ifdef SW_THREAD
    case SWR_PHPSERVER_SET_THREAD: return 1;
#endif
    default: return 0;
    }
}

void lifecycle_install(void *server, uint32_t which) {
    Server *serv = as_server(server);
    switch (which) {
    case SWR_PHPSERVER_INSTALL_ALLOCATORS:
        serv->message_bus.set_allocator(sw_zend_string_allocator());
        if (serv->is_base_mode() || serv->is_thread_mode()) {
            serv->recv_buffer_allocator = sw_zend_string_allocator();
        }
        break;
    case SWR_PHPSERVER_INSTALL_ADMIN_COMMANDS:
        swr_v1_server_admin_register(serv, [] { return zend_memory_usage(true); });
        break;
    default: break;
    }
}
void *lifecycle_create(int64_t mode) {
    auto *serv = new Server((enum Server::Mode) mode);
    if (mode == Server::MODE_BASE) {
        serv->reactor_num = 1;
        serv->worker_num = 1;
    }
    return serv;
}
int lifecycle_start(void *server, const char **error) {
    Server *serv = as_server(server);
    int result = 0;
    zend_try {
        if (serv->start() < 0) {
            *error = serv->get_startup_error_message();
            result = -1;
        }
    } zend_catch {
        result = 1;
    } zend_end_try();
    return result;
}
void lifecycle_release(void *server) {
    delete as_server(server);
}
int64_t lifecycle_add_process(void *server, zval *process) {
    Server *serv = as_server(server);
    if (serv->is_worker_thread()) {
        if (!serv->is_user_worker()) {
            return -2;
        }
        int worker_id = swoole_get_worker_id();
        Worker *worker = serv->get_worker(worker_id);
        worker->redirect_stdin = worker->redirect_stdout = worker->redirect_stderr = false;
        return worker_id - serv->get_core_worker_num();
    }
    int64_t worker_id = -3;
    zend_try {
        auto *worker = static_cast<Worker *>(swr_v1_phpprocess_worker(process, true));
        worker_id = serv->add_worker(worker);
        if (worker_id >= 0) {
            worker->ptr = process;
        } else {
            worker_id = -1;
        }
    } zend_catch {
        worker_id = -3;
    } zend_end_try();
    return worker_id;
}
}  // namespace

// The layouts and constants crates/phpserver mirrors.
static_assert(sizeof(EventData) <= 8192 && alignof(EventData) <= 8,
              "the EventData crates/phpserver's task.rs allocates");
static_assert(offsetof(RecvData, data) == sizeof(DataHead) && offsetof(DgramPacket, socket_addr) == 4 &&
                  offsetof(DgramPacket, length) == 4 + sizeof(network::Address) && sizeof(ExitStatus) == 2 * sizeof(int),
              "the event layouts crates/phpserver's slots read");
static_assert(offsetof(swoole::String, length) == 0 && offsetof(swoole::String, str) == 3 * sizeof(size_t),
              "the swoole::String head crates/phpserver reads");
// The constants crates/phphttp mirrors.
static_assert(HTTP_DELETE == 0 && HTTP_POST == 3 && HTTP_PUT == 4 && HTTP_PATCH == 28, "Pinned llhttp method mapping changed");
static_assert(SW_LOG_TRACE == 1 && SW_LOG_NOTICE == 3 && SW_LOG_WARNING == 4, "crates/phphttp/src/parse.rs log levels");
static_assert(SW_ERROR_SESSION_CLOSED == 1004 && SW_ERROR_SERVER_INVALID_REQUEST == 9010,
              "crates/phphttp/src/parse.rs error codes");
static_assert(SW_HTTP_FORM_KEYLEN == 512 && SW_HTTP_UPLOAD_TMPDIR_SIZE == 256 && SW_STACK_BUFFER_SIZE == 65536,
              "crates/phphttp/src/parse.rs limits");
static_assert(SW_ERROR_PHP_FATAL_ERROR == 503 && SW_ERROR_PHP_RUNTIME_NOTICE == 510 && SW_ERROR_NO_PAYLOAD == 550 &&
                  SW_ERROR_HTTP_CONTEXT_UNAVAILABLE == 7106 && SW_ERROR_SERVER_IS_NOT_REGULAR_FILE == 9013 &&
                  SW_HTTP_HEADER_KEY_SIZE == 128 && SW_HTTP_MAX_APPEND_DATA == 16384,
              "crates/phphttp/src/host.rs codes");
static_assert(PHP_SWOOLE_HOOK_AFTER_RESPONSE == 27, "crates/phphttp/src/response.rs hook");
static_assert(Server::MODE_BASE == 1 && Server::MODE_PROCESS == 2 && Server::MODE_THREAD == 3 &&
                  Server::TASK_IPC_UNIXSOCK == 1 && Server::TASK_IPC_MSGQUEUE == 2 &&
                  Server::TASK_IPC_PREEMPTIVE == 3,
              "server modes");
static_assert(Server::DISPATCH_ROUND == 1 && Server::DISPATCH_FDMOD == 2 && Server::DISPATCH_IDLE_WORKER == 3 &&
                  Server::DISPATCH_IPMOD == 4 && Server::DISPATCH_UIDMOD == 5 && Server::DISPATCH_USERFUNC == 6 &&
                  Server::DISPATCH_CO_CONN_LB == 8 && Server::DISPATCH_CO_REQ_LB == 9 &&
                  Server::DISPATCH_CONCURRENT_LB == 10 && Server::DISPATCH_RESULT_DISCARD_PACKET == -1 &&
                  Server::DISPATCH_RESULT_CLOSE_CONNECTION == -2 && Server::DISPATCH_RESULT_USERFUNC_FALLBACK == -3,
              "dispatch modes");
static_assert(SW_TASK_TMPFILE == 1 && SW_TASK_SERIALIZE == 1 << 1 && SW_TASK_NONBLOCK == 1 << 2 &&
                  SW_TASK_CALLBACK == 1 << 3 && SW_TASK_WAITALL == 1 << 4 && SW_TASK_COROUTINE == 1 << 5 &&
                  SW_TASK_PEEK == 1 << 6 && SW_TASK_NOREPLY == 1 << 7 && SW_MAX_CONCURRENT_TASK == 1024 &&
                  SW_WORKER_BUSY == 1 && SW_WORKER_IDLE == 2 && SW_WORKER_EXIT == 3,
              "task flags and worker states");
static_assert(SW_ERROR_PHP_FATAL_ERROR == 503 && SW_ERROR_INVALID_PARAMS == 505 &&
                  SW_ERROR_OPERATION_NOT_SUPPORT == 507 && SW_ERROR_PHP_RUNTIME_NOTICE == 510 &&
                  SW_ERROR_NO_PAYLOAD == 550 && SW_ERROR_SESSION_NOT_EXIST == 1005 &&
                  SW_ERROR_OUTPUT_SEND_YIELD == 1010 && SW_ERROR_TASK_DISPATCH_FAIL == 2002 &&
                  SW_ERROR_SERVER_UNRELATED_THREAD == 9016 && SW_MAX_FIND_COUNT == 100 &&
                  SW_WORKER_MIN_REQUEST == 10 && SW_SOCK_TCP == 1 && SW_SERVER_EVENT_PAUSE_RECV == 11 &&
                  SW_SERVER_EVENT_RESUME_RECV == 12,
              "error codes and constants crates/phpserver uses");

void php_swoole_server_minit(int module_number) {
#ifdef SW_THREAD
    const bool thread = true;
#else
    const bool thread = false;
#endif
#ifdef SWOOLE_SOCKETS_SUPPORT
    const bool sockets = true;
#else
    const bool sockets = false;
#endif
    static const swr_v1_phpserver_host host = {server_invalid};
    const zend_object_handlers *handlers;
    swoole_server_ce = swr_v1_phpserver_minit(&host, thread, sockets, module_number, &handlers);
    memcpy(&swoole_server_handlers, handlers, sizeof(zend_object_handlers));
    static const swr_v1_phpserver_dispatch_host dispatch = {dispatch_message_bus, dispatch_worker_start};
    swr_v1_phpserver_dispatch_init(&dispatch);
    static const swr_v1_phpserver_settings_host settings = {settings_get};
    swr_v1_phpserver_settings_init(&settings);
    static const swr_v1_phpserver_lifecycle_host lifecycle = {
        lifecycle_install, lifecycle_create, lifecycle_start, lifecycle_release, lifecycle_add_process,
    };
    swr_v1_phpserver_lifecycle_init(&lifecycle);
    static const swr_v1_phpserver_port_host port = {server_invalid, port_socket};
    swoole_server_port_ce = swr_v1_phpserver_port_minit(&port, !thread, sockets);
}

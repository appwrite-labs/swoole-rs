// SPDX-License-Identifier: Apache-2.0
// What crates/phpclients' Swoole\Client and Swoole\Async\Client need from C++
// (swoole_rs_phpclients.h): ext/sockets' php_socket, the PHP error macros, and the
// bailout::trap body that calls into network::Client (crates/clients/src/client.rs).
#include "swoole_rs_phphost.h"
#include "swoole_rs_clients.h"
#include "swoole_rs_clients_record.h"
#include "swoole_rs_phpclients.h"
#include "swoole_proxy.h"

#include <string>

static_assert(SW_SOCKS5_STATE_HANDSHAKE == 1 && SW_SOCKS5_STATE_READY == 4 && SW_HTTP_PROXY_STATE_READY == 2,
              "crates/clients/src/client.rs's proxy states");

using swoole::PacketLength;

// The client's state starts with its public record.
static swr_v1_clients_record *record_of(void *cli) {
    return static_cast<swr_v1_clients_record *>(cli);
}

void swoole_client_php_error(int kind, int level, const char *message) {
    switch (kind) {
    case SWOOLE_CLIENT_ERROR_SYS:
        php_swoole_sys_error(level, "%s", message);
        break;
    case SWOOLE_CLIENT_ERROR_CORE:
        php_swoole_core_error(level, "%s", message);
        break;
    default:
        swoole_rs_php_error(level, kind == SWOOLE_CLIENT_ERROR_FATAL, message);
        break;
    }
}

bool swoole_client_export_socket(void *cli, zval *zsocket) {
#ifdef SWOOLE_SOCKETS_SUPPORT
    int fd = dup(record_of(cli)->socket->fd);
    if (fd < 0) {
        return false;
    }
    // A Socket whatever the import's result, as upstream's php_swoole_convert_to_socket.
    swr_cpp_phpthread_php_socket_create(zsocket, fd);
    return true;
#else
    return false;
#endif
}

void swoole_client_run(void *op) {
    auto *call = static_cast<struct swoole_client_call *>(op);
    auto *cli = static_cast<swr_v1_clients *>(call->cli);
    switch (call->op) {
    case SWOOLE_CLIENT_CONNECT:
        call->result = swr_v1_clients_connect(cli, call->data, call->a, call->timeout, call->b);
        break;
    case SWOOLE_CLIENT_SEND:
        call->result = swr_v1_clients_send(cli, call->data, call->length, call->a);
        break;
    case SWOOLE_CLIENT_SENDTO: {
        // The host is NUL-terminated for the resolver.
        std::string host(call->data, call->length);
        call->result =
            swr_v1_clients_sendto(cli, host.c_str(), host.length(), call->a, call->buffer, call->b, [](int) {
                swoole_warning("only supports SWOOLE_SOCK_(UDP/UDP6/UNIX_DGRAM)");
            });
        break;
    }
    case SWOOLE_CLIENT_SENDFILE:
        call->result = swr_v1_clients_sendfile(cli, call->data, call->a, call->b);
        break;
    case SWOOLE_CLIENT_RECV:
        call->result = swr_v1_clients_recv(cli, call->buffer, call->length, call->a);
        break;
    case SWOOLE_CLIENT_PACKAGE_LENGTH: {
        auto *record = record_of(cli);
        PacketLength pl{call->buffer, (uint32_t) call->length};
        call->result = record->protocol.get_package_length(&record->protocol, record->socket, &pl);
        break;
    }
    case SWOOLE_CLIENT_SHUTDOWN:
        call->result = swr_v1_clients_shutdown(cli, call->a);
        break;
    case SWOOLE_CLIENT_CLOSE:
        call->result = swr_v1_clients_close(cli);
        break;
    case SWOOLE_CLIENT_SLEEP:
        call->result = swr_v1_clients_sleep(cli);
        break;
    case SWOOLE_CLIENT_WAKEUP:
        call->result = swr_v1_clients_wakeup(cli);
        break;
    case SWOOLE_CLIENT_ENABLE_SSL:
        call->result = swr_v1_clients_enable_ssl(cli);
        break;
    case SWOOLE_CLIENT_SSL_HANDSHAKE:
        call->result = swr_v1_clients_ssl_handshake(cli);
        break;
    case SWOOLE_CLIENT_SSL_VERIFY:
        call->result = swr_v1_clients_ssl_verify(cli, call->a);
        break;
    case SWOOLE_CLIENT_SSL_WAIT:
        call->result = swoole_event_set(record_of(cli)->socket, SW_EVENT_WRITE);
        break;
    case SWOOLE_CLIENT_DESTROY: {
        // The client's destruction deletes a pending connect timer.
        auto *record = record_of(cli);
        if (record->protocol.private_data_1) {
            sw_callable_free(record->protocol.private_data_1);
            record->protocol.private_data_1 = nullptr;
        }
        swr_v1_clients_free(cli);
        break;
    }
    case SWOOLE_CLIENT_CALLABLE:
        call->buffer = reinterpret_cast<char *>(sw_callable_create(const_cast<zval *>(call->settings)));
        break;
    case SWOOLE_CLIENT_CALLABLE_FREE:
        sw_callable_free(call->cli);
        break;
    case SWOOLE_CLIENT_CONVERT_FD:
        call->result = php_swoole_convert_to_fd(const_cast<zval *>(call->settings));
        break;
    case SWOOLE_CLIENT_INVOKE: {
        auto *cb = static_cast<zend::Callable *>(call->cli);
        auto *retval = reinterpret_cast<zval *>(call->buffer);
        call->result =
            sw_zend_call_function_ex2(nullptr, cb->ptr(), call->a, const_cast<zval *>(call->settings), retval) == SUCCESS;
        break;
    }
    }
}

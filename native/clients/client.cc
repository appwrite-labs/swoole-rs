// SPDX-License-Identifier: Apache-2.0
// What crates/phpclients' Swoole\Client and Swoole\Async\Client need from C++
// (swoole_rs_phpclients.h): ext/sockets' php_socket, the PHP error macros, and the
// bailout::trap body that calls into network::Client (network_client.cc has the rest).
#include "swoole_rs_phphost.h"
#include "swoole_rs_clients.h"
#include "swoole_rs_phpclients.h"
#include "swoole_client.h"

#include <string>

using swoole::PacketLength;
using swoole::SocketType;
using swoole::network::Client;

static Client *client_of(void *cli) {
    return static_cast<Client *>(cli);
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
    int fd = dup(client_of(cli)->socket->fd);
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
    Client *cli = client_of(call->cli);
    switch (call->op) {
    case SWOOLE_CLIENT_CONNECT:
        call->result = cli->connect(call->data, call->a, call->timeout, call->b);
        break;
    case SWOOLE_CLIENT_SEND:
        call->result = cli->send(call->data, call->length, call->a);
        break;
    case SWOOLE_CLIENT_SENDTO:
        call->result = cli->sendto(std::string(call->data, call->length), call->a, call->buffer, call->b);
        break;
    case SWOOLE_CLIENT_SENDFILE:
        call->result = cli->sendfile(call->data, call->a, call->b);
        break;
    case SWOOLE_CLIENT_RECV:
        call->result = cli->recv(call->buffer, call->length, call->a);
        break;
    case SWOOLE_CLIENT_PACKAGE_LENGTH: {
        PacketLength pl{call->buffer, (uint32_t) call->length};
        call->result = cli->protocol.get_package_length(&cli->protocol, cli->socket, &pl);
        break;
    }
    case SWOOLE_CLIENT_SHUTDOWN:
        call->result = cli->shutdown(call->a);
        break;
    case SWOOLE_CLIENT_CLOSE:
        call->result = cli->close();
        break;
    case SWOOLE_CLIENT_SLEEP:
        call->result = cli->sleep();
        break;
    case SWOOLE_CLIENT_WAKEUP:
        call->result = cli->wakeup();
        break;
    case SWOOLE_CLIENT_ENABLE_SSL:
        call->result = cli->enable_ssl_encrypt();
        break;
    case SWOOLE_CLIENT_SSL_HANDSHAKE:
        call->result = cli->ssl_handshake();
        break;
    case SWOOLE_CLIENT_SSL_VERIFY:
        call->result = cli->ssl_verify(call->a);
        break;
    case SWOOLE_CLIENT_SSL_WAIT:
        call->result = swoole_event_set(cli->socket, SW_EVENT_WRITE);
        break;
    case SWOOLE_CLIENT_DESTROY:
        // The client's destruction deletes a pending connect timer.
        if (cli->protocol.private_data_1) {
            sw_callable_free(cli->protocol.private_data_1);
            cli->protocol.private_data_1 = nullptr;
        }
        swoole_client_free(cli);
        break;
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

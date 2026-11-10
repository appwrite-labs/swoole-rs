/* SPDX-License-Identifier: Apache-2.0 */
/* The network::Socket and IOVector members Rust can't express, inline at the end of
 * swoole_socket.h: C++ exceptions, std::string and std::vector. Everything they
 * call is Rust. */
#pragma once
#include "swoole_rs_reactor.h"
#include <algorithm>

namespace swoole {
namespace network {

inline swr_v1_reactor_iov *swr_check_iov(swr_v1_reactor_iov_prepared prepared) {
    if (prepared.status == -1) throw std::bad_array_new_length();
    if (prepared.status != 0) throw std::bad_alloc();
    return prepared.owner;
}
inline IOVector::IOVector(swr_v1_reactor_iov_prepared prepared)
    : owner_(swr_check_iov(prepared)), iov(prepared.fields->original), iov_iterator(prepared.fields->state.iterator),
      count(prepared.fields->count), remain_count(prepared.fields->state.remain_count),
      index(prepared.fields->state.index), offset_bytes(prepared.fields->state.offset_bytes) {}
inline IOVector::IOVector(const iovec *_iov, int _iovcnt) : IOVector(swr_v1_reactor_iov_prepare(_iov, _iovcnt)) {}

inline SocketType Socket::convert_to_type(std::string &host) {
    const auto plan = swr_v1_socket_host_type(reinterpret_cast<const uint8_t *>(host.data()), host.size());
    host.erase(0, plan.erase);
    return static_cast<SocketType>(plan.socket_type);
}

inline std::vector<std::string> Socket::ssl_get_peer_cert_chain(int limit) const {
    std::vector<std::string> list;
    STACK_OF(X509) *chain = ssl_get_peer_cert_chain();
    if (chain == nullptr) {
        return list;
    }
    int n = std::min(static_cast<int>(sk_X509_num(chain)), limit);
    SW_LOOP_N(n) {
        X509 *cert = sk_X509_value(chain, i);
        auto rv = swr_v1_socket_ssl_read_x509(cert, sw_tg_buffer()->str, sw_tg_buffer()->size, false);
        if (rv > 0) {
            list.emplace_back(sw_tg_buffer()->str, rv);
        }
    }
    return list;
}

}  // namespace network
}  // namespace swoole

/* Constants the Rust socket code mirrors. */
static_assert(SW_SENDFILE_CHUNK_SIZE == 65536, "crates/reactor/src/socket_io.rs");
static_assert(SW_SOCK_NONBLOCK == 1 << 2 && SW_SOCK_CLOEXEC == 1 << 3 && SW_ERROR_SOCKET_POLL_TIMEOUT == 6002,
              "crates/reactor/src/socket_upstream.rs");
static_assert(SW_ERROR_SSL_BAD_CLIENT == 1015 && SW_ERROR_SSL_RESET == 1017 && SW_REDUCE_SIZE == 7,
              "crates/reactor/src/socket_rw.rs's catch_error codes");
static_assert(SW_WAIT == 2 && SW_ERROR == 4 && SW_READY == 5 && SW_OK == 0 && SW_ERR == -1,
              "crates/tls/src/socket.rs's codes");
static_assert(SW_LOG_NOTICE == 3 && SW_ERROR_SSL_VERIFY_FAILED == 1014 && SW_ERROR_SSL_BAD_PROTOCOL == 1016 &&
                  SW_CLOSE == 3 && SW_SOCKET_RETRY_COUNT == 10 && SW_EVENT_READ == 1 << 9 && SW_EVENT_WRITE == 1 << 10,
              "crates/tls/src/upstream.rs");
static_assert(SW_SSL_STATE_READY == 1 && SW_SSL_STATE_WAIT_STREAM == 2 && SW_ERROR_SSL_HANDSHAKE_FAILED == 1018,
              "crates/tls/src/socket.rs's states");

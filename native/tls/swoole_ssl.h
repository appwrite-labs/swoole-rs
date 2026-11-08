/* SPDX-License-Identifier: Apache-2.0 */
// SSLContext: a view over a crates/tls owner. What C++ (ports, clients, master.cc) and the
// compiled core-tests still call; crates/tls defines create() and swoole_ssl_* under their
// upstream symbols.
#pragma once

#include "swoole.h"

#include <string>

#include <openssl/err.h>
#include "swoole_rs_corebase_string_view.h"
#include "swoole_rs_tls_owner.h"
#include "swoole_rs_tls_constants.h"

// swoole_rs_tls_owner.h rejects OpenSSL before 3, so DTLS is always available.
#define SW_SUPPORT_DTLS

namespace swoole {

class SSLContextOwner {
  protected:
    swr_v1_tls_owner_prepared prepared_;
    explicit SSLContextOwner(swr_v1_tls_owner_prepared prepared) : prepared_(prepared) {
        if (!prepared_.owner) throw std::bad_alloc();
    }
    SSLContextOwner(const SSLContextOwner &) = delete;
    SSLContextOwner &operator=(const SSLContextOwner &) = delete;
    ~SSLContextOwner() {
        swr_v1_tls_owner_detach(prepared_.owner);
        swr_v1_tls_owner_free(prepared_.owner);
    }
};

struct SSLContext : private SSLContextOwner {
    SSLContext() : SSLContextOwner(swr_v1_tls_owner_new(swr_v1_tls_warning)) {
        // OpenSSL's userdata is this object; Rust reads its owner from the first pointer.
        if (static_cast<void *>(this) != static_cast<void *>(&prepared_.owner)) std::abort();
        swr_v1_tls_owner_bind(handle(), this);
    }
    SSLContext &operator=(const SSLContext &source) {
        auto result = swr_v1_tls_owner_assign(handle(), source.handle());
        if (result.status == -2) throw std::bad_alloc();
        // -3 comes only from a string whose cache a view here registered.
        RustStringView *views[] = {&ciphers, &ecdh_curve, nullptr, &dhparam, &cert_file, &key_file,
                                   &passphrase, &client_cert_file, &tls_host_name, &cafile, &capath};
        if (result.status == -3 && views[result.index]) views[result.index]->check(result.status);
        return *this;
    }
    const swr_v1_tls_owner *handle() const { return prepared_.owner; }
    uchar &http = prepared_.fields->http;
    uchar &http_v2 = prepared_.fields->http_v2;
    uchar &prefer_server_ciphers = prepared_.fields->prefer_server_ciphers;
    RustStringView ciphers{prepared_.strings[0]};
    RustStringView ecdh_curve{prepared_.strings[1]};
    RustStringView dhparam{prepared_.strings[3]};
    RustStringView cert_file{prepared_.strings[4]};
    RustStringView key_file{prepared_.strings[5]};
    RustStringView passphrase{prepared_.strings[6]};
    RustStringView client_cert_file{prepared_.strings[7]};
    uchar &disable_tls_host_name = prepared_.fields->disable_tls_host_name;
    RustStringView tls_host_name{prepared_.strings[8]};
    RustStringView cafile{prepared_.strings[9]};
    RustStringView capath{prepared_.strings[10]};
    uint8_t &verify_depth = prepared_.fields->verify_depth;
    uchar &disable_compress = prepared_.fields->disable_compress;
    uchar &verify_peer = prepared_.fields->verify_peer;
    uchar &allow_self_signed = prepared_.fields->allow_self_signed;
    uint32_t &protocols = prepared_.fields->protocols;

    bool ready() const {
        return prepared_.fields->context != nullptr;
    }

    bool set_cert_file(const std::string &_cert_file) {
        auto status = swr_v1_tls_owner_set_path(handle(), 4, _cert_file.c_str(), _cert_file.size());
        cert_file.check(status);
        if (status == 1) {
            swoole_warning("ssl cert file[%s] not found", _cert_file.c_str());
            return false;
        }
        return true;
    }

    bool set_key_file(const std::string &_key_file) {
        auto status = swr_v1_tls_owner_set_path(handle(), 5, _key_file.c_str(), _key_file.size());
        key_file.check(status);
        if (status == 1) {
            swoole_warning("ssl key file[%s] not found", _key_file.c_str());
            return false;
        }
        return true;
    }

    bool set_client_cert_file(const std::string &file) {
        auto status = swr_v1_tls_owner_set_path(handle(), 7, file.c_str(), file.size());
        client_cert_file.check(status);
        if (status == 1) {
            swoole_warning("ssl client cert file[%s] not found", file.c_str());
            return false;
        }
        return true;
    }

    bool create();
};
}  // namespace swoole

void swoole_ssl_init();
void swoole_ssl_destroy();
const char *swoole_ssl_get_error();

/* SPDX-License-Identifier: Apache-2.0 */
// SSLContext: crates/tls's Rust owner under upstream's name. What C++ (ports, clients) and the
// test fixtures still call; crates/tls defines create() and swoole_ssl_* under their upstream
// symbols.
#pragma once

#include "swoole.h"

#include <string>

#include <openssl/err.h>
#include "swoole_rs_tls_owner.h"
#include "swoole_rs_tls_constants.h"

// swoole_rs_tls_owner.h rejects OpenSSL before 3, so DTLS is always available.
#define SW_SUPPORT_DTLS

namespace swoole {

// The owner itself, reference counted: make() is a new one and the caller's reference, which
// release() drops (Rust's TlsContext clones add more). C++ never constructs or copies one; its
// fields and string slots are the owner's.
struct SSLContext {
    // The string slots C++ writes, in crates/tls/src/owned.rs's order.
    enum Text : uint32_t { CERT_FILE = 4, KEY_FILE = 5, TLS_HOST_NAME = 8, CAFILE = 9 };

    SSLContext() = delete;
    SSLContext(const SSLContext &) = delete;
    SSLContext &operator=(const SSLContext &) = delete;

    static SSLContext *make() {
        auto *owner = swr_v1_tls_owner_new().owner;
        if (!owner) throw std::bad_alloc();
        return of(owner);
    }
    static SSLContext *of(const swr_v1_tls_owner *owner) {
        return const_cast<SSLContext *>(reinterpret_cast<const SSLContext *>(owner));
    }
    const swr_v1_tls_owner *handle() const { return reinterpret_cast<const swr_v1_tls_owner *>(this); }
    void release() const { swr_v1_tls_owner_free(handle()); }
    struct Release {
        void operator()(const SSLContext *context) const { context->release(); }
    };

    swr_v1_tls_owner_fields &fields() const { return *swr_v1_tls_owner_view(handle()).fields; }
    // The slot's NUL-terminated bytes, empty when unset.
    const char *text(Text index) const {
        return swr_v1_core_string_view(swr_v1_tls_owner_view(handle()).strings[index]).data;
    }
    void set_text(Text index, const std::string &value) const {
        if (swr_v1_core_string_set(swr_v1_tls_owner_view(handle()).strings[index], value.data(), value.size()) != 0) {
            throw std::bad_alloc();
        }
    }

    bool ready() const { return fields().context != nullptr; }

    bool set_cert_file(const std::string &file) const {
        if (set_path(CERT_FILE, file)) return true;
        swoole_warning("ssl cert file[%s] not found", file.c_str());
        return false;
    }
    bool set_key_file(const std::string &file) const {
        if (set_path(KEY_FILE, file)) return true;
        swoole_warning("ssl key file[%s] not found", file.c_str());
        return false;
    }

    bool create();

  private:
    // False when the file can't be read.
    bool set_path(Text index, const std::string &file) const {
        auto status = swr_v1_tls_owner_set_path(handle(), index, file.c_str(), file.size());
        if (status < 0) throw std::bad_alloc();
        return status != 1;
    }
};
}  // namespace swoole

void swoole_ssl_init();
void swoole_ssl_destroy();
const char *swoole_ssl_get_error();

/*
   +----------------------------------------------------------------------+
   | PHP Version 7                                                        |
   +----------------------------------------------------------------------+
   | Copyright (c) 1997-2016 The PHP Group                                |
   +----------------------------------------------------------------------+
   | This source file is subject to version 3.01 of the PHP license,      |
   | that is bundled with this package in the file LICENSE, and is        |
   | available through the world-wide-web at the following url:           |
   | http://www.php.net/license/3_01.txt                                  |
   | If you did not receive a copy of the PHP license and are unable to   |
   | obtain it through the world-wide-web, please send a note to          |
   | license@php.net so we can mail you a copy immediately.               |
   +----------------------------------------------------------------------+
   | Author: Sara Golemon <pollita@php.net>                               |
   +----------------------------------------------------------------------+
*/

#ifndef PHP_SSH2_H
#define PHP_SSH2_H

#include "php_swoole_ssh2.h"

#ifdef SW_HAVE_SSH2LIB

#include <algorithm>
#include <chrono>
#include <climits>

#include "ext/standard/url.h"
#include "main/php_network.h"

#define PHP_SSH2_DEFAULT_PORT 22

/* Exported Constants */
#define PHP_SSH2_FINGERPRINT_MD5 0x0000
#define PHP_SSH2_FINGERPRINT_SHA1 0x0001
#define PHP_SSH2_FINGERPRINT_HEX 0x0000
#define PHP_SSH2_FINGERPRINT_RAW 0x0002
#define PHP_SSH2_FINGERPRINT_SHA256 0x0004

#define PHP_SSH2_TERM_UNIT_CHARS 0x0000
#define PHP_SSH2_TERM_UNIT_PIXELS 0x0001

#define PHP_SSH2_DEFAULT_TERMINAL "vanilla"
#define PHP_SSH2_DEFAULT_TERM_WIDTH 80
#define PHP_SSH2_DEFAULT_TERM_HEIGHT 25
#define PHP_SSH2_DEFAULT_TERM_UNIT PHP_SSH2_TERM_UNIT_CHARS

#define PHP_SSH2_SESSION_RES_NAME "SSH2 Session"
#define PHP_SSH2_CHANNEL_STREAM_NAME "SSH2 Channel"
#define PHP_SSH2_LISTENER_RES_NAME "SSH2 Listener"
#define PHP_SSH2_SFTP_RES_NAME "SSH2 SFTP"
#define PHP_SSH2_PKEY_SUBSYS_RES_NAME "SSH2 Publickey Subsystem"

#define PHP_SSH2_SFTP_STREAM_NAME "SSH2 SFTP File"
#define PHP_SSH2_SFTP_DIRSTREAM_NAME "SSH2 SFTP Directory"
#define PHP_SSH2_SFTP_WRAPPER_NAME "SSH2 SFTP"

#define PHP_SSH2_LISTEN_MAX_QUEUED 128

typedef struct _php_ssh2_sftp_data {
    LIBSSH2_SFTP *sftp;

    zend_resource *session_rsrc;
} php_ssh2_sftp_data;

typedef struct _php_ssh2_listener_data {
    LIBSSH2_SESSION *session;
    LIBSSH2_LISTENER *listener;

    zend_resource *session_rsrc;
} php_ssh2_listener_data;

typedef struct _php_ssh2_pkey_subsys_data {
    LIBSSH2_SESSION *session;
    LIBSSH2_PUBLICKEY *pkey;

    zend_resource *session_rsrc;
} php_ssh2_pkey_subsys_data;

#define SSH2_FETCH_NONAUTHENTICATED_SESSION(session, zsession)                                                         \
    if ((session = (LIBSSH2_SESSION *) zend_fetch_resource(                                                            \
             Z_RES_P(zsession), PHP_SSH2_SESSION_RES_NAME, le_ssh2_session)) == NULL) {                                \
        RETURN_FALSE;                                                                                                  \
    }                                                                                                                  \
    if (libssh2_userauth_authenticated(session)) {                                                                     \
        php_error_docref(NULL, E_WARNING, "Connection already authenticated");                                         \
        RETURN_FALSE;                                                                                                  \
    }

#define SSH2_FETCH_AUTHENTICATED_SESSION(session, zsession)                                                            \
    if ((session = (LIBSSH2_SESSION *) zend_fetch_resource(                                                            \
             Z_RES_P(zsession), PHP_SSH2_SESSION_RES_NAME, le_ssh2_session)) == NULL) {                                \
        RETURN_FALSE;                                                                                                  \
    }                                                                                                                  \
    if (!libssh2_userauth_authenticated(session)) {                                                                    \
        php_error_docref(NULL, E_WARNING, "Connection not authenticated");                                             \
        RETURN_FALSE;                                                                                                  \
    }

typedef struct _php_ssh2_channel_data {
    LIBSSH2_CHANNEL *channel;

    /* Distinguish which stream we should read/write from/to */
    unsigned int streamid;
    char is_blocking;
    bool timed_out;
    bool wait_for_remote_close;
    double timeout;

    /* Resource */
    zend_resource *session_rsrc;

    /* Allow one stream to be closed while the other is kept open */
    unsigned char *refcount;

} php_ssh2_channel_data;

static inline php_ssh2_channel_data *channel_data_create(
    LIBSSH2_CHANNEL *channel, zend_resource *session, bool wait_for_remote_close) {
    auto *data = static_cast<php_ssh2_channel_data *>(ecalloc(1, sizeof(php_ssh2_channel_data)));
    data->channel = channel;
    data->session_rsrc = session;
    data->is_blocking = 1;
    data->wait_for_remote_close = wait_for_remote_close;
    return data;
}

/* Rust (crates/ssh2/src/php.rs): ssh2_connect's session, for the wrappers too. */
extern "C" void swr_ssh2_forward_enter(LIBSSH2_SESSION *session);
extern "C" void swr_ssh2_forward_leave(LIBSSH2_SESSION *session);
extern "C" LIBSSH2_SESSION *php_ssh2_session_connect(const char *host, int port, zval *methods, zval *callbacks);
/* Rust (crates/ssh2/src/php.rs): ssh2_shell's channel, for the ssh2.shell:// wrapper too. */
extern "C" LIBSSH2_CHANNEL *swr_ssh2_shell_channel(
    LIBSSH2_SESSION *session, const char *term, unsigned int term_len, zval *environment, long width, long height, long unit);
/* Rust (crates/ssh2/src/php.rs): the ssh2.*:// wrappers' authentication. */
extern "C" bool swr_ssh2_wrapper_auth(LIBSSH2_SESSION *session,
                                      const char *username,
                                      unsigned int username_len,
                                      const char *password,
                                      unsigned int password_len,
                                      const char *pubkey_file,
                                      const char *privkey_file);
/* Rust (crates/ssh2/src/php.rs): ssh2_exec's channel, for the ssh2.exec:// wrapper too. */
extern "C" LIBSSH2_CHANNEL *swr_ssh2_exec_channel(LIBSSH2_SESSION *session,
                                                  const char *command,
                                                  const char *term,
                                                  unsigned int term_len,
                                                  zval *environment,
                                                  long width,
                                                  long height,
                                                  long unit);
void php_ssh2_sftp_dtor(zend_resource *rsrc);
php_url *php_ssh2_parse_url(const char *url);
php_url *php_ssh2_fopen_wraper_parse_path(const char *path,
                                          const char *type,
                                          php_stream_context *context,
                                          LIBSSH2_SESSION **psession,
                                          zend_resource **presource,
                                          LIBSSH2_SFTP **psftp,
                                          zend_resource **psftp_rsrc);

extern php_stream_ops php_ssh2_channel_stream_ops;

extern php_stream_wrapper php_ssh2_stream_wrapper_shell;
extern php_stream_wrapper php_ssh2_stream_wrapper_exec;
extern php_stream_wrapper php_ssh2_stream_wrapper_tunnel;
extern php_stream_wrapper php_ssh2_stream_wrapper_scp;
extern php_stream_wrapper php_ssh2_sftp_wrapper;

/* Resource list entries */
extern int le_ssh2_session;
extern int le_ssh2_sftp;
extern int le_ssh2_pkey_subsys;

static inline LIBSSH2_SESSION *ssh2_get_session(php_ssh2_channel_data *abstract) {
    return static_cast<LIBSSH2_SESSION *>(abstract->session_rsrc->ptr);
}

template <typename Operation>
static ssize_t session_io(zend_resource *resource, bool may_block, double timeout, bool &timed_out, Operation operation) {
    auto session = static_cast<LIBSSH2_SESSION *>(resource->ptr);
    if (!session) {
        return -1;
    }
    while (resource->ptr == session) {
        if (may_block) {
            ssh2_bound(session);
        } else if (swr_v1_ssh2_is_bound(session)) {
            return LIBSSH2_ERROR_EAGAIN;
        }
        ssize_t result = operation();
        if (result != LIBSSH2_ERROR_EAGAIN || !may_block) {
            return result;
        }
        bool outbound = libssh2_session_block_directions(session) & LIBSSH2_SESSION_BLOCK_OUTBOUND;
        swr_ssh2_forward_enter(session);
        bool ready = swr_v1_ssh2_channel_wait(session, outbound, timeout, &timed_out);
        swr_ssh2_forward_leave(session);
        if (!ready) {
            return -1;
        }
    }
    return -1;
}

// A destructor can yield during request shutdown. Every retry spends the same timeout.
template <typename Operation>
static void ssh2_release_call(LIBSSH2_SESSION *session, double timeout, Operation operation) {
    if (!swoole::Coroutine::get_current()) {
        if (swr_v1_ssh2_is_bound(session) || (EG(flags) & EG_FLAGS_IN_SHUTDOWN)) {
            operation();
            return;
        }
        const auto blocking = libssh2_session_get_blocking(session);
        const auto previous_timeout = libssh2_session_get_timeout(session);
        const double ms = timeout * 1000;
        const long timeout_ms = ms >= static_cast<double>(LONG_MAX) ? LONG_MAX : std::max(1L, static_cast<long>(ms));
        libssh2_session_set_timeout(session, timeout_ms);
        libssh2_session_set_blocking(session, 1);
        operation();
        libssh2_session_set_blocking(session, blocking);
        libssh2_session_set_timeout(session, previous_timeout);
        return;
    }
    const auto start = std::chrono::steady_clock::now();
    while (operation() == LIBSSH2_ERROR_EAGAIN && !swr_v1_ssh2_is_bound(session)) {
        double remaining = timeout - std::chrono::duration<double>(std::chrono::steady_clock::now() - start).count();
        bool timed_out = false;
        bool outbound = libssh2_session_block_directions(session) & LIBSSH2_SESSION_BLOCK_OUTBOUND;
        if (remaining <= 0 || !swr_v1_ssh2_channel_wait(session, outbound, remaining, &timed_out) ||
            swr_v1_ssh2_is_bound(session)) {
            break;
        }
    }
}

// A destructor inside a coroutine can yield even during PHP request shutdown.
template <typename Operation>
static void release_ssh_resource(zend_resource *resource, double timeout, Operation operation) {
    auto session = static_cast<LIBSSH2_SESSION *>(resource->ptr);
    if (!session) {
        return;
    }
    swr_ssh2_forward_enter(session);
    ssh2_release_call(session, swr_v1_ssh2_release_timeout(session, timeout), [&] {
        return resource->ptr == session ? operation() : 0;
    });
    swr_ssh2_forward_leave(session);
}

#endif /* SW_HAVE_SSH2LIB */
#endif /* PHP_SSH2_H */

/*
 * Local variables:
 * tab-width: 4
 * c-basic-offset: 4
 * indent-tabs-mode: t
 * End:
 */

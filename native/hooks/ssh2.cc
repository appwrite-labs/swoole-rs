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

/* What stays C of ext/ssh2 (docs/zend.md, What stays C); its PHP functions are
 * crates/ssh2/src/php.rs.
 * - libssh2's allocator and the session, listener and SFTP resources' data, and
 *   their destructors PHP calls: allocator (emalloc, efree, ecalloc).
 * - the debug, ignore, macerror and disconnect callbacks, which call PHP
 *   functions from inside libssh2, and setting them: bailout.
 * - the session's socket, a C++ SocketImpl, and reading the home directory
 *   (getpwuid): budget.
 * - minit's resource and stream wrapper registration, over the C destructors
 *   and wrapper tables here, and minfo: variadic (php_info_print_table_row). */
#include "swoole_rs_phphost.h"
#ifdef SW_HAVE_SSH2LIB

#include "php_ssh2.h"
#include "php_swoole_ssh2_hook.h"

#include "ext/standard/info.h"
#include "ext/standard/file.h"

#include "main/php_network.h"

/* Internal Constants */
#ifndef SHA_DIGEST_LENGTH
#define SHA_DIGEST_LENGTH 20
#endif

#ifndef MD5_DIGEST_LENGTH
#define MD5_DIGEST_LENGTH 16
#endif

/* True global resources - no need for thread safety here */
int le_ssh2_session;
int le_ssh2_listener;
int le_ssh2_sftp;
int le_ssh2_pkey_subsys;

/* *************
 * Callbacks *
 ************* */

/* {{{ php_ssh2_alloc_cb
 * Wrap emalloc()
 */
static LIBSSH2_ALLOC_FUNC(php_ssh2_alloc_cb) {
    return emalloc(count);
}
/* }}} */

/* {{{ php_ssh2_free_cb
 * Wrap efree()
 */
static LIBSSH2_FREE_FUNC(php_ssh2_free_cb) {
    efree(ptr);
}
/* }}} */

/* {{{ php_ssh2_realloc_cb
 * Wrap erealloc()
 */
static LIBSSH2_REALLOC_FUNC(php_ssh2_realloc_cb) {
    return erealloc(ptr, count);
}
/* }}} */

/* {{{ php_ssh2_debug_cb
 * Debug packets
 */
LIBSSH2_DEBUG_FUNC(php_ssh2_debug_cb) {
    php_ssh2_session_data *data;
    zval args[3];

    if (!abstract || !*abstract) {
        return;
    }
    data = (php_ssh2_session_data *) *abstract;
    if (!data->debug_cb) {
        return;
    }

    ZVAL_STRINGL(&args[0], message, message_len);
    ZVAL_STRINGL(&args[1], language, language_len);
    ZVAL_LONG(&args[2], always_display);

    zval retval;
    if (FAILURE == call_user_function(NULL, NULL, data->debug_cb, &retval, 3, args)) {
        php_error_docref(NULL, E_WARNING, "Failure calling debug callback");
    }

    if (!Z_ISUNDEF(retval)) {
        zval_ptr_dtor(&retval);
    }
}
/* }}} */

/* {{{ php_ssh2_ignore_cb
 * Ignore packets
 */
LIBSSH2_IGNORE_FUNC(php_ssh2_ignore_cb) {
    php_ssh2_session_data *data;
    zval zretval;
    zval args[1];

    if (!abstract || !*abstract) {
        return;
    }
    data = (php_ssh2_session_data *) *abstract;
    if (!data->ignore_cb) {
        return;
    }

    ZVAL_STRINGL(&args[0], message, message_len);

    if (FAILURE == call_user_function(NULL, NULL, data->ignore_cb, &zretval, 1, args)) {
        php_error_docref(NULL, E_WARNING, "Failure calling ignore callback");
    }
    if (Z_TYPE_P(&zretval) != IS_UNDEF) {
        zval_ptr_dtor(&zretval);
    }
}
/* }}} */

/* {{{ php_ssh2_macerror_cb
 * Called when a MAC error occurs, offers the chance to ignore
 * WHY ARE YOU IGNORING MAC ERRORS??????
 */
LIBSSH2_MACERROR_FUNC(php_ssh2_macerror_cb) {
    php_ssh2_session_data *data;
    zval zretval;
    zval args[1];
    int retval = -1;

    if (!abstract || !*abstract) {
        return -1;
    }
    data = (php_ssh2_session_data *) *abstract;
    if (!data->macerror_cb) {
        return -1;
    }

    ZVAL_STRINGL(&args[0], packet, packet_len);

    if (FAILURE == call_user_function(NULL, NULL, data->macerror_cb, &zretval, 1, args)) {
        php_error_docref(NULL, E_WARNING, "Failure calling macerror callback");
    } else {
        retval = zval_is_true(&zretval) ? 0 : -1;
    }
    if (Z_TYPE_P(&zretval) != IS_UNDEF) {
        zval_ptr_dtor(&zretval);
    }

    return retval;
}
/* }}} */

/* {{{ php_ssh2_disconnect_cb
 * Connection closed by foreign host
 */
LIBSSH2_DISCONNECT_FUNC(php_ssh2_disconnect_cb) {
    php_ssh2_session_data *data;
    zval args[3];

    if (!abstract || !*abstract) {
        return;
    }
    data = (php_ssh2_session_data *) *abstract;
    if (!data->disconnect_cb) {
        return;
    }

    ZVAL_LONG(&args[0], reason);
    ZVAL_STRINGL(&args[1], message, message_len);
    ZVAL_STRINGL(&args[2], language, language_len);

    zval retval;
    if (FAILURE == call_user_function(NULL, NULL, data->disconnect_cb, &retval, 3, args)) {
        php_error_docref(NULL, E_WARNING, "Failure calling disconnect callback");
    }

    if (!Z_ISUNDEF(retval)) {
        zval_ptr_dtor(&retval);
    }
}
/* }}} */

/* *****************
 * Userspace API *
 ***************** */

/* {{{ php_ssh2_set_callback
 * Try to set a method if it's passed in with the hash table
 */
static int php_ssh2_set_callback(LIBSSH2_SESSION *session,
                                 HashTable *ht,
                                 const char *callback,
                                 int callback_len,
                                 int callback_type,
                                 php_ssh2_session_data *data) {
    zval *handler, *copyval;
    void *internal_handler;
    zend_string *callback_zstring;

    callback_zstring = zend_string_init(callback, callback_len, 0);
    if ((handler = zend_hash_find(ht, callback_zstring)) == NULL) {
        zend_string_release(callback_zstring);
        return 0;
    }
    zend_string_release(callback_zstring);

    if (!zend_is_callable(handler, 0, NULL)) {
        return -1;
    }

    copyval = (zval *) emalloc(sizeof(zval));
    ZVAL_COPY(copyval, handler);

    switch (callback_type) {
    case LIBSSH2_CALLBACK_IGNORE:
        internal_handler = (void *) php_ssh2_ignore_cb;
        if (data->ignore_cb) {
            zval_ptr_dtor(data->ignore_cb);
        }
        data->ignore_cb = copyval;
        break;
    case LIBSSH2_CALLBACK_DEBUG:
        internal_handler = (void *) php_ssh2_debug_cb;
        if (data->debug_cb) {
            zval_ptr_dtor(data->debug_cb);
        }
        data->debug_cb = copyval;
        break;
    case LIBSSH2_CALLBACK_MACERROR:
        internal_handler = (void *) php_ssh2_macerror_cb;
        if (data->macerror_cb) {
            zval_ptr_dtor(data->macerror_cb);
        }
        data->macerror_cb = copyval;
        break;
    case LIBSSH2_CALLBACK_DISCONNECT:
        internal_handler = (void *) php_ssh2_disconnect_cb;
        if (data->disconnect_cb) {
            zval_ptr_dtor(data->disconnect_cb);
        }
        data->disconnect_cb = copyval;
        break;
    default:
        zval_ptr_dtor(copyval);
        return -1;
    }

    libssh2_session_callback_set(session, callback_type, internal_handler);

    return 0;
}
/* }}} */


/* ssh2_connect is Rust (crates/ssh2/src/php.rs). What it keeps C: the session's
 * socket, a C++ SocketImpl, and its data, emalloc'd with the callbacks' zvals and
 * freed by the session's destructor (allocator); and the callbacks, which call
 * PHP functions from inside libssh2 (bailout). */
extern "C" LIBSSH2_SESSION *swr_ssh2_session_open(const char *host, int port) {
    LIBSSH2_SESSION *session;
    php_ssh2_session_data *data;

    int domain = swoole::network::Address::verify_ip(AF_INET6, host) ? AF_INET6 : AF_INET;

    auto sock = new SocketImpl(domain, SOCK_STREAM, 0);
    if (sock->get_fd() < 0 || !sock->connect(host, port)) {
        php_error_docref(NULL, E_WARNING, "Unable to connect to %s on port %d", host, port);
        delete sock;
        return NULL;
    }

    data = (php_ssh2_session_data *) ecalloc(1, sizeof(php_ssh2_session_data));
    data->socket = sock;

    session = libssh2_session_init_ex(php_ssh2_alloc_cb, php_ssh2_free_cb, php_ssh2_realloc_cb, data);
    if (!session) {
        php_error_docref(NULL, E_WARNING, "Unable to initialize SSH2 session");
        efree(data);
        delete sock;
        return NULL;
    }

    libssh2_banner_set(session, LIBSSH2_SSH_DEFAULT_BANNER " swoole-" SWOOLE_VERSION);
    libssh2_session_set_blocking(session, 0);
    return session;
}

extern "C" void swr_ssh2_session_callbacks(LIBSSH2_SESSION *session, zval *callbacks) {
    php_ssh2_session_data *data = *(php_ssh2_session_data **) libssh2_session_abstract(session);

    /* ignore debug disconnect macerror */

    if (php_ssh2_set_callback(
            session, HASH_OF(callbacks), "ignore", sizeof("ignore") - 1, LIBSSH2_CALLBACK_IGNORE, data)) {
        php_error_docref(NULL, E_WARNING, "Failed setting IGNORE callback");
    }

    if (php_ssh2_set_callback(session, HASH_OF(callbacks), "debug", sizeof("debug") - 1, LIBSSH2_CALLBACK_DEBUG, data)) {
        php_error_docref(NULL, E_WARNING, "Failed setting DEBUG callback");
    }

    if (php_ssh2_set_callback(
            session, HASH_OF(callbacks), "macerror", sizeof("macerror") - 1, LIBSSH2_CALLBACK_MACERROR, data)) {
        php_error_docref(NULL, E_WARNING, "Failed setting MACERROR callback");
    }

    if (php_ssh2_set_callback(
            session, HASH_OF(callbacks), "disconnect", sizeof("disconnect") - 1, LIBSSH2_CALLBACK_DISCONNECT, data)) {
        php_error_docref(NULL, E_WARNING, "Failed setting DISCONNECT callback");
    }
}

extern "C" int swr_ssh2_session_fd(LIBSSH2_SESSION *session) {
    return (*(php_ssh2_session_data **) libssh2_session_abstract(session))->socket->get_fd();
}

extern "C" void swr_ssh2_session_drop(LIBSSH2_SESSION *session) {
    php_ssh2_session_data *data = *(php_ssh2_session_data **) libssh2_session_abstract(session);
    SocketImpl *sock = data->socket;

    libssh2_session_free(session);
    efree(data);
    delete sock;
}

extern "C" void swr_ssh2_session_resource(LIBSSH2_SESSION *session, zval *return_value) {
    zval_ptr_dtor(return_value);
    ZVAL_RES(return_value, zend_register_resource(session, le_ssh2_session));
}

/* ssh2_disconnect is Rust (crates/ssh2/src/php.rs). */

/* ssh2_methods_negotiated and ssh2_fingerprint are Rust (crates/ssh2/src/php.rs),
 * which reads a resource's session through this. */
extern "C" LIBSSH2_SESSION *swr_ssh2_session(zval *zsession) {
    return (LIBSSH2_SESSION *) zend_fetch_resource(Z_RES_P(zsession), PHP_SSH2_SESSION_RES_NAME, le_ssh2_session);
}

/* ssh2_auth_none and ssh2_auth_password are Rust (crates/ssh2/src/php.rs). Stays C:
 * the keyboard-interactive callback, which hands libssh2 a response it frees through
 * the session's allocator, and the password it answers with. */
static const char *password_for_kbd_callback;

extern "C" void swr_ssh2_kbd_password(const char *password) {
    password_for_kbd_callback = password;
}

extern "C" void swr_ssh2_kbd_callback(const char *name,
                                      int name_len,
                                      const char *instruction,
                                      int instruction_len,
                                      int num_prompts,
                                      const LIBSSH2_USERAUTH_KBDINT_PROMPT *prompts,
                                      LIBSSH2_USERAUTH_KBDINT_RESPONSE *responses,
                                      void **abstract) {
    (void) name;
    (void) name_len;
    (void) instruction;
    (void) instruction_len;
    if (num_prompts == 1) {
        responses[0].text = estrdup(password_for_kbd_callback);
        responses[0].length = strlen(password_for_kbd_callback);
    }
    (void) prompts;
    (void) abstract;
}

/* ssh2_auth_pubkey_file, ssh2_auth_pubkey and ssh2_auth_hostbased_file are Rust
 * (crates/ssh2/src/php.rs). Stays C: budget (pw_dir points into libc's own entry,
 * which reading from Rust would take an unsafe block for). */
extern "C" const char *swr_ssh2_home_dir(void) {
    struct passwd *pws = getpwuid(geteuid());
    return pws ? pws->pw_dir : NULL;
}

/* ssh2_forward_listen and ssh2_forward_accept are Rust (crates/ssh2/src/php.rs).
 * What they keep C: the listener resource's data, emalloc'd and freed by its
 * destructor (allocator), and the channel stream over an accepted connection,
 * built on this file's stream ops. */
extern "C" void swr_ssh2_listener_open(zval *zsession,
                                       LIBSSH2_SESSION *session,
                                       LIBSSH2_LISTENER *listener,
                                       zval *return_value) {
    php_ssh2_listener_data *data = (php_ssh2_listener_data *) emalloc(sizeof(php_ssh2_listener_data));
    data->session = session;
    data->session_rsrc = Z_RES_P(zsession);
    Z_ADDREF_P(zsession);
    data->listener = listener;

    zval_ptr_dtor(return_value);
    ZVAL_RES(return_value, zend_register_resource(data, le_ssh2_listener));
}

extern "C" LIBSSH2_LISTENER *swr_ssh2_listener(zval *zlistener) {
    php_ssh2_listener_data *data = (php_ssh2_listener_data *) zend_fetch_resource(
        Z_RES_P(zlistener), PHP_SSH2_LISTENER_RES_NAME, le_ssh2_listener);
    return data && data->session_rsrc->ptr ? data->listener : NULL;
}

extern "C" LIBSSH2_SESSION *swr_ssh2_listener_session(zval *zlistener) {
    return ((php_ssh2_listener_data *) Z_RES_P(zlistener)->ptr)->session;
}

extern "C" void swr_ssh2_accepted_stream(zval *zlistener, LIBSSH2_CHANNEL *channel, zval *return_value) {
    php_ssh2_listener_data *data = (php_ssh2_listener_data *) Z_RES_P(zlistener)->ptr;
    php_ssh2_channel_data *channel_data;
    php_stream *stream;

    channel_data = (php_ssh2_channel_data *) emalloc(sizeof(php_ssh2_channel_data));
    channel_data->channel = channel;
    channel_data->streamid = 0;
    channel_data->is_blocking = 1;
    channel_data->timed_out = false;
    channel_data->timeout = 0;
    channel_data->session_rsrc = data->session_rsrc;
    channel_data->refcount = NULL;

    zval_ptr_dtor(return_value);
    stream = php_stream_alloc(&php_ssh2_channel_stream_ops, channel_data, 0, "r+");
    if (!stream) {
        php_error_docref(NULL, E_WARNING, "Failure allocating stream");
        efree(channel_data);
        libssh2_channel_free(channel);
        RETURN_FALSE;
    }

    GC_ADDREF(channel_data->session_rsrc);

    php_stream_to_zval(stream, return_value);
}

/* ***********************
 * Module Housekeeping *
 *********************** */

static void destroy_session(LIBSSH2_SESSION *session) {
    php_ssh2_session_data **data = (php_ssh2_session_data **) libssh2_session_abstract(session);

    libssh2_session_disconnect(session, "swoole_ssh2 (https://github.com/swoole/swoole-src)");

    if (*data) {
        if ((*data)->ignore_cb) {
            zval_ptr_dtor((*data)->ignore_cb);
        }
        if ((*data)->debug_cb) {
            zval_ptr_dtor((*data)->debug_cb);
        }
        if ((*data)->macerror_cb) {
            zval_ptr_dtor((*data)->macerror_cb);
        }
        if ((*data)->disconnect_cb) {
            zval_ptr_dtor((*data)->disconnect_cb);
        }

        delete (*data)->socket;
        efree(*data);
        *data = NULL;
    }

    libssh2_session_free(session);
}

static php_ssh2_session_data *session_data(LIBSSH2_SESSION *session) {
    return static_cast<php_ssh2_session_data *>(*libssh2_session_abstract(session));
}

extern "C" void swr_ssh2_forward_enter(LIBSSH2_SESSION *session) {
    ++session_data(session)->forwarding.active;
}

extern "C" void swr_ssh2_forward_leave(LIBSSH2_SESSION *session) {
    auto data = session_data(session);
    if (data->forwarding.leave()) {
        destroy_session(session);
    }
}

extern "C" bool swr_ssh2_forward_closed(LIBSSH2_SESSION *session) {
    return session_data(session)->forwarding.close_pending;
}

static void php_ssh2_session_dtor(zend_resource *rsrc) {
    auto session = static_cast<LIBSSH2_SESSION *>(rsrc->ptr);
    auto data = session_data(session);
    if (data->forwarding.request_close(data->socket->get_fd())) {
        destroy_session(session);
    }
}

static void php_ssh2_listener_dtor(zend_resource *rsrc) {
    php_ssh2_listener_data *data = (php_ssh2_listener_data *) rsrc->ptr;
    LIBSSH2_LISTENER *listener = data->listener;
    auto session = data->session;

    if (data->session_rsrc->ptr) {
        swr_ssh2_forward_enter(session);
        libssh2_channel_forward_cancel(listener);
        swr_ssh2_forward_leave(session);
    }
    zend_list_delete(data->session_rsrc);
    efree(data);
}

/* The publickey subsystem's functions are Rust (crates/ssh2/src/php/publickey.rs).
 * Stays C: its resource, and libssh2_publickey_attribute and _list, which
 * libssh2's header lays out and Rust would need a repr(C) for (budget). */
extern "C" void swr_ssh2_publickey_open(zval *zsession,
                                        LIBSSH2_SESSION *session,
                                        LIBSSH2_PUBLICKEY *pkey,
                                        zval *return_value) {
    php_ssh2_pkey_subsys_data *data = (php_ssh2_pkey_subsys_data *) emalloc(sizeof(php_ssh2_pkey_subsys_data));
    data->session = session;
    data->session_rsrc = Z_RES_P(zsession);
    Z_ADDREF_P(zsession);
    data->pkey = pkey;

    zval_ptr_dtor(return_value);
    ZVAL_RES(return_value, zend_register_resource(data, le_ssh2_pkey_subsys));
}

extern "C" LIBSSH2_PUBLICKEY *swr_ssh2_publickey(zval *zpkey) {
    php_ssh2_pkey_subsys_data *data = (php_ssh2_pkey_subsys_data *) zend_fetch_resource(
        Z_RES_P(zpkey), PHP_SSH2_PKEY_SUBSYS_RES_NAME, le_ssh2_pkey_subsys);
    return data ? data->pkey : NULL;
}

/* libssh2_publickey_add_ex with the attributes Rust packed: each a mandatory
 * byte, its name's and value's lengths (size_t), then their bytes. -1, without
 * the call, when `packed` doesn't hold whole attributes. */
extern "C" int swr_ssh2_publickey_add(LIBSSH2_PUBLICKEY *pkey,
                                      const unsigned char *name,
                                      size_t name_len,
                                      const unsigned char *blob,
                                      size_t blob_len,
                                      bool overwrite,
                                      const char *packed,
                                      size_t packed_len) {
    const size_t head = 1 + 2 * sizeof(size_t);
    unsigned long count = 0;
    for (size_t at = 0; at < packed_len; count++) {
        size_t lengths[2];
        if (packed_len - at < head) {
            return -1;
        }
        memcpy(lengths, packed + at + 1, sizeof(lengths));
        at += head;
        if (lengths[0] > packed_len - at || lengths[1] > packed_len - at - lengths[0]) {
            return -1;
        }
        at += lengths[0] + lengths[1];
    }
    libssh2_publickey_attribute *attrs =
        count ? (libssh2_publickey_attribute *) safe_emalloc(count, sizeof(libssh2_publickey_attribute), 0) : NULL;
    for (size_t i = 0, at = 0; i < count; i++) {
        size_t lengths[2];
        memcpy(lengths, packed + at + 1, sizeof(lengths));
        attrs[i].mandatory = packed[at];
        attrs[i].name = packed + at + head;
        attrs[i].name_len = lengths[0];
        attrs[i].value = packed + at + head + lengths[0];
        attrs[i].value_len = lengths[1];
        at += head + lengths[0] + lengths[1];
    }
    int rc = libssh2_publickey_add_ex(pkey, name, name_len, blob, blob_len, overwrite, count, attrs);
    if (attrs) {
        efree(attrs);
    }
    return rc;
}

/* The `index`th listed key, or null past the list's end (libssh2 ends it with
 * a null packet). */
static const libssh2_publickey_list *swr_ssh2_listed(const libssh2_publickey_list *keys, unsigned long index) {
    for (unsigned long i = 0; i < index; i++) {
        if (!keys[i].packet) {
            return NULL;
        }
    }
    return keys[index].packet ? &keys[index] : NULL;
}

extern "C" bool swr_ssh2_publickey_key(const libssh2_publickey_list *keys,
                                       unsigned long index,
                                       bool blob,
                                       zval *out) {
    const libssh2_publickey_list *key = swr_ssh2_listed(keys, index);
    if (!key) {
        return false;
    }
    zval_ptr_dtor(out);
    if (blob) {
        ZVAL_STRINGL(out, (const char *) key->blob, key->blob_len);
    } else {
        ZVAL_STRINGL(out, (const char *) key->name, key->name_len);
    }
    return true;
}

extern "C" bool swr_ssh2_publickey_attribute(const libssh2_publickey_list *keys,
                                             unsigned long index,
                                             unsigned long attribute,
                                             bool value,
                                             zval *out) {
    const libssh2_publickey_list *key = swr_ssh2_listed(keys, index);
    if (!key || attribute >= key->num_attrs) {
        return false;
    }
    const libssh2_publickey_attribute *attr = &key->attrs[attribute];
    zval_ptr_dtor(out);
    if (value) {
        ZVAL_STRINGL(out, attr->value, attr->value_len);
    } else {
        ZVAL_STRINGL(out, attr->name, attr->name_len);
    }
    return true;
}

static void php_ssh2_pkey_subsys_dtor(zend_resource *rsrc) {
    php_ssh2_pkey_subsys_data *data = (php_ssh2_pkey_subsys_data *) rsrc->ptr;
    LIBSSH2_PUBLICKEY *pkey = data->pkey;

    libssh2_publickey_shutdown(pkey);
    zend_list_delete(data->session_rsrc);
    efree(data);
}

/* {{{ PHP_MINIT_FUNCTION
 */
int php_swoole_ssh2_minit(int module_number) {
    le_ssh2_session =
        zend_register_list_destructors_ex(php_ssh2_session_dtor, NULL, PHP_SSH2_SESSION_RES_NAME, module_number);
    le_ssh2_listener =
        zend_register_list_destructors_ex(php_ssh2_listener_dtor, NULL, PHP_SSH2_LISTENER_RES_NAME, module_number);
    le_ssh2_sftp = zend_register_list_destructors_ex(php_ssh2_sftp_dtor, NULL, PHP_SSH2_SFTP_RES_NAME, module_number);
    le_ssh2_pkey_subsys = zend_register_list_destructors_ex(
        php_ssh2_pkey_subsys_dtor, NULL, PHP_SSH2_PKEY_SUBSYS_RES_NAME, module_number);

    REGISTER_LONG_CONSTANT("SSH2_FINGERPRINT_MD5", PHP_SSH2_FINGERPRINT_MD5, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_FINGERPRINT_SHA1", PHP_SSH2_FINGERPRINT_SHA1, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_FINGERPRINT_HEX", PHP_SSH2_FINGERPRINT_HEX, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_FINGERPRINT_RAW", PHP_SSH2_FINGERPRINT_RAW, CONST_CS | CONST_PERSISTENT);

    REGISTER_LONG_CONSTANT("SSH2_FINGERPRINT_SHA256", PHP_SSH2_FINGERPRINT_SHA256, CONST_CS | CONST_PERSISTENT);

    REGISTER_LONG_CONSTANT("SSH2_HOSTKEY_TYPE_UNKNOWN", LIBSSH2_HOSTKEY_TYPE_UNKNOWN, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_HOSTKEY_TYPE_RSA", LIBSSH2_HOSTKEY_TYPE_RSA, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_HOSTKEY_TYPE_DSS", LIBSSH2_HOSTKEY_TYPE_DSS, CONST_CS | CONST_PERSISTENT);
#ifdef LIBSSH2_HOSTKEY_TYPE_ECDSA_256
    REGISTER_LONG_CONSTANT("SSH2_HOSTKEY_TYPE_ECDSA_256", LIBSSH2_HOSTKEY_TYPE_ECDSA_256, CONST_CS | CONST_PERSISTENT);
#endif
#ifdef LIBSSH2_HOSTKEY_TYPE_ECDSA_384
    REGISTER_LONG_CONSTANT("SSH2_HOSTKEY_TYPE_ECDSA_384", LIBSSH2_HOSTKEY_TYPE_ECDSA_384, CONST_CS | CONST_PERSISTENT);
#endif
#ifdef LIBSSH2_HOSTKEY_TYPE_ECDSA_521
    REGISTER_LONG_CONSTANT("SSH2_HOSTKEY_TYPE_ECDSA_521", LIBSSH2_HOSTKEY_TYPE_ECDSA_521, CONST_CS | CONST_PERSISTENT);
#endif
#ifdef LIBSSH2_HOSTKEY_TYPE_ED25519
    REGISTER_LONG_CONSTANT("SSH2_HOSTKEY_TYPE_ED25519", LIBSSH2_HOSTKEY_TYPE_ED25519, CONST_CS | CONST_PERSISTENT);
#endif

    REGISTER_LONG_CONSTANT("SSH2_TERM_UNIT_CHARS", PHP_SSH2_TERM_UNIT_CHARS, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_TERM_UNIT_PIXELS", PHP_SSH2_TERM_UNIT_PIXELS, CONST_CS | CONST_PERSISTENT);

    REGISTER_STRING_CONSTANT("SSH2_DEFAULT_TERMINAL", PHP_SSH2_DEFAULT_TERMINAL, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_DEFAULT_TERM_WIDTH", PHP_SSH2_DEFAULT_TERM_WIDTH, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_DEFAULT_TERM_HEIGHT", PHP_SSH2_DEFAULT_TERM_HEIGHT, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_DEFAULT_TERM_UNIT", PHP_SSH2_DEFAULT_TERM_UNIT, CONST_CS | CONST_PERSISTENT);

    REGISTER_LONG_CONSTANT("SSH2_STREAM_STDIO", 0, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_STREAM_STDERR", SSH_EXTENDED_DATA_STDERR, CONST_CS | CONST_PERSISTENT);

    /* events/revents */
    REGISTER_LONG_CONSTANT("SSH2_POLLIN", LIBSSH2_POLLFD_POLLIN, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_POLLEXT", LIBSSH2_POLLFD_POLLEXT, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_POLLOUT", LIBSSH2_POLLFD_POLLOUT, CONST_CS | CONST_PERSISTENT);

    /* revents only */
    REGISTER_LONG_CONSTANT("SSH2_POLLERR", LIBSSH2_POLLFD_POLLERR, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_POLLHUP", LIBSSH2_POLLFD_POLLHUP, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_POLLNVAL", LIBSSH2_POLLFD_POLLNVAL, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_POLL_SESSION_CLOSED", LIBSSH2_POLLFD_SESSION_CLOSED, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_POLL_CHANNEL_CLOSED", LIBSSH2_POLLFD_CHANNEL_CLOSED, CONST_CS | CONST_PERSISTENT);
    REGISTER_LONG_CONSTANT("SSH2_POLL_LISTENER_CLOSED", LIBSSH2_POLLFD_LISTENER_CLOSED, CONST_CS | CONST_PERSISTENT);

    return (php_register_url_stream_wrapper("ssh2.shell", &php_ssh2_stream_wrapper_shell) == SUCCESS &&
            php_register_url_stream_wrapper("ssh2.exec", &php_ssh2_stream_wrapper_exec) == SUCCESS &&
            php_register_url_stream_wrapper("ssh2.tunnel", &php_ssh2_stream_wrapper_tunnel) == SUCCESS &&
            php_register_url_stream_wrapper("ssh2.scp", &php_ssh2_stream_wrapper_scp) == SUCCESS &&
            php_register_url_stream_wrapper("ssh2.sftp", &php_ssh2_sftp_wrapper) == SUCCESS)
               ? SUCCESS
               : FAILURE;
}
/* }}} */

int php_swoole_ssh2_mshutdown() {
    return (php_unregister_url_stream_wrapper("ssh2.shell") == SUCCESS &&
            php_unregister_url_stream_wrapper("ssh2.exec") == SUCCESS &&
            php_unregister_url_stream_wrapper("ssh2.tunnel") == SUCCESS &&
            php_unregister_url_stream_wrapper("ssh2.scp") == SUCCESS &&
            php_unregister_url_stream_wrapper("ssh2.sftp") == SUCCESS)
               ? SUCCESS
               : FAILURE;
}

void php_swoole_ssh2_minfo() {
    php_info_print_table_row(2, "SSH2 support", "enabled");
    php_info_print_table_row(2, "libssh2 banner", LIBSSH2_SSH_BANNER);
}

#endif /* SW_HAVE_SSH2LIB */

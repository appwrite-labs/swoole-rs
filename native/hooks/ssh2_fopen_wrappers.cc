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

/* What stays C of ext/ssh2's ssh2_fopen_wrappers.cc (docs/zend.md, What stays
 * C); its PHP functions, its shell and exec channels and its wrappers'
 * authentication are crates/ssh2/src/php.rs.
 * - the channel stream ops table and its operations, over
 *   php_ssh2_channel_data (emalloc'd, its refcount byte shared between a
 *   channel's streams) and php_stream's eof bitfield: allocator, budget.
 * - the ssh2.shell, exec, tunnel and scp wrappers and their path parser, which
 *   finds a session resource in EG(regular_list) and returns a php_url:
 *   globals, allocator.
 * - the channel streams ssh2_tunnel, ssh2_shell, ssh2_exec and
 *   ssh2_fetch_stream return: allocator (php_stream_alloc of their data). */
#include "swoole_rs_phphost.h"
#ifdef SW_HAVE_SSH2LIB

#include "php_ssh2.h"
#include "php_swoole_ssh2_hook.h"

void *php_ssh2_zval_from_resource_handle(int handle) {
    zval *val;
    zend_resource *zr;
    ZEND_HASH_FOREACH_VAL(&EG(regular_list), val) {
        zr = Z_RES_P(val);
        if (zr->handle == handle) {
            return val;
        }
    }
    ZEND_HASH_FOREACH_END();
    return NULL;
}

/* **********************
 * channel_stream_ops *
 ********************** */

template <typename Operation>
static ssize_t channel_io(php_ssh2_channel_data *data, bool may_block, Operation operation) {
    auto session = ssh2_get_session(data);
    if (!session) {
        return -1;
    }
    while (data->session_rsrc->ptr == session) {
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
        bool ready = swr_v1_ssh2_channel_wait(session, outbound, data->timeout, &data->timed_out);
        swr_ssh2_forward_leave(session);
        if (!ready) {
            return -1;
        }
    }
    return -1;
}

// Zero means the channel is complete. EAGAIN leaves the stream readable.
static ssize_t channel_completion(php_ssh2_channel_data *data, bool may_block) {
    if (!(libssh2_channel_eof)(data->channel)) {
        return LIBSSH2_ERROR_EAGAIN;
    }
    if (!data->wait_for_remote_close) {
        return 0;
    }
    auto wait_closed = [&] { return libssh2_channel_wait_closed(data->channel); };
    return channel_io(data, may_block, wait_closed);
}

static ssize_t php_ssh2_channel_stream_write(php_stream *stream, const char *buf, size_t count) {
    auto *data = static_cast<php_ssh2_channel_data *>(stream->abstract);
    data->timed_out = false;
    ssize_t result = channel_io(data, data->is_blocking, [&] {
        return (libssh2_channel_write_ex)(data->channel, data->streamid, buf, count);
    });
    return result == LIBSSH2_ERROR_EAGAIN ? 0 : result;
}

static ssize_t php_ssh2_channel_stream_read(php_stream *stream, char *buf, size_t count) {
    auto *data = static_cast<php_ssh2_channel_data *>(stream->abstract);
    data->timed_out = false;
    ssize_t result = channel_io(data, data->is_blocking, [&] {
        return (libssh2_channel_read_ex)(data->channel, data->streamid, buf, count);
    });
    if (!data->session_rsrc->ptr) {
        stream->eof = 1;
        return -1;
    }
    if (result == 0) {
        ssize_t completion = channel_completion(data, data->is_blocking);
        stream->eof = completion == 0;
        if (completion < 0 && completion != LIBSSH2_ERROR_EAGAIN) {
            return -1;
        }
    }
    return result == LIBSSH2_ERROR_EAGAIN ? 0 : result;
}

static int php_ssh2_channel_stream_close(php_stream *stream, int close_handle) {
    php_ssh2_channel_data *abstract = (php_ssh2_channel_data *) stream->abstract;

    if (!abstract->refcount || (--(*(abstract->refcount)) == 0)) {
        /* Last one out, turn off the lights */
        if (abstract->refcount) {
            efree(abstract->refcount);
        }
        if (auto session = ssh2_get_session(abstract)) {
            swr_ssh2_forward_enter(session);
            libssh2_channel_free(abstract->channel);
            swr_ssh2_forward_leave(session);
        }
        zend_list_delete(abstract->session_rsrc);
    }
    efree(abstract);

    return 0;
}

static int php_ssh2_channel_stream_flush(php_stream *stream) {
    php_ssh2_channel_data *abstract = (php_ssh2_channel_data *) stream->abstract;
    auto session = ssh2_get_session(abstract);

    return session ? libssh2_channel_flush_ex(abstract->channel, abstract->streamid) : -1;
}

static int php_ssh2_channel_stream_cast(php_stream *stream, int castas, void **ret) {
    php_ssh2_channel_data *abstract = (php_ssh2_channel_data *) stream->abstract;
    auto session = ssh2_get_session(abstract);
    if (!session) {
        return FAILURE;
    }
    auto **session_data = reinterpret_cast<php_ssh2_session_data **>(libssh2_session_abstract(session));

    switch (castas) {
    case PHP_STREAM_AS_FD:
    case PHP_STREAM_AS_FD_FOR_SELECT:
    case PHP_STREAM_AS_SOCKETD:
        if (ret) {
            *(php_socket_t *) ret = (*session_data)->socket->get_fd();
        }
        return SUCCESS;
    default:
        return FAILURE;
    }
}

static int php_ssh2_channel_stream_set_option(php_stream *stream, int option, int value, void *ptrparam) {
    php_ssh2_channel_data *abstract = (php_ssh2_channel_data *) stream->abstract;
    auto session = ssh2_get_session(abstract);
    int ret;

    switch (option) {
    case PHP_STREAM_OPTION_BLOCKING: {
        ret = abstract->is_blocking;
        abstract->is_blocking = value;
        return ret;
    }
    case PHP_STREAM_OPTION_META_DATA_API: {
        auto *metadata = static_cast<zval *>(ptrparam);
        add_assoc_bool(metadata, "blocked", abstract->is_blocking);
        add_assoc_bool(metadata, "eof", stream->eof);
        add_assoc_bool(metadata, "timed_out", abstract->timed_out);
        if (session) {
            add_assoc_long(metadata, "exit_status", libssh2_channel_get_exit_status(abstract->channel));
            char *signal = nullptr;
            size_t length = 0;
            if (libssh2_channel_get_exit_signal(abstract->channel, &signal, &length, nullptr, nullptr, nullptr, nullptr) == 0 && signal) {
                add_assoc_stringl(metadata, "exit_signal", signal, length);
                libssh2_free(session, signal);
            } else {
                add_assoc_null(metadata, "exit_signal");
            }
        }
        return PHP_STREAM_OPTION_RETURN_OK;
    }
    case PHP_STREAM_OPTION_READ_TIMEOUT: {
#ifdef PHP_SSH2_SESSION_TIMEOUT
        struct timeval tv = *(struct timeval *) ptrparam;
        abstract->timeout = static_cast<double>(tv.tv_sec) + tv.tv_usec / 1000000.0;
        abstract->timed_out = false;
        return PHP_STREAM_OPTION_RETURN_OK;
#else
        php_error_docref(NULL, E_WARNING, "No support for ssh2 stream timeout. Please recompile with libssh2 >= 1.2.9");
        return PHP_STREAM_OPTION_RETURN_ERR;
#endif
    }
    case PHP_STREAM_OPTION_CHECK_LIVENESS: {
        stream->eof = !session || channel_completion(abstract, false) != LIBSSH2_ERROR_EAGAIN;
        return stream->eof ? PHP_STREAM_OPTION_RETURN_ERR : PHP_STREAM_OPTION_RETURN_OK;
    }
    }

    return PHP_STREAM_OPTION_RETURN_NOTIMPL;
}

php_stream_ops php_ssh2_channel_stream_ops = {
    php_ssh2_channel_stream_write,
    php_ssh2_channel_stream_read,
    php_ssh2_channel_stream_close,
    php_ssh2_channel_stream_flush,
    PHP_SSH2_CHANNEL_STREAM_NAME,
    NULL, /* seek */
    php_ssh2_channel_stream_cast,
    NULL, /* stat */
    php_ssh2_channel_stream_set_option,
};

/* *********************
 * Magic Path Helper *
 ********************* */

/* PHP resources stringify as "Resource id #N". Only the authority needs normalization. */
php_url *php_ssh2_parse_url(const char *url) {
    const char *scheme = strstr(url, "://");
    if (!scheme) {
        return nullptr;
    }
    const char *authority = scheme + 3;
    const char *path = strchr(authority, '/');
    php_url *resource;
    constexpr char prefix[] = "Resource id #";
    if (strncmp(authority, prefix, sizeof(prefix) - 1) == 0) {
        char *normalized = estrdup(url);
        const char *id = authority + sizeof(prefix) - 1;
        memcpy(normalized + (authority - url), id, strlen(id) + 1);
        resource = php_url_parse(normalized);
        efree(normalized);
    } else {
        resource = php_url_parse(url);
    }
    // SFTP treats query and fragment delimiters in a filename as literal bytes.
    if (resource && resource->path && path && strpbrk(path, "?#")) {
        zend_string_release(resource->path);
        resource->path = zend_string_init(path, strlen(path), 0);
    }
    return resource;
}

/* {{{ php_ssh2_fopen_wraper_parse_path
 * Parse an ssh2.*:// path
 */
php_url *php_ssh2_fopen_wraper_parse_path(const char *path,
                                          const char *type,
                                          php_stream_context *context,
                                          LIBSSH2_SESSION **psession,
                                          zend_resource **presource,
                                          LIBSSH2_SFTP **psftp,
                                          zend_resource **psftp_rsrc) {
    php_ssh2_sftp_data *sftp_data = NULL;
    LIBSSH2_SESSION *session;
    php_url *resource;
    zval *methods = NULL, *callbacks = NULL, zsession, *tmpzval;
    zend_long resource_id;
    char *username = NULL, *password = NULL, *pubkey_file = NULL, *privkey_file = NULL;
    int username_len = 0, password_len = 0;

    resource = php_ssh2_parse_url(path);
    if (!resource || !resource->path) {
        if (resource) {
            php_url_free(resource);
        }
        return NULL;
    }

    if (strncmp(ZSTR_VAL(resource->scheme), "ssh2.", sizeof("ssh2.") - 1)) {
        /* Not an ssh wrapper */
        php_url_free(resource);
        return NULL;
    }

    if (strcmp(ZSTR_VAL(resource->scheme) + sizeof("ssh2.") - 1, type)) {
        /* Wrong ssh2. wrapper type */
        php_url_free(resource);
        return NULL;
    }

    if (!resource->host) {
        php_url_free(resource);
        return NULL;
    }

    /* Look for a resource ID to reuse a session */
    if (is_numeric_string(ZSTR_VAL(resource->host), ZSTR_LEN(resource->host), &resource_id, NULL, 0) == IS_LONG) {
        php_ssh2_sftp_data *sftp_data;
        zval *zresource;

        if ((zresource = (zval *) php_ssh2_zval_from_resource_handle(resource_id)) == NULL) {
            php_url_free(resource);
            return NULL;
        }

        if (psftp) {
            /* suppress potential warning by passing NULL as resource_type_name */
            sftp_data = (php_ssh2_sftp_data *) zend_fetch_resource(Z_RES_P(zresource), NULL, le_ssh2_sftp);
            if (sftp_data) {
                if (!sftp_data->session_rsrc->ptr) {
                    php_url_free(resource);
                    return nullptr;
                }
                /* Want the sftp layer */
                Z_ADDREF_P(zresource);
                *psftp_rsrc = Z_RES_P(zresource);
                *psftp = sftp_data->sftp;
                *presource = sftp_data->session_rsrc;
                *psession = static_cast<LIBSSH2_SESSION *>(sftp_data->session_rsrc->ptr);
                return resource;
            }
        }
        session =
            (LIBSSH2_SESSION *) zend_fetch_resource(Z_RES_P(zresource), PHP_SSH2_SESSION_RES_NAME, le_ssh2_session);
        if (session) {
            if (psftp) {
                /* We need an sftp layer too */
                LIBSSH2_SFTP *sftp = libssh2_sftp_init(session);

                if (!sftp) {
                    php_url_free(resource);
                    return NULL;
                }
                sftp_data = (php_ssh2_sftp_data *) emalloc(sizeof(php_ssh2_sftp_data));
                sftp_data->sftp = sftp;
                sftp_data->session_rsrc = Z_RES_P(zresource);
                Z_ADDREF_P(zresource);
                *psftp_rsrc = zend_register_resource(sftp_data, le_ssh2_sftp);
                *psftp = sftp;
                *presource = Z_RES_P(zresource);
                *psession = session;
                return resource;
            }
            Z_ADDREF_P(zresource);
            *presource = Z_RES_P(zresource);
            *psession = session;
            return resource;
        }
    }

    /* Fallback on finding it in the context */
    if (ZSTR_VAL(resource->host)[0] == 0 && context && psftp &&
        (tmpzval = php_stream_context_get_option(context, "ssh2", "sftp")) != NULL &&
        Z_TYPE_P(tmpzval) == IS_RESOURCE) {
        php_ssh2_sftp_data *sftp_data;
        sftp_data = (php_ssh2_sftp_data *) zend_fetch_resource(Z_RES_P(tmpzval), PHP_SSH2_SFTP_RES_NAME, le_ssh2_sftp);
        if (sftp_data) {
            if (!sftp_data->session_rsrc->ptr) {
                php_url_free(resource);
                return nullptr;
            }
            Z_ADDREF_P(tmpzval);
            *psftp_rsrc = Z_RES_P(tmpzval);
            *psftp = sftp_data->sftp;
            *presource = sftp_data->session_rsrc;
            *psession = static_cast<LIBSSH2_SESSION *>(sftp_data->session_rsrc->ptr);
            return resource;
        }
    }
    if (ZSTR_VAL(resource->host)[0] == 0 && context &&
        (tmpzval = php_stream_context_get_option(context, "ssh2", "session")) != NULL &&
        Z_TYPE_P(tmpzval) == IS_RESOURCE) {
        session = (LIBSSH2_SESSION *) zend_fetch_resource(Z_RES_P(tmpzval), PHP_SSH2_SESSION_RES_NAME, le_ssh2_session);
        if (session) {
            if (psftp) {
                /* We need an SFTP layer too! */
                LIBSSH2_SFTP *sftp = libssh2_sftp_init(session);
                php_ssh2_sftp_data *sftp_data;

                if (!sftp) {
                    php_url_free(resource);
                    return NULL;
                }
                sftp_data = (php_ssh2_sftp_data *) emalloc(sizeof(php_ssh2_sftp_data));
                sftp_data->sftp = sftp;
                sftp_data->session_rsrc = Z_RES_P(tmpzval);
                Z_ADDREF_P(tmpzval);
                *psftp_rsrc = zend_register_resource(sftp_data, le_ssh2_sftp);
                *psftp = sftp;
                *presource = Z_RES_P(tmpzval);
                *psession = session;
                return resource;
            }
            Z_ADDREF_P(tmpzval);
            *psession = session;
            *presource = Z_RES_P(tmpzval);
            return resource;
        }
    }

    /* Make our own connection then */
    if (!resource->port) {
        resource->port = 22;
    }

    if (context && (tmpzval = php_stream_context_get_option(context, "ssh2", "methods")) != NULL &&
        Z_TYPE_P(tmpzval) == IS_ARRAY) {
        methods = tmpzval;
    }

    if (context && (tmpzval = php_stream_context_get_option(context, "ssh2", "callbacks")) != NULL &&
        Z_TYPE_P(tmpzval) == IS_ARRAY) {
        callbacks = tmpzval;
    }

    if (context && (tmpzval = php_stream_context_get_option(context, "ssh2", "username")) != NULL &&
        Z_TYPE_P(tmpzval) == IS_STRING) {
        username = Z_STRVAL_P(tmpzval);
        username_len = Z_STRLEN_P(tmpzval);
    }

    if (context && (tmpzval = php_stream_context_get_option(context, "ssh2", "password")) != NULL &&
        Z_TYPE_P(tmpzval) == IS_STRING) {
        password = Z_STRVAL_P(tmpzval);
        password_len = Z_STRLEN_P(tmpzval);
    }

    if (context && (tmpzval = php_stream_context_get_option(context, "ssh2", "pubkey_file")) != NULL &&
        Z_TYPE_P(tmpzval) == IS_STRING) {
        pubkey_file = Z_STRVAL_P(tmpzval);
    }

    if (context && (tmpzval = php_stream_context_get_option(context, "ssh2", "privkey_file")) != NULL &&
        Z_TYPE_P(tmpzval) == IS_STRING) {
        privkey_file = Z_STRVAL_P(tmpzval);
    }

    if (resource->user) {
        int len = ZSTR_LEN(resource->user);

        if (len) {
            username = ZSTR_VAL(resource->user);
            username_len = len;
        }
    }

    if (resource->pass) {
        int len = ZSTR_LEN(resource->pass);

        if (len) {
            password = ZSTR_VAL(resource->pass);
            password_len = len;
        }
    }

    if (!username) {
        /* username is a minimum */
        php_url_free(resource);
        return NULL;
    }

    session = php_ssh2_session_connect(ZSTR_VAL(resource->host), resource->port, methods, callbacks);
    if (!session) {
        /* Unable to connect! */
        php_url_free(resource);
        return NULL;
    }

    ZVAL_RES(&zsession, zend_register_resource(session, le_ssh2_session));
    if (!swr_ssh2_wrapper_auth(session, username, username_len, password, password_len, pubkey_file, privkey_file)) {
        php_url_free(resource);
        zend_list_delete(Z_RES(zsession));
        return NULL;
    }

    if (psftp) {
        LIBSSH2_SFTP *sftp;

        sftp = libssh2_sftp_init(session);
        if (!sftp) {
            php_url_free(resource);
            zend_list_delete(Z_RES(zsession));
            return NULL;
        }

        sftp_data = (php_ssh2_sftp_data *) emalloc(sizeof(php_ssh2_sftp_data));
        sftp_data->sftp = sftp;
        sftp_data->session_rsrc = Z_RES(zsession);

        *psftp_rsrc = zend_register_resource(sftp_data, le_ssh2_sftp);
        *psftp = sftp;
    }

    *presource = Z_RES(zsession);
    *psession = session;

    return resource;
}
/* }}} */

/* *****************
 * Shell Wrapper *
 ***************** */

static php_stream *php_ssh2_channel_stream(zend_resource *rsrc, LIBSSH2_CHANNEL *channel, bool wait_for_remote_close);

/* {{{ php_ssh2_shell_open
 * The ssh2.shell:// wrapper's stream: its channel from swr_ssh2_shell_channel (Rust)
 */
static php_stream *php_ssh2_shell_open(LIBSSH2_SESSION *session,
                                       zend_resource *resource,
                                       const char *term,
                                       int term_len,
                                       zval *environment,
                                       long width,
                                       long height,
                                       long type) {
    LIBSSH2_CHANNEL *channel = swr_ssh2_shell_channel(session, term, term_len, environment, width, height, type);
    if (!channel) {
        return NULL;
    }

    /* Turn it into a stream */
    return php_ssh2_channel_stream(resource, channel, true);
}
/* }}} */

/* {{{ php_ssh2_fopen_wrapper_shell
 * ssh2.shell:// fopen wrapper
 */
static php_stream *php_ssh2_fopen_wrapper_shell(php_stream_wrapper *wrapper,
                                                const char *path,
                                                const char *mode,
                                                int options,
                                                zend_string **opened_path,
                                                php_stream_context *context STREAMS_DC) {
    LIBSSH2_SESSION *session = NULL;
    php_stream *stream;
    zval *tmpzval, *environment = NULL;
    const char *terminal = PHP_SSH2_DEFAULT_TERMINAL;
    zend_long width = PHP_SSH2_DEFAULT_TERM_WIDTH;
    zend_long height = PHP_SSH2_DEFAULT_TERM_HEIGHT;
    zend_long type = PHP_SSH2_DEFAULT_TERM_UNIT;
    zend_resource *rsrc = NULL;
    int terminal_len = sizeof(PHP_SSH2_DEFAULT_TERMINAL) - 1;
    php_url *resource;
    char *s;

    resource = php_ssh2_fopen_wraper_parse_path(path, "shell", context, &session, &rsrc, NULL, NULL);
    if (!resource || !session) {
        return NULL;
    }

    if (context && (tmpzval = php_stream_context_get_option(context, "ssh2", "env")) != NULL &&
        Z_TYPE_P(tmpzval) == IS_ARRAY) {
        environment = tmpzval;
    }

    if (context && (tmpzval = php_stream_context_get_option(context, "ssh2", "term")) != NULL &&
        Z_TYPE_P(tmpzval) == IS_STRING) {
        terminal = Z_STRVAL_P(tmpzval);
        terminal_len = Z_STRLEN_P(tmpzval);
    }

    if (context && (tmpzval = php_stream_context_get_option(context, "ssh2", "term_width")) != NULL) {
        zval copyval;
        copyval = *tmpzval;
        convert_to_long(&copyval);
        width = Z_LVAL_P(&copyval);
        zval_ptr_dtor(&copyval);
    }

    if (context && (tmpzval = php_stream_context_get_option(context, "ssh2", "term_height")) != NULL) {
        zval copyval;
        copyval = *tmpzval;
        convert_to_long(&copyval);
        height = Z_LVAL_P(&copyval);
        zval_ptr_dtor(&copyval);
    }

    if (context && (tmpzval = php_stream_context_get_option(context, "ssh2", "term_units")) != NULL) {
        zval copyval;
        copyval = *tmpzval;
        convert_to_long(&copyval);
        type = Z_LVAL_P(&copyval);
        zval_ptr_dtor(&copyval);
    }

    s = resource->path ? ZSTR_VAL(resource->path) : NULL;

    if (s && s[0] == '/') {
        /* Terminal type encoded into URL overrides context terminal type */
        char *p;

        s++;
        p = strchr(s, '/');
        if (p) {
            if (p - s) {
                terminal = s;
                terminal_len = p - terminal;
                s += terminal_len + 1;
            } else {
                /* "null" terminal given, skip it */
                s++;
            }
        } else {
            int len;

            if ((len = strlen(path + 1))) {
                terminal = s;
                terminal_len = len;
                s += len;
            }
        }
    }

    /* TODO: Accept resolution and environment vars as URL style parameters
     * ssh2.shell://hostorresource/terminal/99x99c?envvar=envval&envvar=envval....
     */
    stream = php_ssh2_shell_open(session, rsrc, terminal, terminal_len, environment, width, height, type);
    if (!stream) {
        zend_list_delete(rsrc);
    }
    php_url_free(resource);

    return stream;
}
/* }}} */

static php_stream_wrapper_ops php_ssh2_shell_stream_wops = {php_ssh2_fopen_wrapper_shell,
                                                            NULL, /* stream_close */
                                                            NULL, /* stat */
                                                            NULL, /* stat_url */
                                                            NULL, /* opendir */
                                                            "ssh2.shell"};

php_stream_wrapper php_ssh2_stream_wrapper_shell = {&php_ssh2_shell_stream_wops, NULL, 0};


/* ssh2_shell_resize and ssh2_send_eof are Rust (crates/ssh2/src/php.rs). What
 * they keep C: the stream and its ops table, which the channel data hangs off. */
extern "C" LIBSSH2_CHANNEL *swr_ssh2_stream_channel(zval *zstream, bool abstract_warns) {
    php_stream *stream = (php_stream *) zend_fetch_resource2_ex(zstream, "stream", php_file_le_stream(), php_file_le_pstream());
    if (!stream) {
        return NULL;
    }
    if (stream->ops != &php_ssh2_channel_stream_ops) {
        php_error_docref(NULL, E_WARNING, "Provided stream is not of type " PHP_SSH2_CHANNEL_STREAM_NAME);
        return NULL;
    }
    php_ssh2_channel_data *data = (php_ssh2_channel_data *) stream->abstract;
    if (!data) {
        if (abstract_warns) {
            php_error_docref(NULL, E_WARNING, "Abstract in stream is null");
        }
        return NULL;
    }
    return ssh2_get_session(data) ? data->channel : nullptr;
}

extern "C" LIBSSH2_SESSION *swr_ssh2_stream_session(zval *zstream) {
    php_stream *stream = (php_stream *) Z_RES_P(zstream)->ptr;
    return ssh2_get_session((php_ssh2_channel_data *) stream->abstract);
}

/* ****************
 * Exec Wrapper *
 **************** */

/* {{{ php_ssh2_exec_command
 * The ssh2.exec:// wrapper's stream: its channel from swr_ssh2_exec_channel (Rust)
 */
static php_stream *php_ssh2_exec_command(LIBSSH2_SESSION *session,
                                         zend_resource *rsrc,
                                         char *command,
                                         char *term,
                                         int term_len,
                                         zval *environment,
                                         long width,
                                         long height,
                                         long type) {
    LIBSSH2_CHANNEL *channel =
        swr_ssh2_exec_channel(session, command, term, term_len, environment, width, height, type);
    if (!channel) {
        return NULL;
    }

    /* Turn it into a stream */
    return php_ssh2_channel_stream(rsrc, channel, true);
}
/* }}} */

/* {{{ php_ssh2_fopen_wrapper_exec
 * ssh2.exec:// fopen wrapper
 */
static php_stream *php_ssh2_fopen_wrapper_exec(php_stream_wrapper *wrapper,
                                               const char *path,
                                               const char *mode,
                                               int options,
                                               zend_string **opened_path,
                                               php_stream_context *context STREAMS_DC) {
    LIBSSH2_SESSION *session = NULL;
    php_stream *stream;
    zval *tmpzval, *environment = NULL;
    zend_resource *rsrc = NULL;
    php_url *resource;
    char *terminal = NULL;
    int terminal_len = 0;
    long width = PHP_SSH2_DEFAULT_TERM_WIDTH;
    long height = PHP_SSH2_DEFAULT_TERM_HEIGHT;
    long type = PHP_SSH2_DEFAULT_TERM_UNIT;

    resource = php_ssh2_fopen_wraper_parse_path(path, "exec", context, &session, &rsrc, NULL, NULL);
    if (!resource || !session) {
        return NULL;
    }
    if (!resource->path) {
        php_url_free(resource);
        zend_list_delete(rsrc);
        return NULL;
    }

    if (context && (tmpzval = php_stream_context_get_option(context, "ssh2", "env")) != NULL &&
        Z_TYPE_P(tmpzval) == IS_ARRAY) {
        environment = tmpzval;
    }

    if (context && (tmpzval = php_stream_context_get_option(context, "ssh2", "term")) != NULL &&
        Z_TYPE_P(tmpzval) == IS_STRING) {
        terminal = Z_STRVAL_P(tmpzval);
        terminal_len = Z_STRLEN_P(tmpzval);
    }

    if (context && (tmpzval = php_stream_context_get_option(context, "ssh2", "term_width")) != NULL) {
        zval copyval;
        copyval = *tmpzval;
        convert_to_long(&copyval);
        width = Z_LVAL_P(&copyval);
        zval_ptr_dtor(&copyval);
    }

    if (context && (tmpzval = php_stream_context_get_option(context, "ssh2", "term_height")) != NULL) {
        zval copyval;
        copyval = *tmpzval;
        convert_to_long(&copyval);
        height = Z_LVAL_P(&copyval);
        zval_ptr_dtor(&copyval);
    }

    if (context && (tmpzval = php_stream_context_get_option(context, "ssh2", "term_units")) != NULL) {
        zval *copyval;
        copyval = tmpzval;
        convert_to_long(copyval);
        type = Z_LVAL_P(copyval);
        zval_ptr_dtor(copyval);
    }

    stream = php_ssh2_exec_command(
        session, rsrc, ZSTR_VAL(resource->path) + 1, terminal, terminal_len, environment, width, height, type);
    if (!stream) {
        zend_list_delete(rsrc);
    }
    php_url_free(resource);

    return stream;
}
/* }}} */

static php_stream_wrapper_ops php_ssh2_exec_stream_wops = {php_ssh2_fopen_wrapper_exec,
                                                           NULL, /* stream_close */
                                                           NULL, /* stat */
                                                           NULL, /* stat_url */
                                                           NULL, /* opendir */
                                                           "ssh2.exec"};

php_stream_wrapper php_ssh2_stream_wrapper_exec = {&php_ssh2_exec_stream_wops, NULL, 0};


/* ***************
 * SCP Wrapper *
 *************** */

/* {{{ php_ssh2_scp_xfer
 * Make a stream from a session
 */
static php_stream *php_ssh2_scp_xfer(LIBSSH2_SESSION *session, zend_resource *rsrc, char *filename) {
    LIBSSH2_CHANNEL *channel;
    php_ssh2_channel_data *channel_data;
    php_stream *stream;

#ifdef SW_USE_SSH2_ASYNC_HOOK
    php_ssh2_session_data *session_res = libssh2_session_abstract(session);
#endif
    channel = libssh2_scp_recv(session, filename, NULL);
    if (!channel) {
        char *error;
        libssh2_session_last_error(session, &error, NULL, 0);
        php_error_docref(NULL, E_WARNING, "Unable to request a channel from remote host: %s", error);
        return NULL;
    }

    /* Turn it into a stream */
    channel_data = channel_data_create(channel, rsrc, false);

    stream = php_stream_alloc(&php_ssh2_channel_stream_ops, channel_data, 0, "r");

    return stream;
}
/* }}} */

/* {{{ php_ssh2_fopen_wrapper_scp
 * ssh2.scp:// fopen wrapper (Read mode only, if you want to know why write mode isn't supported as a stream, take a
 * look at the SCP protocol)
 */
static php_stream *php_ssh2_fopen_wrapper_scp(php_stream_wrapper *wrapper,
                                              const char *path,
                                              const char *mode,
                                              int options,
                                              zend_string **opened_path,
                                              php_stream_context *context STREAMS_DC) {
    LIBSSH2_SESSION *session = NULL;
    php_stream *stream;
    zend_resource *rsrc = NULL;
    php_url *resource;

    if (strchr(mode, '+') || strchr(mode, 'a') || strchr(mode, 'w')) {
        return NULL;
    }

    resource = php_ssh2_fopen_wraper_parse_path(path, "scp", context, &session, &rsrc, NULL, NULL);
    if (!resource || !session) {
        return NULL;
    }
    if (!resource->path) {
        php_url_free(resource);
        zend_list_delete(rsrc);
        return NULL;
    }

    stream = php_ssh2_scp_xfer(session, rsrc, ZSTR_VAL(resource->path));
    if (!stream) {
        zend_list_delete(rsrc);
    }
    php_url_free(resource);

    return stream;
}
/* }}} */

static php_stream_wrapper_ops php_ssh2_scp_stream_wops = {php_ssh2_fopen_wrapper_scp,
                                                          NULL, /* stream_close */
                                                          NULL, /* stat */
                                                          NULL, /* stat_url */
                                                          NULL, /* opendir */
                                                          "ssh2.scp"};

php_stream_wrapper php_ssh2_stream_wrapper_scp = {&php_ssh2_scp_stream_wops, NULL, 0};

/* ssh2_scp_recv and ssh2_scp_send are Rust (crates/ssh2/src/php.rs). What
 * ssh2_scp_recv keeps C: the file's
 * struct stat, which libssh2_scp_recv fills and Rust would take an unsafe block
 * to make (budget); only its size is read. */
extern "C" LIBSSH2_CHANNEL *swr_ssh2_scp_recv(LIBSSH2_SESSION *session, const char *path, int64_t *size) {
    struct stat sb = {};
    LIBSSH2_CHANNEL *channel = libssh2_scp_recv(session, path, &sb);
    *size = channel ? sb.st_size : 0;
    return channel;
}


/* ***************************
 * Direct TCP/IP Transport *
 *************************** */

/* {{{ php_ssh2_channel_stream
 * The stream over a channel just opened, its data holding (not referencing) rsrc
 */
static php_stream *php_ssh2_channel_stream(zend_resource *rsrc, LIBSSH2_CHANNEL *channel, bool wait_for_remote_close) {
    php_ssh2_channel_data *channel_data;

    channel_data = channel_data_create(channel, rsrc, wait_for_remote_close);

    return php_stream_alloc(&php_ssh2_channel_stream_ops, channel_data, 0, "r+");
}
/* }}} */

/* {{{ php_ssh2_direct_tcpip
 * Make a stream from a session
 */
static php_stream *php_ssh2_direct_tcpip(LIBSSH2_SESSION *session, zend_resource *rsrc, char *host, int port) {
    LIBSSH2_CHANNEL *channel;

    channel = libssh2_channel_direct_tcpip(session, host, port);
    if (!channel) {
        php_error_docref(NULL, E_WARNING, "Unable to request a channel from remote host");
        return NULL;
    }

    /* Turn it into a stream */
    return php_ssh2_channel_stream(rsrc, channel, false);
}
/* }}} */

/* {{{ php_ssh2_fopen_wrapper_tunnel
 * ssh2.tunnel:// fopen wrapper
 */
static php_stream *php_ssh2_fopen_wrapper_tunnel(php_stream_wrapper *wrapper,
                                                 const char *path,
                                                 const char *mode,
                                                 int options,
                                                 zend_string **opened_path,
                                                 php_stream_context *context STREAMS_DC) {
    LIBSSH2_SESSION *session = NULL;
    php_stream *stream = NULL;
    php_url *resource;
    char *host = NULL;
    int port = 0;
    zend_resource *rsrc;

    resource = php_ssh2_fopen_wraper_parse_path(path, "tunnel", context, &session, &rsrc, NULL, NULL);
    if (!resource || !session) {
        return NULL;
    }

    if (resource->path && ZSTR_VAL(resource->path)[0] == '/') {
        char *colon;

        host = ZSTR_VAL(resource->path) + 1;
        if (*host == '[') {
            /* IPv6 Encapsulated Format */
            host++;
            colon = strstr(host, "]:");
            if (colon) {
                *colon = 0;
                colon += 2;
            }
        } else {
            colon = strchr(host, ':');
            if (colon) {
                *(colon++) = 0;
            }
        }
        if (colon) {
            port = atoi(colon);
        }
    }

    if ((port <= 0) || (port > 65535) || !host || (strlen(host) == 0)) {
        /* Invalid connection criteria */
        php_url_free(resource);
        zend_list_delete(rsrc);
        return NULL;
    }

    stream = php_ssh2_direct_tcpip(session, rsrc, host, port);
    if (!stream) {
        zend_list_delete(rsrc);
    }
    php_url_free(resource);

    return stream;
}
/* }}} */

static php_stream_wrapper_ops php_ssh2_tunnel_stream_wops = {php_ssh2_fopen_wrapper_tunnel,
                                                             NULL, /* stream_close */
                                                             NULL, /* stat */
                                                             NULL, /* stat_url */
                                                             NULL, /* opendir */
                                                             "ssh2.tunnel"};

php_stream_wrapper php_ssh2_stream_wrapper_tunnel = {&php_ssh2_tunnel_stream_wops, NULL, 0};

/* ssh2_tunnel, ssh2_shell and ssh2_exec are Rust (crates/ssh2/src/php.rs). What
 * they keep C: the channel stream, built on this file's stream ops. */
extern "C" void swr_ssh2_session_stream(zval *zsession, LIBSSH2_CHANNEL *channel, bool wait_for_remote_close, zval *return_value) {
    php_stream *stream = php_ssh2_channel_stream(Z_RES_P(zsession), channel, wait_for_remote_close);

    zval_ptr_dtor(return_value);
    if (!stream) {
        RETURN_FALSE;
    }

    /* Ensure that channels are freed BEFORE the sessions they belong to */
    Z_ADDREF_P(zsession);

    php_stream_to_zval(stream, return_value);
}

/* ******************
 * Generic Helper *
 ****************** */

/* ssh2_fetch_stream is Rust (crates/ssh2/src/php.rs). What it keeps C: the
 * substream, a copy of the parent's channel data (allocator), counted in the
 * byte the channel's streams share and built on this file's stream ops. */
extern "C" void swr_ssh2_fetch_substream(zval *zparent, zend_long streamid, zval *return_value) {
    php_ssh2_channel_data *data, *stream_data;
    php_stream *parent, *stream;

    php_stream_from_zval(parent, zparent);

    if (parent->ops != &php_ssh2_channel_stream_ops) {
        php_error_docref(NULL, E_WARNING, "Provided stream is not of type " PHP_SSH2_CHANNEL_STREAM_NAME);
        RETURN_FALSE;
    }

    data = (php_ssh2_channel_data *) parent->abstract;
    if (!data || !ssh2_get_session(data)) {
        RETURN_FALSE;
    }

    if (!data->refcount) {
        data->refcount = (uchar *) emalloc(sizeof(uchar));
        *(data->refcount) = 1;
    }

    if (*(data->refcount) == 255) {
        php_error_docref(NULL, E_WARNING, "Too many streams associated to a single channel");
        RETURN_FALSE;
    }

    stream_data = (php_ssh2_channel_data *) emalloc(sizeof(php_ssh2_channel_data));
    memcpy(stream_data, data, sizeof(php_ssh2_channel_data));
    stream_data->streamid = streamid;
    stream_data->timed_out = false;

    stream = php_stream_alloc(&php_ssh2_channel_stream_ops, stream_data, 0, "r+");
    if (!stream) {
        efree(stream_data);
        php_error_docref(NULL, E_WARNING, "Error opening substream");
        RETURN_FALSE;
    }

    ++*stream_data->refcount;
    php_stream_to_zval(stream, return_value);
}


/*
 * Local variables:
 * tab-width: 4
 * c-basic-offset: 4
 * End:
 * vim600: noet sw=4 ts=4 fdm=marker
 * vim<600: noet sw=4 ts=4
 */

#endif /* SW_HAVE_SSH2LIB */

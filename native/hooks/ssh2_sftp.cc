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

/* What stays C of ext/ssh2's ssh2_sftp.cc (docs/zend.md, What stays C); its PHP
 * functions are crates/ssh2/src/php.rs.
 * - the SFTP file and directory stream ops tables and operations, over their
 *   emalloc'd handle data, and the ssh2.sftp wrapper's opener, url_stat,
 *   unlink, rename, mkdir and rmdir: allocator, globals (the wrapper's
 *   parser).
 * - the SFTP resource's data and destructor: allocator.
 * - LIBSSH2_SFTP_ATTRIBUTES, which ssh2_sftp_stat, lstat and chmod read and
 *   fill through swr_ssh2_sftp_stat and swr_ssh2_sftp_chmod: budget (its
 *   layout would be a repr(C)). */
#include "swoole_rs_phphost.h"
#ifdef SW_HAVE_SSH2LIB

#include "php_ssh2.h"

#include "php_swoole_ssh2_hook.h"

BEGIN_EXTERN_C()
#include "ext/standard/php_string.h"
END_EXTERN_C()

/* *************************
 * Resource Housekeeping *
 ************************* */

void php_ssh2_sftp_dtor(zend_resource *rsrc) {
    php_ssh2_sftp_data *data = (php_ssh2_sftp_data *) rsrc->ptr;

    if (!data) {
        return;
    }

    if (data->session_rsrc->ptr != NULL) {
        libssh2_sftp_shutdown(data->sftp);
    }

    zend_list_delete(data->session_rsrc);

    efree(data);
}

/* *****************
 * SFTP File Ops *
 ***************** */

unsigned long php_ssh2_parse_fopen_modes(char *openmode) {
    unsigned long flags = 0;

    if (strchr(openmode, 'a')) {
        flags |= LIBSSH2_FXF_APPEND;
    }

    if (strchr(openmode, 'w')) {
        flags |= LIBSSH2_FXF_WRITE | LIBSSH2_FXF_TRUNC | LIBSSH2_FXF_CREAT;
    }

    if (strchr(openmode, 'r')) {
        flags |= LIBSSH2_FXF_READ;
    }

    if (strchr(openmode, '+')) {
        flags |= LIBSSH2_FXF_READ | LIBSSH2_FXF_WRITE;
    }

    if (strchr(openmode, 'x')) {
        flags |= LIBSSH2_FXF_WRITE | LIBSSH2_FXF_TRUNC | LIBSSH2_FXF_EXCL | LIBSSH2_FXF_CREAT;
    }

    return flags;
}

static inline int php_ssh2_sftp_attr2ssb(php_stream_statbuf *ssb, LIBSSH2_SFTP_ATTRIBUTES *attrs) {
    memset(ssb, 0, sizeof(php_stream_statbuf));
    if (attrs->flags & LIBSSH2_SFTP_ATTR_SIZE) {
        ssb->sb.st_size = attrs->filesize;
    }

    if (attrs->flags & LIBSSH2_SFTP_ATTR_UIDGID) {
        ssb->sb.st_uid = attrs->uid;
        ssb->sb.st_gid = attrs->gid;
    }
    if (attrs->flags & LIBSSH2_SFTP_ATTR_PERMISSIONS) {
        ssb->sb.st_mode = attrs->permissions;
    }
    if (attrs->flags & LIBSSH2_SFTP_ATTR_ACMODTIME) {
        ssb->sb.st_atime = attrs->atime;
        ssb->sb.st_mtime = attrs->mtime;
    }

    return 0;
}

typedef struct _php_ssh2_sftp_handle_data {
    LIBSSH2_SFTP_HANDLE *handle;
    zend_resource *sftp_rsrc;
} php_ssh2_sftp_handle_data;

static LIBSSH2_SESSION *stream_session(const php_ssh2_sftp_handle_data *data) {
    auto *sftp = static_cast<php_ssh2_sftp_data *>(data->sftp_rsrc->ptr);
    return sftp && sftp->session_rsrc ? static_cast<LIBSSH2_SESSION *>(sftp->session_rsrc->ptr) : nullptr;
}

/* {{{ php_ssh2_sftp_stream_write
 */
static ssize_t php_ssh2_sftp_stream_write(php_stream *stream, const char *buf, size_t count) {
    php_ssh2_sftp_handle_data *data = (php_ssh2_sftp_handle_data *) stream->abstract;
    auto session = stream_session(data);
    return session ? libssh2_sftp_write(data->handle, buf, count) : -1;
}
/* }}} */

/* {{{ php_ssh2_sftp_stream_read
 */
static ssize_t php_ssh2_sftp_stream_read(php_stream *stream, char *buf, size_t count) {
    php_ssh2_sftp_handle_data *data = (php_ssh2_sftp_handle_data *) stream->abstract;
    auto session = stream_session(data);
    ssize_t bytes_read = session ? libssh2_sftp_read(data->handle, buf, count) : -1;
    stream->eof = bytes_read == 0;

    return bytes_read;
}
/* }}} */

/* {{{ php_ssh2_sftp_stream_close
 */
static int php_ssh2_sftp_stream_close(php_stream *stream, int close_handle) {
    php_ssh2_sftp_handle_data *data = (php_ssh2_sftp_handle_data *) stream->abstract;
    if (auto session = stream_session(data)) {
        libssh2_sftp_close(data->handle);
    }
    zend_list_delete(data->sftp_rsrc);
    efree(data);

    return 0;
}
/* }}} */

/* {{{ php_ssh2_sftp_stream_seek
 */
static int php_ssh2_sftp_stream_seek(php_stream *stream, zend_off_t offset, int whence, zend_off_t *newoffset) {
    php_ssh2_sftp_handle_data *data = (php_ssh2_sftp_handle_data *) stream->abstract;
    auto session = stream_session(data);
    if (!session) {
        return -1;
    }

    switch (whence) {
    case SEEK_END: {
        LIBSSH2_SFTP_ATTRIBUTES attrs;
        if (libssh2_sftp_fstat(data->handle, &attrs)) {
            return -1;
        }
        if ((attrs.flags & LIBSSH2_SFTP_ATTR_SIZE) == 0) {
            return -1;
        }
        offset += attrs.filesize;
        break;
    }
    case SEEK_CUR: {
        zend_off_t current_offset = libssh2_sftp_tell(data->handle);

        if (current_offset < 0) {
            return -1;
        }

        offset += current_offset;
        break;
    }
    default:
        return -1;
    }

    libssh2_sftp_seek(data->handle, offset);

    if (newoffset) {
        *newoffset = offset;
    }

    return 0;
}
/* }}} */

/* {{{ php_ssh2_sftp_stream_fstat
 */
static int php_ssh2_sftp_stream_fstat(php_stream *stream, php_stream_statbuf *ssb) {
    php_ssh2_sftp_handle_data *data = (php_ssh2_sftp_handle_data *) stream->abstract;
    LIBSSH2_SFTP_ATTRIBUTES attrs;
    auto session = stream_session(data);
    if (!session) {
        return -1;
    }

    if (libssh2_sftp_fstat(data->handle, &attrs)) {
        return -1;
    }

    return php_ssh2_sftp_attr2ssb(ssb, &attrs);
}
/* }}} */

static php_stream_ops php_ssh2_sftp_stream_ops = {
    php_ssh2_sftp_stream_write,
    php_ssh2_sftp_stream_read,
    php_ssh2_sftp_stream_close,
    NULL, /* flush */
    PHP_SSH2_SFTP_STREAM_NAME,
    php_ssh2_sftp_stream_seek,
    NULL, /* cast */
    php_ssh2_sftp_stream_fstat,
    NULL, /* set_option */
};

/* {{{ php_ssh2_sftp_stream_opener
 */

static php_stream *php_ssh2_sftp_stream_opener(php_stream_wrapper *wrapper,
                                               const char *filename,
                                               const char *mode,
                                               int options,
                                               zend_string **opened_path,
                                               php_stream_context *context STREAMS_DC) {
    php_ssh2_sftp_handle_data *data;
    LIBSSH2_SESSION *session = NULL;
    LIBSSH2_SFTP *sftp = NULL;
    LIBSSH2_SFTP_HANDLE *handle;
    php_stream *stream;
    zend_resource *rsrc = NULL, *sftp_rsrc = NULL;
    php_url *resource;
    unsigned long flags;
    long perms = 0644;

    resource = php_ssh2_fopen_wraper_parse_path(filename, "sftp", context, &session, &rsrc, &sftp, &sftp_rsrc);
    if (!resource || !session || !sftp || !sftp_rsrc) {
        return NULL;
    }

    flags = php_ssh2_parse_fopen_modes((char *) mode);

    handle = libssh2_sftp_open(sftp, ZSTR_VAL(resource->path), flags, perms);
    if (!handle) {
        php_error_docref(NULL, E_WARNING, "Unable to open %s on remote host", filename);
        php_url_free(resource);
        zend_list_delete(sftp_rsrc);
        return NULL;
    }

    data = (php_ssh2_sftp_handle_data *) emalloc(sizeof(php_ssh2_sftp_handle_data));
    *data = {handle, sftp_rsrc};

    stream = php_stream_alloc(&php_ssh2_sftp_stream_ops, data, 0, mode);
    if (!stream) {
        libssh2_sftp_close(handle);
        zend_list_delete(sftp_rsrc);
        efree(data);
    }
    php_url_free(resource);

    return stream;
}
/* }}} */

/* **********************
 * SFTP Directory Ops *
 ********************** */

/* {{{ php_ssh2_sftp_dirstream_read
 */
static ssize_t php_ssh2_sftp_dirstream_read(php_stream *stream, char *buf, size_t count) {
    php_ssh2_sftp_handle_data *data = (php_ssh2_sftp_handle_data *) stream->abstract;
    php_stream_dirent *ent = (php_stream_dirent *) buf;
    auto session = stream_session(data);
    int bytesread = session ? libssh2_sftp_readdir(data->handle, ent->d_name, sizeof(ent->d_name) - 1, NULL) : -1;
    stream->eof = bytesread == 0;
    zend_string *basename;

    if (bytesread <= 0) {
        return bytesread;
    }
    ent->d_name[bytesread] = 0;

    basename = php_basename(ent->d_name, bytesread, NULL, 0);
    if (!basename) {
        return 0;
    }

    bytesread = MIN(sizeof(ent->d_name) - 1, basename->len);
    memcpy(ent->d_name, basename->val, bytesread);
    ent->d_name[bytesread] = 0;
    zend_string_release(basename);

    return sizeof(php_stream_dirent);
}
/* }}} */

static php_stream_ops php_ssh2_sftp_dirstream_ops = {
    NULL, /* write */
    php_ssh2_sftp_dirstream_read,
    php_ssh2_sftp_stream_close,
    NULL, /* flush */
    PHP_SSH2_SFTP_DIRSTREAM_NAME,
    NULL, /* seek */
    NULL, /* cast */
    NULL, /* fstat */
    NULL, /* set_option */
};

/* {{{ php_ssh2_sftp_dirstream_opener
 */
static php_stream *php_ssh2_sftp_dirstream_opener(php_stream_wrapper *wrapper,
                                                  const char *filename,
                                                  const char *mode,
                                                  int options,
                                                  zend_string **opened_path,
                                                  php_stream_context *context STREAMS_DC) {
    php_ssh2_sftp_handle_data *data;
    LIBSSH2_SESSION *session = NULL;
    LIBSSH2_SFTP *sftp = NULL;
    LIBSSH2_SFTP_HANDLE *handle;
    php_stream *stream;
    zend_resource *rsrc = NULL, *sftp_rsrc = NULL;
    php_url *resource;

    resource = php_ssh2_fopen_wraper_parse_path(filename, "sftp", context, &session, &rsrc, &sftp, &sftp_rsrc);
    if (!resource || !session || !sftp) {
        return NULL;
    }

    handle = libssh2_sftp_opendir(sftp, ZSTR_VAL(resource->path));
    if (!handle) {
        php_error_docref(NULL, E_WARNING, "Unable to open %s on remote host", filename);
        php_url_free(resource);
        zend_list_delete(sftp_rsrc);
        return NULL;
    }

    data = (php_ssh2_sftp_handle_data *) emalloc(sizeof(php_ssh2_sftp_handle_data));
    *data = {handle, sftp_rsrc};

    stream = php_stream_alloc(&php_ssh2_sftp_dirstream_ops, data, 0, mode);
    if (!stream) {
        libssh2_sftp_close(handle);
        zend_list_delete(sftp_rsrc);
        efree(data);
    }
    php_url_free(resource);

    return stream;
}
/* }}} */

/* ****************
 * SFTP Wrapper *
 **************** */

/* {{{ php_ssh2_sftp_urlstat
 */
static int php_ssh2_sftp_urlstat(
    php_stream_wrapper *wrapper, const char *url, int flags, php_stream_statbuf *ssb, php_stream_context *context) {
    LIBSSH2_SFTP_ATTRIBUTES attrs;
    LIBSSH2_SESSION *session = NULL;
    LIBSSH2_SFTP *sftp = NULL;
    zend_resource *rsrc = NULL, *sftp_rsrc = NULL;
    php_url *resource;

    resource = php_ssh2_fopen_wraper_parse_path(url, "sftp", context, &session, &rsrc, &sftp, &sftp_rsrc);
    if (!resource || !session || !sftp || !resource->path) {
        return -1;
    }

    if (libssh2_sftp_stat_ex(sftp,
                             ZSTR_VAL(resource->path),
                             ZSTR_LEN(resource->path),
                             (flags & PHP_STREAM_URL_STAT_LINK) ? LIBSSH2_SFTP_LSTAT : LIBSSH2_SFTP_STAT,
                             &attrs)) {
        php_url_free(resource);
        // zend_list_delete(sftp_rsrcid);
        return -1;
    }

    php_url_free(resource);

    /* parse_path addrefs the resource, but we're not holding on to it so we have to delref it before we leave */
    // zend_list_delete(sftp_rsrcid);

    return php_ssh2_sftp_attr2ssb(ssb, &attrs);
}
/* }}} */

/* {{{ php_ssh2_sftp_unlink
 */
static int php_ssh2_sftp_unlink(php_stream_wrapper *wrapper,
                                const char *url,
                                int options,
                                php_stream_context *context) {
    LIBSSH2_SESSION *session = NULL;
    LIBSSH2_SFTP *sftp = NULL;
    zend_resource *rsrc = NULL, *sftp_rsrc = NULL;
    php_url *resource;
    int result;

    resource = php_ssh2_fopen_wraper_parse_path(url, "sftp", context, &session, &rsrc, &sftp, &sftp_rsrc);
    if (!resource || !session || !sftp || !resource->path) {
        if (resource) {
            php_url_free(resource);
        }
        return 0;
    }

    result = libssh2_sftp_unlink(sftp, ZSTR_VAL(resource->path));
    php_url_free(resource);

    // zend_list_delete(sftp_rsrcid);

    /* libssh2 uses 0 for success and the streams API uses 0 for failure, so invert */
    return (result == 0) ? -1 : 0;
}
/* }}} */

/* {{{ php_ssh2_sftp_rename
 */
static int php_ssh2_sftp_rename(
    php_stream_wrapper *wrapper, const char *url_from, const char *url_to, int options, php_stream_context *context) {
    LIBSSH2_SESSION *session = NULL;
    LIBSSH2_SFTP *sftp = NULL;
    zend_resource *rsrc = NULL, *sftp_rsrc = NULL;
    php_url *resource, *resource_to;
    int result;

    if (strncmp(url_from, "ssh2.sftp://", sizeof("ssh2.sftp://") - 1) ||
        strncmp(url_to, "ssh2.sftp://", sizeof("ssh2.sftp://") - 1)) {
        return 0;
    }

    resource_to = php_ssh2_parse_url(url_to);
    if (!resource_to || !resource_to->path) {
        if (resource_to) {
            php_url_free(resource_to);
        }
        return 0;
    }

    resource = php_ssh2_fopen_wraper_parse_path(url_from, "sftp", context, &session, &rsrc, &sftp, &sftp_rsrc);
    if (!resource || !session || !sftp || !resource->path) {
        if (resource) {
            php_url_free(resource);
        }
        php_url_free(resource_to);
        return 0;
    }

    result = libssh2_sftp_rename(sftp, ZSTR_VAL(resource->path), ZSTR_VAL(resource_to->path));
    php_url_free(resource);
    php_url_free(resource_to);

    // zend_list_delete(sftp_rsrcid);

    /* libssh2 uses 0 for success and the streams API uses 0 for failure, so invert */
    return (result == 0) ? -1 : 0;
}
/* }}} */

/* {{{ php_ssh2_sftp_mkdir
 */
static int php_ssh2_sftp_mkdir(
    php_stream_wrapper *wrapper, const char *url, int mode, int options, php_stream_context *context) {
    LIBSSH2_SESSION *session = NULL;
    LIBSSH2_SFTP *sftp = NULL;
    zend_resource *rsrc = NULL, *sftp_rsrc = NULL;
    php_url *resource;
    int result;

    resource = php_ssh2_fopen_wraper_parse_path(url, "sftp", context, &session, &rsrc, &sftp, &sftp_rsrc);
    if (!resource || !session || !sftp || !resource->path) {
        if (resource) {
            php_url_free(resource);
        }
        return 0;
    }

    if (options & PHP_STREAM_MKDIR_RECURSIVE) {
        /* Just attempt to make every directory, some will fail, but we only care about the last success/failure */
        char *p = ZSTR_VAL(resource->path);
        while ((p = strchr(p + 1, '/'))) {
            libssh2_sftp_mkdir_ex(sftp, ZSTR_VAL(resource->path), p - ZSTR_VAL(resource->path), mode);
        }
    }

    result = libssh2_sftp_mkdir(sftp, ZSTR_VAL(resource->path), mode);
    php_url_free(resource);

    // zend_list_delete(sftp_rsrcid);

    /* libssh2 uses 0 for success and the streams API uses 0 for failure, so invert */
    return (result == 0) ? -1 : 0;
}
/* }}} */

/* {{{ php_ssh2_sftp_rmdir
 */
static int php_ssh2_sftp_rmdir(php_stream_wrapper *wrapper, const char *url, int options, php_stream_context *context) {
    LIBSSH2_SESSION *session = NULL;
    LIBSSH2_SFTP *sftp = NULL;
    zend_resource *rsrc = NULL, *sftp_rsrc = NULL;
    php_url *resource;
    int result;

    resource = php_ssh2_fopen_wraper_parse_path(url, "sftp", context, &session, &rsrc, &sftp, &sftp_rsrc);
    if (!resource || !session || !sftp || !resource->path) {
        if (resource) {
            php_url_free(resource);
        }
        return 0;
    }

    result = libssh2_sftp_rmdir(sftp, ZSTR_VAL(resource->path));
    php_url_free(resource);

    // zend_list_delete(sftp_rsrcid);

    /* libssh2 uses 0 for success and the streams API uses 0 for failure, so invert */
    return (result == 0) ? -1 : 0;
}
/* }}} */

static php_stream_wrapper_ops php_ssh2_sftp_wrapper_ops = {
    php_ssh2_sftp_stream_opener,
    NULL, /* close */
    NULL, /* stat */
    php_ssh2_sftp_urlstat,
    php_ssh2_sftp_dirstream_opener,
    PHP_SSH2_SFTP_WRAPPER_NAME,
    php_ssh2_sftp_unlink,
    php_ssh2_sftp_rename,
    php_ssh2_sftp_mkdir,
    php_ssh2_sftp_rmdir,
};

php_stream_wrapper php_ssh2_sftp_wrapper = {
    &php_ssh2_sftp_wrapper_ops,
    NULL,
    1,
};

/* *****************
 * Userspace API *
 ***************** */

/* ssh2_sftp is Rust (crates/ssh2/src/php.rs). What it keeps C: the SFTP
 * resource's data, emalloc'd here and freed by its destructor (allocator). */
extern "C" void swr_ssh2_sftp_open(zval *zsession, LIBSSH2_SESSION *session, LIBSSH2_SFTP *sftp, zval *return_value) {
    php_ssh2_sftp_data *data = (php_ssh2_sftp_data *) emalloc(sizeof(php_ssh2_sftp_data));
    data->session = session;
    data->sftp = sftp;
    data->session_rsrc = Z_RES_P(zsession);
    Z_ADDREF_P(zsession);

    zval_ptr_dtor(return_value);
    ZVAL_RES(return_value, zend_register_resource(data, le_ssh2_sftp));
}

/* Much of the stuff below can be done via wrapper ops as of PHP5, but is included here for PHP 4.3 users */

/* ssh2_sftp_rename, _unlink, _mkdir, _rmdir, _symlink, _readlink and _realpath are Rust
 * (crates/ssh2/src/php.rs); they read an SFTP resource through these. */
extern "C" LIBSSH2_SFTP *swr_ssh2_sftp(zval *zsftp) {
    php_ssh2_sftp_data *data =
        (php_ssh2_sftp_data *) zend_fetch_resource(Z_RES_P(zsftp), PHP_SSH2_SFTP_RES_NAME, le_ssh2_sftp);
    return data ? data->sftp : NULL;
}

extern "C" LIBSSH2_SESSION *swr_ssh2_sftp_session(zval *zsftp) {
    return ((php_ssh2_sftp_data *) Z_RES_P(zsftp)->ptr)->session;
}

extern "C" bool swr_ssh2_sftp_session_closed(zval *zsftp) {
    return ((php_ssh2_sftp_data *) Z_RES_P(zsftp)->ptr)->session_rsrc->ptr == NULL;
}


/* ssh2_sftp_chmod, ssh2_sftp_stat and ssh2_sftp_lstat are Rust
 * (crates/ssh2/src/php.rs). What they keep C: LIBSSH2_SFTP_ATTRIBUTES, which
 * libssh2's header lays out and Rust would need a repr(C) for (budget); these
 * fill it or read it out, each call retried through the hook. */
extern "C" int swr_ssh2_sftp_stat(LIBSSH2_SFTP *sftp,
                                  LIBSSH2_SESSION *session,
                                  const char *path,
                                  unsigned int path_len,
                                  int stat_type,
                                  unsigned long *flags,
                                  libssh2_uint64_t *size,
                                  unsigned long *uid,
                                  unsigned long *gid,
                                  unsigned long *permissions,
                                  unsigned long *atime,
                                  unsigned long *mtime) {
    /* Zeroed: only the attributes the server sent are filled, and every one is copied out. */
    LIBSSH2_SFTP_ATTRIBUTES attrs = {};
    int rc = libssh2_sftp_stat_ex(sftp, path, path_len, stat_type, &attrs);
    if (rc) {
        return rc;
    }
    *flags = attrs.flags;
    *size = attrs.filesize;
    *uid = attrs.uid;
    *gid = attrs.gid;
    *permissions = attrs.permissions;
    *atime = attrs.atime;
    *mtime = attrs.mtime;
    return 0;
}

extern "C" int swr_ssh2_sftp_chmod(
    LIBSSH2_SFTP *sftp, LIBSSH2_SESSION *session, const char *path, unsigned int path_len, unsigned long permissions) {
    LIBSSH2_SFTP_ATTRIBUTES attrs = {};
    attrs.permissions = permissions;
    attrs.flags = LIBSSH2_SFTP_ATTR_PERMISSIONS;
    return libssh2_sftp_stat_ex(sftp, path, path_len, LIBSSH2_SFTP_SETSTAT, &attrs);
}


/* ssh2_sftp_readlink and ssh2_sftp_realpath are Rust (crates/ssh2/src/php.rs). */

/*
 * Local variables:
 * tab-width: 4
 * c-basic-offset: 4
 * indent-tabs-mode: t
 * End:
 */

#endif /* SW_HAVE_SSH2LIB */

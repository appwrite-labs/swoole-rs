/*
  +----------------------------------------------------------------------+
  | Copyright (c) The PHP Group                                          |
  +----------------------------------------------------------------------+
  | This source file is subject to version 3.01 of the PHP license,      |
  | that is bundled with this package in the file LICENSE, and is        |
  | available through the world-wide-web at the following url:           |
  | https://www.php.net/license/3_01.txt                                 |
  | If you did not receive a copy of the PHP license and are unable to   |
  | obtain it through the world-wide-web, please send a note to          |
  | license@php.net so we can mail you a copy immediately.               |
  +----------------------------------------------------------------------+
  | Authors: Wez Furlong <wez@thebrainroom.com>                          |
  +----------------------------------------------------------------------+
*/

/* What is left here of swoole-src's copy of PHP's plain-files wrapper: the stdio stream (its
 * eight operations, fopen and fopen_from_fd), the directory stream and the two tables. The
 * wrapper's path operations and the fopen mode helpers are crates/phpruntime's plain_files.
 *
 * It stays C. Enabling the hook copies these operations over PHP's own php_stream_stdio_ops,
 * so they run on streams PHP itself opened: stream->abstract is PHP's php_stdio_stream_data,
 * a struct private to PHP's main/streams/plain_wrapper.c whose fields differ by PHP version
 * and by HAVE_FLUSHIO and HAVE_MMAP. Streams are made with php_stream_alloc and the
 * persistent-stream macros, which take the caller's file and line, and php_stream_dirent
 * gained d_type in 8.3. The Windows branches are gone: this build has none. */

#include "php_swoole.h"
#include "php_open_temporary_file.h"
#include "ext/standard/file.h"
#include "ext/standard/flock_compat.h"
#include "ext/standard/php_filestat.h"

#include <fcntl.h>

#if PHP_VERSION_ID >= 80400
#ifdef HAVE_SYS_WAIT_H
#include <sys/wait.h>
#endif
#ifdef HAVE_SYS_FILE_H
#include <sys/file.h>
#endif
#else
#if HAVE_SYS_WAIT_H
#include <sys/wait.h>
#endif
#if HAVE_SYS_FILE_H
#include <sys/file.h>
#endif
#endif

#ifdef HAVE_SYS_MMAN_H
#include <sys/mman.h>
#endif

#include "swoole_rs_phpruntime.h"
#include "swoole_file_hook.h"

#define SW_ASYNC_FILE_PROTOCOL "async.file"
typedef ssize_t php_stream_size_t;

/* PHP's plain-files wrapper and stdio ops, saved at RINIT: a hook swaps them out. */
static php_stream_wrapper ori_php_plain_files_wrapper;
static php_stream_ops ori_php_stream_stdio_ops;

#define sw_php_stream_fopen_from_fd_rel(fd, mode, persistent_id, zero_position)                                         \
    _sw_php_stream_fopen_from_fd((fd), (mode), (persistent_id), (zero_position) STREAMS_REL_CC)

#define sw_php_stream_fopen_from_fd_int_rel(fd, mode, persistent_id)                                                    \
    _sw_php_stream_fopen_from_fd_int((fd), (mode), (persistent_id) STREAMS_REL_CC)

#define sw_php_stream_fopen_rel(path, mode, opened_path, options)                                                       \
    _sw_php_stream_fopen(path, mode, opened_path, options STREAMS_REL_CC)

#define PLAIN_WRAP_BUF_SIZE(st) (st)

#if PHP_VERSION_ID >= 80100
#if !defined(HAVE_FDATASYNC)
#define fdatasync fsync
#elif defined(__APPLE__)
// The symbol is present, however not in the headers
extern int fdatasync(int);
#endif
#endif


static php_stream_size_t sw_php_stdiop_write(php_stream *stream, const char *buf, size_t count);
static php_stream_size_t sw_php_stdiop_read(php_stream *stream, char *buf, size_t count);
static int sw_php_stdiop_close(php_stream *stream, int close_handle);
static int sw_php_stdiop_stat(php_stream *stream, php_stream_statbuf *ssb);
static int sw_php_stdiop_flush(php_stream *stream);
static int sw_php_stdiop_seek(php_stream *stream, zend_off_t offset, int whence, zend_off_t *newoffset);
static int sw_php_stdiop_set_option(php_stream *stream, int option, int value, void *ptrparam);
static int sw_php_stdiop_cast(php_stream *stream, int castas, void **ret);
static php_stream *_sw_php_stream_fopen_from_fd_int(int fd, const char *mode, const char *persistent_id STREAMS_DC);
#if PHP_VERSION_ID >= 80200
static php_stream *_sw_php_stream_fopen_from_fd(int fd, const char *mode, const char *persistent_id, bool zero_position STREAMS_DC);
#else
static php_stream *_sw_php_stream_fopen_from_fd(int fd, const char *mode, const char *persistent_id STREAMS_DC);
#endif

/* {{{ ------- STDIO stream implementation -------*/

typedef struct {
    FILE *file;
    int fd;					/* underlying file descriptor */
    unsigned is_process_pipe:1;	                /* use pclose instead of fclose */
    unsigned is_pipe:1;		                /* stream is an actual pipe, currently Windows only*/
    unsigned cached_fstat:1;	                /* sb is valid */
    unsigned is_pipe_blocking:1;                /* allow blocking read() on pipes, currently Windows only */
    unsigned no_forced_fstat:1;                 /* Use fstat cache even if forced */
    unsigned is_seekable:1;		        /* don't try and seek, if not set */
    unsigned can_poll:1;
    unsigned _reserved:25;

    int lock_flag;			        /* stores the lock state */
    zend_string *temp_name;	                /* if non-null, this is the path to a temporary file that
						 * is to be deleted when the stream is closed */
#ifdef HAVE_FLUSHIO
    char last_op;
#endif

#ifdef HAVE_MMAP
    char *last_mapped_addr;
    size_t last_mapped_len;
#endif

    zend_stat_t sb;
} php_stdio_stream_data;
#define PHP_STDIOP_GET_FD(anfd, data)	anfd = (data)->file ? fileno((data)->file) : (data)->fd

static php_stream_ops sw_php_stream_stdio_ops = {
    sw_php_stdiop_write,
    sw_php_stdiop_read,
    sw_php_stdiop_close,
    sw_php_stdiop_flush,
    "STDIO/coroutine",
    sw_php_stdiop_seek,
    sw_php_stdiop_cast,
    sw_php_stdiop_stat,
    sw_php_stdiop_set_option,
};

static int do_fstat(php_stdio_stream_data *d, int force) {
    if (!d->cached_fstat || force) {
        int fd;
        int r;

        PHP_STDIOP_GET_FD(fd, d);
        r = fstat(fd, &d->sb);
        d->cached_fstat = r == 0;

        return r;
    }
    return 0;
}

static php_stream *_sw_php_stream_fopen_from_fd_int(int fd, const char *mode, const char *persistent_id STREAMS_DC) {
    php_stdio_stream_data *self = (php_stdio_stream_data *) pemalloc(sizeof(*self), persistent_id);
    memset(self, 0, sizeof(*self));
    self->file = NULL;
    self->is_seekable = 1;
    self->is_pipe = 0;
    self->lock_flag = LOCK_UN;
    self->is_process_pipe = 0;
    self->temp_name = NULL;
    self->fd = fd;
    return php_stream_alloc_rel(&sw_php_stream_stdio_ops, self, persistent_id, mode);
}

static void _sw_detect_is_seekable(php_stdio_stream_data *self) {
#if defined(S_ISFIFO) && defined(S_ISCHR)
    if (self->fd >= 0 && do_fstat(self, 0) == 0) {
        self->is_seekable = !(S_ISFIFO(self->sb.st_mode) || S_ISCHR(self->sb.st_mode));
        self->is_pipe = S_ISFIFO(self->sb.st_mode);
        self->can_poll = S_ISFIFO(self->sb.st_mode) || S_ISSOCK(self->sb.st_mode) || S_ISCHR(self->sb.st_mode);
        if (self->can_poll) {
            swoole_coroutine_socket_create(self->fd);
        }
    }
#endif
}

static php_stream *_sw_php_stream_fopen_from_fd(int fd, const char *mode, const char *persistent_id, bool zero_position STREAMS_DC) {
    php_stream *stream = sw_php_stream_fopen_from_fd_int_rel(fd, mode, persistent_id);

    if (stream) {
        php_stdio_stream_data *self = (php_stdio_stream_data *) stream->abstract;

        _sw_detect_is_seekable(self);
        if (!self->is_seekable) {
            stream->flags |= PHP_STREAM_FLAG_NO_SEEK;
            stream->position = -1;
        } else if (zero_position) {
            ZEND_ASSERT(lseek(self->fd, 0, SEEK_CUR) == 0);
            stream->position = 0;
        }
        else {
            stream->position = lseek(self->fd, 0, SEEK_CUR);
#ifdef ESPIPE
            /* FIXME: Is this code still needed? */
            if (stream->position == (zend_off_t)-1 && errno == ESPIPE) {
                stream->flags |= PHP_STREAM_FLAG_NO_SEEK;
                self->is_seekable = 0;
            }
#endif
        }
    }

    return stream;
}

static php_stream_size_t sw_php_stdiop_write(php_stream *stream, const char *buf, size_t count) {
    php_stdio_stream_data *data = (php_stdio_stream_data *) stream->abstract;

    assert(data != NULL);

    if (data->fd >= 0) {
        ssize_t bytes_written = write(data->fd, buf, count);
        if (bytes_written < 0) {
            if (PHP_IS_TRANSIENT_ERROR(errno)) {
                return 0;
            }
            if (errno == EINTR) {
                /* TODO: Should this be treated as a proper error or not? */
                return bytes_written;
            }
            if (!(stream->flags & PHP_STREAM_FLAG_SUPPRESS_ERRORS)) {
                php_error_docref(NULL, E_NOTICE, "Write of %zu bytes failed with errno=%d %s", count, errno, strerror(errno));
            }
        }
        return bytes_written;
    } else {
#ifdef HAVE_FLUSHIO
        if (data->is_seekable && data->last_op == 'r') {
            fseek(data->file, 0, SEEK_CUR);
        }
        data->last_op = 'w';
#endif

        return (ssize_t) fwrite(buf, 1, count, data->file);
    }
}

static php_stream_size_t sw_php_stdiop_read(php_stream *stream, char *buf, size_t count) {
    php_stdio_stream_data *data = (php_stdio_stream_data *) stream->abstract;
    ssize_t ret;

    assert(data != NULL);

    if (data->fd >= 0) {
        ret = read(data->fd, buf, PLAIN_WRAP_BUF_SIZE(count));

        if (ret == (ssize_t) -1 && errno == EINTR) {
            /* Read was interrupted, retry once,
             If read still fails, giveup with feof==0
             so script can retry if desired */
            ret = read(data->fd, buf, PLAIN_WRAP_BUF_SIZE(count));
        }

        if (ret < 0) {
            if (PHP_IS_TRANSIENT_ERROR(errno)) {
                /* Not an error. */
                ret = 0;
            } else if (errno == EINTR) {
                /* TODO: Should this be treated as a proper error or not? */
            } else {
                if (!(stream->flags & PHP_STREAM_FLAG_SUPPRESS_ERRORS)) {
                    php_error_docref(NULL, E_NOTICE, "Read of %zu bytes failed with errno=%d %s", count, errno, strerror(errno));
                }

                /* TODO: Remove this special-case? */
                if (errno != EBADF) {
                    stream->eof = 1;
                }
            }
        } else if (ret == 0) {
            stream->eof = 1;
        }

    } else {
#ifdef HAVE_FLUSHIO
        if (data->is_seekable && data->last_op == 'w')
            fseek(data->file, 0, SEEK_CUR);
        data->last_op = 'r';
#endif
        ret = fread(buf, 1, count, data->file);

        stream->eof = feof(data->file);
    }
    return ret;
}

static int sw_php_stdiop_close(php_stream *stream, int close_handle) {
    int ret;
    php_stdio_stream_data *data = (php_stdio_stream_data *) stream->abstract;

    assert(data != NULL);

#ifdef HAVE_MMAP
    if (data->last_mapped_addr) {
        munmap(data->last_mapped_addr, data->last_mapped_len);
        data->last_mapped_addr = NULL;
    }
#endif

    if (close_handle) {
        if (data->file) {
            if (data->is_process_pipe) {
                errno = 0;
                ret = pclose(data->file);

#if HAVE_SYS_WAIT_H
                if (WIFEXITED(ret)) {
                    ret = WEXITSTATUS(ret);
                }
#endif
            } else {
                ret = fclose(data->file);
                data->file = NULL;
            }
        } else if (data->fd != -1) {
            if ((data->lock_flag & LOCK_EX) || (data->lock_flag & LOCK_SH)) {
                swoole_coroutine_flock(data->fd, LOCK_UN);
            }
            ret = close(data->fd);
            data->fd = -1;
        } else {
            return 0; /* everything should be closed already -> success */
        }
        if (data->temp_name) {
            unlink(ZSTR_VAL(data->temp_name));
            /* temporary streams are never persistent */
            zend_string_release_ex(data->temp_name, 0);
            data->temp_name = NULL;
        }
    } else {
        ret = 0;
        data->file = NULL;
        data->fd = -1;
    }

    pefree(data, stream->is_persistent);

    return ret;
}

static int sw_php_stdiop_flush(php_stream *stream) {
    php_stdio_stream_data *data = (php_stdio_stream_data *) stream->abstract;

    assert(data != NULL);

    /*
     * stdio buffers data in user land. By calling fflush(3), this
     * data is send to the kernel using write(2). fsync'ing is
     * something completely different.
     */
    if (data->file) {
        return fflush(data->file);
    }
    return 0;
}

#if PHP_VERSION_ID >= 80100
static int php_stdiop_sync(php_stream *stream, bool dataonly)
{
    php_stdio_stream_data *data = (php_stdio_stream_data*)stream->abstract;
    FILE *fp;
    int fd;

    if (php_stream_cast(stream, PHP_STREAM_AS_STDIO, (void**)&fp, REPORT_ERRORS) == FAILURE) {
        return -1;
    }

    if (sw_php_stdiop_flush(stream) == 0) {
        PHP_STDIOP_GET_FD(fd, data);
        if (dataonly) {
            return fdatasync(fd);
        } else {
            return fsync(fd);
        }
    }
    return -1;
}
#endif

static int sw_php_stdiop_seek(php_stream *stream, zend_off_t offset, int whence, zend_off_t *newoffset) {
    php_stdio_stream_data *data = (php_stdio_stream_data *) stream->abstract;
    int ret;

    assert(data != NULL);

    if (!data->is_seekable) {
        php_error_docref(NULL, E_WARNING, "Cannot seek on this stream");
        return -1;
    }

    if (data->fd >= 0) {
        zend_off_t result;

        result = lseek(data->fd, offset, whence);
        if (result == (zend_off_t) -1) return -1;

        *newoffset = result;
        return 0;

    } else {
        ret = fseek(data->file, offset, whence);
        *newoffset = ftell(data->file);
        return ret;
    }
}

static int sw_php_stdiop_cast(php_stream *stream, int castas, void **ret) {
    php_socket_t fd;
    php_stdio_stream_data *data = (php_stdio_stream_data *) stream->abstract;

    assert(data != NULL);

    /* as soon as someone touches the stdio layer, buffering may ensue,
     * so we need to stop using the fd directly in that case */

    switch (castas) {
    case PHP_STREAM_AS_STDIO:
        if (ret) {
            if (data->file == NULL) {
                /* we were opened as a plain file descriptor, so we
                 * need fdopen now */
                char fixed_mode[5];
                swr_v1_phpruntime_plain_fdopen_mode(stream->mode, (uint8_t *) fixed_mode);
                data->file = fdopen(data->fd, fixed_mode);
                if (data->file == NULL) {
                    return FAILURE;
                }
            }

            *(FILE **) ret = data->file;
            data->fd = SOCK_ERR;
        }
        return SUCCESS;

    case PHP_STREAM_AS_FD_FOR_SELECT:
        PHP_STDIOP_GET_FD(fd, data);
        if (SOCK_ERR == fd) {
            return FAILURE;
        }
        if (ret) {
            *(php_socket_t *) ret = fd;
        }
        return SUCCESS;

    case PHP_STREAM_AS_FD:
        PHP_STDIOP_GET_FD(fd, data);

        if (SOCK_ERR == fd) {
            return FAILURE;
        }
        if (data->file) {
            fflush(data->file);
        }
        if (ret) {
            *(php_socket_t *) ret = fd;
        }
        return SUCCESS;
    default:
        return FAILURE;
    }
}

static int sw_php_stdiop_stat(php_stream *stream, php_stream_statbuf *ssb) {
    int ret;
    php_stdio_stream_data *data = (php_stdio_stream_data *) stream->abstract;

    assert(data != NULL);
    if ((ret = do_fstat(data, 1)) == 0) {
        memcpy(&ssb->sb, &data->sb, sizeof(ssb->sb));
    }

    return ret;
}

static int sw_php_stdiop_set_option(php_stream *stream, int option, int value, void *ptrparam) {
    php_stdio_stream_data *data = (php_stdio_stream_data *) stream->abstract;
    size_t size;
    int fd;
#ifdef O_NONBLOCK
    /* FIXME: make this work for win32 */
    int flags;
    int oldval;
#endif

    PHP_STDIOP_GET_FD(fd, data);

    switch (option) {
    case PHP_STREAM_OPTION_BLOCKING:
        if (fd == -1) return -1;
#ifdef O_NONBLOCK
        flags = fcntl(fd, F_GETFL, 0);
        oldval = (flags & O_NONBLOCK) ? 0 : 1;
        if (value)
            flags &= ~O_NONBLOCK;
        else
            flags |= O_NONBLOCK;

        if (-1 == fcntl(fd, F_SETFL, flags)) return -1;
        return oldval;
#else
        return -1; /* not yet implemented */
#endif

    case PHP_STREAM_OPTION_WRITE_BUFFER:

        if (data->file == NULL) {
            return -1;
        }

        if (ptrparam)
            size = *(size_t *) ptrparam;
        else
            size = BUFSIZ;

        switch (value) {
        case PHP_STREAM_BUFFER_NONE:
            return setvbuf(data->file, NULL, _IONBF, 0);

        case PHP_STREAM_BUFFER_LINE:
            return setvbuf(data->file, NULL, _IOLBF, size);

        case PHP_STREAM_BUFFER_FULL:
            return setvbuf(data->file, NULL, _IOFBF, size);

        default:
            return -1;
        }
        break;

    case PHP_STREAM_OPTION_LOCKING:
        if (fd == -1) {
            return -1;
        }
#if PHP_VERSION_ID >= 80300
        if ((uintptr_t) ptrparam == PHP_STREAM_LOCK_SUPPORTED) {
            return 0;
        }
#else
        if ((zend_uintptr_t) ptrparam == PHP_STREAM_LOCK_SUPPORTED) {
            return 0;
        }
#endif

        if (!swoole_coroutine_flock(fd, value)) {
            data->lock_flag = value;
            return 0;
        } else {
            return -1;
        }
        break;

    case PHP_STREAM_OPTION_MMAP_API:
#if HAVE_MMAP
    {
        php_stream_mmap_range *range = (php_stream_mmap_range*)ptrparam;
        int prot, flags;

        switch (value) {
        case PHP_STREAM_MMAP_SUPPORTED:
            return fd == -1 ? PHP_STREAM_OPTION_RETURN_ERR : PHP_STREAM_OPTION_RETURN_OK;

        case PHP_STREAM_MMAP_MAP_RANGE:
            if (do_fstat(data, 1) != 0) {
                return PHP_STREAM_OPTION_RETURN_ERR;
            }
            if (range->offset > (size_t) data->sb.st_size) {
                range->offset = data->sb.st_size;
            }
            if (range->length == 0 ||
                range->length > data->sb.st_size - range->offset) {
                range->length = data->sb.st_size - range->offset;
            }
            switch (range->mode) {
            case PHP_STREAM_MAP_MODE_READONLY:
                prot = PROT_READ;
                flags = MAP_PRIVATE;
                break;
            case PHP_STREAM_MAP_MODE_READWRITE:
                prot = PROT_READ | PROT_WRITE;
                flags = MAP_PRIVATE;
                break;
            case PHP_STREAM_MAP_MODE_SHARED_READONLY:
                prot = PROT_READ;
                flags = MAP_SHARED;
                break;
            case PHP_STREAM_MAP_MODE_SHARED_READWRITE:
                prot = PROT_READ | PROT_WRITE;
                flags = MAP_SHARED;
                break;
            default:
                return PHP_STREAM_OPTION_RETURN_ERR;
            }
            range->mapped = (char*)mmap(NULL, range->length, prot, flags, fd, range->offset);
            if (range->mapped == (char*)MAP_FAILED) {
                range->mapped = NULL;
                return PHP_STREAM_OPTION_RETURN_ERR;
            }
            /* remember the mapping */
            data->last_mapped_addr = range->mapped;
            data->last_mapped_len = range->length;
            return PHP_STREAM_OPTION_RETURN_OK;

        case PHP_STREAM_MMAP_UNMAP:
            if (data->last_mapped_addr) {
                munmap(data->last_mapped_addr, data->last_mapped_len);
                data->last_mapped_addr = NULL;

                return PHP_STREAM_OPTION_RETURN_OK;
            }
            return PHP_STREAM_OPTION_RETURN_ERR;
        }
    }
#endif
        return PHP_STREAM_OPTION_RETURN_NOTIMPL;

#if PHP_VERSION_ID >= 80100
    case PHP_STREAM_OPTION_SYNC_API:
        switch (value) {
        case PHP_STREAM_SYNC_SUPPORTED:
            return fd == -1 ? PHP_STREAM_OPTION_RETURN_ERR : PHP_STREAM_OPTION_RETURN_OK;
        case PHP_STREAM_SYNC_FSYNC:
            return php_stdiop_sync(stream, 0) == 0 ? PHP_STREAM_OPTION_RETURN_OK : PHP_STREAM_OPTION_RETURN_ERR;
        case PHP_STREAM_SYNC_FDSYNC:
            return php_stdiop_sync(stream, 1) == 0 ? PHP_STREAM_OPTION_RETURN_OK : PHP_STREAM_OPTION_RETURN_ERR;
        }
        /* Invalid option passed */
        return PHP_STREAM_OPTION_RETURN_ERR;
#endif

    case PHP_STREAM_OPTION_TRUNCATE_API:
        switch (value) {
        case PHP_STREAM_TRUNCATE_SUPPORTED:
            return fd == -1 ? PHP_STREAM_OPTION_RETURN_ERR : PHP_STREAM_OPTION_RETURN_OK;

        case PHP_STREAM_TRUNCATE_SET_SIZE: {
            ptrdiff_t new_size = *(ptrdiff_t*)ptrparam;
            if (new_size < 0) {
                return PHP_STREAM_OPTION_RETURN_ERR;
            }
            return ftruncate(fd, new_size) == 0 ? PHP_STREAM_OPTION_RETURN_OK : PHP_STREAM_OPTION_RETURN_ERR;
        }
        }
        return PHP_STREAM_OPTION_RETURN_NOTIMPL;

    case PHP_STREAM_OPTION_META_DATA_API:
        if (fd == -1) return -1;
#ifdef O_NONBLOCK
        flags = fcntl(fd, F_GETFL, 0);

        add_assoc_bool((zval *) ptrparam, "timed_out", 0);
        add_assoc_bool((zval *) ptrparam, "blocked", (flags & O_NONBLOCK) ? 0 : 1);
        add_assoc_bool((zval *) ptrparam, "eof", stream->eof);

        return PHP_STREAM_OPTION_RETURN_OK;
#endif
        return -1;
    default:
        return PHP_STREAM_OPTION_RETURN_NOTIMPL;
    }
}
/* }}} */

/* {{{ plain files opendir/readdir implementation */
static php_stream_size_t php_plain_files_dirstream_read(php_stream *stream, char *buf, size_t count) {
    DIR *dir = (DIR *) stream->abstract;
    struct dirent *result;
    php_stream_dirent *ent = (php_stream_dirent *) buf;

    /* avoid problems if someone mis-uses the stream */
    if (count != sizeof(php_stream_dirent)) {
        return -1;
    }

    result = readdir(dir);
    if (result) {
        PHP_STRLCPY(ent->d_name, result->d_name, sizeof(ent->d_name), strlen(result->d_name));
#if PHP_VERSION_ID >= 80300
#ifdef _DIRENT_HAVE_D_TYPE
        ent->d_type = result->d_type;
#else
        ent->d_type = DT_UNKNOWN;
#endif
#endif
        return sizeof(php_stream_dirent);
    }
    return 0;
}

static int php_plain_files_dirstream_close(php_stream *stream, int close_handle) {
    return closedir((DIR *) stream->abstract);
}

static int php_plain_files_dirstream_rewind(php_stream *stream, zend_off_t offset, int whence, zend_off_t *newoffs) {
    rewinddir((DIR *) stream->abstract);
    return 0;
}

static php_stream_ops php_plain_files_dirstream_ops = {
    NULL,
    php_plain_files_dirstream_read,
    php_plain_files_dirstream_close,
    NULL,
    "dir",
    php_plain_files_dirstream_rewind,
    NULL, /* cast */
    NULL, /* stat */
    NULL  /* set_option */
};

static php_stream *php_plain_files_dir_opener(php_stream_wrapper *wrapper,
                                              const char *path,
                                              const char *mode,
                                              int options,
                                              zend_string **opened_path,
                                              php_stream_context *context STREAMS_DC) {
    DIR *dir = NULL;
    php_stream *stream = NULL;

#ifdef HAVE_GLOB
    if (options & STREAM_USE_GLOB_DIR_OPEN) {
        return php_glob_stream_wrapper.wops->dir_opener(
            (php_stream_wrapper *) &php_glob_stream_wrapper, path, mode, options, opened_path, context STREAMS_REL_CC);
    }
#endif

    if (((options & STREAM_DISABLE_OPEN_BASEDIR) == 0) && php_check_open_basedir(path)) {
        return NULL;
    }

    dir = opendir(path);

    if (dir) {
        stream = php_stream_alloc(&php_plain_files_dirstream_ops, dir, 0, mode);
        if (stream == NULL) closedir(dir);
    }

    return stream;
}
/* }}} */

static php_stream *_sw_php_stream_fopen(const char *filename,
                                    const char *mode,
                                    zend_string **opened_path,
                                    int options STREAMS_DC) {
    char _realpath[MAXPATHLEN];
    int open_flags;
    int fd;
    php_stream *ret;
    int persistent = options & STREAM_OPEN_PERSISTENT;
    char *persistent_id = NULL;

    if (!swr_v1_phpruntime_plain_open_flags(mode, &open_flags)) {
        php_stream_wrapper_log_error(&php_plain_files_wrapper, options, "`%s' is not a valid mode for fopen", mode);
        return NULL;
    }

    if (options & STREAM_ASSUME_REALPATH) {
        /* Not strlcpy: PHP's headers turn it into php_strlcpy where libc has none, a symbol a
         * PHP built against glibc 2.38 or newer doesn't export, so the binary wouldn't load there. */
        snprintf(_realpath, sizeof(_realpath), "%s", filename);
    } else {
        if (expand_filepath(filename, _realpath) == NULL) {
            return NULL;
        }
    }

    if (persistent) {
        spprintf(&persistent_id, 0, "streams_stdio_%d_%s", open_flags, _realpath);
        switch (php_stream_from_persistent_id(persistent_id, &ret)) {
        case PHP_STREAM_PERSISTENT_SUCCESS:
            if (opened_path) {
                // TODO: avoid reallocation???
                *opened_path = zend_string_init(_realpath, strlen(_realpath), 0);
            }
            /* fall through */
#if PHP_VERSION_ID >= 80100
            ZEND_FALLTHROUGH;
#endif

        case PHP_STREAM_PERSISTENT_FAILURE:
            efree(persistent_id);
            return ret;
        }
    }
    fd = open(_realpath, open_flags, 0666);
    if (fd != -1) {
        ret = sw_php_stream_fopen_from_fd_rel(fd, mode, persistent_id, (open_flags & O_APPEND) == 0);
        if (ret) {
            if (opened_path) {
                *opened_path = zend_string_init(_realpath, strlen(_realpath), 0);
            }
            if (persistent_id) {
                efree(persistent_id);
            }

            /* WIN32 always set ISREG flag */
            /* sanity checks for include/require.
             * We check these after opening the stream, so that we save
             * on fstat() syscalls */
            if (options & STREAM_OPEN_FOR_INCLUDE) {
                php_stdio_stream_data *self = (php_stdio_stream_data *) ret->abstract;
                int r;

                r = do_fstat(self, 0);
                if ((r == 0 && !S_ISREG(self->sb.st_mode))) {
                    if (opened_path) {
                        zend_string_release_ex(*opened_path, 0);
                        *opened_path = NULL;
                    }
                    php_stream_close(ret);
                    return NULL;
                }

                /* Make sure the fstat result is reused when we later try to get the
                 * file size. */
                self->no_forced_fstat = 1;
            }

            if (options & STREAM_USE_BLOCKING_PIPE) {
                php_stdio_stream_data *self = (php_stdio_stream_data *) ret->abstract;
                self->is_pipe_blocking = 1;
            }

            return ret;
        }
        close(fd);
    }
    if (persistent_id) {
        efree(persistent_id);
    }
    return NULL;
}

static php_stream *php_plain_files_stream_opener(php_stream_wrapper *wrapper,
                                 const char *path,
                                 const char *mode,
                                 int options,
                                 zend_string **opened_path,
                                 php_stream_context *context STREAMS_DC) {
    if (strncasecmp(path, SW_ASYNC_FILE_PROTOCOL "://", sizeof(SW_ASYNC_FILE_PROTOCOL "://") - 1) == 0) {
        path += sizeof(SW_ASYNC_FILE_PROTOCOL "://") - 1;
    }
    if (((options & STREAM_DISABLE_OPEN_BASEDIR) == 0) && php_check_open_basedir(path)) {
        return NULL;
    }

    php_stream *stream;
    /** phar_open_archive_fp, cannot use async-io */
    if (EG(current_execute_data) && EG(current_execute_data)->func &&
        ZEND_USER_CODE(EG(current_execute_data)->func->type)) {
        const zend_op *opline = EG(current_execute_data)->opline;
        if (opline && opline->opcode == ZEND_INCLUDE_OR_EVAL &&
            (opline->extended_value & (ZEND_INCLUDE | ZEND_INCLUDE_ONCE | ZEND_REQUIRE | ZEND_REQUIRE_ONCE))) {
            goto _open_for_include;
        }
    }
    /** include file, cannot use async-io */
    if (options & STREAM_OPEN_FOR_INCLUDE) {
    _open_for_include:
        stream = php_stream_fopen_rel(path, mode, opened_path, options);
        if (stream == NULL) {
            return NULL;
        }
        stream->ops = &ori_php_stream_stdio_ops;
        return stream;
    }

    return sw_php_stream_fopen_rel(path, mode, opened_path, options);
}

/* `value` is what the option says: the touch's times, an owner's or group's name, or an id
 * or mode. crates/phpruntime's plain_files does the rest. */
static int php_plain_files_metadata(
    php_stream_wrapper *wrapper, const char *url, int option, void *value, php_stream_context *context) {
    bool named = option == PHP_STREAM_META_OWNER_NAME || option == PHP_STREAM_META_GROUP_NAME;
    long number = 0;
    if (option == PHP_STREAM_META_OWNER || option == PHP_STREAM_META_GROUP) {
        number = *(long *) value;
    } else if (option == PHP_STREAM_META_ACCESS) {
        number = (long) *(zend_long *) value;
    }
    return swr_v1_phpruntime_plain_metadata(url, option, option == PHP_STREAM_META_TOUCH ? value : NULL,
                                            named ? value : NULL, number);
}

static php_stream_wrapper_ops sw_php_plain_files_wrapper_ops = {
    php_plain_files_stream_opener,
    NULL,
    NULL,
    swr_v1_phpruntime_plain_url_stat,
    php_plain_files_dir_opener,
    "plainfile/coroutine",
    swr_v1_phpruntime_plain_unlink,
    swr_v1_phpruntime_plain_rename,
    swr_v1_phpruntime_plain_mkdir,
    swr_v1_phpruntime_plain_rmdir,
    php_plain_files_metadata
};

PHPAPI php_stream_wrapper sw_php_plain_files_wrapper = {
    &sw_php_plain_files_wrapper_ops,
    NULL,
    0
};

void php_swoole_runtime_rinit(void) {
    if (!swr_v1_phpruntime_rinit()) {
        return;
    }
    memcpy(&ori_php_plain_files_wrapper, &php_plain_files_wrapper, sizeof(php_plain_files_wrapper));
    memcpy(&ori_php_stream_stdio_ops, &php_stream_stdio_ops, sizeof(php_stream_stdio_ops));
}

/* The runtime's stream_ops: hook or restore the stdio ops (stdio) or the plain-files wrapper. */
void php_swoole_runtime_stream_ops(uint8_t stdio, uint8_t hook) {
    if (stdio) {
        memcpy(&php_stream_stdio_ops, hook ? &sw_php_stream_stdio_ops : &ori_php_stream_stdio_ops,
               sizeof(php_stream_stdio_ops));
    } else {
        memcpy(&php_plain_files_wrapper, hook ? &sw_php_plain_files_wrapper : &ori_php_plain_files_wrapper,
               sizeof(php_plain_files_wrapper));
    }
}

php_stream *php_swoole_create_stream_from_pipe(int fd, const char *mode, const char *persistent_id STREAMS_DC) {
    return _sw_php_stream_fopen_from_fd(fd, mode, persistent_id, false STREAMS_CC);
}

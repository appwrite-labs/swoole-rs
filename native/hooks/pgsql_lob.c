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
  | Authors: Edin Kadribasic <edink@emini.dk>                            |
  |          Ilia Alshanestsky <ilia@prohost.org>                        |
  |          Wez Furlong <wez@php.net>                                   |
  +----------------------------------------------------------------------+
*/

#include "config.h"
#include "php.h"

#ifdef SW_USE_PGSQL
#include "php_pdo_pgsql_int.h"
#include "ext/pdo/php_pdo_error.h"

static ssize_t pgsql_lob_write(php_stream *stream, const char *buf, size_t count)
{
    struct pdo_pgsql_lob_self *self = stream->abstract;
    return lo_write(self->conn, self->lfd, buf, count);
}

static ssize_t pgsql_lob_read(php_stream *stream, char *buf, size_t count)
{
    struct pdo_pgsql_lob_self *self = stream->abstract;
    return lo_read(self->conn, self->lfd, buf, count);
}

static int pgsql_lob_close(php_stream *stream, int close_handle)
{
    struct pdo_pgsql_lob_self *self = stream->abstract;
    pdo_pgsql_db_handle *H = (pdo_pgsql_db_handle *)(Z_PDO_DBH_P(&self->dbh))->driver_data;

    if (close_handle) {
        lo_close(self->conn, self->lfd);
    }
    pdo_pgsql_unregister_lob(H, php_stream_get_resource_id(stream));
    zval_ptr_dtor(&self->dbh);
    efree(self);
    return 0;
}

static int pgsql_lob_flush(php_stream *stream)
{
    return 0;
}

static int pgsql_lob_seek(php_stream *stream, zend_off_t offset, int whence,
        zend_off_t *newoffset)
{
    struct pdo_pgsql_lob_self *self = stream->abstract;
#if defined(ZEND_ENABLE_ZVAL_LONG64) && (PHP_VERSION_ID >= 80400 || defined(HAVE_PG_LO64))
    zend_off_t pos = lo_lseek64(self->conn, self->lfd, offset, whence);
#else
    zend_off_t pos = lo_lseek(self->conn, self->lfd, offset, whence);
#endif
    *newoffset = pos;
    return pos >= 0 ? 0 : -1;
}

const php_stream_ops pdo_pgsql_lob_stream_ops = {
    pgsql_lob_write,
    pgsql_lob_read,
    pgsql_lob_close,
    pgsql_lob_flush,
    "pdo_pgsql lob stream",
    pgsql_lob_seek,
    NULL,
    NULL,
    NULL
};

static bool lob_resource(zend_object *dbh, int lfd, Oid oid, zval *output)
{
    struct pdo_pgsql_lob_self *self = ecalloc(1, sizeof(*self));
    pdo_pgsql_db_handle *H = (pdo_pgsql_db_handle *)(php_pdo_dbh_fetch_inner(dbh))->driver_data;

    ZVAL_OBJ(&self->dbh, dbh);
    self->lfd = lfd;
    self->oid = oid;
    self->conn = pdo_pgsql_server(H);

    php_stream *stm = php_stream_alloc(&pdo_pgsql_lob_stream_ops, self, 0, "r+b");

    if (stm) {
        GC_ADDREF(dbh);
        pdo_pgsql_register_lob(H, php_stream_get_resource_id(stm), stm->res);
        php_stream_to_zval(stm, output);
        return true;
    }

    efree(self);
    return false;
}

void pdo_pgsql_close_resource(zend_resource *resource)
{
    if (resource->type >= 0) {
        zend_list_close(resource);
    }
}

/* php_stream_to_zval consumes the stream resource. */
bool pdo_pgsql_lob_resource(zend_execute_data *execute_data, int fd, Oid oid, zval *output)
{
    return lob_resource(Z_OBJ_P(ZEND_THIS), fd, oid, output);
}

bool pdo_pgsql_stmt_lob_stream(pdo_stmt_t *stmt, int fd, Oid oid, zval *output)
{
#if PHP_VERSION_ID >= 80500
    return lob_resource(stmt->database_object_handle, fd, oid, output);
#else
    return lob_resource(Z_OBJ(stmt->database_object_handle), fd, oid, output);
#endif
}
#endif

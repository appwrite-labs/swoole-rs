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

/* php_stream_ops supplies the buffer bounds, stream payload and resource ID. */
extern ssize_t pdo_pgsql_lob_write(void *, const char *, size_t);
extern int pdo_pgsql_lob_read(void *, char *, size_t);
extern void pdo_pgsql_lob_close(void *, bool, zend_long);
extern bool pdo_pgsql_lob_seek(void *, zend_off_t, int, zend_off_t *);

static ssize_t pgsql_lob_write(php_stream *stream, const char *buf, size_t count) {
    return pdo_pgsql_lob_write(stream->abstract, buf, count);
}

static ssize_t pgsql_lob_read(php_stream *stream, char *buf, size_t count) {
    return pdo_pgsql_lob_read(stream->abstract, buf, count);
}

static int pgsql_lob_close(php_stream *stream, int close_handle) {
    pdo_pgsql_lob_close(stream->abstract, close_handle != 0, php_stream_get_resource_id(stream));
    return 0;
}

static int pgsql_lob_flush(php_stream *stream) {
    return 0;
}

zend_off_t pdo_pgsql_seek_descriptor(PGconn *conn, int fd, zend_off_t offset, int whence) {
#if defined(ZEND_ENABLE_ZVAL_LONG64) && (PHP_VERSION_ID >= 80400 || defined(HAVE_PG_LO64))
    return lo_lseek64(conn, fd, offset, whence);
#else
    return lo_lseek(conn, fd, offset, whence);
#endif
}

static int pgsql_lob_seek(php_stream *stream, zend_off_t offset, int whence, zend_off_t *position) {
    return pdo_pgsql_lob_seek(stream->abstract, offset, whence, position) ? 0 : -1;
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

pdo_dbh_t *pdo_pgsql_lob_dbh(zval *database) {
    return Z_PDO_DBH_P(database);
}

extern void *pdo_pgsql_lob_owner(pdo_pgsql_db_handle *, zval *, int, Oid);

static bool lob_resource(zend_object *dbh, int fd, Oid oid, zval *output) {
    php_stream *stream = php_stream_alloc(&pdo_pgsql_lob_stream_ops, NULL, 0, "r+b");
    if (!stream) {
        return false;
    }
    zval database;
    ZVAL_OBJ(&database, dbh);
    pdo_pgsql_db_handle *handle = php_pdo_dbh_fetch_inner(dbh)->driver_data;
    stream->abstract = pdo_pgsql_lob_owner(handle, &database, fd, oid);
    pdo_pgsql_register_lob(handle, php_stream_get_resource_id(stream), stream->res);
    php_stream_to_zval(stream, output);
    return true;
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

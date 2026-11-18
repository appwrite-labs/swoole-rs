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
    zend_hash_index_del(*pdo_pgsql_lob_streams(H), php_stream_get_resource_id(stream));
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

#if PHP_VERSION_ID >= 80500
php_stream *pdo_pgsql_create_lob_stream(zend_object *dbh, int lfd, Oid oid)
{
#else
php_stream *pdo_pgsql_create_lob_stream(zval *value, int lfd, Oid oid)
{
    zend_object *dbh = Z_OBJ_P(value);
#endif
    php_stream *stm;
    struct pdo_pgsql_lob_self *self = ecalloc(1, sizeof(*self));
    pdo_pgsql_db_handle *H = (pdo_pgsql_db_handle *)(php_pdo_dbh_fetch_inner(dbh))->driver_data;

    ZVAL_OBJ(&self->dbh, dbh);
    self->lfd = lfd;
    self->oid = oid;
    self->conn = pdo_pgsql_server(H);

    stm = php_stream_alloc(&pdo_pgsql_lob_stream_ops, self, 0, "r+b");

    if (stm) {
        HashTable **streams = pdo_pgsql_lob_streams(H);
        if (!*streams) {
            *streams = pemalloc(sizeof(HashTable), php_pdo_dbh_fetch_inner(dbh)->is_persistent);
            zend_hash_init(*streams, 0, NULL, NULL, 1);
        }
        GC_ADDREF(dbh);
        zend_hash_index_add_ptr(*pdo_pgsql_lob_streams(H), php_stream_get_resource_id(stm), stm->res);
        return stm;
    }

    efree(self);
    return NULL;
}

void pdo_pgsql_close_lob_streams(pdo_dbh_t *dbh)
{
    zend_resource *res;
    pdo_pgsql_db_handle *H = (pdo_pgsql_db_handle *)dbh->driver_data;
    if (*pdo_pgsql_lob_streams(H)) {
        ZEND_HASH_REVERSE_FOREACH_PTR(*pdo_pgsql_lob_streams(H), res) {
            if (res->type >= 0) {
                zend_list_close(res);
            }
        } ZEND_HASH_FOREACH_END();
    }
}

Oid pdo_pgsql_create_lob(pdo_dbh_t *dbh);
bool pdo_pgsql_open_lob(pdo_dbh_t *dbh, Oid oid, const char *mode, int *fd);
bool pdo_pgsql_unlink_lob(pdo_dbh_t *dbh, Oid oid);

static bool parse_oid(const char *text, Oid *oid)
{
    *oid = (Oid)strtoul(text, NULL, 10);
    return *oid != 0 || (errno != ERANGE && errno != EINVAL);
}

#if PHP_VERSION_ID >= 80400
void pgsqlLOBCreate_internal(INTERNAL_FUNCTION_PARAMETERS)
#else
PHP_METHOD(PDO_PGSql_Ext, pgsqlLOBCreate)
#endif
{
    ZEND_PARSE_PARAMETERS_NONE();
    pdo_dbh_t *dbh = Z_PDO_DBH_P(ZEND_THIS);
    PDO_CONSTRUCT_CHECK;
    PDO_DBH_CLEAR_ERR();
    Oid oid = pdo_pgsql_create_lob(dbh);
    if (oid != InvalidOid) {
        RETURN_STR(zend_ulong_to_str(oid));
    }
    PDO_HANDLE_DBH_ERR();
    RETURN_FALSE;
}

#if PHP_VERSION_ID >= 80400
void pgsqlLOBOpen_internal(INTERNAL_FUNCTION_PARAMETERS)
#else
PHP_METHOD(PDO_PGSql_Ext, pgsqlLOBOpen)
#endif
{
    char *text, *mode = "rb";
    size_t length, mode_length;
    if (zend_parse_parameters(ZEND_NUM_ARGS(), "s|s", &text, &length, &mode, &mode_length) == FAILURE) {
        RETURN_THROWS();
    }
    Oid oid;
    if (!parse_oid(text, &oid)) {
        RETURN_FALSE;
    }
    pdo_dbh_t *dbh = Z_PDO_DBH_P(ZEND_THIS);
    PDO_CONSTRUCT_CHECK;
    PDO_DBH_CLEAR_ERR();
    int fd = -1;
    if (pdo_pgsql_open_lob(dbh, oid, mode, &fd)) {
#if PHP_VERSION_ID >= 80500
        php_stream *stream = pdo_pgsql_create_lob_stream(Z_OBJ_P(ZEND_THIS), fd, oid);
#else
        php_stream *stream = pdo_pgsql_create_lob_stream(ZEND_THIS, fd, oid);
#endif
        if (stream) {
            php_stream_to_zval(stream, return_value);
            return;
        }
    }
    PDO_HANDLE_DBH_ERR();
    RETURN_FALSE;
}

#if PHP_VERSION_ID >= 80400
void pgsqlLOBUnlink_internal(INTERNAL_FUNCTION_PARAMETERS)
#else
PHP_METHOD(PDO_PGSql_Ext, pgsqlLOBUnlink)
#endif
{
    char *text;
    size_t length;
    if (zend_parse_parameters(ZEND_NUM_ARGS(), "s", &text, &length) == FAILURE) {
        RETURN_THROWS();
    }
    Oid oid;
    if (!parse_oid(text, &oid)) {
        RETURN_FALSE;
    }
    pdo_dbh_t *dbh = Z_PDO_DBH_P(ZEND_THIS);
    PDO_CONSTRUCT_CHECK;
    PDO_DBH_CLEAR_ERR();
    if (pdo_pgsql_unlink_lob(dbh, oid)) {
        RETURN_TRUE;
    }
    PDO_HANDLE_DBH_ERR();
    RETURN_FALSE;
}

#if PHP_VERSION_ID >= 80400
PHP_METHOD(PDO_PGSql_Ext, pgsqlLOBCreate)
{
    pgsqlLOBCreate_internal(INTERNAL_FUNCTION_PARAM_PASSTHRU);
}
PHP_METHOD(PDO_PGSql_Ext, pgsqlLOBOpen)
{
    pgsqlLOBOpen_internal(INTERNAL_FUNCTION_PARAM_PASSTHRU);
}
PHP_METHOD(PDO_PGSql_Ext, pgsqlLOBUnlink)
{
    pgsqlLOBUnlink_internal(INTERNAL_FUNCTION_PARAM_PASSTHRU);
}
#endif
#endif

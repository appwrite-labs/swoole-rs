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

/* Stays C (version): PDO_CONSTRUCT_CHECK raises the version's error. */
/* PDO_CONSTRUCT_CHECK uses the active frame for the version-specific error.
 * RETURN_THROWS references the return slot but never writes it. */
void pdo_pgsql_method_dbh(zend_execute_data *execute_data, pdo_dbh_t **out)
{
    zval *return_value = NULL;
    pdo_dbh_t *dbh = Z_PDO_DBH_P(ZEND_THIS);
    PDO_CONSTRUCT_CHECK;
    *out = dbh;
}

/* Stays C (version): PDO_DBH_CLEAR_ERR. */
void pdo_pgsql_clear_method_error(pdo_dbh_t *dbh)
{
    PDO_DBH_CLEAR_ERR();
}

/* Stays C (version): PDO_HANDLE_DBH_ERR. */
void pdo_pgsql_handle_method_error(pdo_dbh_t *dbh)
{
    PDO_HANDLE_DBH_ERR();
}

/* Stays C (allocator): ZVAL_STRING of libpq's strings before PQfreemem. */
/* Copy the libpq-owned strings before releasing the notification. */
bool pdo_pgsql_take_notify(PGconn *conn, zval *channel, int *pid, zval *payload)
{
    PGnotify *note = PQnotifies(conn);
    if (!note) {
        return false;
    }
    ZVAL_STRING(channel, note->relname);
    *pid = note->be_pid;
    if (note->extra && note->extra[0]) {
        ZVAL_STRING(payload, note->extra);
    }
    PQfreemem(note);
    return true;
}

#if PHP_VERSION_ID >= 80400
#include "zend_attributes.h"

/* Use the version's generated signatures and constants for PDO's subclass. */
#define zim_Pdo_Pgsql_escapeIdentifier pgsql_escape_identifier
#define zim_Pdo_Pgsql_copyFromArray pgsqlCopyFromArray_internal
#define zim_Pdo_Pgsql_copyFromFile pgsqlCopyFromFile_internal
#define zim_Pdo_Pgsql_copyToArray pgsqlCopyToArray_internal
#define zim_Pdo_Pgsql_copyToFile pgsqlCopyToFile_internal
#define zim_Pdo_Pgsql_lobCreate pgsqlLOBCreate_internal
#define zim_Pdo_Pgsql_lobOpen pgsqlLOBOpen_internal
#define zim_Pdo_Pgsql_lobUnlink pgsqlLOBUnlink_internal
#define zim_Pdo_Pgsql_getNotify pgsqlGetNotify_internal
#define zim_Pdo_Pgsql_getPid pgsqlGetPid_internal
#define zim_Pdo_Pgsql_setNoticeCallback zim_PDO_PGSql_Ext_pgsqlSetNoticeCallback
#include "pdo_pgsql_arginfo.h"


#endif

/* Stays C (version, globals, allocator): 8.4's Pdo\Pgsql, CG(class_table) and zend_string_init. */
void pdo_pgsql_bind_subclass(void)
{
#if PHP_VERSION_ID >= 80400
    zend_class_entry *ce = zend_hash_str_find_ptr(CG(class_table), ZEND_STRL("pdo\\pgsql"));
    if (!ce) {
        ce = register_class_Pdo_Pgsql(php_pdo_get_dbh_ce());
        ce->create_object = pdo_dbh_new;
    } else {
        /* PDO unregisters the driver mapping but retains the original class. */
        for (const zend_function_entry *entry = class_Pdo_Pgsql_methods; entry->fname; ++entry) {
            zend_string *key = zend_string_init(entry->fname, strlen(entry->fname), false);
            zend_str_tolower(ZSTR_VAL(key), ZSTR_LEN(key));
            zend_function *method = zend_hash_find_ptr(&ce->function_table, key);
            zend_string_release(key);
            if (method && method->type == ZEND_INTERNAL_FUNCTION && method->common.scope == ce) {
                method->internal_function.handler = entry->handler;
            }
        }
    }
    extern const pdo_driver_t swoole_pdo_pgsql_driver;
    php_pdo_register_driver_specific_ce(&swoole_pdo_pgsql_driver, ce);
#endif
}
#endif

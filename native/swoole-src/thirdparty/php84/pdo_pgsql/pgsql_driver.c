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

#include "php_swoole_pgsql.h"

#if PHP_VERSION_ID >= 80400 && PHP_VERSION_ID < 80500
#include "php.h"
#include "php_ini.h"
#include "main/php_network.h"
#include "ext/pdo/php_pdo.h"
#include "ext/pdo/php_pdo_driver.h"
#include "ext/pdo/php_pdo_error.h"
#include "ext/standard/file.h"
#include "php_pdo_pgsql_int.h"
#include "zend_exceptions.h"
#include "zend_smart_str.h"
#include "pgsql_driver_arginfo.h"

void pdo_pgsql_notice(void *context, const char *message) /* {{{ */
{
    pdo_dbh_t *dbh = (pdo_dbh_t *) context;
    zend_fcall_info_cache *fc = *pdo_pgsql_notice_callback(dbh->driver_data);
    if (fc) {
        zval zarg;
        ZVAL_STRING(&zarg, message);
        zend_call_known_fcc(fc, NULL, 1, &zarg, NULL);
        zval_ptr_dtor_str(&zarg);
    }
}
/* }}} */

void pdo_pgsql_cleanup_notice_callback(pdo_pgsql_db_handle *H) /* {{{ */
{
    zend_fcall_info_cache **callback = pdo_pgsql_notice_callback(H);
    if (*callback) {
        zend_fcc_dtor(*callback);
        efree(*callback);
        *callback = NULL;
    }
}
/* }}} */

/* The handle's close: C's state (the LOB streams and their table, and the notice
 * callback), then the handle and its connection (pdo_pgsql_free). */
void pdo_pgsql_handle_closer(pdo_dbh_t *dbh) {
    pdo_pgsql_db_handle *H = (pdo_pgsql_db_handle *) dbh->driver_data;
    if (H) {
        HashTable **streams = pdo_pgsql_lob_streams(H);
        if (*streams) {
            pdo_pgsql_close_lob_streams(dbh);
            zend_hash_destroy(*streams);
            pefree(*streams, dbh->is_persistent);
            *streams = NULL;
        }
        pdo_pgsql_cleanup_notice_callback(H);
        dbh->driver_data = NULL;
        pdo_pgsql_free(H);
    }
}

static bool pgsql_handle_preparer(pdo_dbh_t *dbh, zend_string *sql, pdo_stmt_t *stmt, zval *driver_options) {
    pdo_pgsql_db_handle *H = (pdo_pgsql_db_handle *) dbh->driver_data;
    pdo_pgsql_stmt *S = ecalloc(1, sizeof(pdo_pgsql_stmt));
    int scrollable;
    int ret;
    zend_string *nsql = NULL;
    int emulate = 0;
    int execute_only = 0;

    S->H = H;
    stmt->driver_data = S;
    stmt->methods = &swoole_pgsql_stmt_methods;

    scrollable = pdo_attr_lval(driver_options, PDO_ATTR_CURSOR, PDO_CURSOR_FWDONLY) == PDO_CURSOR_SCROLL;

    if (scrollable) {
        if (S->cursor_name) {
            efree(S->cursor_name);
        }
        spprintf(&S->cursor_name, 0, "pdo_crsr_%08x", pdo_pgsql_next_stmt_id(H));
        emulate = 1;
    } else if (driver_options) {
        if (pdo_attr_lval(driver_options, PDO_ATTR_EMULATE_PREPARES, pdo_pgsql_emulate_prepares(H)) == 1) {
            emulate = 1;
        }
        if (pdo_attr_lval(driver_options, PDO_PGSQL_ATTR_DISABLE_PREPARES, pdo_pgsql_disable_prepares(H)) == 1) {
            execute_only = 1;
        }
    } else {
        emulate = pdo_pgsql_emulate_prepares(H);
        execute_only = pdo_pgsql_disable_prepares(H);
    }

    if (emulate) {
        stmt->supports_placeholders = PDO_PLACEHOLDER_NONE;
    } else {
        stmt->supports_placeholders = PDO_PLACEHOLDER_NAMED;
        stmt->named_rewrite_template = "$%d";
    }

    ret = pdo_parse_params(stmt, sql, &nsql);

    if (ret == -1) {
        /* couldn't grok it */
        strcpy(dbh->error_code, stmt->error_code);
        return false;
    } else if (ret == 1) {
        /* query was re-written */
        S->query = nsql;
    } else {
        S->query = zend_string_copy(sql);
    }

    if (!emulate && !execute_only) {
        /* prepared query: set the query name and defer the
           actual prepare until the first execute call */
        spprintf(&S->stmt_name, 0, "pdo_stmt_%08x", pdo_pgsql_next_stmt_id(H));
    }

    return true;
}

void pgsqlGetNotify_internal(INTERNAL_FUNCTION_PARAMETERS) {
    pdo_dbh_t *dbh;
    pdo_pgsql_db_handle *H;
    zend_long result_type = PDO_FETCH_USE_DEFAULT;
    zend_long ms_timeout = 0;
    PGnotify *pgsql_notify;

    if (FAILURE == zend_parse_parameters(ZEND_NUM_ARGS(), "|ll", &result_type, &ms_timeout)) {
        RETURN_THROWS();
    }

    dbh = Z_PDO_DBH_P(ZEND_THIS);
    PDO_CONSTRUCT_CHECK;

    if (result_type == PDO_FETCH_USE_DEFAULT) {
        result_type = dbh->default_fetch_type;
    }

    if (result_type != PDO_FETCH_BOTH && result_type != PDO_FETCH_ASSOC && result_type != PDO_FETCH_NUM) {
        zend_argument_value_error(1, "must be one of PDO::FETCH_BOTH, PDO::FETCH_ASSOC, or PDO::FETCH_NUM");
        RETURN_THROWS();
    }

    if (ms_timeout < 0) {
        zend_argument_value_error(2, "must be greater than or equal to 0");
        RETURN_THROWS();
#ifdef ZEND_ENABLE_ZVAL_LONG64
    } else if (ms_timeout > INT_MAX) {
        php_error_docref(NULL, E_WARNING, "Timeout was shrunk to %d", INT_MAX);
        ms_timeout = INT_MAX;
#endif
    }

    H = (pdo_pgsql_db_handle *) dbh->driver_data;

    if (!PQconsumeInput(pdo_pgsql_server(H))) {
        pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, NULL);
        PDO_HANDLE_DBH_ERR();
        RETURN_FALSE;
    }
    pgsql_notify = PQnotifies(pdo_pgsql_server(H));

    if (ms_timeout && !pgsql_notify) {
        php_pollfd_for_ms(PQsocket(pdo_pgsql_server(H)), PHP_POLLREADABLE, (int) ms_timeout);

        if (!PQconsumeInput(pdo_pgsql_server(H))) {
            pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, NULL);
            PDO_HANDLE_DBH_ERR();
            RETURN_FALSE;
        }
        pgsql_notify = PQnotifies(pdo_pgsql_server(H));
    }

    if (!pgsql_notify) {
        RETURN_FALSE;
    }

    array_init(return_value);
    if (result_type == PDO_FETCH_NUM || result_type == PDO_FETCH_BOTH) {
        add_index_string(return_value, 0, pgsql_notify->relname);
        add_index_long(return_value, 1, pgsql_notify->be_pid);
        if (pgsql_notify->extra && pgsql_notify->extra[0]) {
            add_index_string(return_value, 2, pgsql_notify->extra);
        }
    }
    if (result_type == PDO_FETCH_ASSOC || result_type == PDO_FETCH_BOTH) {
        add_assoc_string(return_value, "message", pgsql_notify->relname);
        add_assoc_long(return_value, "pid", pgsql_notify->be_pid);
        if (pgsql_notify->extra && pgsql_notify->extra[0]) {
            add_assoc_string(return_value, "payload", pgsql_notify->extra);
        }
    }

    PQfreemem(pgsql_notify);
}

/* {{{ Get asynchronous notification */
PHP_METHOD(PDO_PGSql_Ext, pgsqlGetNotify) {
    pgsqlGetNotify_internal(INTERNAL_FUNCTION_PARAM_PASSTHRU);
}
/* }}} */

void pgsqlGetPid_internal(INTERNAL_FUNCTION_PARAMETERS) {
    pdo_dbh_t *dbh;
    pdo_pgsql_db_handle *H;

    ZEND_PARSE_PARAMETERS_NONE();

    dbh = Z_PDO_DBH_P(ZEND_THIS);
    PDO_CONSTRUCT_CHECK;

    H = (pdo_pgsql_db_handle *) dbh->driver_data;

    RETURN_LONG(PQbackendPID(pdo_pgsql_server(H)));
}

/* {{{ Get backend(server) pid */
PHP_METHOD(PDO_PGSql_Ext, pgsqlGetPid) {
    pgsqlGetPid_internal(INTERNAL_FUNCTION_PARAM_PASSTHRU);
}
/* }}} */

/* {{{ Sets a callback to receive DB notices (after client_min_messages has been set) */
PHP_METHOD(PDO_PGSql_Ext, pgsqlSetNoticeCallback) {
    zend_fcall_info fci = empty_fcall_info;
    zend_fcall_info_cache fcc = empty_fcall_info_cache;
    if (FAILURE == zend_parse_parameters(ZEND_NUM_ARGS(), "F!", &fci, &fcc)) {
        RETURN_THROWS();
    }

    pdo_dbh_t *dbh = Z_PDO_DBH_P(ZEND_THIS);
    PDO_CONSTRUCT_CHECK_WITH_CLEANUP(cleanup);

    pdo_pgsql_db_handle *H = (pdo_pgsql_db_handle *) dbh->driver_data;

    pdo_pgsql_cleanup_notice_callback(H);

    if (ZEND_FCC_INITIALIZED(fcc)) {
        *pdo_pgsql_notice_callback(H) = emalloc(sizeof(zend_fcall_info_cache));
        zend_fcc_dup(*pdo_pgsql_notice_callback(H), &fcc);
    }

    return;

cleanup:
    if (ZEND_FCC_INITIALIZED(fcc)) {
        zend_fcc_dtor(&fcc);
    }
    RETURN_THROWS();
}
/* }}} */

static const zend_function_entry *pdo_pgsql_get_driver_methods(pdo_dbh_t *dbh, int kind) {
    switch (kind) {
    case PDO_DBH_DRIVER_METHOD_KIND_DBH:
        return class_PDO_PGSql_Ext_methods;
    default:
        return NULL;
    }
}

const struct pdo_dbh_methods swoole_pdo_pgsql_methods = {pdo_pgsql_handle_closer,
                                                     pgsql_handle_preparer,
                                                     pdo_pgsql_handle_doer,
                                                     pdo_pgsql_handle_quoter,
                                                     pdo_pgsql_handle_begin,
                                                     pdo_pgsql_handle_commit,
                                                     pdo_pgsql_handle_rollback,
                                                     pdo_pgsql_set_attr,
                                                     pdo_pgsql_last_insert_id,
                                                     pdo_pgsql_fetch_error_func,
                                                     pdo_pgsql_get_attribute,
                                                     pdo_pgsql_check_liveness,     /* check_liveness */
                                                     pdo_pgsql_get_driver_methods, /* get_driver_methods */
                                                     NULL,
                                                     pdo_pgsql_handle_in_transaction,
                                                     NULL, /* get_gc */
                                                     pdo_pgsql_scanner};

static int pdo_pgsql_handle_factory(pdo_dbh_t *dbh, zval *driver_options) {
    return pdo_pgsql_connect(dbh, dbh->data_source, dbh->username, dbh->password, driver_options);
}

const pdo_driver_t swoole_pdo_pgsql_driver = {PDO_DRIVER_HEADER(pgsql), pdo_pgsql_handle_factory};
#endif

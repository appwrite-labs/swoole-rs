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
#include "pgsql_driver_arginfo.h"

/* Zend owns the callable cache and its version-specific trampoline. */
void pdo_pgsql_call_notice(void *callback, const char *message)
{
#if PHP_VERSION_ID >= 80400
    zval argument;
    ZVAL_STRING(&argument, message);
    zend_call_known_fcc(callback, NULL, 1, &argument, NULL);
    zval_ptr_dtor_str(&argument);
#endif
}

void pdo_pgsql_release_notice(void *callback)
{
#if PHP_VERSION_ID >= 80400
    zend_fcall_info_cache *cache = callback;
    zend_fcc_dtor(cache);
    efree(cache);
#endif
}

pdo_pgsql_db_handle *pdo_pgsql_take_handle(pdo_dbh_t *dbh) {
    pdo_pgsql_db_handle *handle = dbh->driver_data;
    dbh->driver_data = NULL;
    return handle;
}

/* PDO's statement layout and the driver's PHP-version-specific storage. */
void pdo_pgsql_init_statement(pdo_stmt_t *stmt)
{
    pdo_pgsql_stmt *state = ecalloc(1, sizeof(*state));
    state->H = stmt->dbh->driver_data;
    stmt->driver_data = state;
    stmt->methods = &swoole_pgsql_stmt_methods;
}

void pdo_pgsql_name_statement(pdo_stmt_t *stmt, const char *name, bool cursor)
{
    pdo_pgsql_stmt *state = stmt->driver_data;
    if (cursor) {
        state->cursor_name = estrdup(name);
    } else {
        state->stmt_name = estrdup(name);
    }
}

bool pdo_pgsql_parse_statement(pdo_stmt_t *stmt, zend_string *sql, bool emulate, bool unbuffered)
{
    pdo_pgsql_stmt *state = stmt->driver_data;
    stmt->supports_placeholders = emulate ? PDO_PLACEHOLDER_NONE : PDO_PLACEHOLDER_NAMED;
    if (!emulate) {
        stmt->named_rewrite_template = "$%d";
    }
#if PHP_VERSION_ID >= 80500
    state->is_unbuffered = unbuffered;
#endif
    zend_string *rewritten = NULL;
    int parsed = pdo_parse_params(stmt, sql, &rewritten);
    if (parsed == -1) {
        memcpy(stmt->dbh->error_code, stmt->error_code, sizeof(stmt->error_code));
        return false;
    }
    state->query = parsed == 1 ? rewritten : zend_string_copy(sql);
    return true;
}

extern bool pgsql_handle_preparer(pdo_dbh_t *, zend_string *, pdo_stmt_t *, zval *);

#if PHP_VERSION_ID >= 80400
/* {{{ Sets a callback to receive DB notices (after client_min_messages has been set) */
PHP_METHOD(PDO_PGSql_Ext, pgsqlSetNoticeCallback)
{
	zend_fcall_info fci = empty_fcall_info;
	zend_fcall_info_cache fcc = empty_fcall_info_cache;
	if (FAILURE == zend_parse_parameters(ZEND_NUM_ARGS(), "F!", &fci, &fcc)) {
		RETURN_THROWS();
	}

	pdo_dbh_t *dbh = Z_PDO_DBH_P(ZEND_THIS);
	PDO_CONSTRUCT_CHECK_WITH_CLEANUP(cleanup);

	pdo_pgsql_db_handle *H = (pdo_pgsql_db_handle *)dbh->driver_data;

    zend_fcall_info_cache *next = NULL;
    if (ZEND_FCC_INITIALIZED(fcc)) {
        next = emalloc(sizeof(*next));
        zend_fcc_dup(next, &fcc);
    }
    pdo_pgsql_replace_notice_callback(H, next);

	return;

cleanup:
	if (ZEND_FCC_INITIALIZED(fcc)) {
		zend_fcc_dtor(&fcc);
	}
	RETURN_THROWS();
}
/* }}} */

#endif

static const zend_function_entry *pdo_pgsql_get_driver_methods(pdo_dbh_t *dbh, int kind)
{
	switch (kind) {
		case PDO_DBH_DRIVER_METHOD_KIND_DBH:
			return class_PDO_PGSql_Ext_methods;
		default:
			return NULL;
	}
}

const struct pdo_dbh_methods swoole_pdo_pgsql_methods = {
	pdo_pgsql_handle_closer,
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
	pdo_pgsql_check_liveness,	/* check_liveness */
	pdo_pgsql_get_driver_methods,  /* get_driver_methods */
	NULL,
	pdo_pgsql_handle_in_transaction,
	NULL, /* get_gc */
#if PHP_VERSION_ID >= 80400
	pdo_pgsql_scanner
#endif
};

static int pdo_pgsql_handle_factory(pdo_dbh_t *dbh, zval *driver_options) {
	return pdo_pgsql_connect(dbh, dbh->data_source, dbh->username, dbh->password, driver_options);
}

const pdo_driver_t swoole_pdo_pgsql_driver = {
	PDO_DRIVER_HEADER(pgsql),
	pdo_pgsql_handle_factory
};
#endif

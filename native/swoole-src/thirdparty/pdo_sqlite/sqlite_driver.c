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
  | Author: Wez Furlong <wez@php.net>                                    |
  +----------------------------------------------------------------------+
*/

#define SW_USE_SQLITE_HOOK
#include "php_swoole_sqlite.h"

#include "php.h"
#include "php_ini.h"
#include "ext/standard/info.h"
#include "ext/pdo/php_pdo.h"
#include "ext/pdo/php_pdo_driver.h"
#include "thirdparty/pdo_sqlite/php_pdo_sqlite.h"
#include "thirdparty/pdo_sqlite/php_pdo_sqlite_int.h"
#include "zend_exceptions.h"

extern void pdo_sqlite_sqlstate(int code, pdo_error_type *state);

int _pdo_sqlite_error(pdo_dbh_t *dbh, pdo_stmt_t *stmt, const char *file, int line) /* {{{ */
{
	pdo_sqlite_db_handle *H = (pdo_sqlite_db_handle *)dbh->driver_data;
	pdo_error_type *pdo_err = stmt ? &stmt->error_code : &dbh->error_code;
	pdo_sqlite_error_info *einfo = &H->einfo;

	einfo->errcode = sqlite3_errcode(H->db);
	einfo->file = file;
	einfo->line = line;

	pdo_sqlite_sqlstate(einfo->errcode, pdo_err);
	if (einfo->errcode != SQLITE_OK) {
		if (einfo->errmsg) {
			pefree(einfo->errmsg, dbh->is_persistent);
		}
		einfo->errmsg = pestrdup((char*)sqlite3_errmsg(H->db), dbh->is_persistent);
	} else { /* no error */
		return 0;
	}
	if (!dbh->methods) {
		pdo_throw_exception(einfo->errcode, einfo->errmsg, pdo_err);
	}

	return einfo->errcode;
}
/* }}} */

extern void swr_pdo_sqlite_error_info(zval *, unsigned int, const char *);

static void pdo_sqlite_fetch_error_func(pdo_dbh_t *dbh, pdo_stmt_t *stmt, zval *info)
{
	pdo_sqlite_error_info *einfo = &((pdo_sqlite_db_handle *)dbh->driver_data)->einfo;
	swr_pdo_sqlite_error_info(info, einfo->errcode, einfo->errmsg);
}

static void sqlite_handle_closer(pdo_dbh_t *dbh) /* {{{ */
{
	pdo_sqlite_db_handle *H = (pdo_sqlite_db_handle *)dbh->driver_data;

	if (H) {
		pdo_sqlite_error_info *einfo = &H->einfo;

		if (H->db) {
#ifdef HAVE_SW_SQLITE3_CLOSE_V2
			sqlite3_close_v2(H->db);
#else
			sqlite3_close(H->db);
#endif
			H->db = NULL;
		}
		if (einfo->errmsg) {
			pefree(einfo->errmsg, dbh->is_persistent);
			einfo->errmsg = NULL;
		}
		pefree(H, dbh->is_persistent);
		dbh->driver_data = NULL;
	}
}
/* }}} */

extern bool swr_pdo_sqlite_prepare(pdo_dbh_t *, const zend_string *, zval *, sqlite3_stmt **);

static bool sqlite_handle_preparer(pdo_dbh_t *dbh, zend_string *sql, pdo_stmt_t *stmt, zval *driver_options)
{
	pdo_sqlite_db_handle *H = (pdo_sqlite_db_handle *)dbh->driver_data;
	pdo_sqlite_stmt *S = ecalloc(1, sizeof(pdo_sqlite_stmt));

	S->H = H;
	stmt->driver_data = S;
	stmt->methods = &swoole_sqlite_stmt_methods;
	stmt->supports_placeholders = PDO_PLACEHOLDER_POSITIONAL|PDO_PLACEHOLDER_NAMED;

	return swr_pdo_sqlite_prepare(dbh, sql, driver_options, &S->stmt);
}

/* The bundled connection record is still allocated by the C factory. */
sqlite3 *swr_pdo_sqlite_connection(pdo_dbh_t *dbh)
{
	return ((pdo_sqlite_db_handle *)dbh->driver_data)->db;
}

int swr_pdo_sqlite_transaction_mode(pdo_dbh_t *dbh)
{
	return ((pdo_sqlite_db_handle *)dbh->driver_data)->transaction_mode;
}

void swr_pdo_sqlite_record_error(pdo_dbh_t *dbh)
{
	pdo_sqlite_error(dbh);
}

extern zend_long swr_pdo_sqlite_exec(pdo_dbh_t *, const zend_string *);
extern bool swr_pdo_sqlite_begin(pdo_dbh_t *);
extern bool swr_pdo_sqlite_commit(pdo_dbh_t *);
extern bool swr_pdo_sqlite_rollback(pdo_dbh_t *);
extern bool swr_pdo_sqlite_in_transaction(pdo_dbh_t *);

extern zend_string *swr_pdo_sqlite_last_insert_id(pdo_dbh_t *, const zend_string *);
extern zend_string *swr_pdo_sqlite_quote(pdo_dbh_t *, const zend_string *, enum pdo_param_type);

void swr_pdo_sqlite_set_transaction_mode(pdo_dbh_t *dbh, int mode)
{
	((pdo_sqlite_db_handle *)dbh->driver_data)->transaction_mode = mode;
}

extern int swr_pdo_sqlite_get_attribute(pdo_dbh_t *, zend_long, zval *);
extern bool swr_pdo_sqlite_set_attribute(pdo_dbh_t *, zend_long, zval *);

#if PHP_VERSION_ID >= 80400
#include <sys/uio.h>
extern int pdo_sqlite_scan_token(struct iovec, size_t *);

/* Stays C (version): PHP 8.4 added PDO's scanner cursor. Its end includes the terminal NUL. */
static int pdo_sqlite_scanner(pdo_scanner_t *s)
{
    size_t consumed;
    s->tok = s->cur;
    size_t length = s->cur < s->end ? (size_t)(s->end - s->cur - 1) : 0;
    int token = pdo_sqlite_scan_token((struct iovec){(void *)s->cur, length}, &consumed);
    s->cur += consumed;
    return token;
}
#endif

static const struct pdo_dbh_methods sqlite_methods = {
	sqlite_handle_closer,
	sqlite_handle_preparer,
	swr_pdo_sqlite_exec,
	swr_pdo_sqlite_quote,
	swr_pdo_sqlite_begin,
	swr_pdo_sqlite_commit,
	swr_pdo_sqlite_rollback,
	swr_pdo_sqlite_set_attribute,
	swr_pdo_sqlite_last_insert_id,
	pdo_sqlite_fetch_error_func,
	swr_pdo_sqlite_get_attribute,
	NULL,	/* check_liveness: not needed */
	NULL,
	NULL,	/* persistent_shutdown: nothing to release */
	swr_pdo_sqlite_in_transaction,
	NULL,
#if PHP_VERSION_ID >= 80400
	pdo_sqlite_scanner
#endif
};

bool swr_sqlite_open_basedir_active(void)
{
	return PG(open_basedir) && *PG(open_basedir);
}

extern bool swr_pdo_sqlite_open(pdo_dbh_t *, const char *, sqlite3 **, zval *);

static int pdo_sqlite_handle_factory(pdo_dbh_t *dbh, zval *driver_options) /* {{{ */
{
	pdo_sqlite_db_handle *H = pecalloc(1, sizeof(pdo_sqlite_db_handle), dbh->is_persistent);

	dbh->driver_data = H;
	bool opened = swr_pdo_sqlite_open(dbh, dbh->data_source, &H->db, driver_options);
	dbh->methods = &sqlite_methods;

	return opened;
}
/* }}} */

const pdo_driver_t swoole_pdo_sqlite_driver = {
	PDO_DRIVER_HEADER(sqlite),
	pdo_sqlite_handle_factory
};

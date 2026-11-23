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

#if defined(__APPLE__)
// If more than one usage, a Zend macro could be created
// around this runtime check
#include <Availability.h>
#endif

static int pdo_sqlite_stmt_dtor(pdo_stmt_t *stmt)
{
	pdo_sqlite_stmt *S = (pdo_sqlite_stmt*)stmt->driver_data;

	if (S->stmt) {
		sqlite3_finalize(S->stmt);
		S->stmt = NULL;
	}
	efree(S);
	return 1;
}

extern bool swr_sqlite_execute(pdo_stmt_t *, sqlite3_stmt *, sqlite3 *, uint8_t *, bool);
extern bool swr_sqlite_fetch(pdo_stmt_t *, sqlite3_stmt *, uint8_t *);
extern void swr_sqlite_before_bind(sqlite3_stmt *, uint8_t *, bool);

void swr_sqlite_stmt_error(pdo_stmt_t *stmt)
{
	pdo_sqlite_error_stmt(stmt);
}

static int pdo_sqlite_stmt_execute(pdo_stmt_t *stmt)
{
	pdo_sqlite_stmt *S = stmt->driver_data;
	return swr_sqlite_execute(stmt, S->stmt, S->H->db, &S->cursor, stmt->executed);
}

extern bool swr_sqlite_bind_null(pdo_stmt_t *, sqlite3_stmt *, int);
extern bool swr_sqlite_bind_integer(pdo_stmt_t *, sqlite3_stmt *, int, int64_t);
extern bool swr_sqlite_bind_bytes(pdo_stmt_t *, sqlite3_stmt *, int, const char *, size_t, bool);

static int pdo_sqlite_stmt_param_hook(pdo_stmt_t *stmt, struct pdo_bound_param_data *param,
		enum pdo_param_event event_type)
{
	if (event_type != PDO_PARAM_EVT_EXEC_PRE) {
		return 1;
	}
	pdo_sqlite_stmt *S = stmt->driver_data;
	swr_sqlite_before_bind(S->stmt, &S->cursor, stmt->executed);
	if (!param->is_param) {
		return 1;
	}
	if (param->paramno == -1) {
		param->paramno = sqlite3_bind_parameter_index(S->stmt, ZSTR_VAL(param->name)) - 1;
	}

	enum pdo_param_type kind = PDO_PARAM_TYPE(param->param_type);
	if (kind == PDO_PARAM_STMT) {
		return 0;
	}
	zval *value = &param->parameter;
	ZVAL_DEREF(value);
	if (kind == PDO_PARAM_NULL || Z_TYPE_P(value) == IS_NULL) {
		return swr_sqlite_bind_null(stmt, S->stmt, param->paramno + 1);
	}
	if (kind == PDO_PARAM_INT || kind == PDO_PARAM_BOOL) {
		convert_to_long(value);
		return swr_sqlite_bind_integer(stmt, S->stmt, param->paramno + 1, Z_LVAL_P(value));
	}

	bool blob = kind == PDO_PARAM_LOB;
	if (blob && Z_TYPE_P(value) == IS_RESOURCE) {
		php_stream *stream = NULL;
		php_stream_from_zval_no_verify(stream, value);
		if (!stream) {
			pdo_raise_impl_error(stmt->dbh, stmt, "HY105", "Expected a stream resource");
			return 0;
		}
		zend_string *bytes = php_stream_copy_to_mem(stream, PHP_STREAM_COPY_ALL, 0);
		zval_ptr_dtor(value);
		ZVAL_STR(value, bytes ? bytes : ZSTR_EMPTY_ALLOC());
	} else if (!try_convert_to_string(value)) {
		return 0;
	}
	return swr_sqlite_bind_bytes(stmt, S->stmt, param->paramno + 1, Z_STRVAL_P(value), Z_STRLEN_P(value), blob);
}

static int pdo_sqlite_stmt_fetch(pdo_stmt_t *stmt,
	enum pdo_fetch_orientation ori, zend_long offset)
{
	pdo_sqlite_stmt *S = stmt->driver_data;
	return swr_sqlite_fetch(stmt, S->stmt, &S->cursor);
}

static int pdo_sqlite_stmt_describe(pdo_stmt_t *stmt, int colno)
{
	pdo_sqlite_stmt *S = (pdo_sqlite_stmt*)stmt->driver_data;
	const char *str;

	if(colno >= sqlite3_column_count(S->stmt)) {
		/* error invalid column */
		pdo_sqlite_error_stmt(stmt);
		return 0;
	}

	str = sqlite3_column_name(S->stmt, colno);
	stmt->columns[colno].name = zend_string_init(str, strlen(str), 0);
	stmt->columns[colno].maxlen = SIZE_MAX;
	stmt->columns[colno].precision = 0;

	return 1;
}

/* Copy the borrowed SQLite buffer into Zend's string, including its short-string cache. */
zend_string *swr_sqlite_column_string(sqlite3_stmt *stmt, int colno, bool blob)
{
	const void *bytes = blob ? sqlite3_column_blob(stmt, colno) : sqlite3_column_text(stmt, colno);
	zval value;
	ZVAL_STRINGL_FAST(&value, bytes, sqlite3_column_bytes(stmt, colno));
	return Z_STR(value);
}

extern bool swr_sqlite_get_col(pdo_stmt_t *, sqlite3_stmt *, int, zval *);

static int pdo_sqlite_stmt_get_col(
		pdo_stmt_t *stmt, int colno, zval *result, enum pdo_param_type *type)
{
	pdo_sqlite_stmt *S = (pdo_sqlite_stmt*)stmt->driver_data;
	return swr_sqlite_get_col(stmt, S->stmt, colno, result);
}

/* Zend owns the interned native type names for the life of the request. */
const zend_string *swr_sqlite_known_type(unsigned int id)
{
	return ZSTR_KNOWN(id);
}

extern bool swr_sqlite_column_meta(pdo_stmt_t *, sqlite3_stmt *, zend_long, zval *);

static int pdo_sqlite_stmt_col_meta(pdo_stmt_t *stmt, zend_long colno, zval *return_value)
{
	pdo_sqlite_stmt *S = (pdo_sqlite_stmt*)stmt->driver_data;
	return swr_sqlite_column_meta(stmt, S->stmt, colno, return_value) ? SUCCESS : FAILURE;
}

static int pdo_sqlite_stmt_cursor_closer(pdo_stmt_t *stmt)
{
	pdo_sqlite_stmt *S = (pdo_sqlite_stmt*)stmt->driver_data;
	sqlite3_reset(S->stmt);
	return 1;
}

extern bool swr_sqlite_stmt_flag(sqlite3_stmt *, zend_long, zval *);
extern bool swr_sqlite_stmt_explain(sqlite3_stmt *, zval *, bool,
	int (*)(sqlite3_stmt *), int (*)(sqlite3_stmt *, int));

void swr_sqlite_explain_type_error(zval *value)
{
	zend_type_error("explain mode must be of type int, %s given", zend_zval_value_name(value));
}

/* SQLite 3.43 added stmt_explain. The macOS SDK may be newer than the running OS. */
static bool sqlite_stmt_explain(sqlite3_stmt *stmt, zval *value, bool write)
{
#if SQLITE_VERSION_NUMBER >= 3043000
#ifdef __APPLE__
	if (__builtin_available(macOS 14.2, *))
#endif
	{
		return swr_sqlite_stmt_explain(stmt, value, write, sqlite3_stmt_isexplain, sqlite3_stmt_explain);
	}
#endif
	zend_value_error("explain statement unsupported");
	return false;
}

static int pdo_sqlite_stmt_get_attribute(pdo_stmt_t *stmt, zend_long attr, zval *value)
{
	pdo_sqlite_stmt *S = stmt->driver_data;
	if (attr == PDO_SQLITE_ATTR_EXPLAIN_STATEMENT) {
		return sqlite_stmt_explain(S->stmt, value, false);
	}
	return swr_sqlite_stmt_flag(S->stmt, attr, value);
}

static int pdo_sqlite_stmt_set_attribute(pdo_stmt_t *stmt, zend_long attr, zval *value)
{
	pdo_sqlite_stmt *S = stmt->driver_data;
	return attr == PDO_SQLITE_ATTR_EXPLAIN_STATEMENT && sqlite_stmt_explain(S->stmt, value, true);
}

const struct pdo_stmt_methods swoole_sqlite_stmt_methods = {
	pdo_sqlite_stmt_dtor,
	pdo_sqlite_stmt_execute,
	pdo_sqlite_stmt_fetch,
	pdo_sqlite_stmt_describe,
	pdo_sqlite_stmt_get_col,
	pdo_sqlite_stmt_param_hook,
	pdo_sqlite_stmt_set_attribute, /* set_attr */
	pdo_sqlite_stmt_get_attribute, /* get_attr */
	pdo_sqlite_stmt_col_meta,
	NULL, /* next_rowset */
	pdo_sqlite_stmt_cursor_closer
};

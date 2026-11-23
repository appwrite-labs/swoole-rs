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
  | Author: Ard Biesheuvel <abies@php.net>                               |
  +----------------------------------------------------------------------+
*/

#include "php_swoole_firebird.h"


#include "php.h"
#include "php_ini.h"
#include "ext/standard/info.h"
#include "ext/pdo/php_pdo.h"
#include "ext/pdo/php_pdo_driver.h"
#include "php_pdo_firebird_int.h"

/* free the allocated space for passing field values to the db and back */
static void php_firebird_free_sqlda(XSQLDA const *sqlda) /* {{{ */
{
	int i;

	for (i = 0; i < sqlda->sqld; ++i) {
		XSQLVAR const *var = &sqlda->sqlvar[i];

		if (var->sqlind) {
			efree(var->sqlind);
		}
	}
}
/* }}} */

/* The destructor is Rust (crates/firebird/src/statement.rs); it reads and frees the record
 * through these. */
bool swr_firebird_stmt_dtor(pdo_stmt_t *stmt);

bool swr_firebird_db_usable(pdo_stmt_t *stmt)
{
	/* Note: if the server object is already gone then the statement was closed already as well. */
#if PHP_VERSION_ID >= 80500
	return php_pdo_stmt_valid_db_obj_handle(stmt);
#else
	/* TODO: for master, move this check to a separate function shared between pdo drivers.
	 *       pdo_pgsql and pdo_mysql do this exact same thing */
	return !Z_ISUNDEF(stmt->database_object_handle)
		&& IS_OBJ_VALID(EG(objects_store).object_buckets[Z_OBJ_HANDLE(stmt->database_object_handle)])
		&& !(OBJ_FLAGS(Z_OBJ(stmt->database_object_handle)) & IS_OBJ_FREE_CALLED);
#endif
}

void swr_firebird_stmt_free(pdo_stmt_t *stmt)
{
	pdo_firebird_stmt *S = (pdo_firebird_stmt*)stmt->driver_data;

	zend_hash_destroy(S->named_params);
	FREE_HASHTABLE(S->named_params);

	/* clean up the input descriptor */
	if (S->in_sqlda) {
		php_firebird_free_sqlda(S->in_sqlda);
		efree(S->in_sqlda);
	}

	php_firebird_free_sqlda(&S->out_sqlda);
	efree(S);
}

/* called by PDO to clean up a statement handle */
static int pdo_firebird_stmt_dtor(pdo_stmt_t *stmt) /* {{{ */
{
	return swr_firebird_stmt_dtor(stmt);
}
/* }}} */

/* The execute is Rust (crates/firebird/src/statement.rs); it reads the record through these. */
bool swr_firebird_execute(pdo_stmt_t *stmt);

XSQLDA *swr_firebird_in_sqlda(pdo_stmt_t *stmt)
{
	return ((pdo_firebird_stmt *)stmt->driver_data)->in_sqlda;
}

char swr_firebird_statement_type(pdo_stmt_t *stmt)
{
	return ((pdo_firebird_stmt *)stmt->driver_data)->statement_type;
}

void swr_firebird_cursor_reset(pdo_stmt_t *stmt)
{
	((pdo_firebird_stmt *)stmt->driver_data)->cursor_open = 0;
}

void swr_firebird_alloc_output(pdo_stmt_t *stmt)
{
	pdo_firebird_stmt *S = (pdo_firebird_stmt *)stmt->driver_data;

	for (unsigned int i = 0; i < (unsigned) S->out_sqlda.sqld; i++) {
		XSQLVAR *var = &S->out_sqlda.sqlvar[i];
		if (var->sqlind) {
			efree(var->sqlind);
		}
		var->sqlind = (void*)ecalloc(1, var->sqllen + 2 * sizeof(short));
		var->sqldata = &((char*)var->sqlind)[sizeof(short)];
	}
}

void swr_firebird_executed(pdo_stmt_t *stmt)
{
	pdo_firebird_stmt *S = (pdo_firebird_stmt *)stmt->driver_data;

	*S->name = 0;
	S->cursor_open = S->out_sqlda.sqln && (S->statement_type != isc_info_sql_stmt_exec_procedure);
	S->exhausted = !S->out_sqlda.sqln; /* There are data to fetch */
}

/* called by PDO to execute a prepared query */
static int pdo_firebird_stmt_execute(pdo_stmt_t *stmt) /* {{{ */
{
	return swr_firebird_execute(stmt);
}
/* }}} */

/* The fetch is Rust (crates/firebird/src/statement.rs); it reads the record through these. */
bool swr_firebird_fetch(pdo_stmt_t *stmt);

bool swr_firebird_exhausted(pdo_stmt_t *stmt)
{
	return ((pdo_firebird_stmt *)stmt->driver_data)->exhausted;
}

void swr_firebird_set_exhausted(pdo_stmt_t *stmt)
{
	((pdo_firebird_stmt *)stmt->driver_data)->exhausted = 1;
}

bool swr_firebird_is_procedure(pdo_stmt_t *stmt)
{
	return ((pdo_firebird_stmt *)stmt->driver_data)->statement_type == isc_info_sql_stmt_exec_procedure;
}

XSQLDA *swr_firebird_out_sqlda(pdo_stmt_t *stmt)
{
	return &((pdo_firebird_stmt *)stmt->driver_data)->out_sqlda;
}

/* called by PDO to fetch the next row from a statement */
static int pdo_firebird_stmt_fetch(pdo_stmt_t *stmt, /* {{{ */
	enum pdo_fetch_orientation ori, zend_long offset)
{
	return swr_firebird_fetch(stmt);
}
/* }}} */

/* The column description and metadata are Rust (crates/firebird/src/statement.rs). */
void swr_firebird_describe(XSQLVAR *var, bool fetch_table_names, struct pdo_column_data *col);
void swr_firebird_column_meta(XSQLVAR *var, zval *return_value);

static int pdo_firebird_stmt_describe(pdo_stmt_t *stmt, int colno) /* {{{ */
{
	pdo_firebird_stmt *S = (pdo_firebird_stmt*)stmt->driver_data;
	swr_firebird_describe(&S->out_sqlda.sqlvar[colno], S->H->fetch_table_names, &stmt->columns[colno]);
	return 1;
}
/* }}} */

static int pdo_firebird_stmt_get_column_meta(pdo_stmt_t *stmt, zend_long colno, zval *return_value)
{
	pdo_firebird_stmt *S = (pdo_firebird_stmt *) stmt->driver_data;
	swr_firebird_column_meta(&S->out_sqlda.sqlvar[colno], return_value);
	return 1;
}

/* Reading a BLOB is Rust (crates/firebird/src/statement.rs), given its id's bytes. */
bool swr_firebird_fetch_blob(pdo_stmt_t *stmt, const ISC_SCHAR *id, size_t id_len, zval *result);

bool swr_firebird_value(pdo_dbh_t *dbh, XSQLVAR const *var, bool null, const ISC_SCHAR *data, size_t len, zval *result);
/* A TIME or TIMESTAMP WITH TIME ZONE (Firebird 4 headers) is Rust too. */
bool swr_firebird_time_tz(pdo_dbh_t *dbh, XSQLVAR const *var, const ISC_SCHAR *data, size_t len, zval *result);

int pdo_firebird_stmt_get_col(
		pdo_stmt_t *stmt, int colno, zval *result, enum pdo_param_type *type)
{
	pdo_firebird_stmt *S = (pdo_firebird_stmt*)stmt->driver_data;
	XSQLVAR const *var = &S->out_sqlda.sqlvar[colno];

	/* Null, the scaled numbers, the strings, the unscaled integers, BOOLEAN, FLOAT,
	 * DOUBLE and the dates and times are Rust (crates/firebird/src/statement.rs). */
	if (swr_firebird_value(stmt->dbh, var, *var->sqlind == -1, var->sqldata,
			var->sqllen + ((var->sqltype & ~1) == SQL_VARYING ? 2 : 0), result)) {
		return 1;
	}

	if ((var->sqltype & ~1) == SQL_BLOB) {
		return swr_firebird_fetch_blob(stmt, var->sqldata, sizeof(ISC_QUAD), result);
	}
	/* Upstream returned 0 once it set a time-zone value, and 1 otherwise. */
	return !swr_firebird_time_tz(stmt->dbh, var, var->sqldata, var->sqllen, result);
}

/* Parameter binding is Rust (crates/firebird/src/param.rs), given the descriptor's
 * variables (a count of -1 without one) and the value, dereferenced; it reaches
 * the variables' storage and the statement record through these. */
bool swr_firebird_param_hook(pdo_stmt_t *stmt, XSQLVAR *vars, short count, enum pdo_param_event event,
	bool is_param, zend_string *name, zend_long *paramno, zval *value);

HashTable *swr_firebird_named_params(pdo_stmt_t *stmt)
{
	return ((pdo_firebird_stmt *)stmt->driver_data)->named_params;
}

void swr_firebird_alloc_param(XSQLVAR *var)
{
	if (var->sqlind) {
		efree(var->sqlind);
	}
	var->sqlind = (void*)emalloc(var->sqllen + 2*sizeof(short));
	var->sqldata = &((char*)var->sqlind)[sizeof(short)];
}

void swr_firebird_set_null(XSQLVAR *var, bool null)
{
	if (var->sqlind) {
		*var->sqlind = null ? -1 : 0;
	}
}

void swr_firebird_set_data(XSQLVAR *var, const char *data, size_t len)
{
	if (var->sqlind && len <= (size_t) var->sqllen) {
		memcpy(var->sqldata, data, len);
	}
}

bool swr_firebird_param_stream(pdo_stmt_t *stmt, zval *parameter)
{
	php_stream *stm = NULL;

	php_stream_from_zval_no_verify(stm, parameter);
	if (!stm) {
		pdo_raise_impl_error(stmt->dbh, stmt, "HY105", "Expected a stream resource");
		return false;
	}
	zend_string *mem = php_stream_copy_to_mem(stm, PHP_STREAM_COPY_ALL, 0);
	zval_ptr_dtor(parameter);
	ZVAL_STR(parameter, mem ? mem : ZSTR_EMPTY_ALLOC());
	return true;
}

static int pdo_firebird_stmt_param_hook(pdo_stmt_t *stmt, struct pdo_bound_param_data *param, /* {{{ */
	enum pdo_param_event event_type)
{
	pdo_firebird_stmt *S = (pdo_firebird_stmt*)stmt->driver_data;
	XSQLDA *sqlda = param->is_param ? S->in_sqlda : &S->out_sqlda;
	zval *value = &param->parameter;

	ZVAL_DEREF(value);
	return swr_firebird_param_hook(stmt, sqlda ? sqlda->sqlvar : NULL, sqlda ? sqlda->sqld : -1, event_type,
		param->is_param, param->name, &param->paramno, value);
}
/* }}} */

/* The cursor's naming, name and closer are Rust (crates/firebird/src/statement.rs);
 * they reach the statement record through these. */
bool swr_firebird_stmt_set_attribute(pdo_stmt_t *stmt, zend_long attr, zval *val);
int pdo_firebird_stmt_get_attribute(pdo_stmt_t *stmt, zend_long attr, zval *val);
bool swr_firebird_cursor_close(pdo_stmt_t *stmt);

pdo_dbh_t *swr_firebird_stmt_dbh(pdo_stmt_t *stmt)
{
	return stmt->dbh;
}

isc_stmt_handle *swr_firebird_stmt_handle(pdo_stmt_t *stmt)
{
	return &((pdo_firebird_stmt *)stmt->driver_data)->stmt;
}

const char *swr_firebird_cursor_name(pdo_stmt_t *stmt)
{
	return ((pdo_firebird_stmt *)stmt->driver_data)->name;
}

bool swr_firebird_cursor_open(pdo_stmt_t *stmt)
{
	return ((pdo_firebird_stmt *)stmt->driver_data)->cursor_open;
}

void swr_firebird_cursor_named(pdo_stmt_t *stmt, const char *name)
{
	pdo_firebird_stmt *S = (pdo_firebird_stmt *)stmt->driver_data;
	strlcpy(S->name, name, sizeof(S->name));
}

void swr_firebird_cursor_closed(pdo_stmt_t *stmt)
{
	pdo_firebird_stmt *S = (pdo_firebird_stmt *)stmt->driver_data;
	*S->name = 0;
	S->cursor_open = 0;
}

static int pdo_firebird_stmt_set_attribute(pdo_stmt_t *stmt, zend_long attr, zval *val) /* {{{ */
{
	return swr_firebird_stmt_set_attribute(stmt, attr, val);
}
/* }}} */

static int pdo_firebird_stmt_cursor_closer(pdo_stmt_t *stmt) /* {{{ */
{
	return swr_firebird_cursor_close(stmt);
}
/* }}} */


const struct pdo_stmt_methods swoole_firebird_stmt_methods = { /* {{{ */
	pdo_firebird_stmt_dtor,
	pdo_firebird_stmt_execute,
	pdo_firebird_stmt_fetch,
	pdo_firebird_stmt_describe,
	pdo_firebird_stmt_get_col,
	pdo_firebird_stmt_param_hook,
	pdo_firebird_stmt_set_attribute,
	pdo_firebird_stmt_get_attribute,
	pdo_firebird_stmt_get_column_meta,
	NULL, /* next_rowset_func */
	pdo_firebird_stmt_cursor_closer
};
/* }}} */

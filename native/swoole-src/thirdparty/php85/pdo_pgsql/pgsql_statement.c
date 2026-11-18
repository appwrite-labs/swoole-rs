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

#define SW_USE_PGSQL_HOOK
#include "php_swoole_pgsql.h"

#if PHP_VERSION_ID >= 80500
#include "php.h"
#include "php_ini.h"
#include "ext/standard/info.h"
#include "ext/pdo/php_pdo.h"
#include "ext/pdo/php_pdo_driver.h"
#include "php_pdo_pgsql.h"
#include "php_pdo_pgsql_int.h"
#ifdef HAVE_NETINET_IN_H
#include <netinet/in.h>
#endif

/* from postgresql/src/include/catalog/pg_type.h */
#define BYTEAOID    17
#define OIDOID      26

#define FIN_CLOSE   0x2
#define FIN_ABORT   0x4

static void pgsql_stmt_finish(pdo_pgsql_stmt *S, int fin_mode)
{
	pdo_pgsql_db_handle *H = S->H;

	pdo_pgsql_finish_result(S->is_running_unbuffered ? pdo_pgsql_server(H) : NULL, &S->result, (fin_mode & FIN_ABORT) != 0);
	S->is_running_unbuffered = false;

	if (S->stmt_name && S->is_prepared && (fin_mode & FIN_CLOSE)) {
		pdo_pgsql_close_statement(H, S->stmt_name);
		S->is_prepared = false;
	}
	if ((fin_mode & FIN_CLOSE) && *pdo_pgsql_running_stmt(H) == S) {
		*pdo_pgsql_running_stmt(H) = NULL;
	}
}

static int pgsql_stmt_dtor(pdo_stmt_t *stmt)
{
	pdo_pgsql_stmt *S = (pdo_pgsql_stmt*)stmt->driver_data;
	bool server_obj_usable = php_pdo_stmt_valid_db_obj_handle(stmt);

	pgsql_stmt_finish(S, server_obj_usable ? FIN_CLOSE|FIN_ABORT : 0);

	if (S->stmt_name) {
		efree(S->stmt_name);
		S->stmt_name = NULL;
	}
	pdo_pgsql_free_parameters(S->params.owner);
	if (S->query) {
		zend_string_release(S->query);
		S->query = NULL;
	}

	if (S->cursor_name) {
		if (server_obj_usable) {
			pdo_pgsql_close_cursor(S->H, S->cursor_name);
		}
		efree(S->cursor_name);
		S->cursor_name = NULL;
	}

	efree(S);
	stmt->driver_data = NULL;
	return 1;
}

static int pgsql_stmt_execute(pdo_stmt_t *stmt)
{
	pdo_pgsql_stmt *S = (pdo_pgsql_stmt*)stmt->driver_data;
	pdo_pgsql_db_handle *H = S->H;

	bool in_trans = stmt->dbh->methods->in_transaction(stmt->dbh);

	/* in unbuffered mode, finish any running statement: libpq explicitely prohibits this
	 * and returns a PGRES_FATAL_ERROR when PQgetResult gets called for stmt 2 if DEALLOCATE
	 * was called for stmt 1 inbetween
	 * (maybe it will change with pipeline mode in libpq 14?) */
	if (S->is_unbuffered && *pdo_pgsql_running_stmt(H)) {
		pgsql_stmt_finish(*pdo_pgsql_running_stmt(H), FIN_CLOSE);
		*pdo_pgsql_running_stmt(H) = NULL;
	}
	/* ensure that we free any previous unfetched results */
	pgsql_stmt_finish(S, 0);

	S->current_row = 0;

	if (S->cursor_name) {
		if (!pdo_pgsql_open_cursor(stmt->dbh, stmt, &S->result, S->cursor_name, &S->is_prepared, ZSTR_VAL(stmt->active_query_string))) {
			return 0;
		}
	} else if (!pdo_pgsql_dispatch(stmt, S, S->is_unbuffered)) {
		return 0;
	}
	*pdo_pgsql_running_stmt(H) = S;

	if (S->is_unbuffered) {
		S->is_running_unbuffered = true;
		(void)PQsetSingleRowMode(pdo_pgsql_server(H));
		/* no matter if it returns 0: PQ then transparently fallbacks to full result fetching */

		/* try a first fetch to at least have column names and so on */
		S->result = PQgetResult(pdo_pgsql_server(S->H));
	}

	if (!pdo_pgsql_execution_result(stmt->dbh, stmt, S->result)) {
		return 0;
	}

	if (in_trans && !stmt->dbh->methods->in_transaction(stmt->dbh)) {
		pdo_pgsql_close_lob_streams(stmt->dbh);
	}

	return 1;
}

static int pgsql_stmt_get_col(pdo_stmt_t *stmt, int colno, zval *result, enum pdo_param_type *type)
{
	pdo_pgsql_stmt *S = (pdo_pgsql_stmt*)stmt->driver_data;
	if (!S->result) {
		return 0;
	}

	/* We have already increased count by 1 in pgsql_stmt_fetch() */
	if (PQgetisnull(S->result, S->current_row - 1, colno)) { /* Check if we got NULL */
		ZVAL_NULL(result);
	} else {
		char *ptr = PQgetvalue(S->result, S->current_row - 1, colno);
		size_t len = PQgetlength(S->result, S->current_row - 1, colno);

		Oid oid_type = PQftype(S->result, colno);
		switch (oid_type) {
			case OIDOID: {
				char *end_ptr;
				Oid oid = (Oid)strtoul(ptr, &end_ptr, 10);
				if (type && *type == PDO_PARAM_LOB) {
					/* If column was bound as LOB, return a stream. */
					int loid = lo_open(pdo_pgsql_server(S->H), oid, INV_READ);
					if (loid >= 0) {
						php_stream *stream = pdo_pgsql_create_lob_stream(stmt->database_object_handle, loid, oid);
						if (stream) {
							php_stream_to_zval(stream, result);
							return 1;
						}
					}
					return 0;
				} else {
					/* Otherwise return OID as integer. */
					ZVAL_LONG(result, oid);
				}
				break;
			}

			case BYTEAOID: {
				size_t tmp_len;
				char *tmp_ptr = (char *)PQunescapeBytea((unsigned char *) ptr, &tmp_len);
				if (!tmp_ptr) {
					/* PQunescapeBytea returned an error */
					return 0;
				}

				zend_string *str = zend_string_init(tmp_ptr, tmp_len, 0);
				php_stream *stream = php_stream_memory_open(TEMP_STREAM_READONLY, str);
				php_stream_to_zval(stream, result);
				zend_string_release(str);
				PQfreemem(tmp_ptr);
				break;
			}

			default:
				pdo_pgsql_scalar_column(oid_type, (struct iovec){ptr, len}, result);
				break;
		}
	}

	return 1;
}

static int pgsql_stmt_get_attr(pdo_stmt_t *stmt, zend_long attr, zval *val)
{
	pdo_pgsql_stmt *S = (pdo_pgsql_stmt*)stmt->driver_data;

	switch (attr) {
#ifdef HAVE_PG_RESULT_MEMORY_SIZE
		case PDO_PGSQL_ATTR_RESULT_MEMORY_SIZE:
			if(stmt->executed) {
				ZVAL_LONG(val, PQresultMemorySize(S->result));
			} else {
				char *tmp;
				spprintf(&tmp, 0, "statement '%s' has not been executed yet", S->stmt_name);

				pdo_pgsql_error_stmt_msg(stmt, 0, "HY000", tmp);
				efree(tmp);

				ZVAL_NULL(val);
			}
			return 1;
#endif

		default:
			(void)S;
			return 0;
	}
}

const struct pdo_stmt_methods swoole_pgsql_stmt_methods = {
	pgsql_stmt_dtor,
	pgsql_stmt_execute,
	pdo_pgsql_stmt_fetch,
	pdo_pgsql_stmt_describe,
	pgsql_stmt_get_col,
	pdo_pgsql_stmt_param_hook,
	NULL, /* set_attr */
	pgsql_stmt_get_attr,
	pdo_pgsql_stmt_get_column_meta,
	NULL,  /* next_rowset */
	pdo_pgsql_stmt_cursor_closer
};
#endif
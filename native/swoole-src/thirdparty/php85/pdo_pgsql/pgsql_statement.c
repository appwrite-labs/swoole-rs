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
#define BOOLOID     16
#define BYTEAOID    17
#define INT2OID     21
#define INT4OID     23
#define INT8OID     20
#define OIDOID      26
#define FLOAT4OID 700
#define FLOAT8OID 701

#define FIN_DISCARD 0x1
#define FIN_CLOSE   0x2
#define FIN_ABORT   0x4



static void pgsql_stmt_finish(pdo_pgsql_stmt *S, int fin_mode)
{
	pdo_pgsql_db_handle *H = S->H;

	if (S->is_running_unbuffered && S->result && (fin_mode & FIN_ABORT)) {
		PGcancel *cancel = PQgetCancel(pdo_pgsql_server(H));
		char errbuf[256];
		PQcancel(cancel, errbuf, 256);
		PQfreeCancel(cancel);
		S->is_running_unbuffered = false;
	}

	if (S->result) {
		/* free the resource */
		PQclear(S->result);
		S->result = NULL;
	}

	if (S->is_running_unbuffered) {
		/* https://postgresql.org/docs/current/libpq-async.html:
		 * "PQsendQuery cannot be called again until PQgetResult has returned NULL"
		 * And as all single-row functions are connection-wise instead of statement-wise,
		 * any new single-row query has to make sure no preceding one is still running.
		 */
		// @todo Implement !(fin_mode & FIN_DISCARD)
		//       instead of discarding results we could store them to their statement
		//       so that their fetch() will get them (albeit not in lazy mode anymore).
		while ((S->result = PQgetResult(pdo_pgsql_server(H)))) {
			PQclear(S->result);
			S->result = NULL;
		}
		S->is_running_unbuffered = false;
	}

	if (S->stmt_name && S->is_prepared && (fin_mode & FIN_CLOSE)) {
		PGresult *res;
#ifndef HAVE_PQCLOSEPREPARED
		// TODO (??) libpq does not support close statement protocol < postgres 17
		// check if we can circumvent this.
		char *q = NULL;
		spprintf(&q, 0, "DEALLOCATE %s", S->stmt_name);
		res = PQexec(pdo_pgsql_server(H), q);
		efree(q);
#else
		res = PQclosePrepared(pdo_pgsql_server(H), S->stmt_name);
#endif
		if (res) {
			PQclear(res);
		}

		S->is_prepared = false;
		if (*pdo_pgsql_running_stmt(H) == S) {
			*pdo_pgsql_running_stmt(H) = NULL;
		}
	}
}

static int pgsql_stmt_dtor(pdo_stmt_t *stmt)
{
	pdo_pgsql_stmt *S = (pdo_pgsql_stmt*)stmt->driver_data;
	bool server_obj_usable = php_pdo_stmt_valid_db_obj_handle(stmt);

	pgsql_stmt_finish(S, FIN_DISCARD|(server_obj_usable ? FIN_CLOSE|FIN_ABORT : 0));

	if (S->stmt_name) {
		efree(S->stmt_name);
		S->stmt_name = NULL;
	}
	if (S->param_lengths) {
		efree(S->param_lengths);
		S->param_lengths = NULL;
	}
	if (S->param_values) {
		efree(S->param_values);
		S->param_values = NULL;
	}
	if (S->param_formats) {
		efree(S->param_formats);
		S->param_formats = NULL;
	}
	if (S->param_types) {
		efree(S->param_types);
		S->param_types = NULL;
	}
	if (S->query) {
		zend_string_release(S->query);
		S->query = NULL;
	}

	if (S->cursor_name) {
		if (server_obj_usable) {
			pdo_pgsql_db_handle *H = S->H;
			char *q = NULL;
			PGresult *res;

			spprintf(&q, 0, "CLOSE %s", S->cursor_name);
			res = PQexec(pdo_pgsql_server(H), q);
			efree(q);
			if (res) PQclear(res);
		}
		efree(S->cursor_name);
		S->cursor_name = NULL;
	}

	if(S->cols) {
		efree(S->cols);
		S->cols = NULL;
	}
	efree(S);
	stmt->driver_data = NULL;
	return 1;
}

static int pgsql_stmt_execute(pdo_stmt_t *stmt)
{
	pdo_pgsql_stmt *S = (pdo_pgsql_stmt*)stmt->driver_data;
	pdo_pgsql_db_handle *H = S->H;
	ExecStatusType status;
	int dispatch_result = 1;

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
		char *q = NULL;

		if (S->is_prepared) {
			spprintf(&q, 0, "CLOSE %s", S->cursor_name);
			PQclear(PQexec(pdo_pgsql_server(H), q));
			efree(q);
		}

		spprintf(&q, 0, "DECLARE %s SCROLL CURSOR WITH HOLD FOR %s", S->cursor_name, ZSTR_VAL(stmt->active_query_string));
		S->result = PQexec(pdo_pgsql_server(H), q);
		efree(q);

		/* check if declare failed */
		status = PQresultStatus(S->result);
		if (status != PGRES_COMMAND_OK && status != PGRES_TUPLES_OK) {
			pdo_pgsql_error_stmt(stmt, status, pdo_pgsql_sqlstate(S->result));
			return 0;
		}
		PQclear(S->result);

		/* the cursor was declared correctly */
		S->is_prepared = true;

		/* fetch to be able to get the number of tuples later, but don't advance the cursor pointer */
		spprintf(&q, 0, "FETCH FORWARD 0 FROM %s", S->cursor_name);
		S->result = PQexec(pdo_pgsql_server(H), q);
		efree(q);
	} else if (S->stmt_name) {
		/* using a prepared statement */

		if (!S->is_prepared) {
stmt_retry:
			/* we deferred the prepare until now, because we didn't
			 * know anything about the parameter types; now we do */
			S->result = PQprepare(pdo_pgsql_server(H), S->stmt_name, ZSTR_VAL(S->query),
						stmt->bound_params ? zend_hash_num_elements(stmt->bound_params) : 0,
						S->param_types);
			status = PQresultStatus(S->result);
			switch (status) {
				case PGRES_COMMAND_OK:
				case PGRES_TUPLES_OK:
					/* it worked */
					S->is_prepared = true;
					PQclear(S->result);
					S->result = NULL;
					break;
				default: {
					char *sqlstate = pdo_pgsql_sqlstate(S->result);
					/* 42P05 means that the prepared statement already existed. this can happen if you use
					 * a connection pooling software line pgpool which doesn't close the db-connection once
					 * php disconnects. if php dies (no chance to run RSHUTDOWN) during execution it has no
					 * chance to DEALLOCATE the prepared statements it has created. so, if we hit a 42P05 we
					 * deallocate it and retry ONCE (thies 2005.12.15)
					 */
					if (sqlstate && !strcmp(sqlstate, "42P05")) {
						PGresult *res;
#ifndef HAVE_PQCLOSEPREPARED
						char buf[100]; /* stmt_name == "pdo_crsr_%08x" */
						snprintf(buf, sizeof(buf), "DEALLOCATE %s", S->stmt_name);
						res = PQexec(pdo_pgsql_server(H), buf);
#else
						res = PQclosePrepared(pdo_pgsql_server(H), S->stmt_name);
#endif
						if (res) {
							PQclear(res);
						}
						goto stmt_retry;
					} else {
						pdo_pgsql_error_stmt(stmt, status, sqlstate);
						return 0;
					}
				}
			}
		}
		if (S->is_unbuffered) {
			dispatch_result = PQsendQueryPrepared(pdo_pgsql_server(H), S->stmt_name,
					stmt->bound_params ?
						zend_hash_num_elements(stmt->bound_params) :
						0,
					(const char**)S->param_values,
					S->param_lengths,
					S->param_formats,
					0);
		} else {
			S->result = PQexecPrepared(pdo_pgsql_server(H), S->stmt_name,
				stmt->bound_params ?
					zend_hash_num_elements(stmt->bound_params) :
					0,
				(const char**)S->param_values,
				S->param_lengths,
				S->param_formats,
				0);
		}
	} else if (stmt->supports_placeholders == PDO_PLACEHOLDER_NAMED) {
		/* execute query with parameters */
		if (S->is_unbuffered) {
			dispatch_result = PQsendQueryParams(pdo_pgsql_server(H), ZSTR_VAL(S->query),
					stmt->bound_params ? zend_hash_num_elements(stmt->bound_params) : 0,
					S->param_types,
					(const char**)S->param_values,
					S->param_lengths,
					S->param_formats,
					0);
		} else {
			S->result = PQexecParams(pdo_pgsql_server(H), ZSTR_VAL(S->query),
				stmt->bound_params ? zend_hash_num_elements(stmt->bound_params) : 0,
				S->param_types,
				(const char**)S->param_values,
				S->param_lengths,
				S->param_formats,
				0);
		}
	} else {
		/* execute plain query (with embedded parameters) */
		if (S->is_unbuffered) {
			dispatch_result = PQsendQuery(pdo_pgsql_server(H), ZSTR_VAL(stmt->active_query_string));
		} else {
			S->result = PQexec(pdo_pgsql_server(H), ZSTR_VAL(stmt->active_query_string));
		}
	}

	*pdo_pgsql_running_stmt(H) = S;

	if (S->is_unbuffered) {
		if (!dispatch_result) {
			pdo_pgsql_error_stmt(stmt, 0, NULL);
			*pdo_pgsql_running_stmt(H) = NULL;
			return 0;
		}
		S->is_running_unbuffered = true;
		(void)PQsetSingleRowMode(pdo_pgsql_server(H));
		/* no matter if it returns 0: PQ then transparently fallbacks to full result fetching */

		/* try a first fetch to at least have column names and so on */
		S->result = PQgetResult(pdo_pgsql_server(S->H));
	}

	status = PQresultStatus(S->result);

	if (status != PGRES_COMMAND_OK && status != PGRES_TUPLES_OK && status != PGRES_SINGLE_TUPLE) {
		pdo_pgsql_error_stmt(stmt, status, pdo_pgsql_sqlstate(S->result));
		return 0;
	}

	stmt->column_count = (int) PQnfields(S->result);
	if (S->cols == NULL) {
		S->cols = ecalloc(stmt->column_count, sizeof(pdo_pgsql_column));
	}

	if (status == PGRES_COMMAND_OK) {
		stmt->row_count = ZEND_ATOL(PQcmdTuples(S->result));
	} else {
		stmt->row_count = (zend_long)PQntuples(S->result);
	}

	if (in_trans && !stmt->dbh->methods->in_transaction(stmt->dbh)) {
		pdo_pgsql_close_lob_streams(stmt->dbh);
	}

	return 1;
}

static int pgsql_stmt_param_hook(pdo_stmt_t *stmt, struct pdo_bound_param_data *param,
		enum pdo_param_event event_type)
{
	pdo_pgsql_stmt *S = (pdo_pgsql_stmt*)stmt->driver_data;

	if (stmt->supports_placeholders == PDO_PLACEHOLDER_NAMED && param->is_param) {
		switch (event_type) {
			case PDO_PARAM_EVT_FREE:
				if (param->driver_data) {
					efree(param->driver_data);
				}
				break;

			case PDO_PARAM_EVT_NORMALIZE:
				/* decode name from $1, $2 into 0, 1 etc. */
				if (param->name) {
					if (ZSTR_VAL(param->name)[0] == '$') {
						param->paramno = ZEND_ATOL(ZSTR_VAL(param->name) + 1);
					} else {
						/* resolve parameter name to rewritten name */
						zend_string *namevar;

						if (stmt->bound_param_map && (namevar = zend_hash_find_ptr(stmt->bound_param_map,
								param->name)) != NULL) {
							param->paramno = ZEND_ATOL(ZSTR_VAL(namevar) + 1);
							param->paramno--;
						} else {
							pdo_pgsql_error_stmt_msg(stmt, 0, "HY093", ZSTR_VAL(param->name));
							return 0;
						}
					}
				}
				break;

			case PDO_PARAM_EVT_ALLOC:
				if (!stmt->bound_param_map) {
					return 1;
				}
				if (!zend_hash_index_exists(stmt->bound_param_map, param->paramno)) {
					pdo_pgsql_error_stmt_msg(stmt, 0, "HY093", "parameter was not defined");
					return 0;
				}
				ZEND_FALLTHROUGH;
			case PDO_PARAM_EVT_EXEC_POST:
			case PDO_PARAM_EVT_FETCH_PRE:
			case PDO_PARAM_EVT_FETCH_POST:
				/* work is handled by EVT_NORMALIZE */
				return 1;

			case PDO_PARAM_EVT_EXEC_PRE:
				if (!stmt->bound_param_map) {
					return 1;
				}
				if (!S->param_values) {
					S->param_values = ecalloc(
							zend_hash_num_elements(stmt->bound_param_map),
							sizeof(char*));
					S->param_lengths = ecalloc(
							zend_hash_num_elements(stmt->bound_param_map),
							sizeof(int));
					S->param_formats = ecalloc(
							zend_hash_num_elements(stmt->bound_param_map),
							sizeof(int));
					S->param_types = ecalloc(
							zend_hash_num_elements(stmt->bound_param_map),
							sizeof(Oid));
				}
				if (param->paramno >= 0) {
					zval *parameter;

					/*
					if (param->paramno >= zend_hash_num_elements(stmt->bound_params)) {
						pdo_raise_impl_error(stmt->dbh, stmt, "HY093", "parameter was not defined");
						return 0;
					}
					*/

					if (Z_ISREF(param->parameter)) {
						parameter = Z_REFVAL(param->parameter);
					} else {
						parameter = &param->parameter;
					}

					if (PDO_PARAM_TYPE(param->param_type) == PDO_PARAM_LOB &&
							Z_TYPE_P(parameter) == IS_RESOURCE) {
						php_stream *stm;
						php_stream_from_zval_no_verify(stm, parameter);
						if (stm) {
							if (php_stream_is(stm, &pdo_pgsql_lob_stream_ops)) {
								struct pdo_pgsql_lob_self *self = (struct pdo_pgsql_lob_self*)stm->abstract;
								pdo_pgsql_bound_param *P = param->driver_data;

								if (P == NULL) {
									P = ecalloc(1, sizeof(*P));
									param->driver_data = P;
								}
								P->oid = htonl(self->oid);
								S->param_values[param->paramno] = (char*)&P->oid;
								S->param_lengths[param->paramno] = sizeof(P->oid);
								S->param_formats[param->paramno] = 1;
								S->param_types[param->paramno] = OIDOID;
								return 1;
							} else {
								zend_string *str = php_stream_copy_to_mem(stm, PHP_STREAM_COPY_ALL, 0);
								if (str != NULL) {
									ZVAL_STR(parameter, str);
								} else {
									ZVAL_EMPTY_STRING(parameter);
								}
							}
						} else {
							/* expected a stream resource */
							pdo_pgsql_error_stmt(stmt, PGRES_FATAL_ERROR, "HY105");
							return 0;
						}
					}

					if (PDO_PARAM_TYPE(param->param_type) == PDO_PARAM_NULL ||
							Z_TYPE_P(parameter) == IS_NULL) {
						S->param_values[param->paramno] = NULL;
						S->param_lengths[param->paramno] = 0;
					} else if (Z_TYPE_P(parameter) == IS_FALSE || Z_TYPE_P(parameter) == IS_TRUE) {
						S->param_values[param->paramno] = Z_TYPE_P(parameter) == IS_TRUE ? "t" : "f";
						S->param_lengths[param->paramno] = 1;
						S->param_formats[param->paramno] = 0;
					} else {
						convert_to_string(parameter);
						S->param_values[param->paramno] = Z_STRVAL_P(parameter);
						S->param_lengths[param->paramno] = Z_STRLEN_P(parameter);
						S->param_formats[param->paramno] = 0;
					}

					if (PDO_PARAM_TYPE(param->param_type) == PDO_PARAM_LOB) {
						S->param_types[param->paramno] = 0;
						S->param_formats[param->paramno] = 1;
					} else {
						S->param_types[param->paramno] = 0;
					}
				}
				break;
		}
	} else if (param->is_param && event_type == PDO_PARAM_EVT_NORMALIZE) {
		/* We need to manually convert to a pg native boolean value */
		if (PDO_PARAM_TYPE(param->param_type) == PDO_PARAM_BOOL &&
			((param->param_type & PDO_PARAM_INPUT_OUTPUT) != PDO_PARAM_INPUT_OUTPUT)) {
			const char *s = zend_is_true(&param->parameter) ? "t" : "f";
			param->param_type = PDO_PARAM_STR;
			zval_ptr_dtor(&param->parameter);
			ZVAL_STRINGL(&param->parameter, s, 1);
		}
	}
	return 1;
}

static int pgsql_stmt_fetch(pdo_stmt_t *stmt,
	enum pdo_fetch_orientation ori, zend_long offset)
{
	pdo_pgsql_stmt *S = (pdo_pgsql_stmt*)stmt->driver_data;

	if (S->cursor_name) {
		if (!pdo_pgsql_fetch_cursor(stmt->dbh, stmt, &S->result, S->cursor_name, ori, offset)) {
			return 0;
		}
		S->current_row = 1;
		return 1;
	} else {
		if (S->is_running_unbuffered && S->current_row >= stmt->row_count) {
			ExecStatusType status;

			/* @todo in unbuffered mode, PQ allows multiple queries to be passed:
			 *       column_count should be recomputed on each iteration */

			if(S->result) {
				PQclear(S->result);
				S->result = NULL;
			}

			S->result = PQgetResult(pdo_pgsql_server(S->H));
			status = PQresultStatus(S->result);

			if (status != PGRES_COMMAND_OK && status != PGRES_TUPLES_OK && status != PGRES_SINGLE_TUPLE) {
				pdo_pgsql_error_stmt(stmt, status, pdo_pgsql_sqlstate(S->result));
				return 0;
			}

			stmt->row_count = (zend_long)PQntuples(S->result);
			S->current_row = 0;

			if (!stmt->row_count) {
				S->is_running_unbuffered = false;
				/* libpq requires looping until getResult returns null */
				pgsql_stmt_finish(S, 0);
			}
		}
		if (S->current_row < stmt->row_count) {
			S->current_row++;
			return 1;
		} else {
			return 0;
		}
	}
}

static int pgsql_stmt_describe(pdo_stmt_t *stmt, int colno)
{
    pdo_pgsql_stmt *S = (pdo_pgsql_stmt *) stmt->driver_data;
    if (!S->result) {
        return 0;
    }
    return pdo_pgsql_describe_column(S->result, colno, &stmt->columns[colno], &S->cols[colno].pgsql_type);
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

		switch (S->cols[colno].pgsql_type) {
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
				pdo_pgsql_scalar_column(S->cols[colno].pgsql_type, (struct iovec){ptr, len}, result);
				break;
		}
	}

	return 1;
}

static int pgsql_stmt_get_column_meta(pdo_stmt_t *stmt, zend_long colno, zval *return_value)
{
    pdo_pgsql_stmt *S = (pdo_pgsql_stmt *) stmt->driver_data;
    if (!S->result || colno < 0 || colno >= stmt->column_count) {
        return FAILURE;
    }
    return pdo_pgsql_column_meta(S->H, S->cols[colno].pgsql_type, PQftable(S->result, colno), return_value);
}

static int pdo_pgsql_stmt_cursor_closer(pdo_stmt_t *stmt)
{
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
	pgsql_stmt_fetch,
	pgsql_stmt_describe,
	pgsql_stmt_get_col,
	pgsql_stmt_param_hook,
	NULL, /* set_attr */
	pgsql_stmt_get_attr,
	pgsql_stmt_get_column_meta,
	NULL,  /* next_rowset */
	pdo_pgsql_stmt_cursor_closer
};
#endif
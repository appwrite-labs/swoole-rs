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

#if PHP_VERSION_ID >= 80300 && PHP_VERSION_ID < 80400

#include "php.h"
#include "php_ini.h"
#include "ext/standard/info.h"
#include "pdo/php_pdo.h"
#include "pdo/php_pdo_driver.h"
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



static int pgsql_stmt_dtor(pdo_stmt_t *stmt)
{
	pdo_pgsql_stmt *S = (pdo_pgsql_stmt*)stmt->driver_data;
	bool server_obj_usable = !Z_ISUNDEF(stmt->database_object_handle)
		&& IS_OBJ_VALID(EG(objects_store).object_buckets[Z_OBJ_HANDLE(stmt->database_object_handle)])
		&& !(OBJ_FLAGS(Z_OBJ(stmt->database_object_handle)) & IS_OBJ_FREE_CALLED);

	if (S->result) {
		/* free the resource */
		PQclear(S->result);
		S->result = NULL;
	}

	if (S->stmt_name) {
		if (S->is_prepared && server_obj_usable) {
			pdo_pgsql_close_statement(S->H, S->stmt_name);
		}
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

	bool in_trans = stmt->dbh->methods->in_transaction(stmt->dbh);

	/* ensure that we free any previous unfetched results */
	if(S->result) {
		PQclear(S->result);
		S->result = NULL;
	}

	S->current_row = 0;

	if (S->cursor_name) {
		if (!pdo_pgsql_open_cursor(stmt->dbh, stmt, &S->result, S->cursor_name, &S->is_prepared, ZSTR_VAL(stmt->active_query_string))) {
			return 0;
		}
	} else if (S->stmt_name) {
		/* using a prepared statement */

		if (!S->is_prepared) {
			int count = stmt->bound_params ? zend_hash_num_elements(stmt->bound_params) : 0;
			if (!pdo_pgsql_prepare_statement(stmt->dbh, stmt, &S->result, S->stmt_name,
					ZSTR_VAL(S->query), count, S->params.types)) {
				return 0;
			}
			S->is_prepared = true;
		}
		S->result = PQexecPrepared(pdo_pgsql_server(H), S->stmt_name,
				stmt->bound_params ?
					zend_hash_num_elements(stmt->bound_params) :
					0,
				S->params.values,
				S->params.lengths,
				S->params.formats,
				0);
	} else if (stmt->supports_placeholders == PDO_PLACEHOLDER_NAMED) {
		/* execute query with parameters */
		S->result = PQexecParams(pdo_pgsql_server(H), ZSTR_VAL(S->query),
				stmt->bound_params ? zend_hash_num_elements(stmt->bound_params) : 0,
				S->params.types,
				S->params.values,
				S->params.lengths,
				S->params.formats,
				0);
	} else {
		/* execute plain query (with embedded parameters) */
		S->result = PQexec(pdo_pgsql_server(H), ZSTR_VAL(stmt->active_query_string));
	}
	if (!pdo_pgsql_execution_result(stmt->dbh, stmt, S->result)) {
		return 0;
	}
	if (S->cols == NULL) {
		S->cols = ecalloc(stmt->column_count, sizeof(pdo_pgsql_column));
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
			case PDO_PARAM_EVT_NORMALIZE:
				return pdo_pgsql_normalize_parameter(stmt->dbh, stmt, param->name, &param->paramno);
			case PDO_PARAM_EVT_ALLOC:
				return pdo_pgsql_check_parameter(stmt->dbh, stmt, stmt->bound_param_map, param->paramno);
			case PDO_PARAM_EVT_FREE:
			case PDO_PARAM_EVT_EXEC_POST:
			case PDO_PARAM_EVT_FETCH_PRE:
			case PDO_PARAM_EVT_FETCH_POST:
				/* work is handled by EVT_NORMALIZE */
				return 1;

			case PDO_PARAM_EVT_EXEC_PRE:
				if (!stmt->bound_param_map) {
					return 1;
				}
				if (!S->params.owner) {
					pdo_pgsql_init_parameters(&S->params, zend_hash_num_elements(stmt->bound_param_map));
				}
				if (param->paramno >= 0) {
					zval *parameter;

					if ((size_t) param->paramno >= S->params.count) {
						pdo_pgsql_error_stmt_msg(stmt, 0, "HY093", "parameter was not defined");
						return 0;
					}
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
								return pdo_pgsql_bind_oid(S->params.owner, param->paramno, self->oid);
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

					return pdo_pgsql_bind_value(S->params.owner, param->paramno, PDO_PARAM_TYPE(param->param_type), parameter);
				}
				break;
		}
	} else if (param->is_param && event_type == PDO_PARAM_EVT_NORMALIZE) {
		return pdo_pgsql_emulated_parameter(&param->param_type, &param->parameter);
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
						php_stream *stream = pdo_pgsql_create_lob_stream(&stmt->database_object_handle, loid, oid);
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

const struct pdo_stmt_methods swoole_pgsql_stmt_methods = {
	pgsql_stmt_dtor,
	pgsql_stmt_execute,
	pgsql_stmt_fetch,
	pgsql_stmt_describe,
	pgsql_stmt_get_col,
	pgsql_stmt_param_hook,
	NULL, /* set_attr */
	NULL, /* get_attr */
	pgsql_stmt_get_column_meta,
	NULL,  /* next_rowset */
	pdo_pgsql_stmt_cursor_closer
};
#endif

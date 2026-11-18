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

#if PHP_VERSION_ID >= 80200 && PHP_VERSION_ID < 80300

#include "php.h"
#include "php_ini.h"
#include "ext/standard/info.h"
#include "pdo/php_pdo.h"
#include "pdo/php_pdo_driver.h"
#include "php_pdo_pgsql_int.h"
#ifdef HAVE_NETINET_IN_H
#include <netinet/in.h>
#endif

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

	efree(S);
	stmt->driver_data = NULL;
	return 1;
}

static int pgsql_stmt_execute(pdo_stmt_t *stmt)
{
	pdo_pgsql_stmt *S = (pdo_pgsql_stmt*)stmt->driver_data;

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
	} else if (!pdo_pgsql_dispatch(stmt, S, false)) {
		return 0;
	}
	if (!pdo_pgsql_execution_result(stmt->dbh, stmt, S->result)) {
		return 0;
	}

	if (in_trans && !stmt->dbh->methods->in_transaction(stmt->dbh)) {
		pdo_pgsql_close_lob_streams(stmt->dbh);
	}

	return 1;
}

const struct pdo_stmt_methods swoole_pgsql_stmt_methods = {
	pgsql_stmt_dtor,
	pgsql_stmt_execute,
	pdo_pgsql_stmt_fetch,
	pdo_pgsql_stmt_describe,
	pdo_pgsql_stmt_get_col,
	pdo_pgsql_stmt_param_hook,
	NULL, /* set_attr */
	NULL, /* get_attr */
	pdo_pgsql_stmt_get_column_meta,
	NULL,  /* next_rowset */
	pdo_pgsql_stmt_cursor_closer
};
#endif

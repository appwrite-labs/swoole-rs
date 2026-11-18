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
#include "main/php_network.h"
#include "pdo/php_pdo.h"
#include "pdo/php_pdo_driver.h"
#include "pdo/php_pdo_error.h"
#include "ext/standard/file.h"
#include "php_pdo_pgsql_int.h"
#include "zend_exceptions.h"
#include "pgsql_driver_arginfo.h"

void pdo_pgsql_notice(void *context, const char *message) /* {{{ */
{
}
/* }}} */

/* {{{ pdo_pgsql_create_lob_stream */
static ssize_t pgsql_lob_write(php_stream *stream, const char *buf, size_t count)
{
	struct pdo_pgsql_lob_self *self = (struct pdo_pgsql_lob_self*)stream->abstract;
	return lo_write(self->conn, self->lfd, (char*)buf, count);
}

static ssize_t pgsql_lob_read(php_stream *stream, char *buf, size_t count)
{
	struct pdo_pgsql_lob_self *self = (struct pdo_pgsql_lob_self*)stream->abstract;
	return lo_read(self->conn, self->lfd, buf, count);
}

static int pgsql_lob_close(php_stream *stream, int close_handle)
{
	struct pdo_pgsql_lob_self *self = (struct pdo_pgsql_lob_self*)stream->abstract;
	pdo_pgsql_db_handle *H = (pdo_pgsql_db_handle *)(Z_PDO_DBH_P(&self->dbh))->driver_data;

	if (close_handle) {
		lo_close(self->conn, self->lfd);
	}
	zend_hash_index_del(*pdo_pgsql_lob_streams(H), php_stream_get_resource_id(stream));
	zval_ptr_dtor(&self->dbh);
	efree(self);
	return 0;
}

static int pgsql_lob_flush(php_stream *stream)
{
	return 0;
}

static int pgsql_lob_seek(php_stream *stream, zend_off_t offset, int whence,
		zend_off_t *newoffset)
{
	struct pdo_pgsql_lob_self *self = (struct pdo_pgsql_lob_self*)stream->abstract;
#if defined(HAVE_PG_LO64) && defined(ZEND_ENABLE_ZVAL_LONG64)
	zend_off_t pos = lo_lseek64(self->conn, self->lfd, offset, whence);
#else
	zend_off_t pos = lo_lseek(self->conn, self->lfd, offset, whence);
#endif
	*newoffset = pos;
	return pos >= 0 ? 0 : -1;
}

const php_stream_ops pdo_pgsql_lob_stream_ops = {
	pgsql_lob_write,
	pgsql_lob_read,
	pgsql_lob_close,
	pgsql_lob_flush,
	"pdo_pgsql lob stream",
	pgsql_lob_seek,
	NULL,
	NULL,
	NULL
};

php_stream *pdo_pgsql_create_lob_stream(zval *dbh, int lfd, Oid oid)
{
	php_stream *stm;
	struct pdo_pgsql_lob_self *self = ecalloc(1, sizeof(*self));
	pdo_pgsql_db_handle *H = (pdo_pgsql_db_handle *)(Z_PDO_DBH_P(dbh))->driver_data;

	ZVAL_COPY_VALUE(&self->dbh, dbh);
	self->lfd = lfd;
	self->oid = oid;
	self->conn = pdo_pgsql_server(H);

	stm = php_stream_alloc(&pdo_pgsql_lob_stream_ops, self, 0, "r+b");

	if (stm) {
		HashTable **streams = pdo_pgsql_lob_streams(H);
		if (!*streams) {
			*streams = pemalloc(sizeof(HashTable), Z_PDO_DBH_P(dbh)->is_persistent);
			zend_hash_init(*streams, 0, NULL, NULL, 1);
		}
		Z_ADDREF_P(dbh);
		zend_hash_index_add_ptr(*pdo_pgsql_lob_streams(H), php_stream_get_resource_id(stm), stm->res);
		return stm;
	}

	efree(self);
	return NULL;
}
/* }}} */

void pdo_pgsql_close_lob_streams(pdo_dbh_t *dbh)
{
	zend_resource *res;
	pdo_pgsql_db_handle *H = (pdo_pgsql_db_handle *)dbh->driver_data;
	if (*pdo_pgsql_lob_streams(H)) {
		ZEND_HASH_REVERSE_FOREACH_PTR(*pdo_pgsql_lob_streams(H), res) {
			if (res->type >= 0) {
				zend_list_close(res);
			}
		} ZEND_HASH_FOREACH_END();
	}
}

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
		dbh->driver_data = NULL;
		pdo_pgsql_free(H);
	}
}

static bool pgsql_handle_preparer(pdo_dbh_t *dbh, zend_string *sql, pdo_stmt_t *stmt, zval *driver_options)
{
	pdo_pgsql_db_handle *H = (pdo_pgsql_db_handle *)dbh->driver_data;
	pdo_pgsql_stmt *S = ecalloc(1, sizeof(pdo_pgsql_stmt));
	int scrollable;
	int ret;
	zend_string *nsql = NULL;
	int emulate = 0;
	int execute_only = 0;

	S->H = H;
	stmt->driver_data = S;
	stmt->methods = &swoole_pgsql_stmt_methods;

	scrollable = pdo_attr_lval(driver_options, PDO_ATTR_CURSOR,
		PDO_CURSOR_FWDONLY) == PDO_CURSOR_SCROLL;

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

	if (!emulate && PQprotocolVersion(pdo_pgsql_server(H)) <= 2) {
		emulate = 1;
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

/* {{{ Returns true if the copy worked fine or false if error */
PHP_METHOD(PDO_PGSql_Ext, pgsqlCopyFromArray)
{
	pdo_dbh_t *dbh;
	pdo_pgsql_db_handle *H;

	zval *pg_rows;

	char *table_name, *pg_delim = NULL, *pg_null_as = NULL, *pg_fields = NULL;
	size_t table_name_len, pg_delim_len = 0, pg_null_as_len = 0, pg_fields_len;
	char *query;

	PGresult *pgsql_result;
	ExecStatusType status;

	if (zend_parse_parameters(ZEND_NUM_ARGS(), "sa|sss!",
					&table_name, &table_name_len, &pg_rows,
					&pg_delim, &pg_delim_len, &pg_null_as, &pg_null_as_len, &pg_fields, &pg_fields_len) == FAILURE) {
		RETURN_THROWS();
	}

	if (!zend_hash_num_elements(Z_ARRVAL_P(pg_rows))) {
		zend_argument_value_error(2, "cannot be empty");
		RETURN_THROWS();
	}

	dbh = Z_PDO_DBH_P(ZEND_THIS);
	PDO_CONSTRUCT_CHECK;
	PDO_DBH_CLEAR_ERR();

	/* using pre-9.0 syntax as PDO_pgsql is 7.4+ compatible */
	if (pg_fields) {
		spprintf(&query, 0, "COPY %s (%s) FROM STDIN WITH DELIMITER E'%c' NULL AS E'%s'", table_name, pg_fields, (pg_delim_len ? *pg_delim : '\t'), (pg_null_as_len ? pg_null_as : "\\\\N"));
	} else {
		spprintf(&query, 0, "COPY %s FROM STDIN WITH DELIMITER E'%c' NULL AS E'%s'", table_name, (pg_delim_len ? *pg_delim : '\t'), (pg_null_as_len ? pg_null_as : "\\\\N"));
	}

	/* Obtain db Handle */
	H = (pdo_pgsql_db_handle *)dbh->driver_data;

	while ((pgsql_result = PQgetResult(pdo_pgsql_server(H)))) {
		PQclear(pgsql_result);
	}
	pgsql_result = PQexec(pdo_pgsql_server(H), query);

	efree(query);
	query = NULL;

	if (pgsql_result) {
		status = PQresultStatus(pgsql_result);
	} else {
		status = (ExecStatusType) PQstatus(pdo_pgsql_server(H));
	}

	if (status == PGRES_COPY_IN && pgsql_result) {
		int command_failed = 0;
		size_t buffer_len = 0;
		zval *tmp;

		PQclear(pgsql_result);
		ZEND_HASH_FOREACH_VAL(Z_ARRVAL_P(pg_rows), tmp) {
			size_t query_len;
			if (!try_convert_to_string(tmp)) {
				efree(query);
				RETURN_THROWS();
			}

			if (buffer_len < Z_STRLEN_P(tmp)) {
				buffer_len = Z_STRLEN_P(tmp);
				query = erealloc(query, buffer_len + 2); /* room for \n\0 */
			}
			memcpy(query, Z_STRVAL_P(tmp), Z_STRLEN_P(tmp));
			query_len = Z_STRLEN_P(tmp);
			if (query[query_len - 1] != '\n') {
				query[query_len++] = '\n';
			}
			query[query_len] = '\0';
			if (PQputCopyData(pdo_pgsql_server(H), query, query_len) != 1) {
				efree(query);
				pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, NULL);
				PDO_HANDLE_DBH_ERR();
				RETURN_FALSE;
			}
		} ZEND_HASH_FOREACH_END();
		if (query) {
			efree(query);
		}

		if (PQputCopyEnd(pdo_pgsql_server(H), NULL) != 1) {
			pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, NULL);
			PDO_HANDLE_DBH_ERR();
			RETURN_FALSE;
		}

		while ((pgsql_result = PQgetResult(pdo_pgsql_server(H)))) {
			if (PGRES_COMMAND_OK != PQresultStatus(pgsql_result)) {
				pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, pdo_pgsql_sqlstate(pgsql_result));
				command_failed = 1;
			}
			PQclear(pgsql_result);
		}

		PDO_HANDLE_DBH_ERR();
		RETURN_BOOL(!command_failed);
	} else {
		pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, pdo_pgsql_sqlstate(pgsql_result));
		PQclear(pgsql_result);
		PDO_HANDLE_DBH_ERR();
		RETURN_FALSE;
	}
}
/* }}} */

/* {{{ Returns true if the copy worked fine or false if error */
PHP_METHOD(PDO_PGSql_Ext, pgsqlCopyFromFile)
{
	pdo_dbh_t *dbh;
	pdo_pgsql_db_handle *H;

	char *table_name, *filename, *pg_delim = NULL, *pg_null_as = NULL, *pg_fields = NULL;
	size_t  table_name_len, filename_len, pg_delim_len = 0, pg_null_as_len = 0, pg_fields_len;
	char *query;
	PGresult *pgsql_result;
	ExecStatusType status;
	php_stream *stream;

	if (zend_parse_parameters(ZEND_NUM_ARGS(), "sp|sss!",
				&table_name, &table_name_len, &filename, &filename_len,
				&pg_delim, &pg_delim_len, &pg_null_as, &pg_null_as_len, &pg_fields, &pg_fields_len) == FAILURE) {
		RETURN_THROWS();
	}

	/* Obtain db Handler */
	dbh = Z_PDO_DBH_P(ZEND_THIS);
	PDO_CONSTRUCT_CHECK;
	PDO_DBH_CLEAR_ERR();

	stream = php_stream_open_wrapper_ex(filename, "rb", 0, NULL, FG(default_context));
	if (!stream) {
		pdo_pgsql_error_msg(dbh, PGRES_FATAL_ERROR, "Unable to open the file");
		PDO_HANDLE_DBH_ERR();
		RETURN_FALSE;
	}

	/* using pre-9.0 syntax as PDO_pgsql is 7.4+ compatible */
	if (pg_fields) {
		spprintf(&query, 0, "COPY %s (%s) FROM STDIN WITH DELIMITER E'%c' NULL AS E'%s'", table_name, pg_fields, (pg_delim_len ? *pg_delim : '\t'), (pg_null_as_len ? pg_null_as : "\\\\N"));
	} else {
		spprintf(&query, 0, "COPY %s FROM STDIN WITH DELIMITER E'%c' NULL AS E'%s'", table_name, (pg_delim_len ? *pg_delim : '\t'), (pg_null_as_len ? pg_null_as : "\\\\N"));
	}

	H = (pdo_pgsql_db_handle *)dbh->driver_data;

	while ((pgsql_result = PQgetResult(pdo_pgsql_server(H)))) {
		PQclear(pgsql_result);
	}
	pgsql_result = PQexec(pdo_pgsql_server(H), query);

	efree(query);

	if (pgsql_result) {
		status = PQresultStatus(pgsql_result);
	} else {
		status = (ExecStatusType) PQstatus(pdo_pgsql_server(H));
	}

	if (status == PGRES_COPY_IN && pgsql_result) {
		char *buf;
		int command_failed = 0;
		size_t line_len = 0;

		PQclear(pgsql_result);
		while ((buf = php_stream_get_line(stream, NULL, 0, &line_len)) != NULL) {
			if (PQputCopyData(pdo_pgsql_server(H), buf, line_len) != 1) {
				efree(buf);
				pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, NULL);
				php_stream_close(stream);
				PDO_HANDLE_DBH_ERR();
				RETURN_FALSE;
			}
			efree(buf);
		}
		php_stream_close(stream);

		if (PQputCopyEnd(pdo_pgsql_server(H), NULL) != 1) {
			pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, NULL);
			PDO_HANDLE_DBH_ERR();
			RETURN_FALSE;
		}

		while ((pgsql_result = PQgetResult(pdo_pgsql_server(H)))) {
			if (PGRES_COMMAND_OK != PQresultStatus(pgsql_result)) {
				pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, pdo_pgsql_sqlstate(pgsql_result));
				command_failed = 1;
			}
			PQclear(pgsql_result);
		}

		PDO_HANDLE_DBH_ERR();
		RETURN_BOOL(!command_failed);
	} else {
		php_stream_close(stream);
		pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, pdo_pgsql_sqlstate(pgsql_result));
		PQclear(pgsql_result);
		PDO_HANDLE_DBH_ERR();
		RETURN_FALSE;
	}
}
/* }}} */

/* {{{ Returns true if the copy worked fine or false if error */
PHP_METHOD(PDO_PGSql_Ext, pgsqlCopyToFile)
{
	pdo_dbh_t *dbh;
	pdo_pgsql_db_handle *H;

	char *table_name, *pg_delim = NULL, *pg_null_as = NULL, *pg_fields = NULL, *filename = NULL;
	size_t table_name_len, pg_delim_len = 0, pg_null_as_len = 0, pg_fields_len, filename_len;
	char *query;

	PGresult *pgsql_result;
	ExecStatusType status;

	php_stream *stream;

	if (zend_parse_parameters(ZEND_NUM_ARGS(), "sp|sss!",
					&table_name, &table_name_len, &filename, &filename_len,
					&pg_delim, &pg_delim_len, &pg_null_as, &pg_null_as_len, &pg_fields, &pg_fields_len) == FAILURE) {
		RETURN_THROWS();
	}

	dbh = Z_PDO_DBH_P(ZEND_THIS);
	PDO_CONSTRUCT_CHECK;
	PDO_DBH_CLEAR_ERR();

	H = (pdo_pgsql_db_handle *)dbh->driver_data;

	stream = php_stream_open_wrapper_ex(filename, "wb", 0, NULL, FG(default_context));
	if (!stream) {
		pdo_pgsql_error_msg(dbh, PGRES_FATAL_ERROR, "Unable to open the file for writing");
		PDO_HANDLE_DBH_ERR();
		RETURN_FALSE;
	}

	while ((pgsql_result = PQgetResult(pdo_pgsql_server(H)))) {
		PQclear(pgsql_result);
	}

	/* using pre-9.0 syntax as PDO_pgsql is 7.4+ compatible */
	if (pg_fields) {
		spprintf(&query, 0, "COPY %s (%s) TO STDIN WITH DELIMITER E'%c' NULL AS E'%s'", table_name, pg_fields, (pg_delim_len ? *pg_delim : '\t'), (pg_null_as_len ? pg_null_as : "\\\\N"));
	} else {
		spprintf(&query, 0, "COPY %s TO STDIN WITH DELIMITER E'%c' NULL AS E'%s'", table_name, (pg_delim_len ? *pg_delim : '\t'), (pg_null_as_len ? pg_null_as : "\\\\N"));
	}
	pgsql_result = PQexec(pdo_pgsql_server(H), query);
	efree(query);

	if (pgsql_result) {
		status = PQresultStatus(pgsql_result);
	} else {
		status = (ExecStatusType) PQstatus(pdo_pgsql_server(H));
	}

	if (status == PGRES_COPY_OUT && pgsql_result) {
		PQclear(pgsql_result);
		while (1) {
			char *csv = NULL;
			int ret = PQgetCopyData(pdo_pgsql_server(H), &csv, 0);

			if (ret == -1) {
				break; /* done */
			} else if (ret > 0) {
				if (php_stream_write(stream, csv, ret) != (size_t)ret) {
					pdo_pgsql_error_msg(dbh, PGRES_FATAL_ERROR, "Unable to write to file");
					PQfreemem(csv);
					php_stream_close(stream);
					PDO_HANDLE_DBH_ERR();
					RETURN_FALSE;
				} else {
					PQfreemem(csv);
				}
			} else {
				pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, NULL);
				php_stream_close(stream);
				PDO_HANDLE_DBH_ERR();
				RETURN_FALSE;
			}
		}
		php_stream_close(stream);

		while ((pgsql_result = PQgetResult(pdo_pgsql_server(H)))) {
			PQclear(pgsql_result);
		}
		RETURN_TRUE;
	} else {
		php_stream_close(stream);
		pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, pdo_pgsql_sqlstate(pgsql_result));
		PQclear(pgsql_result);
		PDO_HANDLE_DBH_ERR();
		RETURN_FALSE;
	}
}
/* }}} */

/* {{{ Returns true if the copy worked fine or false if error */
PHP_METHOD(PDO_PGSql_Ext, pgsqlCopyToArray)
{
	pdo_dbh_t *dbh;
	pdo_pgsql_db_handle *H;

	char *table_name, *pg_delim = NULL, *pg_null_as = NULL, *pg_fields = NULL;
	size_t table_name_len, pg_delim_len = 0, pg_null_as_len = 0, pg_fields_len;
	char *query;

	PGresult *pgsql_result;
	ExecStatusType status;

	if (zend_parse_parameters(ZEND_NUM_ARGS(), "s|sss!",
		&table_name, &table_name_len,
		&pg_delim, &pg_delim_len, &pg_null_as, &pg_null_as_len, &pg_fields, &pg_fields_len) == FAILURE) {
		RETURN_THROWS();
	}

	dbh = Z_PDO_DBH_P(ZEND_THIS);
	PDO_CONSTRUCT_CHECK;
	PDO_DBH_CLEAR_ERR();

	H = (pdo_pgsql_db_handle *)dbh->driver_data;

	while ((pgsql_result = PQgetResult(pdo_pgsql_server(H)))) {
		PQclear(pgsql_result);
	}

	/* using pre-9.0 syntax as PDO_pgsql is 7.4+ compatible */
	if (pg_fields) {
		spprintf(&query, 0, "COPY %s (%s) TO STDIN WITH DELIMITER E'%c' NULL AS E'%s'", table_name, pg_fields, (pg_delim_len ? *pg_delim : '\t'), (pg_null_as_len ? pg_null_as : "\\\\N"));
	} else {
		spprintf(&query, 0, "COPY %s TO STDIN WITH DELIMITER E'%c' NULL AS E'%s'", table_name, (pg_delim_len ? *pg_delim : '\t'), (pg_null_as_len ? pg_null_as : "\\\\N"));
	}
	pgsql_result = PQexec(pdo_pgsql_server(H), query);
	efree(query);

	if (pgsql_result) {
		status = PQresultStatus(pgsql_result);
	} else {
		status = (ExecStatusType) PQstatus(pdo_pgsql_server(H));
	}

	if (status == PGRES_COPY_OUT && pgsql_result) {
		PQclear(pgsql_result);
                array_init(return_value);

		while (1) {
			char *csv = NULL;
			int ret = PQgetCopyData(pdo_pgsql_server(H), &csv, 0);
			if (ret == -1) {
				break; /* copy done */
			} else if (ret > 0) {
				add_next_index_stringl(return_value, csv, ret);
				PQfreemem(csv);
			} else {
				pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, NULL);
				PDO_HANDLE_DBH_ERR();
				RETURN_FALSE;
			}
		}

		while ((pgsql_result = PQgetResult(pdo_pgsql_server(H)))) {
			PQclear(pgsql_result);
		}
	} else {
		pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, pdo_pgsql_sqlstate(pgsql_result));
		PQclear(pgsql_result);
		PDO_HANDLE_DBH_ERR();
		RETURN_FALSE;
	}
}
/* }}} */

/* {{{ Creates a new large object, returning its identifier.  Must be called inside a transaction. */
PHP_METHOD(PDO_PGSql_Ext, pgsqlLOBCreate)
{
	pdo_dbh_t *dbh;
	pdo_pgsql_db_handle *H;
	Oid lfd;

	ZEND_PARSE_PARAMETERS_NONE();

	dbh = Z_PDO_DBH_P(ZEND_THIS);
	PDO_CONSTRUCT_CHECK;
	PDO_DBH_CLEAR_ERR();

	H = (pdo_pgsql_db_handle *)dbh->driver_data;
	lfd = lo_creat(pdo_pgsql_server(H), INV_READ|INV_WRITE);

	if (lfd != InvalidOid) {
		zend_string *buf = strpprintf(0, ZEND_ULONG_FMT, (zend_long) lfd);

		RETURN_STR(buf);
	}

	pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, NULL);
	PDO_HANDLE_DBH_ERR();
	RETURN_FALSE;
}
/* }}} */

/* {{{ Opens an existing large object stream.  Must be called inside a transaction. */
PHP_METHOD(PDO_PGSql_Ext, pgsqlLOBOpen)
{
	pdo_dbh_t *dbh;
	pdo_pgsql_db_handle *H;
	Oid oid;
	int lfd;
	char *oidstr;
	size_t oidstrlen;
	char *modestr = "rb";
	size_t modestrlen;
	int mode = INV_READ;
	char *end_ptr;

	if (FAILURE == zend_parse_parameters(ZEND_NUM_ARGS(), "s|s",
				&oidstr, &oidstrlen, &modestr, &modestrlen)) {
		RETURN_THROWS();
	}

	oid = (Oid)strtoul(oidstr, &end_ptr, 10);
	if (oid == 0 && (errno == ERANGE || errno == EINVAL)) {
		RETURN_FALSE;
	}

	if (strpbrk(modestr, "+w")) {
		mode = INV_READ|INV_WRITE;
	}

	dbh = Z_PDO_DBH_P(ZEND_THIS);
	PDO_CONSTRUCT_CHECK;
	PDO_DBH_CLEAR_ERR();

	H = (pdo_pgsql_db_handle *)dbh->driver_data;

	lfd = lo_open(pdo_pgsql_server(H), oid, mode);

	if (lfd >= 0) {
		php_stream *stream = pdo_pgsql_create_lob_stream(ZEND_THIS, lfd, oid);
		if (stream) {
			php_stream_to_zval(stream, return_value);
			return;
		}
	} else {
		pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, NULL);
	}

	PDO_HANDLE_DBH_ERR();
	RETURN_FALSE;
}
/* }}} */

/* {{{ Deletes the large object identified by oid.  Must be called inside a transaction. */
PHP_METHOD(PDO_PGSql_Ext, pgsqlLOBUnlink)
{
	pdo_dbh_t *dbh;
	pdo_pgsql_db_handle *H;
	Oid oid;
	char *oidstr, *end_ptr;
	size_t oidlen;

	if (FAILURE == zend_parse_parameters(ZEND_NUM_ARGS(), "s",
				&oidstr, &oidlen)) {
		RETURN_THROWS();
	}

	oid = (Oid)strtoul(oidstr, &end_ptr, 10);
	if (oid == 0 && (errno == ERANGE || errno == EINVAL)) {
		RETURN_FALSE;
	}

	dbh = Z_PDO_DBH_P(ZEND_THIS);
	PDO_CONSTRUCT_CHECK;
	PDO_DBH_CLEAR_ERR();

	H = (pdo_pgsql_db_handle *)dbh->driver_data;

	if (1 == lo_unlink(pdo_pgsql_server(H), oid)) {
		RETURN_TRUE;
	}

	pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, NULL);
	PDO_HANDLE_DBH_ERR();
	RETURN_FALSE;
}
/* }}} */

/* {{{ Get asynchronous notification */
PHP_METHOD(PDO_PGSql_Ext, pgsqlGetNotify)
{
	pdo_dbh_t *dbh;
	pdo_pgsql_db_handle *H;
	zend_long result_type = PDO_FETCH_USE_DEFAULT;
	zend_long ms_timeout = 0;
	PGnotify *pgsql_notify;

	if (FAILURE == zend_parse_parameters(ZEND_NUM_ARGS(), "|ll",
				&result_type, &ms_timeout)) {
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

	H = (pdo_pgsql_db_handle *)dbh->driver_data;

	if (!PQconsumeInput(pdo_pgsql_server(H))) {
		pdo_pgsql_error(dbh, PGRES_FATAL_ERROR, NULL);
		PDO_HANDLE_DBH_ERR();
		RETURN_FALSE;
	}
	pgsql_notify = PQnotifies(pdo_pgsql_server(H));

	if (ms_timeout && !pgsql_notify) {
		php_pollfd_for_ms(PQsocket(pdo_pgsql_server(H)), PHP_POLLREADABLE, (int)ms_timeout);

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
/* }}} */

/* {{{ Get backend(server) pid */
PHP_METHOD(PDO_PGSql_Ext, pgsqlGetPid)
{
	pdo_dbh_t *dbh;
	pdo_pgsql_db_handle *H;

	ZEND_PARSE_PARAMETERS_NONE();

	dbh = Z_PDO_DBH_P(ZEND_THIS);
	PDO_CONSTRUCT_CHECK;

	H = (pdo_pgsql_db_handle *)dbh->driver_data;

	RETURN_LONG(PQbackendPID(pdo_pgsql_server(H)));
}
/* }}} */

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
	NULL /* get_gc */
};

static int pdo_pgsql_handle_factory(pdo_dbh_t *dbh, zval *driver_options) {
	return pdo_pgsql_connect(dbh, dbh->data_source, dbh->username, dbh->password, driver_options);
}

const pdo_driver_t swoole_pdo_pgsql_driver = {
	PDO_DRIVER_HEADER(pgsql),
	pdo_pgsql_handle_factory
};
#endif

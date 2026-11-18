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

#include "php_swoole_odbc.h"

#if PHP_VERSION_ID >= 80500
#include "php.h"
#include "php_ini.h"
#include "ext/standard/info.h"
#include "ext/pdo/php_pdo.h"
#include "ext/pdo/php_pdo_driver.h"
#include "php_pdo_odbc.h"
//#include "php_pdo_odbc_int.h"

/* Buffer size; bigger columns than this become a "long column" */
#define LONG_COLUMN_BUFFER_SIZE (ZEND_MM_PAGE_SIZE- ZSTR_MAX_OVERHEAD)

enum pdo_odbc_conv_result {
	PDO_ODBC_CONV_NOT_REQUIRED,
	PDO_ODBC_CONV_OK,
	PDO_ODBC_CONV_FAIL
};

static int pdo_odbc_sqltype_is_unicode(pdo_odbc_stmt *S, SQLSMALLINT sqltype)
{
	if (!S->assume_utf8) return 0;
	switch (sqltype) {
#ifdef SQL_WCHAR
		case SQL_WCHAR:
			return 1;
#endif
#ifdef SQL_WLONGVARCHAR
		case SQL_WLONGVARCHAR:
			return 1;
#endif
#ifdef SQL_WVARCHAR
		case SQL_WVARCHAR:
			return 1;
#endif
		default:
			return 0;
	}
}

static int pdo_odbc_utf82ucs2(pdo_stmt_t *stmt, int is_unicode, const char *buf,
	zend_ulong buflen, zend_ulong *outlen)
{
#ifdef PHP_WIN32
	if (is_unicode && buflen) {
		pdo_odbc_stmt *S = (pdo_odbc_stmt*)stmt->driver_data;
		DWORD ret;

		ret = MultiByteToWideChar(CP_UTF8, 0, buf, buflen, NULL, 0);
		if (ret == 0) {
			/*printf("%s:%d %d [%d] %.*s\n", __FILE__, __LINE__, GetLastError(), buflen, buflen, buf);*/
			return PDO_ODBC_CONV_FAIL;
		}

		ret *= sizeof(WCHAR);

		if (S->convbufsize <= ret) {
			S->convbufsize = ret + sizeof(WCHAR);
			S->convbuf = erealloc(S->convbuf, S->convbufsize);
		}

		ret = MultiByteToWideChar(CP_UTF8, 0, buf, buflen, (LPWSTR)S->convbuf, S->convbufsize / sizeof(WCHAR));
		if (ret == 0) {
			/*printf("%s:%d %d [%d] %.*s\n", __FILE__, __LINE__, GetLastError(), buflen, buflen, buf);*/
			return PDO_ODBC_CONV_FAIL;
		}

		ret *= sizeof(WCHAR);
		*outlen = ret;
		return PDO_ODBC_CONV_OK;
	}
#endif
	return PDO_ODBC_CONV_NOT_REQUIRED;
}

static int pdo_odbc_ucs22utf8(int is_unicode, zval *result)
{
#ifdef PHP_WIN32
	ZEND_ASSERT(Z_TYPE_P(result) == IS_STRING);
	if (is_unicode && Z_STRLEN_P(result) != 0) {
		DWORD ret;

		ret = WideCharToMultiByte(CP_UTF8, 0, (LPCWSTR) Z_STRVAL_P(result), Z_STRLEN_P(result)/sizeof(WCHAR), NULL, 0, NULL, NULL);
		if (ret == 0) {
			return PDO_ODBC_CONV_FAIL;
		}

		zend_string *str = zend_string_alloc(ret, 0);
		ret = WideCharToMultiByte(CP_UTF8, 0, (LPCWSTR) Z_STRVAL_P(result), Z_STRLEN_P(result)/sizeof(WCHAR), ZSTR_VAL(str), ZSTR_LEN(str), NULL, NULL);
		if (ret == 0) {
			zend_string_efree(str);
			return PDO_ODBC_CONV_FAIL;
		}

		ZSTR_VAL(str)[ret] = '\0';
		zval_ptr_dtor_str(result);
		ZVAL_STR(result, str);
		return PDO_ODBC_CONV_OK;
	}
#endif
	return PDO_ODBC_CONV_NOT_REQUIRED;
}

static void free_cols(pdo_stmt_t *stmt, pdo_odbc_stmt *S)
{
	if (S->cols) {
		int i;

		for (i = 0; i < S->col_count; i++) {
			if (S->cols[i].data) {
				efree(S->cols[i].data);
			}
		}
		efree(S->cols);
		S->cols = NULL;
		S->col_count = 0;
	}
}

static int odbc_stmt_dtor(pdo_stmt_t *stmt)
{
	pdo_odbc_stmt *S = (pdo_odbc_stmt*)stmt->driver_data;

	if (S->stmt != SQL_NULL_HANDLE && php_pdo_stmt_valid_db_obj_handle(stmt)) {
		if (stmt->executed) {
			SQLCloseCursor(S->stmt);
		}
		SQLFreeHandle(SQL_HANDLE_STMT, S->stmt);
		S->stmt = SQL_NULL_HANDLE;
	}

	free_cols(stmt, S);
	if (S->convbuf) {
		efree(S->convbuf);
	}
	efree(S);

	return 1;
}

bool swr_odbc_supply_data(pdo_stmt_t *stmt, SQLRETURN *status) {
    pdo_odbc_stmt *S = (pdo_odbc_stmt *) stmt->driver_data;
    SQLRETURN rc = *status, rc1;
    char *buf = NULL;
#line 171
	while (rc == SQL_NEED_DATA) {
		struct pdo_bound_param_data *param;

		rc = SQLParamData(S->stmt, (SQLPOINTER*)&param);
		if (rc == SQL_NEED_DATA) {
			php_stream *stm;
			int len;
			pdo_odbc_param *P;
			zval *parameter;

			P = (pdo_odbc_param*)param->driver_data;
			if (Z_ISREF(param->parameter)) {
				parameter = Z_REFVAL(param->parameter);
			} else {
				parameter = &param->parameter;
			}
			if (Z_TYPE_P(parameter) != IS_RESOURCE) {
				/* they passed in a string */
				zend_ulong ulen;
				convert_to_string(parameter);

				switch (pdo_odbc_utf82ucs2(stmt, P->is_unicode,
							Z_STRVAL_P(parameter),
							Z_STRLEN_P(parameter),
							&ulen)) {
					case PDO_ODBC_CONV_NOT_REQUIRED:
						rc1 = SQLPutData(S->stmt, Z_STRVAL_P(parameter),
							Z_STRLEN_P(parameter));
						if (rc1 != SQL_SUCCESS && rc1 != SQL_SUCCESS_WITH_INFO) {
							rc = rc1;
						}
						break;
					case PDO_ODBC_CONV_OK:
						rc1 = SQLPutData(S->stmt, S->convbuf, ulen);
						if (rc1 != SQL_SUCCESS && rc1 != SQL_SUCCESS_WITH_INFO) {
							rc = rc1;
						}
						break;
					case PDO_ODBC_CONV_FAIL:
						pdo_odbc_stmt_error("error converting input string");
						SQLCloseCursor(S->stmt);
						if (buf) {
							efree(buf);
						}
						return 0;
				}
				continue;
			}

			/* we assume that LOBs are binary and don't need charset
			 * conversion */

			php_stream_from_zval_no_verify(stm, parameter);
			if (!stm) {
				/* shouldn't happen either */
				pdo_odbc_stmt_error("input LOB is no longer a stream");
				SQLCloseCursor(S->stmt);
				if (buf) {
					efree(buf);
				}
				return 0;
			}

			/* now suck data from the stream and stick it into the database */
			if (buf == NULL) {
				buf = emalloc(8192);
			}

			do {
				len = php_stream_read(stm, buf, 8192);
				if (len == 0) {
					break;
				}
				rc1 = SQLPutData(S->stmt, buf, len);
				if (rc1 != SQL_SUCCESS && rc1 != SQL_SUCCESS_WITH_INFO) {
					rc = rc1;
				}
			} while (1);
		}
	}

	if (buf) {
		efree(buf);
	}
    *status = rc;
    return true;
}

static const int bind_lines[] = {388, 404};
static const char stmt_file[] = __FILE__;
#define ODBC_CONVERT_RESULT(s, u, z) pdo_odbc_ucs22utf8(u, z)
static const int execute_lines[] = {261, 265};
static const int fetch_lines[] = {549, 558};
static const int describe_lines[] = {595, 606, 630};
static const int attr_lines[] = {801, 831};
#include "odbc_statement_abi.h"

const struct pdo_stmt_methods swoole_odbc_stmt_methods = {
	odbc_stmt_dtor,
	odbc_stmt_execute,
	odbc_stmt_fetch,
	odbc_stmt_describe,
	odbc_stmt_get_col,
	odbc_stmt_param_hook,
	odbc_stmt_set_param,
	odbc_stmt_get_attr,
	odbc_stmt_get_column_meta,
	odbc_stmt_next_rowset,
	odbc_stmt_close_cursor
};
#endif